#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod clipboard;
mod config;
mod draw;
mod state;

use capture::{HardwareScreenCapturer, ScreenCapturer};
use clipboard::{ClipboardService, SystemClipboard};
use draw::Tool;
use image::RgbaImage;
use state::AppState;
use std::cell::RefCell;
use std::rc::Rc;

slint::include_modules!();

/// Dynamically enables Per-Monitor DPI V2 on Windows 10 (1703+) and Windows 11.
/// Guarantees that xcap physical screen captures map 1:1 to Slint overlay without blur.
#[cfg(target_os = "windows")]
fn init_windows_dpi() {
    use std::ffi::c_void;

    extern "system" {
        fn LoadLibraryA(lpLibFileName: *const u8) -> *mut c_void;
        fn GetProcAddress(
            hModule: *mut c_void,
            lpProcName: *const u8,
        ) -> Option<unsafe extern "system" fn()>;
    }

    type SetProcessDpiAwarenessContextFn = unsafe extern "system" fn(*mut c_void) -> i32;
    type SetProcessDpiAwareFn = unsafe extern "system" fn() -> i32;

    unsafe {
        let handle = LoadLibraryA(b"user32.dll\0".as_ptr());
        if !handle.is_null() {
            if let Some(proc) = GetProcAddress(handle, b"SetProcessDpiAwarenessContext\0".as_ptr()) {
                let func: SetProcessDpiAwarenessContextFn = std::mem::transmute(proc);
                // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = ((HANDLE)-4)
                let _ = func(-4isize as *mut c_void);
            } else if let Some(proc) = GetProcAddress(handle, b"SetProcessDPIAware\0".as_ptr()) {
                let func: SetProcessDpiAwareFn = std::mem::transmute(proc);
                let _ = func();
            }
        }
    }
}

/// Helper to synchronize the Slint canvas image with an in-memory RGBA buffer (DRY)
fn sync_overlay(overlay_weak: &slint::Weak<OverlayWindow>, img: &RgbaImage) {
    let slint_img = capture::rgba_to_slint_image(img);
    if let Some(overlay) = overlay_weak.upgrade() {
        overlay.set_background_image(slint_img);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    init_windows_dpi();

    println!("LocalShot: Initializing 100% offline screen capture...");

    // 1. Hardware Screen Capture directly into RAM (Zero disk residue)
    let capturer = HardwareScreenCapturer;
    let frame = capturer.capture()?;
    println!(
        "LocalShot: Screen captured in RAM: {}x{} pixels.",
        frame.width, frame.height
    );

    let initial_slint_image = frame.slint_image.clone();

    // 2. Centralized App State (Single Responsibility & KISS)
    let state_rc = Rc::new(RefCell::new(AppState::new(frame)));

    // 3. Initialize Slint overlay window
    let overlay = OverlayWindow::new()?;
    overlay.set_background_image(initial_slint_image);
    overlay.set_has_selection(false);
    overlay.set_dimension_text("".into());
    overlay.set_active_tool(Tool::Select.as_str_id().into());
    overlay.set_active_color_idx(0);

    let overlay_weak = overlay.as_weak();

    // Tool selection from toolbar
    {
        let state_rc = state_rc.clone();
        overlay.on_tool_selected(move |tool_str| {
            let tool = Tool::from_str_id(tool_str.as_str());
            state_rc.borrow_mut().set_tool(tool);
            println!("LocalShot: Active tool changed to: {:?}", tool);
        });
    }

    // Color selection from palette
    {
        let state_rc = state_rc.clone();
        overlay.on_color_selected(move |idx| {
            state_rc.borrow_mut().set_color_idx(idx as usize);
            println!("LocalShot: Color changed to palette index: {}", idx);
        });
    }

    // Live dragging selection box
    {
        let overlay_weak = overlay_weak.clone();
        overlay.on_selection_changed(move |_x, _y, w, h| {
            if let Some(overlay) = overlay_weak.upgrade() {
                let wi = w.round() as u32;
                let hi = h.round() as u32;
                overlay.set_dimension_text(format!("{} x {}", wi, hi).into());
            }
        });
    }

    // Finalize selection box
    {
        let state_rc = state_rc.clone();
        overlay.on_selection_finished(move |x, y, w, h| {
            let mut state = state_rc.borrow_mut();
            state.selection.set_bounds(
                x.round() as i32,
                y.round() as i32,
                w.round() as u32,
                h.round() as u32,
            );
            println!(
                "LocalShot: Selection confirmed at ({}, {}) [{}x{}]",
                state.selection.x, state.selection.y, state.selection.w, state.selection.h
            );
        });
    }

    // In-Selection Drawing Start
    {
        let state_rc = state_rc.clone();
        overlay.on_draw_started(move |rx, ry| {
            let mut state = state_rc.borrow_mut();
            let abs_x = state.selection.x + rx.round() as i32;
            let abs_y = state.selection.y + ry.round() as i32;
            state.start_drawing(abs_x, abs_y);
        });
    }

    // In-Selection Drawing Move (High performance live preview via dirty-rectangle restoration)
    {
        let state_rc = state_rc.clone();
        let overlay_weak = overlay_weak.clone();

        overlay.on_draw_moved(move |rx, ry| {
            let mut state = state_rc.borrow_mut();
            let abs_x = state.selection.x + rx.round() as i32;
            let abs_y = state.selection.y + ry.round() as i32;

            state.add_drawing_point(abs_x, abs_y);

            if let Some(ann) = state.build_current_annotation((abs_x, abs_y)) {
                let preview = state.render_preview(Some(&ann));
                sync_overlay(&overlay_weak, preview);
            }
        });
    }

    // In-Selection Drawing Finish (Commit to History & Update Composited Cache)
    {
        let state_rc = state_rc.clone();
        let overlay_weak = overlay_weak.clone();

        overlay.on_draw_finished(move |rx, ry| {
            let mut state = state_rc.borrow_mut();
            let abs_x = state.selection.x + rx.round() as i32;
            let abs_y = state.selection.y + ry.round() as i32;

            if let Some(ann) = state.build_current_annotation((abs_x, abs_y)) {
                state.commit_annotation(ann);
                state.finish_drawing();
                sync_overlay(&overlay_weak, &state.composited_cache);
            } else {
                state.finish_drawing();
            }
        });
    }

    // Undo action (Ctrl+Z or Undo button)
    {
        let state_rc = state_rc.clone();
        let overlay_weak = overlay_weak.clone();

        overlay.on_undo_requested(move || {
            let mut state = state_rc.borrow_mut();
            if state.undo() {
                println!(
                    "LocalShot: Undo applied. Remaining annotations: {}",
                    state.annotations.len()
                );
                sync_overlay(&overlay_weak, &state.composited_cache);
            }
        });
    }

    // Copy action (Ctrl+C, Enter or Copy button)
    {
        let state_rc = state_rc.clone();
        let overlay_weak = overlay_weak.clone();

        overlay.on_copy_requested(move || {
            let crop = state_rc.borrow().get_final_crop();
            if let Some(cropped) = crop {
                let (w, h) = cropped.dimensions();
                let clipboard = SystemClipboard;
                match clipboard.copy_image(&cropped) {
                    Ok(()) => {
                        println!(
                            "LocalShot: Successfully copied {}x{} annotated screenshot to clipboard!",
                            w, h
                        );
                    }
                    Err(e) => {
                        eprintln!("LocalShot: Clipboard copy error: {}", e);
                    }
                }
            }
            if let Some(overlay) = overlay_weak.upgrade() {
                let _ = overlay.hide();
            }
        });
    }

    // Save action (Ctrl+S or Save button)
    {
        let state_rc = state_rc.clone();
        let overlay_weak = overlay_weak.clone();

        overlay.on_save_requested(move || {
            let crop = state_rc.borrow().get_final_crop();
            if let Some(cropped) = crop {
                let default_dir = config::default_save_dir();
                let default_name = config::default_filename();

                // Open native save dialog (IFileDialog on Windows, portal/zenity on Linux)
                let dialog = rfd::FileDialog::new()
                    .set_title("Save Screenshot As")
                    .set_directory(&default_dir)
                    .set_file_name(&default_name)
                    .add_filter("PNG Image (*.png)", &["png"]);

                if let Some(target_path) = dialog.save_file() {
                    match cropped.save(&target_path) {
                        Ok(()) => {
                            println!("LocalShot: Saved screenshot to {}", target_path.display());
                            if let Some(overlay) = overlay_weak.upgrade() {
                                let _ = overlay.hide();
                            }
                        }
                        Err(e) => {
                            eprintln!("LocalShot: Save file error: {}", e);
                        }
                    }
                } else {
                    println!("LocalShot: Save cancelled by user.");
                }
            }
        });
    }

    // Close action (Esc or Close button)
    {
        let overlay_weak = overlay_weak.clone();
        overlay.on_close_requested(move || {
            println!("LocalShot: Dismissed by user.");
            if let Some(overlay) = overlay_weak.upgrade() {
                let _ = overlay.hide();
            }
        });
    }

    println!("LocalShot: Running overlay with Dual Toolbar & Drawing Engine...");
    overlay.run()?;

    println!("LocalShot: Sanitizing memory...");
    state_rc.borrow_mut().sanitize();
    drop(state_rc);
    println!("LocalShot: Clean shutdown completed.");
    Ok(())
}

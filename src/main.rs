#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod clipboard;
mod config;
mod daemon;
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
    
    // UI va Rasm o'lchamlari 1:1 tushishi uchun Slint masshtabini o'chiramiz (faqat Windows)
    std::env::set_var("SLINT_SCALE_FACTOR", "1");
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

    if !daemon::init_daemon() {
        println!("LocalShot is already running in the background.");
        return Ok(());
    }

    let (tx, rx) = std::sync::mpsc::channel();
    
    // Spawn background hotkey/tray listener
    daemon::spawn_background_worker(move || {
        let _ = tx.send(());
    });

    println!("LocalShot: Running in background mode. Press PrtScn to capture.");

    // Initial capture on startup (so double-clicking shortcut works immediately)
    let _ = tx.send(());

    loop {
        // Block until hotkey or tray icon is clicked
        if rx.recv().is_err() {
            break;
        }

        println!("LocalShot: Initializing 100% offline screen capture...");

        let capturer = HardwareScreenCapturer;
        let frame = match capturer.capture() {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Failed to capture screen: {}", e);
                continue;
            }
        };

        let initial_slint_image = frame.slint_image.clone();
        let state_rc = Rc::new(RefCell::new(AppState::new(frame)));

        let overlay = OverlayWindow::new()?;
        overlay.set_background_image(initial_slint_image);
        overlay.set_has_selection(false);
        overlay.set_dimension_text("".into());
        overlay.set_active_tool(Tool::Select.as_str_id().into());
        overlay.set_active_color_idx(0);

        let overlay_weak = overlay.as_weak();

        {
            let state_rc = state_rc.clone();
            overlay.on_tool_selected(move |tool_str| {
                let tool = Tool::from_str_id(tool_str.as_str());
                state_rc.borrow_mut().set_tool(tool);
            });
        }

        {
            let state_rc = state_rc.clone();
            overlay.on_color_selected(move |idx| {
                state_rc.borrow_mut().set_color_idx(idx as usize);
            });
        }

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

        {
            let state_rc = state_rc.clone();
            overlay.on_selection_finished(move |x, y, w, h| {
                state_rc.borrow_mut().selection.set_bounds(
                    x.round() as i32, y.round() as i32, w.round() as u32, h.round() as u32,
                );
            });
        }

        {
            let state_rc = state_rc.clone();
            overlay.on_draw_started(move |rx, ry| {
                let mut state = state_rc.borrow_mut();
                let abs_x = state.selection.x + rx.round() as i32;
                let abs_y = state.selection.y + ry.round() as i32;
                state.start_drawing(abs_x, abs_y);
            });
        }

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

        {
            let state_rc = state_rc.clone();
            let overlay_weak = overlay_weak.clone();
            overlay.on_undo_requested(move || {
                let mut state = state_rc.borrow_mut();
                if state.undo() {
                    sync_overlay(&overlay_weak, &state.composited_cache);
                }
            });
        }

        {
            let state_rc = state_rc.clone();
            overlay.on_copy_requested(move || {
                if let Some(cropped) = state_rc.borrow().get_final_crop() {
                    let _ = SystemClipboard.copy_image(&cropped);
                }
                let _ = slint::quit_event_loop();
            });
        }

        {
            let state_rc = state_rc.clone();
            overlay.on_save_requested(move || {
                if let Some(cropped) = state_rc.borrow().get_final_crop() {
                    let dialog = rfd::FileDialog::new()
                        .set_title("Save Screenshot As")
                        .set_directory(&config::default_save_dir())
                        .set_file_name(&config::default_filename())
                        .add_filter("PNG Image (*.png)", &["png"]);
                    if let Some(target_path) = dialog.save_file() {
                        let _ = cropped.save(&target_path);
                        let _ = slint::quit_event_loop();
                    }
                }
            });
        }

        {
            overlay.on_close_requested(move || {
                let _ = slint::quit_event_loop();
            });
        }

        // Run the overlay for this capture session
        overlay.run()?;
        
        // Free memory when closed
        state_rc.borrow_mut().sanitize();
    }
    
    Ok(())
}

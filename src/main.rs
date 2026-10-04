#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod clipboard;
mod config;
mod daemon;
mod draw;
mod state;

use capture::{HardwareScreenCapturer, ScreenCapturer};
use clipboard::{ClipboardService, SystemClipboard};
use daemon::DaemonInitResult;
use draw::Tool;
use image::RgbaImage;
use state::AppState;
use std::sync::{Arc, Mutex};

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

    std::env::set_var("SLINT_SCALE_FACTOR", "1");
}

/// Helper to synchronize the Slint canvas image with an in-memory RGBA buffer
fn sync_overlay(overlay_weak: &slint::Weak<OverlayWindow>, img: &RgbaImage) {
    let slint_img = capture::rgba_to_slint_image(img);
    if let Some(overlay) = overlay_weak.upgrade() {
        overlay.set_background_image(slint_img);
    }
}

/// Captures the screen, resets state and UI controls, then reveals the overlay window
fn perform_capture(overlay: &OverlayWindow, state_lock: &Arc<Mutex<Option<AppState>>>) -> bool {
    let capturer = HardwareScreenCapturer;
    // Attempt capture with a brief retry for desktop/portal focus transition delays
    let mut capture_res = capturer.capture();
    if capture_res.is_err() {
        std::thread::sleep(std::time::Duration::from_millis(150));
        capture_res = capturer.capture();
    }

    match capture_res {
        Ok(frame) => {
            let slint_img = frame.slint_image.clone();
            let mut guard = state_lock.lock().unwrap();
            if let Some(state) = guard.as_mut() {
                state.reset(frame);
            } else {
                *guard = Some(AppState::new(frame));
            }

            overlay.set_background_image(slint_img);
            overlay.set_is_selecting(false);
            overlay.set_has_selection(false);
            overlay.set_sel_x(0.0);
            overlay.set_sel_y(0.0);
            overlay.set_sel_w(0.0);
            overlay.set_sel_h(0.0);
            overlay.set_dimension_text("".into());
            overlay.set_active_tool(Tool::Select.as_str_id().into());
            overlay.set_active_color_idx(0);

            let _ = overlay.show();
            true
        }
        Err(e) => {
            eprintln!("LocalShot: Screen capture error: {}", e);
            false
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    init_windows_dpi();

    // Check single instance and IPC (if already running, signal existing instance to capture)
    if daemon::check_or_signal_existing_instance() == DaemonInitResult::AlreadyRunningSignaled {
        println!("LocalShot is already running. Triggered screen capture in active instance.");
        return Ok(());
    }

    let is_daemon_mode = std::env::args().any(|a| a == "--daemon" || a == "--background");
    let is_windows = cfg!(target_os = "windows");

    let _daemon_state = daemon::init_daemon();

    let overlay = OverlayWindow::new()?;
    let state_lock = Arc::new(Mutex::new(None));
    let overlay_weak = overlay.as_weak();

    // Setup global background daemon callbacks
    {
        let overlay_weak = overlay_weak.clone();
        let state_lock = state_lock.clone();
        daemon::set_capture_callback(move || {
            let state_lock = state_lock.clone();
            let _ = overlay_weak.upgrade_in_event_loop(move |overlay| {
                perform_capture(&overlay, &state_lock);
            });
        });
    }

    daemon::set_quit_callback(|| {
        let _ = slint::invoke_from_event_loop(|| {
            let _ = slint::quit_event_loop();
        });
    });

    // Tool selection
    {
        let state_lock = state_lock.clone();
        overlay.on_tool_selected(move |tool_str| {
            let tool = Tool::from_str_id(tool_str.as_str());
            if let Some(state) = state_lock.lock().unwrap().as_mut() {
                state.set_tool(tool);
            }
        });
    }

    // Color selection
    {
        let state_lock = state_lock.clone();
        overlay.on_color_selected(move |idx| {
            if let Some(state) = state_lock.lock().unwrap().as_mut() {
                state.set_color_idx(idx as usize);
            }
        });
    }

    // Selection changed
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

    // Selection finished
    {
        let state_lock = state_lock.clone();
        overlay.on_selection_finished(move |x, y, w, h| {
            if let Some(state) = state_lock.lock().unwrap().as_mut() {
                state.selection.set_bounds(
                    x.round() as i32,
                    y.round() as i32,
                    w.round() as u32,
                    h.round() as u32,
                );
            }
        });
    }

    // Draw started
    {
        let state_lock = state_lock.clone();
        overlay.on_draw_started(move |rx, ry| {
            if let Some(state) = state_lock.lock().unwrap().as_mut() {
                let abs_x = state.selection.x + rx.round() as i32;
                let abs_y = state.selection.y + ry.round() as i32;
                state.start_drawing(abs_x, abs_y);
            }
        });
    }

    // Draw moved
    {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        overlay.on_draw_moved(move |rx, ry| {
            let mut guard = state_lock.lock().unwrap();
            if let Some(state) = guard.as_mut() {
                let abs_x = state.selection.x + rx.round() as i32;
                let abs_y = state.selection.y + ry.round() as i32;
                state.add_drawing_point(abs_x, abs_y);
                if let Some(ann) = state.build_current_annotation((abs_x, abs_y)) {
                    let preview = state.render_preview(Some(&ann));
                    sync_overlay(&overlay_weak, preview);
                }
            }
        });
    }

    // Draw finished
    {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        overlay.on_draw_finished(move |rx, ry| {
            let mut guard = state_lock.lock().unwrap();
            if let Some(state) = guard.as_mut() {
                let abs_x = state.selection.x + rx.round() as i32;
                let abs_y = state.selection.y + ry.round() as i32;
                if let Some(ann) = state.build_current_annotation((abs_x, abs_y)) {
                    state.commit_annotation(ann);
                    state.finish_drawing();
                    sync_overlay(&overlay_weak, &state.composited_cache);
                } else {
                    state.finish_drawing();
                }
            }
        });
    }

    // Undo requested
    {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        overlay.on_undo_requested(move || {
            let mut guard = state_lock.lock().unwrap();
            if let Some(state) = guard.as_mut() {
                if state.undo() {
                    sync_overlay(&overlay_weak, &state.composited_cache);
                }
            }
        });
    }

    // Copy action (Ctrl+C, Enter or Copy button)
    {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        overlay.on_copy_requested(move || {
            let crop = {
                let guard = state_lock.lock().unwrap();
                guard.as_ref().and_then(|s| s.get_final_crop())
            };
            if let Some(cropped) = crop {
                let _ = SystemClipboard.copy_image(&cropped);
            }
            if let Some(overlay) = overlay_weak.upgrade() {
                let _ = overlay.hide();
            }
            if let Some(state) = state_lock.lock().unwrap().as_mut() {
                state.sanitize();
            }
            if !is_windows {
                let _ = slint::quit_event_loop();
            }
        });
    }

    // Save action (Ctrl+S or Save button)
    {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        overlay.on_save_requested(move || {
            let crop = {
                let guard = state_lock.lock().unwrap();
                guard.as_ref().and_then(|s| s.get_final_crop())
            };
            if let Some(cropped) = crop {
                let dialog = rfd::FileDialog::new()
                    .set_title("Save Screenshot As")
                    .set_directory(config::default_save_dir())
                    .set_file_name(config::default_filename())
                    .add_filter("PNG Image (*.png)", &["png"]);

                if let Some(target_path) = dialog.save_file() {
                    let _ = cropped.save(&target_path);
                    if let Some(overlay) = overlay_weak.upgrade() {
                        let _ = overlay.hide();
                    }
                    if let Some(state) = state_lock.lock().unwrap().as_mut() {
                        state.sanitize();
                    }
                    if !is_windows {
                        let _ = slint::quit_event_loop();
                    }
                }
            }
        });
    }

    // Close action (Esc or Close button)
    {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        overlay.on_close_requested(move || {
            if let Some(overlay) = overlay_weak.upgrade() {
                let _ = overlay.hide();
            }
            if let Some(state) = state_lock.lock().unwrap().as_mut() {
                state.sanitize();
            }
            if !is_windows {
                let _ = slint::quit_event_loop();
            }
        });
    }

    // If started directly (not in silent daemon/background mode), take initial capture
    if !is_daemon_mode {
        if !perform_capture(&overlay, &state_lock) {
            eprintln!("LocalShot: Could not capture screen, aborting without opening blank window.");
            return Ok(());
        }
    } else {
        println!("LocalShot: Running silently in tray mode. Press PrtScn or Ctrl+Alt+S to capture.");
    }

    // Start Slint Event Loop
    overlay.run()?;

    if let Some(state) = state_lock.lock().unwrap().as_mut() {
        state.sanitize();
    }

    Ok(())
}

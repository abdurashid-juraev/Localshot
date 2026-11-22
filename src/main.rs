#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// Item 7: deny unwrap_used for all non-test code.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

mod capture;
mod clipboard;
mod config;
mod coord;
mod daemon;
mod draw;
mod interaction;
mod keyboard;
mod state;
mod updater;

use capture::{HardwareScreenCapturer, ScreenCapturer};
use clipboard::{ClipboardService, SystemClipboard};
use daemon::DaemonInitResult;
use draw::Tool;
use image::RgbaImage;
use interaction::SelectionMachine;
use parking_lot::Mutex;
use rfd::{MessageButtons, MessageDialog, MessageLevel};
use slint::{PhysicalPosition, PhysicalSize};
use state::{AppState, SelectionRect};
use std::sync::Arc;

#[allow(clippy::unwrap_used)]
mod ui {
    slint::include_modules!();
}
use ui::*;

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
            if let Some(proc) = GetProcAddress(handle, b"SetProcessDpiAwarenessContext\0".as_ptr())
            {
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

#[cfg(target_os = "windows")]
fn setup_windows_overlay() {
    use std::ffi::c_void;

    extern "system" {
        fn GetCurrentThreadId() -> u32;
        fn EnumThreadWindows(
            dwThreadId: u32,
            lpfn: Option<unsafe extern "system" fn(*mut c_void, isize) -> i32>,
            lParam: isize,
        ) -> i32;
        fn GetWindowLongPtrW(hWnd: *mut c_void, nIndex: i32) -> isize;
        fn SetWindowLongPtrW(hWnd: *mut c_void, nIndex: i32, dwNewLong: isize) -> isize;
        fn GetForegroundWindow() -> *mut c_void;
        fn GetWindowThreadProcessId(hWnd: *mut c_void, lpdwProcessId: *mut u32) -> u32;
        fn AttachThreadInput(idAttach: u32, idAttachTo: u32, fAttach: i32) -> i32;
        fn SetForegroundWindow(hWnd: *mut c_void) -> i32;
        fn SetFocus(hWnd: *mut c_void) -> *mut c_void;
    }

    unsafe extern "system" fn enum_proc(hwnd: *mut c_void, lparam: isize) -> i32 {
        let target = &mut *(lparam as *mut *mut c_void);
        *target = hwnd;
        0
    }

    let mut hwnd: *mut c_void = std::ptr::null_mut();
    let thread_id = unsafe { GetCurrentThreadId() };
    unsafe {
        EnumThreadWindows(thread_id, Some(enum_proc), &mut hwnd as *mut _ as isize);
    }

    if !hwnd.is_null() {
        const GWL_EXSTYLE: i32 = -20;
        const WS_EX_TOOLWINDOW: isize = 0x00000080;
        let ex_style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
        unsafe {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style | WS_EX_TOOLWINDOW);
        }

        let foreground_hwnd = unsafe { GetForegroundWindow() };
        let foreground_thread =
            unsafe { GetWindowThreadProcessId(foreground_hwnd, std::ptr::null_mut()) };

        if foreground_thread != 0 && foreground_thread != thread_id {
            unsafe {
                AttachThreadInput(thread_id, foreground_thread, 1);
                SetForegroundWindow(hwnd);
                SetFocus(hwnd);
                AttachThreadInput(thread_id, foreground_thread, 0);
            }
        } else {
            unsafe {
                SetForegroundWindow(hwnd);
                SetFocus(hwnd);
            }
        }
    }
}

/// Helper to synchronize the Slint canvas image with an in-memory RGBA buffer
fn sync_overlay(overlay_weak: &slint::Weak<OverlayWindow>, img: &RgbaImage) {
    let slint_img = capture::rgba_to_slint_image(img);
    if let Some(overlay) = overlay_weak.upgrade() {
        overlay.set_background_image(slint_img);
    }
}

/// Updates Slint selection properties and dual toolbar layout from current AppState
fn sync_selection_to_overlay(overlay: &OverlayWindow, state: &AppState) {
    let scale = overlay.window().scale_factor();
    let frame_w = state.raw_base.width();
    let frame_h = state.raw_base.height();

    let (lx, ly, lw, lh) = coord::physical_to_logical_rect(
        state.selection.x,
        state.selection.y,
        state.selection.w,
        state.selection.h,
        scale,
    );

    overlay.set_sel_x(lx);
    overlay.set_sel_y(ly);
    overlay.set_sel_w(lw);
    overlay.set_sel_h(lh);

    let is_selecting = state.machine == SelectionMachine::Selecting;
    let has_selection = state.machine != SelectionMachine::Idle && !state.selection.is_empty();

    overlay.set_is_selecting(is_selecting);
    overlay.set_has_selection(has_selection);

    if has_selection || is_selecting {
        overlay.set_dimension_text(format!("{} x {}", state.selection.w, state.selection.h).into());
    } else {
        overlay.set_dimension_text("".into());
    }

    if state.machine == SelectionMachine::Selected && !state.selection.is_empty() {
        let screen_logical = (
            coord::physical_to_logical(frame_w as f32, scale),
            coord::physical_to_logical(frame_h as f32, scale),
        );
        let sel_logical = interaction::RectF {
            x: lx,
            y: ly,
            w: lw,
            h: lh,
        };
        let vert_size = (48.0, 368.0);
        let horiz_size = (156.0, 46.0);

        let pos = interaction::calculate_toolbar_positions(
            sel_logical,
            screen_logical,
            vert_size,
            horiz_size,
        );

        overlay.set_vert_toolbar_x(pos.vert.0);
        overlay.set_vert_toolbar_y(pos.vert.1);
        overlay.set_horiz_toolbar_x(pos.horiz.0);
        overlay.set_horiz_toolbar_y(pos.horiz.1);
        overlay.set_show_toolbars(true);
    } else {
        overlay.set_show_toolbars(false);
    }
}

/// Maps an interaction resize handle to its corresponding cursor identifier
fn cursor_for_handle(handle: interaction::Handle) -> &'static str {
    match handle {
        interaction::Handle::TopLeft | interaction::Handle::BottomRight => "nwse-resize",
        interaction::Handle::TopRight | interaction::Handle::BottomLeft => "nesw-resize",
        interaction::Handle::Top | interaction::Handle::Bottom => "ns-resize",
        interaction::Handle::Left | interaction::Handle::Right => "ew-resize",
    }
}

/// Positions the overlay window precisely over the target monitor,
/// resets OS-level cascading offsets, and reveals the window.
fn position_and_show_overlay(overlay: &OverlayWindow, x: i32, y: i32, width: u32, height: u32) {
    let window = overlay.window();
    window.set_position(PhysicalPosition::new(x, y));
    window.set_size(PhysicalSize::new(width, height));
    if let Err(e) = overlay.show() {
        eprintln!("LocalShot: overlay show error: {}", e);
    }
    // Reinforce position post-show to eliminate OS cascading/DWM shadow drift on repeat toggles
    window.set_position(PhysicalPosition::new(x, y));
    window.set_size(PhysicalSize::new(width, height));

    #[cfg(target_os = "windows")]
    setup_windows_overlay();
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
            let (x, y, w, h) = (frame.x, frame.y, frame.width, frame.height);
            let mut guard = state_lock.lock();
            if let Some(state) = guard.as_mut() {
                state.reset(frame);
            } else {
                *guard = Some(AppState::new(frame));
            }
            // Drop guard before touching UI
            drop(guard);

            overlay.set_background_image(slint_img);
            overlay.set_is_selecting(false);
            overlay.set_has_selection(false);
            overlay.set_show_toolbars(false);
            overlay.set_sel_x(0.0);
            overlay.set_sel_y(0.0);
            overlay.set_sel_w(0.0);
            overlay.set_sel_h(0.0);
            overlay.set_dimension_text("".into());
            overlay.set_active_tool(Tool::Select.as_str_id().into());
            overlay.set_active_color_idx(0);
            overlay.set_cursor_name("crosshair".into());

            position_and_show_overlay(overlay, x, y, w, h);
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

    #[cfg(target_os = "linux")]
    if std::env::var("WAYLAND_APP_ID").is_err() {
        std::env::set_var("WAYLAND_APP_ID", "localshot-overlay");
    }

    // Item 11: handle --check-updates / --update BEFORE the single-instance check.
    if std::env::args().any(|a| a == "--check-updates" || a == "--update") {
        updater::check_for_updates(true);
        return Ok(());
    }

    // Check single instance and IPC (if already running, signal existing instance to capture)
    if daemon::check_or_signal_existing_instance() == DaemonInitResult::AlreadyRunningSignaled {
        println!("LocalShot is already running. Triggered screen capture in active instance.");
        return Ok(());
    }

    let is_daemon_mode = std::env::args().any(|a| a == "--daemon" || a == "--background");
    let is_windows = cfg!(target_os = "windows");

    let _daemon_state = daemon::init_daemon();

    // Spawn non-blocking background auto-update check (respects 24h cooldown)
    updater::spawn_background_update_check();

    let overlay = OverlayWindow::new()?;
    let state_lock: Arc<Mutex<Option<AppState>>> = Arc::new(Mutex::new(None));
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

    // Helper closure to dismiss overlay, sanitize state, and exit event loop if needed
    let dismiss = {
        let overlay_weak = overlay_weak.clone();
        let state_lock = state_lock.clone();
        move || {
            if let Some(overlay) = overlay_weak.upgrade() {
                let _ = overlay.hide();
            }
            if let Some(state) = state_lock.lock().as_mut() {
                state.sanitize();
            }
            if !is_windows {
                let _ = slint::quit_event_loop();
            }
        }
    };

    // Copy action (Ctrl+C, Enter or Copy button)
    let copy_action = {
        let state_lock = state_lock.clone();
        let dismiss = dismiss.clone();
        move || {
            let crop = {
                let guard = state_lock.lock();
                guard.as_ref().and_then(|s| s.get_final_crop())
            };
            if let Some(cropped) = crop {
                match SystemClipboard.copy_image(&cropped) {
                    Ok(()) => dismiss(),
                    Err(e) => {
                        MessageDialog::new()
                            .set_title("LocalShot — Copy Failed")
                            .set_description(format!("Could not copy image to clipboard:\n{}", e))
                            .set_buttons(MessageButtons::Ok)
                            .set_level(MessageLevel::Error)
                            .show();
                    }
                }
            }
        }
    };

    // Save action (Ctrl+S or Save button)
    let save_action = {
        let state_lock = state_lock.clone();
        let dismiss = dismiss.clone();
        move || {
            let crop = {
                let guard = state_lock.lock();
                guard.as_ref().and_then(|s| s.get_final_crop())
            };
            if let Some(cropped) = crop {
                let dialog = rfd::FileDialog::new()
                    .set_title("Save Screenshot As")
                    .set_directory(config::default_save_dir())
                    .set_file_name(config::default_filename())
                    .add_filter("PNG Image (*.png)", &["png"]);

                if let Some(mut target_path) = dialog.save_file() {
                    if target_path.extension().map(|e| e.to_ascii_lowercase()) != Some("png".into())
                    {
                        let mut name = target_path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                        name.push_str(".png");
                        target_path.set_file_name(name);
                    }

                    match cropped.save(&target_path) {
                        Ok(()) => dismiss(),
                        Err(e) => {
                            MessageDialog::new()
                                .set_title("LocalShot — Save Failed")
                                .set_description(format!(
                                    "Could not save screenshot to {}:\n{}",
                                    target_path.display(),
                                    e
                                ))
                                .set_buttons(MessageButtons::Ok)
                                .set_level(MessageLevel::Error)
                                .show();
                        }
                    }
                }
            }
        }
    };

    // Copy / Save / Close / Undo / Redo button bindings
    {
        let copy_action = copy_action.clone();
        overlay.on_copy_requested(move || {
            copy_action();
        });
    }

    {
        let save_action = save_action.clone();
        overlay.on_save_requested(move || {
            save_action();
        });
    }

    {
        let dismiss = dismiss.clone();
        overlay.on_close_requested(move || {
            dismiss();
        });
    }

    // Undo requested
    {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        overlay.on_undo_requested(move || {
            let mut guard = state_lock.lock();
            if let Some(state) = guard.as_mut() {
                if state.undo() {
                    sync_overlay(&overlay_weak, state.composited_image());
                }
            }
        });
    }

    overlay.on_redo_requested(|| {
        // No-op until redo stack is implemented in stage 5.
    });

    // Tool selection
    {
        let state_lock = state_lock.clone();
        overlay.on_tool_selected(move |tool_str| {
            let tool = Tool::from_str_id(tool_str.as_str());
            if let Some(state) = state_lock.lock().as_mut() {
                state.set_tool(tool);
            }
        });
    }

    // Color selection
    {
        let state_lock = state_lock.clone();
        overlay.on_color_selected(move |idx| {
            if let Some(state) = state_lock.lock().as_mut() {
                state.set_color_idx(idx as usize);
            }
        });
    }

    // Unified pointer/touch event handling (kind: down / move / up)
    {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        overlay.on_touch_event(move |kind, lx, ly, _modifiers, pressed| {
            let Some(overlay) = overlay_weak.upgrade() else {
                return;
            };
            let scale = overlay.window().scale_factor();
            let (phys_x, phys_y) = coord::logical_to_physical_point(lx, ly, scale);

            let mut guard = state_lock.lock();
            let Some(state) = guard.as_mut() else {
                return;
            };

            let frame_w = state.raw_base.width();
            let frame_h = state.raw_base.height();

            match kind.as_str() {
                "down" => {
                    match state.machine {
                        SelectionMachine::Idle => {
                            state.machine = SelectionMachine::Selecting;
                            state.drag_start = (phys_x, phys_y);
                            state.selection = SelectionRect {
                                x: phys_x,
                                y: phys_y,
                                w: 0,
                                h: 0,
                            };
                        }
                        SelectionMachine::Selected => {
                            // Test resize handles first with physical pixel tolerance
                            if let Some(handle) =
                                interaction::hit_test_handle(&state.selection, (phys_x, phys_y), 8)
                            {
                                state.machine = SelectionMachine::Resizing(handle);
                                state.drag_start = (phys_x, phys_y);
                                state.initial_rect = state.selection;
                            } else if state.tool == Tool::Select
                                && interaction::hit_test_selection(
                                    &state.selection,
                                    (phys_x, phys_y),
                                )
                            {
                                state.machine = SelectionMachine::Moving;
                                state.drag_start = (phys_x, phys_y);
                                state.initial_rect = state.selection;
                            } else if state.tool != Tool::Select
                                && interaction::hit_test_selection(
                                    &state.selection,
                                    (phys_x, phys_y),
                                )
                            {
                                state.machine = SelectionMachine::Drawing;
                                state.start_drawing(phys_x, phys_y);
                            } else {
                                // Clicked outside existing selection -> start fresh selection
                                state.machine = SelectionMachine::Selecting;
                                state.drag_start = (phys_x, phys_y);
                                state.selection = SelectionRect {
                                    x: phys_x,
                                    y: phys_y,
                                    w: 0,
                                    h: 0,
                                };
                            }
                        }
                        _ => {}
                    }
                    sync_selection_to_overlay(&overlay, state);
                }
                "move" => {
                    // Update hover cursor when not actively dragging
                    if !pressed && state.machine == SelectionMachine::Selected {
                        let cursor = if let Some(h) =
                            interaction::hit_test_handle(&state.selection, (phys_x, phys_y), 8)
                        {
                            cursor_for_handle(h)
                        } else if interaction::hit_test_selection(
                            &state.selection,
                            (phys_x, phys_y),
                        ) {
                            if state.tool == Tool::Select {
                                "move"
                            } else {
                                "crosshair"
                            }
                        } else {
                            "crosshair"
                        };
                        overlay.set_cursor_name(cursor.into());
                    }

                    if pressed {
                        match state.machine {
                            SelectionMachine::Selecting => {
                                state.selection =
                                    interaction::normalize_rect(state.drag_start, (phys_x, phys_y));
                                sync_selection_to_overlay(&overlay, state);
                            }
                            SelectionMachine::Moving => {
                                let delta =
                                    (phys_x - state.drag_start.0, phys_y - state.drag_start.1);
                                state.selection = interaction::apply_move(
                                    &state.initial_rect,
                                    delta,
                                    frame_w,
                                    frame_h,
                                );
                                sync_selection_to_overlay(&overlay, state);
                            }
                            SelectionMachine::Resizing(handle) => {
                                let delta =
                                    (phys_x - state.drag_start.0, phys_y - state.drag_start.1);
                                state.selection = interaction::apply_resize(
                                    &state.initial_rect,
                                    handle,
                                    delta,
                                    frame_w,
                                    frame_h,
                                    16,
                                );
                                sync_selection_to_overlay(&overlay, state);
                            }
                            SelectionMachine::Drawing => {
                                state.add_drawing_point(phys_x, phys_y);
                                if let Some(ann) = state.build_current_annotation((phys_x, phys_y))
                                {
                                    let preview = state.render_preview(Some(&ann));
                                    sync_overlay(&overlay_weak, preview);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                "up" => {
                    match state.machine {
                        SelectionMachine::Selecting => {
                            if state.selection.w > 10 && state.selection.h > 10 {
                                state.machine = SelectionMachine::Selected;
                            } else {
                                state.machine = SelectionMachine::Idle;
                                state.selection = SelectionRect::default();
                            }
                        }
                        SelectionMachine::Moving | SelectionMachine::Resizing(_) => {
                            state.machine = SelectionMachine::Selected;
                        }
                        SelectionMachine::Drawing => {
                            if let Some(ann) = state.build_current_annotation((phys_x, phys_y)) {
                                state.commit_annotation(ann);
                                state.finish_drawing();
                                sync_overlay(&overlay_weak, state.composited_image());
                            } else {
                                state.finish_drawing();
                            }
                            state.machine = SelectionMachine::Selected;
                        }
                        _ => {}
                    }
                    sync_selection_to_overlay(&overlay, state);
                }
                _ => {}
            }
        });
    }

    // Escape handler: cancels current drag/edit first, clears selection next, closes overlay if idle
    {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        let dismiss = dismiss.clone();
        overlay.on_escape_pressed(move || {
            let Some(overlay) = overlay_weak.upgrade() else {
                return;
            };
            let mut guard = state_lock.lock();
            let Some(state) = guard.as_mut() else {
                dismiss();
                return;
            };

            match state.machine {
                SelectionMachine::Selecting => {
                    // Cancel active selection drag
                    state.selection = SelectionRect::default();
                    state.machine = SelectionMachine::Idle;
                    sync_selection_to_overlay(&overlay, state);
                }
                SelectionMachine::Moving | SelectionMachine::Resizing(_) => {
                    // Cancel active move or resize, restore initial rect
                    state.selection = state.initial_rect;
                    state.machine = SelectionMachine::Selected;
                    sync_selection_to_overlay(&overlay, state);
                }
                SelectionMachine::Drawing => {
                    // Cancel active drawing stroke
                    state.finish_drawing();
                    sync_overlay(&overlay_weak, state.composited_image());
                    state.machine = SelectionMachine::Selected;
                    sync_selection_to_overlay(&overlay, state);
                }
                SelectionMachine::Selected => {
                    // Clear selection and return to idle
                    state.selection = SelectionRect::default();
                    state.machine = SelectionMachine::Idle;
                    sync_selection_to_overlay(&overlay, state);
                }
                SelectionMachine::Idle => {
                    drop(guard);
                    dismiss();
                }
            }
        });
    }

    // Keyboard shortcuts (normalized across Latin QWERTY and Cyrillic Russian/Uzbek layouts)
    {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        let copy_action = copy_action.clone();
        let save_action = save_action.clone();
        overlay.on_key_event(move |text, ctrl, shift, alt| {
            let Some(key) = keyboard::normalize_key_text(&text) else {
                return;
            };
            let Some(overlay) = overlay_weak.upgrade() else {
                return;
            };

            if ctrl {
                match key {
                    'a' => {
                        let mut guard = state_lock.lock();
                        if let Some(state) = guard.as_mut() {
                            let w = state.raw_base.width();
                            let h = state.raw_base.height();
                            state.selection.set_bounds(0, 0, w, h);
                            state.machine = SelectionMachine::Selected;
                            sync_selection_to_overlay(&overlay, state);
                        }
                    }
                    'c' => {
                        copy_action();
                    }
                    's' => {
                        save_action();
                    }
                    'z' if !shift => {
                        let mut guard = state_lock.lock();
                        if let Some(state) = guard.as_mut() {
                            if state.undo() {
                                sync_overlay(&overlay_weak, state.composited_image());
                            }
                        }
                    }
                    _ => {}
                }
            } else if !alt {
                let tool = match key {
                    'p' => Some(Tool::Pen),
                    'l' => Some(Tool::Line),
                    'a' => Some(Tool::Arrow),
                    'r' => Some(Tool::Rectangle),
                    'm' => Some(Tool::Marker),
                    'b' => Some(Tool::Redact),
                    _ => None,
                };
                if let Some(t) = tool {
                    let mut guard = state_lock.lock();
                    if let Some(state) = guard.as_mut() {
                        let new_tool = if state.tool == t { Tool::Select } else { t };
                        state.set_tool(new_tool);
                        overlay.set_active_tool(new_tool.as_str_id().into());
                    }
                }
            }
        });
    }

    // If started directly (not in silent daemon/background mode), take initial capture
    if !is_daemon_mode {
        if !perform_capture(&overlay, &state_lock) {
            eprintln!(
                "LocalShot: Could not capture screen, aborting without opening blank window."
            );
            return Ok(());
        }
    } else {
        println!(
            "LocalShot: Running silently in tray mode. Press PrtScn or Ctrl+Alt+S to capture."
        );
    }

    // Start Slint Event Loop
    overlay.run()?;

    if let Some(state) = state_lock.lock().as_mut() {
        state.sanitize();
    }

    Ok(())
}

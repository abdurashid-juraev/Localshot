#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// Item 7: deny unwrap_used for all non-test code.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

mod clipboard;
mod keyboard;

use clipboard::{ClipboardService, SystemClipboard};
use image::RgbaImage;
use localshot::{
    capture::{self, HardwareScreenCapturer, ScreenCapturer},
    config, coord,
    daemon::{self, DaemonInitResult, IpcCommand},
    draw::{self, Tool},
    interaction::{self, SelectionMachine},
    linux_desktop,
    state::{AppState, SelectionRect},
    updater,
};
use parking_lot::Mutex;
use rfd::{MessageButtons, MessageDialog, MessageLevel};
use slint::{PhysicalPosition, PhysicalSize};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use zeroize::Zeroize;

#[allow(clippy::unwrap_used)]
mod ui {
    slint::include_modules!();
}
use ui::*;

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

use capture::MAX_SAFE_TEXTURE_DIM;

/// Converts a physical bounding-box preview buffer into `preview-image` on the overlay.
fn sync_preview_layer(overlay: &OverlayWindow, bx: u32, by: u32, bw: u32, bh: u32, rgba: &[u8]) {
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(bw, bh);
    let dst = buffer.make_mut_bytes();
    let len = dst.len().min(rgba.len());
    dst[..len].copy_from_slice(&rgba[..len]);
    let img = slint::Image::from_rgba8(buffer);

    let scale = overlay.window().scale_factor();
    let (lx, ly, lw, lh) = coord::physical_to_logical_rect(bx as i32, by as i32, bw, bh, scale);

    overlay.set_preview_image(img);
    overlay.set_preview_x(lx);
    overlay.set_preview_y(ly);
    overlay.set_preview_w(lw);
    overlay.set_preview_h(lh);
}

/// Clears the live preview layer without touching `background-image`.
fn clear_preview_layer(overlay: &OverlayWindow) {
    overlay.set_preview_image(slint::Image::default());
    overlay.set_preview_x(0.0);
    overlay.set_preview_y(0.0);
    overlay.set_preview_w(0.0);
    overlay.set_preview_h(0.0);
}

/// Synchronizes `background-image` (or `bg-tiles` if the frame exceeds `MAX_SAFE_TEXTURE_DIM`)
/// with an in-memory RGBA buffer. Called ONLY on capture, commit, undo, or redo.
fn sync_overlay(overlay_weak: &slint::Weak<OverlayWindow>, img: &RgbaImage) {
    let Some(overlay) = overlay_weak.upgrade() else {
        return;
    };
    let (w, h) = img.dimensions();
    if w <= MAX_SAFE_TEXTURE_DIM && h <= MAX_SAFE_TEXTURE_DIM {
        overlay.set_bg_tiles(slint::ModelRc::default());
        overlay.set_background_image(capture::rgba_to_slint_image(img));
        return;
    }

    let scale = overlay.window().scale_factor();
    let mut tiles = Vec::new();
    for ty in (0..h).step_by(MAX_SAFE_TEXTURE_DIM as usize) {
        let th = MAX_SAFE_TEXTURE_DIM.min(h - ty);
        for tx in (0..w).step_by(MAX_SAFE_TEXTURE_DIM as usize) {
            let tw = MAX_SAFE_TEXTURE_DIM.min(w - tx);
            let sub = image::imageops::crop_imm(img, tx, ty, tw, th).to_image();
            let tile_img = capture::rgba_to_slint_image(&sub);
            let (lx, ly, lw, lh) =
                coord::physical_to_logical_rect(tx as i32, ty as i32, tw, th, scale);
            tiles.push(ImageTile {
                img: tile_img,
                x: lx,
                y: ly,
                w: lw,
                h: lh,
            });
        }
    }
    overlay.set_background_image(slint::Image::default());
    overlay.set_bg_tiles(slint::ModelRc::new(slint::VecModel::from(tiles)));
}

/// Creates the Slint overlay window with automatic fallback to the software renderer
/// if the default GL context cannot be created (VM, RDP, or old drivers).
fn create_overlay_with_fallback() -> Result<OverlayWindow, slint::PlatformError> {
    match OverlayWindow::new() {
        Ok(win) => Ok(win),
        Err(err) => {
            eprintln!(
                "LocalShot: Default renderer failed ({}), falling back to software renderer.",
                err
            );
            let _ = slint::BackendSelector::new()
                .renderer_name("software".into())
                .select();
            OverlayWindow::new()
        }
    }
}

/// Reports current process RSS memory (best-effort) after dismissing buffers.
fn report_memory_after_dismiss() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        unsafe {
            malloc_trim(0);
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(statm) = std::fs::read_to_string("/proc/self/statm") {
            if let Some(rss_pages) = statm
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<u64>().ok())
            {
                let rss_mb = (rss_pages * 4096) as f64 / (1024.0 * 1024.0);
                eprintln!(
                    "LocalShot: Dismissed buffers; process RSS = {:.1} MiB",
                    rss_mb
                );
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("LocalShot: Dismissed and released all image buffers.");
    }
}

/// Updates Slint tool, thickness, active color, and undo/redo availability from AppState.
fn sync_tool_and_history_ui(overlay: &OverlayWindow, state: &AppState) {
    let c = state.active_color();
    overlay.set_active_draw_color(slint::Color::from_argb_u8(255, c[0], c[1], c[2]));
    overlay.set_active_thickness(state.active_thickness());
    overlay.set_text_font_size(state.thickness_for(Tool::Text) as f32);
    overlay.set_can_undo(state.can_undo());
    overlay.set_can_redo(state.can_redo());
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
        let vert_size = (48.0, 492.0);
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

    #[cfg(target_os = "linux")]
    let is_wayland = linux_desktop::is_wayland_session();
    #[cfg(not(target_os = "linux"))]
    let is_wayland = false;

    if is_wayland {
        // Wayland xdg-toplevel does not support global positioning or always-on-top;
        // fullscreen xdg-toplevel covers the active output.
        window.set_fullscreen(true);
        if let Err(e) = overlay.show() {
            eprintln!("LocalShot: overlay show error: {}", e);
        }
    } else {
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
        #[cfg(target_os = "linux")]
        linux_desktop::setup_x11_overlay_window();
    }
}

/// Returns whether capture retries should be attempted (disabled on Wayland so
/// the XDG Desktop Portal permission dialog is not spammed every 150 ms).
fn should_retry_capture(is_wayland: bool) -> bool {
    !is_wayland
}

/// Captures the screen with retry + backoff (skipped on Wayland where portal
/// permission dialogs can stay open for seconds), returning a user-friendly error on failure.
fn capture_with_retry() -> Result<capture::CapturedFrame, String> {
    let capturer = HardwareScreenCapturer;
    #[cfg(target_os = "linux")]
    let is_wayland = linux_desktop::is_wayland_session();
    #[cfg(not(target_os = "linux"))]
    let is_wayland = false;

    if !should_retry_capture(is_wayland) {
        return capturer.capture().map_err(|e| e.to_string());
    }

    let backoffs = [100u64, 250u64];
    let mut last_err = String::new();

    for attempt in 0..=backoffs.len() {
        match capturer.capture() {
            Ok(frame) => return Ok(frame),
            Err(e) => {
                last_err = e.to_string();
                if let Some(&delay_ms) = backoffs.get(attempt) {
                    std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                }
            }
        }
    }
    Err(last_err)
}

fn show_error_dialog(title: &str, message: &str) {
    eprintln!("{}: {}", title, message);
    MessageDialog::new()
        .set_title(title)
        .set_description(message)
        .set_buttons(MessageButtons::Ok)
        .set_level(MessageLevel::Error)
        .show();
}

fn show_info_notification(title: &str, message: &str) {
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("notify-send")
            .args(["-a", "LocalShot", title, message])
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let escaped_title = title.replace('\'', "''");
        let escaped_msg = message.replace('\'', "''");
        let script = format!(
            "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
             $xml = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
             $texts = $xml.GetElementsByTagName('text'); \
             $texts.Item(0).AppendChild($xml.CreateTextNode('{}')) | Out-Null; \
             $texts.Item(1).AppendChild($xml.CreateTextNode('{}')) | Out-Null; \
             $toast = [Windows.UI.Notifications.ToastNotification]::new($xml); \
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('LocalShot').Show($toast);",
            escaped_title, escaped_msg
        );
        let _ = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
                &script,
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        println!("{}: {}", title, message);
    }
}

fn run_guarded<F: FnOnce()>(label: &str, f: F) {
    if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        let msg = if let Some(s) = payload.downcast_ref::<&str>() {
            *s
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.as_str()
        } else {
            "unknown panic payload"
        };
        eprintln!("LocalShot: Recovered from panic in {}: {}", label, msg);
    }
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".to_string());
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            *s
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.as_str()
        } else {
            "non-string panic payload"
        };
        let line = format!(
            "[{}] panic at {}: {}\n",
            chrono::Local::now().to_rfc3339(),
            location,
            payload
        );
        eprintln!("LocalShot panic: {}", line.trim_end());
        if let Some(base) = dirs::cache_dir().or_else(dirs::data_local_dir) {
            let log_dir = base.join("localshot");
            if std::fs::create_dir_all(&log_dir).is_ok() {
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(log_dir.join("panic.log"))
                {
                    use std::io::Write;
                    let _ = file.write_all(line.as_bytes());
                }
            }
        }
    }));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliAction {
    Area,
    Full,
    Daemon,
    Quit,
    CheckUpdates,
    WaylandHelp,
    Oneshot,
}

fn parse_cli_args<'a, I>(args: I) -> CliAction
where
    I: IntoIterator<Item = &'a str>,
{
    let mut action = CliAction::Area;
    for arg in args {
        match arg {
            "--check-updates" | "--update" => return CliAction::CheckUpdates,
            "--wayland-help" | "--shortcuts-help" => return CliAction::WaylandHelp,
            "--quit" => return CliAction::Quit,
            "--daemon" | "--background" => action = CliAction::Daemon,
            "--full" => action = CliAction::Full,
            "--oneshot" => action = CliAction::Oneshot,
            "--area" => action = CliAction::Area,
            _ => {}
        }
    }
    action
}

fn spawn_detached_daemon_and_send(cmd: IpcCommand) -> Result<(), String> {
    if daemon::send_ipc_command(cmd).is_ok() {
        return Ok(());
    }

    let exe = std::env::current_exe().map_err(|e| format!("Failed to locate executable: {}", e))?;
    let mut command = std::process::Command::new(exe);
    command
        .arg("--daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        const DETACHED_PROCESS: u32 = 0x00000008;
        command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }

    command
        .spawn()
        .map_err(|e| format!("Failed to start LocalShot daemon: {}", e))?;

    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        if daemon::send_ipc_command(cmd).is_ok() {
            return Ok(());
        }
    }

    Err("Timed out waiting for LocalShot daemon IPC socket".to_string())
}

/// Captures the screen, resets state and UI controls, then reveals the overlay window.
/// If the overlay is already active, focuses the existing overlay without recapturing.
fn perform_capture(overlay: &OverlayWindow, state_lock: &Arc<Mutex<Option<AppState>>>) -> bool {
    if daemon::is_overlay_active() {
        if let Err(e) = overlay.show() {
            eprintln!("LocalShot: overlay focus error: {}", e);
        }
        #[cfg(target_os = "windows")]
        setup_windows_overlay();
        return true;
    }

    let perf_enabled = std::env::var_os("LOCALSHOT_PERF").is_some();
    let perf_start = std::time::Instant::now();

    match capture_with_retry() {
        Ok(frame) => {
            let capture_ms = perf_start.elapsed().as_secs_f64() * 1000.0;
            let slint_img = frame.slint_image.clone();
            let (x, y, w, h) = (frame.x, frame.y, frame.width, frame.height);
            let mut guard = state_lock.lock();
            if let Some(state) = guard.as_mut() {
                state.reset(frame);
            } else {
                *guard = Some(AppState::new(frame));
            }
            drop(guard);

            daemon::set_overlay_active(true);
            clear_preview_layer(overlay);
            if let Some(img) = slint_img {
                overlay.set_bg_tiles(slint::ModelRc::default());
                overlay.set_background_image(img);
            } else {
                let overlay_weak = overlay.as_weak();
                let tiled_img = {
                    let guard = state_lock.lock();
                    guard.as_ref().map(|s| s.composited_image().clone())
                };
                if let Some(img) = tiled_img {
                    sync_overlay(&overlay_weak, &img);
                }
            }
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
            overlay.set_active_thickness(0);
            overlay.set_text_font_size(config::TEXT_DEFAULT_SIZE as f32);
            overlay.set_can_undo(false);
            overlay.set_can_redo(false);
            overlay.set_show_color_picker(false);
            overlay.set_text_editing(false);
            overlay.set_text_edit_value("".into());
            overlay.set_active_draw_color(slint::Color::from_argb_u8(
                255,
                config::PALETTE[0].rgba[0],
                config::PALETTE[0].rgba[1],
                config::PALETTE[0].rgba[2],
            ));
            overlay.set_loupe_enabled(cfg!(feature = "loupe"));
            overlay.set_loupe_visible(false);
            overlay.set_cursor_name("crosshair".into());

            position_and_show_overlay(overlay, x, y, w, h);
            if perf_enabled {
                let total_ms = perf_start.elapsed().as_secs_f64() * 1000.0;
                eprintln!(
                    "[PERF] capture_ms={:.2} overlay_ready_ms={:.2} size={}x{}",
                    capture_ms, total_ms, w, h
                );
            }
            true
        }
        Err(e) => {
            show_error_dialog(
                "LocalShot — Capture Failed",
                &format!("Could not capture screen:\n{}", e),
            );
            false
        }
    }
}

/// Captures the cursor's monitor, copies to clipboard, saves to default folder,
/// and shows a notification without opening the overlay.
fn perform_full_capture() {
    match capture_with_retry() {
        Ok(frame) => {
            let mut raw = frame.raw_image;
            drop(frame.slint_image);

            let copy_res = SystemClipboard.copy_image(&raw);
            let save_res = config::save_image_unique_in_dir(
                &raw,
                &config::default_save_dir(),
                &config::default_filename(),
            );
            raw.as_mut().zeroize();

            match (copy_res, save_res) {
                (Ok(()), Ok(path)) => {
                    show_info_notification(
                        "LocalShot — Full Screen Captured",
                        &format!("Copied to clipboard and saved to {}", path.display()),
                    );
                }
                (Err(ce), Ok(path)) => {
                    show_error_dialog(
                        "LocalShot — Partial Capture Warning",
                        &format!(
                            "Saved to {}, but failed to copy to clipboard:\n{}",
                            path.display(),
                            ce
                        ),
                    );
                }
                (Ok(()), Err(se)) => {
                    show_error_dialog(
                        "LocalShot — Partial Capture Warning",
                        &format!("Copied to clipboard, but failed to save file:\n{}", se),
                    );
                }
                (Err(ce), Err(se)) => {
                    show_error_dialog(
                        "LocalShot — Full Capture Failed",
                        &format!("Clipboard error: {}\nSave error: {}", ce, se),
                    );
                }
            }
        }
        Err(e) => {
            show_error_dialog(
                "LocalShot — Full Capture Failed",
                &format!("Could not capture screen:\n{}", e),
            );
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    install_panic_hook();

    #[cfg(target_os = "linux")]
    if std::env::var("WAYLAND_APP_ID").is_err() {
        std::env::set_var("WAYLAND_APP_ID", "localshot-overlay");
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli_action = parse_cli_args(args.iter().map(String::as_str));

    match cli_action {
        CliAction::CheckUpdates => {
            updater::check_for_updates_direct(true);
            return Ok(());
        }
        CliAction::WaylandHelp => {
            #[cfg(target_os = "linux")]
            linux_desktop::show_wayland_help_dialog();
            #[cfg(not(target_os = "linux"))]
            println!("{}", linux_desktop::wayland_shortcuts_help_text());
            return Ok(());
        }
        CliAction::Quit => {
            match daemon::send_ipc_command(IpcCommand::Quit) {
                Ok(_) => println!("LocalShot daemon signaled to quit."),
                Err(e) => println!("LocalShot daemon is not running ({}).", e),
            }
            return Ok(());
        }
        CliAction::Area => {
            if let Err(e) = spawn_detached_daemon_and_send(IpcCommand::CaptureArea) {
                show_error_dialog("LocalShot — Daemon Error", &e);
            }
            return Ok(());
        }
        CliAction::Full => {
            if let Err(e) = spawn_detached_daemon_and_send(IpcCommand::CaptureFull) {
                show_error_dialog("LocalShot — Daemon Error", &e);
            }
            return Ok(());
        }
        CliAction::Daemon | CliAction::Oneshot => {}
    }

    let is_oneshot = cli_action == CliAction::Oneshot;
    if !is_oneshot
        && daemon::check_or_signal_existing_instance(IpcCommand::Ping)
            == DaemonInitResult::AlreadyRunningSignaled
    {
        println!("LocalShot daemon is already running.");
        return Ok(());
    }

    let _daemon_state = if is_oneshot {
        None
    } else {
        Some(daemon::init_daemon())
    };

    // Spawn non-blocking background auto-update check (respects 24h cooldown)
    updater::spawn_background_update_check();

    let overlay = create_overlay_with_fallback()?;
    let state_lock: Arc<Mutex<Option<AppState>>> = Arc::new(Mutex::new(None));
    let last_save_dir: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let pending_text_pos: Arc<Mutex<Option<(i32, i32)>>> = Arc::new(Mutex::new(None));
    let overlay_weak = overlay.as_weak();

    // Setup global background daemon callbacks
    if !is_oneshot {
        {
            let overlay_weak = overlay_weak.clone();
            let state_lock = state_lock.clone();
            daemon::set_capture_callback(move || {
                let state_lock = state_lock.clone();
                let _ = overlay_weak.upgrade_in_event_loop(move |overlay| {
                    run_guarded("capture_callback", || {
                        perform_capture(&overlay, &state_lock);
                    });
                });
            });
        }

        daemon::set_full_capture_callback(|| {
            std::thread::spawn(|| {
                run_guarded("full_capture_callback", perform_full_capture);
            });
        });

        daemon::set_quit_callback(|| {
            let _ = slint::invoke_from_event_loop(|| {
                let _ = slint::quit_event_loop();
            });
        });

        daemon::set_update_callback(|| {
            std::thread::spawn(|| {
                run_guarded("update_callback", || {
                    updater::check_for_updates(true);
                });
            });
        });
    }

    // Helper closure to dismiss overlay, release image buffers, and sanitize state.
    // The daemon process stays alive on BOTH Windows and Linux until Quit.
    let dismiss = {
        let overlay_weak = overlay_weak.clone();
        let state_lock = state_lock.clone();
        let pending_text_pos = pending_text_pos.clone();
        move || {
            if let Some(overlay) = overlay_weak.upgrade() {
                let _ = overlay.hide();
                overlay.set_background_image(slint::Image::default());
                overlay.set_bg_tiles(slint::ModelRc::default());
                clear_preview_layer(&overlay);
                overlay.set_is_selecting(false);
                overlay.set_has_selection(false);
                overlay.set_show_toolbars(false);
                overlay.set_show_color_picker(false);
                overlay.set_text_editing(false);
                overlay.set_text_edit_value("".into());
                overlay.set_loupe_visible(false);
            }
            *pending_text_pos.lock() = None;
            {
                let mut guard = state_lock.lock();
                if let Some(state) = guard.as_mut() {
                    state.sanitize();
                }
                *guard = None;
            }
            report_memory_after_dismiss();
            daemon::set_overlay_active(false);
            if is_oneshot {
                let _ = slint::quit_event_loop();
            }
        }
    };

    // Copy action (Ctrl+C, Enter or Copy button)
    let copy_action = {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        let dismiss = dismiss.clone();
        move || {
            let crop = {
                let guard = state_lock.lock();
                guard.as_ref().and_then(|s| s.get_final_crop())
            };
            if let Some(mut cropped) = crop {
                if is_oneshot {
                    // Hide overlay before SetExtLinux::wait() so the screen isn't blocked
                    // while waiting for another application to claim clipboard ownership.
                    if let Some(overlay) = overlay_weak.upgrade() {
                        let _ = overlay.hide();
                    }
                }
                let res = if is_oneshot {
                    clipboard::copy_rgba_oneshot(&cropped)
                } else {
                    SystemClipboard
                        .copy_image(&cropped)
                        .map_err(|e| e.to_string())
                };
                cropped.as_mut().zeroize();
                match res {
                    Ok(()) => dismiss(),
                    Err(e) => {
                        show_error_dialog(
                            "LocalShot — Copy Failed",
                            &format!("Could not copy image to clipboard:\n{}", e),
                        );
                    }
                }
            }
        }
    };

    // Save action (Ctrl+S or Save button)
    let save_action = {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        let last_save_dir = last_save_dir.clone();
        let dismiss = dismiss.clone();
        move || {
            let crop = {
                let guard = state_lock.lock();
                guard.as_ref().and_then(|s| s.get_final_crop())
            };
            if let Some(mut cropped) = crop {
                // Hide overlay before opening rfd dialog so the dialog isn't obscured.
                if let Some(overlay) = overlay_weak.upgrade() {
                    let _ = overlay.hide();
                }

                let initial_dir = last_save_dir
                    .lock()
                    .clone()
                    .unwrap_or_else(config::default_save_dir);

                let dialog = rfd::FileDialog::new()
                    .set_title("Save Screenshot As")
                    .set_directory(initial_dir)
                    .set_file_name(config::default_filename())
                    .add_filter("PNG Image (*.png)", &["png"])
                    .add_filter("JPEG Image (*.jpg, *.jpeg)", &["jpg", "jpeg"])
                    .add_filter("BMP Image (*.bmp)", &["bmp"]);

                if let Some(selected_path) = dialog.save_file() {
                    let target_path = config::ensure_supported_extension(&selected_path);
                    if let Some(parent) = target_path.parent() {
                        if !parent.as_os_str().is_empty() {
                            *last_save_dir.lock() = Some(parent.to_path_buf());
                        }
                    }

                    dismiss();

                    std::thread::spawn(move || {
                        let save_res = config::save_image_atomic(&cropped, &target_path);
                        cropped.as_mut().zeroize();
                        if let Err(e) = save_res {
                            show_error_dialog(
                                "LocalShot — Save Failed",
                                &format!(
                                    "Could not save screenshot to {}:\n{}",
                                    target_path.display(),
                                    e
                                ),
                            );
                        }
                    });
                } else {
                    cropped.as_mut().zeroize();
                    // User canceled the dialog: restore the overlay with existing annotations.
                    if let Some(overlay) = overlay_weak.upgrade() {
                        let _ = overlay.show();
                        #[cfg(target_os = "windows")]
                        setup_windows_overlay();
                    }
                }
            }
        }
    };

    // Copy / Save / Close / Undo / Redo button bindings
    {
        let copy_action = copy_action.clone();
        overlay.on_copy_requested(move || {
            run_guarded("on_copy_requested", &copy_action);
        });
    }

    {
        let save_action = save_action.clone();
        overlay.on_save_requested(move || {
            run_guarded("on_save_requested", &save_action);
        });
    }

    {
        let dismiss = dismiss.clone();
        overlay.on_close_requested(move || {
            run_guarded("on_close_requested", &dismiss);
        });
    }

    // Helper to commit active inline text annotation if one is open
    let commit_pending_text = {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        let pending_text_pos = pending_text_pos.clone();
        move || {
            let Some(overlay) = overlay_weak.upgrade() else {
                return;
            };
            if !overlay.get_text_editing() {
                return;
            }
            let text = overlay.get_text_edit_value().to_string();
            overlay.set_text_editing(false);
            overlay.set_text_edit_value("".into());
            overlay.invoke_focus_key_handler();
            let pos = pending_text_pos.lock().take();

            if !text.trim().is_empty() {
                if let Some((px, py)) = pos {
                    let mut guard = state_lock.lock();
                    if let Some(state) = guard.as_mut() {
                        let ann = draw::Annotation::Text {
                            x: px,
                            y: py,
                            text,
                            color: state.active_color(),
                            font_size: state.thickness_for(Tool::Text) as f32,
                        };
                        state.commit_annotation(ann);
                        sync_overlay(&overlay_weak, state.composited_image());
                        sync_tool_and_history_ui(&overlay, state);
                    }
                }
            }
        }
    };

    // Undo requested
    {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        let commit_pending_text = commit_pending_text.clone();
        overlay.on_undo_requested(move || {
            run_guarded("on_undo_requested", || {
                commit_pending_text();
                let mut guard = state_lock.lock();
                if let Some(state) = guard.as_mut() {
                    if state.undo() {
                        sync_overlay(&overlay_weak, state.composited_image());
                    }
                    if let Some(overlay) = overlay_weak.upgrade() {
                        sync_tool_and_history_ui(&overlay, state);
                    }
                }
            });
        });
    }

    // Redo requested
    {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        let commit_pending_text = commit_pending_text.clone();
        overlay.on_redo_requested(move || {
            run_guarded("on_redo_requested", || {
                commit_pending_text();
                let mut guard = state_lock.lock();
                if let Some(state) = guard.as_mut() {
                    if state.redo() {
                        sync_overlay(&overlay_weak, state.composited_image());
                    }
                    if let Some(overlay) = overlay_weak.upgrade() {
                        sync_tool_and_history_ui(&overlay, state);
                    }
                }
            });
        });
    }

    // Tool selection
    {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        let commit_pending_text = commit_pending_text.clone();
        overlay.on_tool_selected(move |tool_str| {
            run_guarded("on_tool_selected", || {
                commit_pending_text();
                let tool = Tool::from_str_id(tool_str.as_str());
                let mut guard = state_lock.lock();
                if let Some(state) = guard.as_mut() {
                    state.set_tool(tool);
                    if let Some(overlay) = overlay_weak.upgrade() {
                        sync_tool_and_history_ui(&overlay, state);
                    }
                }
            });
        });
    }

    // Color selection
    {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        overlay.on_color_selected(move |idx| {
            run_guarded("on_color_selected", || {
                let mut guard = state_lock.lock();
                if let Some(state) = guard.as_mut() {
                    state.set_color_idx(idx as usize);
                    if let Some(overlay) = overlay_weak.upgrade() {
                        overlay.set_show_color_picker(false);
                        sync_tool_and_history_ui(&overlay, state);
                    }
                }
            });
        });
    }

    // Thickness delta (UI +/- and mouse wheel)
    {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        overlay.on_thickness_delta(move |delta| {
            run_guarded("on_thickness_delta", || {
                let mut guard = state_lock.lock();
                if let Some(state) = guard.as_mut() {
                    state.adjust_active_thickness(delta);
                    if let Some(overlay) = overlay_weak.upgrade() {
                        sync_tool_and_history_ui(&overlay, state);
                    }
                }
            });
        });
    }

    // Custom color HSV picker
    {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        overlay.on_custom_color_hsv_changed(move |h, s, v| {
            run_guarded("on_custom_color_hsv_changed", || {
                let rgba = config::hsv_to_rgba(h, s, v);
                let hue_rgba = config::hsv_to_rgba(h, 1.0, 1.0);
                let hex = config::format_hex_color(rgba);
                let mut guard = state_lock.lock();
                if let Some(state) = guard.as_mut() {
                    state.set_custom_color(rgba);
                    if let Some(overlay) = overlay_weak.upgrade() {
                        overlay.set_picker_h(h);
                        overlay.set_picker_s(s);
                        overlay.set_picker_v(v);
                        overlay.set_picker_hue_color(slint::Color::from_argb_u8(
                            255,
                            hue_rgba[0],
                            hue_rgba[1],
                            hue_rgba[2],
                        ));
                        overlay.set_custom_color(slint::Color::from_argb_u8(
                            255, rgba[0], rgba[1], rgba[2],
                        ));
                        overlay.set_custom_color_hex(hex.into());
                        overlay.set_active_color_idx(config::PALETTE.len() as i32);
                        sync_tool_and_history_ui(&overlay, state);
                    }
                }
            });
        });
    }

    // Custom color hex input
    {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        overlay.on_custom_color_hex_changed(move |hex_str| {
            run_guarded("on_custom_color_hex_changed", || {
                if let Some(rgba) = config::parse_hex_color(hex_str.as_str()) {
                    let (h, s, v) = config::rgba_to_hsv(rgba);
                    let hue_rgba = config::hsv_to_rgba(h, 1.0, 1.0);
                    let mut guard = state_lock.lock();
                    if let Some(state) = guard.as_mut() {
                        state.set_custom_color(rgba);
                        if let Some(overlay) = overlay_weak.upgrade() {
                            overlay.set_picker_h(h);
                            overlay.set_picker_s(s);
                            overlay.set_picker_v(v);
                            overlay.set_picker_hue_color(slint::Color::from_argb_u8(
                                255,
                                hue_rgba[0],
                                hue_rgba[1],
                                hue_rgba[2],
                            ));
                            overlay.set_custom_color(slint::Color::from_argb_u8(
                                255, rgba[0], rgba[1], rgba[2],
                            ));
                            overlay.set_active_color_idx(config::PALETTE.len() as i32);
                            sync_tool_and_history_ui(&overlay, state);
                        }
                    }
                }
            });
        });
    }

    // Inline text commit / cancel callbacks
    {
        let commit_pending_text = commit_pending_text.clone();
        overlay.on_text_commit_requested(move |_text| {
            run_guarded("on_text_commit_requested", &commit_pending_text);
        });
    }
    {
        let overlay_weak = overlay_weak.clone();
        let pending_text_pos = pending_text_pos.clone();
        overlay.on_text_cancel_requested(move || {
            run_guarded("on_text_cancel_requested", || {
                *pending_text_pos.lock() = None;
                if let Some(overlay) = overlay_weak.upgrade() {
                    overlay.set_text_editing(false);
                    overlay.set_text_edit_value("".into());
                    overlay.invoke_focus_key_handler();
                }
            });
        });
    }

    // Coalesced preview state: at most one preview render per display frame (~16 ms)
    #[derive(Default)]
    struct PreviewCoalescer {
        last_render: Option<std::time::Instant>,
        pending: Option<((i32, i32), bool)>,
    }
    let preview_coalescer = Arc::new(Mutex::new(PreviewCoalescer::default()));
    let preview_timer = Rc::new(slint::Timer::default());

    // Unified pointer/touch event handling (kind: down / move / up)
    {
        let state_lock = state_lock.clone();
        let overlay_weak = overlay_weak.clone();
        let preview_coalescer = preview_coalescer.clone();
        let preview_timer = preview_timer.clone();
        let commit_pending_text = commit_pending_text.clone();
        let pending_text_pos = pending_text_pos.clone();
        overlay.on_touch_event(move |kind, lx, ly, modifiers, pressed| {
            let Some(overlay) = overlay_weak.upgrade() else {
                return;
            };
            if kind.as_str() == "down" {
                if overlay.get_text_editing() {
                    commit_pending_text();
                }
                overlay.set_show_color_picker(false);
            }
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
                    let mut start_text_edit = false;
                    let mut text_bounds = (0.0_f32, 0.0_f32, 100.0_f32, 40.0_f32);
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
                            } else if state.tool == Tool::Text
                                && interaction::hit_test_selection(
                                    &state.selection,
                                    (phys_x, phys_y),
                                )
                            {
                                *pending_text_pos.lock() = Some((phys_x, phys_y));
                                let (sel_lx, sel_ly, sel_lw, sel_lh) =
                                    coord::physical_to_logical_rect(
                                        state.selection.x,
                                        state.selection.y,
                                        state.selection.w,
                                        state.selection.h,
                                        scale,
                                    );
                                let max_w = (sel_lx + sel_lw - lx).max(40.0);
                                let max_h = (sel_ly + sel_lh - ly).max(28.0);
                                text_bounds = (lx, ly, max_w, max_h);
                                start_text_edit = true;
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
                    if start_text_edit {
                        sync_tool_and_history_ui(&overlay, state);
                        drop(guard);
                        overlay.set_text_edit_x(text_bounds.0);
                        overlay.set_text_edit_y(text_bounds.1);
                        overlay.set_text_edit_max_w(text_bounds.2);
                        overlay.set_text_edit_max_h(text_bounds.3);
                        overlay.set_text_edit_value("".into());
                        overlay.set_text_editing(true);
                        overlay.invoke_focus_text_editor();
                    }
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
                            } else if state.tool == Tool::Text {
                                "text"
                            } else {
                                "crosshair"
                            }
                        } else {
                            "crosshair"
                        };
                        overlay.set_cursor_name(cursor.into());
                    }

                    #[cfg(feature = "loupe")]
                    {
                        let (bytes, lw_px, lh_px, hex) =
                            state::render_loupe_rgba(state.composited_image(), phys_x, phys_y);
                        let mut buf =
                            slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(lw_px, lh_px);
                        let dst = buf.make_mut_bytes();
                        let len = dst.len().min(bytes.len());
                        dst[..len].copy_from_slice(&bytes[..len]);
                        overlay.set_loupe_image(slint::Image::from_rgba8(buf));
                        let screen_lw = coord::physical_to_logical(frame_w as f32, scale);
                        let screen_lh = coord::physical_to_logical(frame_h as f32, scale);
                        let pos_x = if lx + 142.0 > screen_lw {
                            (lx - 134.0).max(4.0)
                        } else {
                            lx + 16.0
                        };
                        let pos_y = if ly + 160.0 > screen_lh {
                            (ly - 150.0).max(4.0)
                        } else {
                            ly + 16.0
                        };
                        overlay.set_loupe_x(pos_x);
                        overlay.set_loupe_y(pos_y);
                        overlay.set_loupe_text(format!("({}, {}) {}", phys_x, phys_y, hex).into());
                        overlay.set_loupe_visible(true);
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
                                let clamped_pt = interaction::clamp_point_to_selection(
                                    (phys_x, phys_y),
                                    &state.selection,
                                );
                                state.add_drawing_point(clamped_pt.0, clamped_pt.1);
                                let now = std::time::Instant::now();
                                let frame_interval = std::time::Duration::from_millis(16);
                                let should_render_now = {
                                    let mut coal = preview_coalescer.lock();
                                    let due = coal
                                        .last_render
                                        .map(|t| now.duration_since(t) >= frame_interval)
                                        .unwrap_or(true);
                                    if due {
                                        coal.last_render = Some(now);
                                        coal.pending = None;
                                        true
                                    } else {
                                        coal.pending = Some((clamped_pt, modifiers.shift));
                                        false
                                    }
                                };

                                if should_render_now {
                                    preview_timer.stop();
                                    if let Some((bx, by, bw, bh, rgba)) =
                                        state.render_preview_layer(clamped_pt, modifiers.shift)
                                    {
                                        sync_preview_layer(&overlay, bx, by, bw, bh, &rgba);
                                    }
                                } else if !preview_timer.running() {
                                    let state_lock_t = state_lock.clone();
                                    let overlay_weak_t = overlay_weak.clone();
                                    let coalescer_t = preview_coalescer.clone();
                                    preview_timer.start(
                                        slint::TimerMode::SingleShot,
                                        frame_interval,
                                        move || {
                                            let pending = {
                                                let mut coal = coalescer_t.lock();
                                                coal.last_render = Some(std::time::Instant::now());
                                                coal.pending.take()
                                            };
                                            if let Some((pt, shift)) = pending {
                                                let preview_data = {
                                                    let guard = state_lock_t.lock();
                                                    guard.as_ref().and_then(|s| {
                                                        if s.machine == SelectionMachine::Drawing {
                                                            s.render_preview_layer(pt, shift)
                                                        } else {
                                                            None
                                                        }
                                                    })
                                                };
                                                if let (Some(ov), Some((bx, by, bw, bh, rgba))) =
                                                    (overlay_weak_t.upgrade(), preview_data)
                                                {
                                                    sync_preview_layer(&ov, bx, by, bw, bh, &rgba);
                                                }
                                            }
                                        },
                                    );
                                }
                            }
                            _ => {}
                        }
                    }
                }
                "up" => {
                    match state.machine {
                        SelectionMachine::Selecting => {
                            if state.selection.w >= interaction::MIN_SELECTION_SIZE
                                && state.selection.h >= interaction::MIN_SELECTION_SIZE
                            {
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
                            preview_timer.stop();
                            {
                                let mut coal = preview_coalescer.lock();
                                coal.pending = None;
                                coal.last_render = None;
                            }
                            clear_preview_layer(&overlay);
                            let clamped_pt = interaction::clamp_point_to_selection(
                                (phys_x, phys_y),
                                &state.selection,
                            );
                            if let Some(ann) = state
                                .build_current_annotation_with_redact(clamped_pt, modifiers.shift)
                            {
                                state.commit_annotation(ann);
                                state.finish_drawing();
                                sync_overlay(&overlay_weak, state.composited_image());
                                sync_tool_and_history_ui(&overlay, state);
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
            if overlay.get_text_editing() {
                *pending_text_pos.lock() = None;
                overlay.set_text_editing(false);
                overlay.set_text_edit_value("".into());
                overlay.invoke_focus_key_handler();
                return;
            }
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
                    preview_timer.stop();
                    {
                        let mut coal = preview_coalescer.lock();
                        coal.pending = None;
                        coal.last_render = None;
                    }
                    clear_preview_layer(&overlay);
                    state.finish_drawing();
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
                            sync_tool_and_history_ui(&overlay, state);
                        }
                    }
                    'y' | 'z' => {
                        let mut guard = state_lock.lock();
                        if let Some(state) = guard.as_mut() {
                            if state.redo() {
                                sync_overlay(&overlay_weak, state.composited_image());
                            }
                            sync_tool_and_history_ui(&overlay, state);
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
                    't' => Some(Tool::Text),
                    _ => None,
                };
                if let Some(t) = tool {
                    let mut guard = state_lock.lock();
                    if let Some(state) = guard.as_mut() {
                        let new_tool = if state.tool == t { Tool::Select } else { t };
                        state.set_tool(new_tool);
                        overlay.set_active_tool(new_tool.as_str_id().into());
                        sync_tool_and_history_ui(&overlay, state);
                    }
                }
            }
        });
    }

    if is_oneshot {
        if !perform_capture(&overlay, &state_lock) {
            eprintln!(
                "LocalShot: Could not capture screen, aborting without opening blank window."
            );
            return Ok(());
        }
    } else {
        println!("LocalShot: Running silently in daemon mode.");
    }

    // Keep daemon alive on both Windows and Linux until explicit Quit.
    slint::run_event_loop_until_quit()?;

    {
        let mut guard = state_lock.lock();
        if let Some(state) = guard.as_mut() {
            state.sanitize();
        }
        *guard = None;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cli_args() {
        assert_eq!(parse_cli_args(std::iter::empty::<&str>()), CliAction::Area);
        assert_eq!(parse_cli_args(["--area"]), CliAction::Area);
        assert_eq!(parse_cli_args(["--full"]), CliAction::Full);
        assert_eq!(parse_cli_args(["--daemon"]), CliAction::Daemon);
        assert_eq!(parse_cli_args(["--background"]), CliAction::Daemon);
        assert_eq!(parse_cli_args(["--quit"]), CliAction::Quit);
        assert_eq!(parse_cli_args(["--check-updates"]), CliAction::CheckUpdates);
        assert_eq!(parse_cli_args(["--update"]), CliAction::CheckUpdates);
        assert_eq!(parse_cli_args(["--wayland-help"]), CliAction::WaylandHelp);
        assert_eq!(parse_cli_args(["--shortcuts-help"]), CliAction::WaylandHelp);
        assert_eq!(parse_cli_args(["--oneshot"]), CliAction::Oneshot);
        assert!(should_retry_capture(false));
        assert!(!should_retry_capture(true));
    }
}

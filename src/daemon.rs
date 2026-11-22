use interprocess::local_socket::{
    traits::{ListenerExt as _, Stream as _},
    GenericFilePath, ListenerOptions, Name, Stream as LocalSocketStream, ToFsName,
};
use parking_lot::Mutex;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq, Eq)]
pub enum DaemonInitResult {
    PrimaryInstance,
    AlreadyRunningSignaled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcCommand {
    CaptureArea,
    CaptureFull,
    Quit,
    Ping,
}

const MAX_IPC_LINE_BYTES: usize = 64;
const IPC_TIMEOUT: Duration = Duration::from_millis(500);
const TRIGGER_DEBOUNCE: Duration = Duration::from_millis(300);
#[cfg(target_os = "windows")]
const TRAY_HIDE_DELAY: Duration = Duration::from_millis(140);

static OVERLAY_ACTIVE: AtomicBool = AtomicBool::new(false);
static LAST_TRIGGER: Mutex<Option<Instant>> = Mutex::new(None);

pub fn is_overlay_active() -> bool {
    OVERLAY_ACTIVE.load(Ordering::SeqCst)
}

pub fn set_overlay_active(active: bool) {
    OVERLAY_ACTIVE.store(active, Ordering::SeqCst);
}

fn should_accept_trigger_at(last: &mut Option<Instant>, now: Instant, window: Duration) -> bool {
    if let Some(prev) = *last {
        if now.duration_since(prev) < window {
            return false;
        }
    }
    *last = Some(now);
    true
}

fn should_accept_trigger() -> bool {
    let mut guard = LAST_TRIGGER.lock();
    should_accept_trigger_at(&mut guard, Instant::now(), TRIGGER_DEBOUNCE)
}

// Store callbacks as Arc<dyn Fn()> so we can clone the Arc out of the
// lock before calling, preventing calling a callback while holding the lock.
type DaemonCallback = Arc<dyn Fn() + Send + Sync + 'static>;
static CAPTURE_AREA_CALLBACK: Mutex<Option<DaemonCallback>> = Mutex::new(None);
static CAPTURE_FULL_CALLBACK: Mutex<Option<DaemonCallback>> = Mutex::new(None);
static QUIT_CALLBACK: Mutex<Option<DaemonCallback>> = Mutex::new(None);
static UPDATE_CALLBACK: Mutex<Option<DaemonCallback>> = Mutex::new(None);

pub fn set_capture_callback<F: Fn() + Send + Sync + 'static>(cb: F) {
    *CAPTURE_AREA_CALLBACK.lock() = Some(Arc::new(cb));
}

pub fn set_full_capture_callback<F: Fn() + Send + Sync + 'static>(cb: F) {
    *CAPTURE_FULL_CALLBACK.lock() = Some(Arc::new(cb));
}

pub fn set_quit_callback<F: Fn() + Send + Sync + 'static>(cb: F) {
    *QUIT_CALLBACK.lock() = Some(Arc::new(cb));
}

#[allow(dead_code)]
pub fn set_update_callback<F: Fn() + Send + Sync + 'static>(cb: F) {
    *UPDATE_CALLBACK.lock() = Some(Arc::new(cb));
}

fn invoke_guarded(cb: Option<DaemonCallback>) {
    if let Some(cb) = cb {
        if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cb();
        })) {
            let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                *s
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.as_str()
            } else {
                "unknown panic payload"
            };
            eprintln!("LocalShot: Recovered from callback panic: {}", msg);
        }
    }
}

pub fn trigger_capture() {
    if !should_accept_trigger() {
        return;
    }
    let cb = CAPTURE_AREA_CALLBACK.lock().clone();
    invoke_guarded(cb);
}

pub fn trigger_full_capture() {
    if !should_accept_trigger() {
        return;
    }
    let cb = CAPTURE_FULL_CALLBACK.lock().clone();
    invoke_guarded(cb);
}

pub fn trigger_quit() {
    let cb = QUIT_CALLBACK.lock().clone();
    invoke_guarded(cb);
}

#[allow(dead_code)]
pub fn trigger_update_check() {
    let cb = UPDATE_CALLBACK.lock().clone();
    if cb.is_some() {
        invoke_guarded(cb);
    } else {
        std::thread::spawn(|| {
            let _ = std::panic::catch_unwind(|| {
                crate::updater::check_for_updates(true);
            });
        });
    }
}

pub fn parse_ipc_command(raw: &str) -> Result<IpcCommand, &'static str> {
    if raw.len() > MAX_IPC_LINE_BYTES {
        return Err("command exceeds 64 bytes");
    }
    let trimmed = raw.trim_end_matches(['\r', '\n']);
    match trimmed {
        "CAPTURE_AREA" => Ok(IpcCommand::CaptureArea),
        "CAPTURE_FULL" => Ok(IpcCommand::CaptureFull),
        "QUIT" => Ok(IpcCommand::Quit),
        "PING" => Ok(IpcCommand::Ping),
        _ => Err("unknown command"),
    }
}

impl IpcCommand {
    pub fn as_wire_str(self) -> &'static str {
        match self {
            Self::CaptureArea => "CAPTURE_AREA",
            Self::CaptureFull => "CAPTURE_FULL",
            Self::Quit => "QUIT",
            Self::Ping => "PING",
        }
    }
}

#[cfg(unix)]
fn current_uid() -> u32 {
    extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid() }
}

#[derive(Debug, Clone)]
pub struct IpcEndpoint {
    pub raw: String,
    pub unix_path: Option<PathBuf>,
    pub verify_tmp_owner: bool,
}

impl IpcEndpoint {
    pub fn to_socket_name(&self) -> std::io::Result<Name<'_>> {
        self.raw.as_str().to_fs_name::<GenericFilePath>()
    }
}

#[cfg(target_os = "windows")]
fn windows_username_and_session() -> (String, u32) {
    let raw_user = std::env::var("USERNAME").unwrap_or_else(|_| "user".to_string());
    let sanitized: String = raw_user
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let user = if sanitized.is_empty() {
        "user".to_string()
    } else {
        sanitized
    };

    extern "system" {
        fn GetCurrentProcessId() -> u32;
        fn ProcessIdToSessionId(dwProcessId: u32, pSessionId: *mut u32) -> i32;
    }
    let mut session_id: u32 = 0;
    unsafe {
        let pid = GetCurrentProcessId();
        let _ = ProcessIdToSessionId(pid, &mut session_id);
    }
    (user, session_id)
}

pub fn default_ipc_endpoint() -> IpcEndpoint {
    #[cfg(target_os = "windows")]
    {
        let (user, session_id) = windows_username_and_session();
        IpcEndpoint {
            raw: format!(r"\\.\pipe\localshot-{}-{}", user, session_id),
            unix_path: None,
            verify_tmp_owner: false,
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
            let trimmed = runtime_dir.trim();
            if !trimmed.is_empty() {
                let path = PathBuf::from(trimmed).join("localshot.sock");
                return IpcEndpoint {
                    raw: path.to_string_lossy().into_owned(),
                    unix_path: Some(path),
                    verify_tmp_owner: false,
                };
            }
        }
        let uid = current_uid();
        let path = PathBuf::from(format!("/tmp/localshot-{}.sock", uid));
        IpcEndpoint {
            raw: path.to_string_lossy().into_owned(),
            unix_path: Some(path),
            verify_tmp_owner: true,
        }
    }
}

#[cfg(unix)]
fn ensure_unix_socket_owner(path: &Path, verify_owner: bool) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    if !verify_owner || !path.exists() {
        return Ok(());
    }
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing to use symlinked socket path",
        ));
    }
    let expected_uid = current_uid();
    if meta.uid() != expected_uid {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "socket file is owned by another user",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_unix_socket_owner(_path: &Path, _verify_owner: bool) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn apply_unix_socket_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn apply_unix_socket_permissions(_path: &Path) {}

fn read_ipc_line_bounded(stream: &mut LocalSocketStream) -> Result<String, &'static str> {
    let _ = stream.set_recv_timeout(Some(IPC_TIMEOUT));
    let _ = stream.set_send_timeout(Some(IPC_TIMEOUT));
    let mut reader = BufReader::new(stream.take((MAX_IPC_LINE_BYTES + 2) as u64));
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => Err("empty command"),
        Ok(_) => {
            if !line.ends_with('\n') && line.len() > MAX_IPC_LINE_BYTES {
                return Err("command exceeds 64 bytes");
            }
            Ok(line)
        }
        Err(_) => Err("read error or timeout"),
    }
}

fn handle_ipc_client<F>(mut stream: LocalSocketStream, on_cmd: &F)
where
    F: Fn(IpcCommand) + Send + Sync,
{
    let response = match read_ipc_line_bounded(&mut stream) {
        Ok(line) => match parse_ipc_command(&line) {
            Ok(IpcCommand::Ping) => "OK PONG\n",
            Ok(cmd) => {
                on_cmd(cmd);
                "OK\n"
            }
            Err(_) => "ERR\n",
        },
        Err(_) => "ERR\n",
    };
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

pub fn send_ipc_command_at(endpoint: &IpcEndpoint, cmd: IpcCommand) -> std::io::Result<String> {
    if let Some(path) = &endpoint.unix_path {
        ensure_unix_socket_owner(path, endpoint.verify_tmp_owner)?;
    }
    let name = endpoint.to_socket_name()?;
    let mut stream = LocalSocketStream::connect(name)?;
    let _ = stream.set_recv_timeout(Some(IPC_TIMEOUT));
    let _ = stream.set_send_timeout(Some(IPC_TIMEOUT));

    let msg = format!("{}\n", cmd.as_wire_str());
    stream.write_all(msg.as_bytes())?;
    stream.flush()?;

    let mut reader = BufReader::new(stream.take(128));
    let mut resp = String::new();
    let _ = reader.read_line(&mut resp);
    Ok(resp.trim().to_string())
}

pub fn send_ipc_command(cmd: IpcCommand) -> std::io::Result<String> {
    send_ipc_command_at(&default_ipc_endpoint(), cmd)
}

pub fn bind_ipc_listener_at<F>(
    endpoint: &IpcEndpoint,
    on_cmd: F,
) -> std::io::Result<std::thread::JoinHandle<()>>
where
    F: Fn(IpcCommand) + Send + Sync + 'static,
{
    if let Some(path) = &endpoint.unix_path {
        ensure_unix_socket_owner(path, endpoint.verify_tmp_owner)?;
        if path.exists() {
            // Detect stale socket: if PING fails, remove stale socket and rebind.
            if send_ipc_command_at(endpoint, IpcCommand::Ping).is_err() {
                let _ = std::fs::remove_file(path);
            } else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    "daemon already listening on socket",
                ));
            }
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
    }

    let name = endpoint.to_socket_name()?;
    let listener = ListenerOptions::new().name(name).create_sync()?;

    if let Some(path) = &endpoint.unix_path {
        apply_unix_socket_permissions(path);
    }

    let handle = std::thread::Builder::new()
        .name("localshot-ipc".into())
        .spawn(move || {
            for conn in listener.incoming() {
                match conn {
                    Ok(stream) => {
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            handle_ipc_client(stream, &on_cmd);
                        }));
                    }
                    Err(_) => break,
                }
            }
        })?;

    Ok(handle)
}

pub fn check_or_signal_existing_instance(cmd: IpcCommand) -> DaemonInitResult {
    let endpoint = default_ipc_endpoint();
    if send_ipc_command_at(&endpoint, cmd).is_ok() {
        return DaemonInitResult::AlreadyRunningSignaled;
    }

    let bind_res = bind_ipc_listener_at(&endpoint, |ipc_cmd| match ipc_cmd {
        IpcCommand::CaptureArea => trigger_capture(),
        IpcCommand::CaptureFull => trigger_full_capture(),
        IpcCommand::Quit => trigger_quit(),
        IpcCommand::Ping => {}
    });

    match bind_res {
        Ok(_handle) => DaemonInitResult::PrimaryInstance,
        Err(_) => {
            if send_ipc_command_at(&endpoint, cmd).is_ok() {
                DaemonInitResult::AlreadyRunningSignaled
            } else {
                DaemonInitResult::PrimaryInstance
            }
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
pub fn generate_tray_icon_rgba(size: u32, light_taskbar: bool) -> Vec<u8> {
    let size = size.max(1);
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;

    for y in 0..size {
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            let nx = (x as f32 + 0.5) / s;
            let ny = (y as f32 + 0.5) / s;
            let dx = nx - 0.5;
            let dy = ny - 0.5;
            let dist = (dx * dx + dy * dy).sqrt();

            let margin = 0.08;
            let radius = 0.14;
            let in_box =
                (margin..=(1.0 - margin)).contains(&nx) && (margin..=(1.0 - margin)).contains(&ny);

            let cx = nx.clamp(margin + radius, 1.0 - margin - radius);
            let cy = ny.clamp(margin + radius, 1.0 - margin - radius);
            let corner_dist = ((nx - cx).powi(2) + (ny - cy).powi(2)).sqrt();

            if in_box && corner_dist <= radius {
                let border = corner_dist > radius - 0.06
                    || nx < margin + 0.05
                    || nx > 1.0 - margin - 0.05
                    || ny < margin + 0.05
                    || ny > 1.0 - margin - 0.05;

                if light_taskbar {
                    if border {
                        rgba[idx] = 15;
                        rgba[idx + 1] = 23;
                        rgba[idx + 2] = 42;
                        rgba[idx + 3] = 255;
                    } else {
                        let t = (nx + ny) * 0.5;
                        rgba[idx] = (55.0 * (1.0 - t) + 190.0 * t) as u8;
                        rgba[idx + 1] = (48.0 * (1.0 - t) + 24.0 * t) as u8;
                        rgba[idx + 2] = (163.0 * (1.0 - t) + 93.0 * t) as u8;
                        rgba[idx + 3] = 255;
                    }
                } else {
                    let t = (nx + ny) * 0.5;
                    rgba[idx] = (79.0 * (1.0 - t) + 219.0 * t) as u8;
                    rgba[idx + 1] = (70.0 * (1.0 - t) + 39.0 * t) as u8;
                    rgba[idx + 2] = (229.0 * (1.0 - t) + 119.0 * t) as u8;
                    rgba[idx + 3] = 255;
                }

                if dist <= 0.28 {
                    rgba[idx] = 56;
                    rgba[idx + 1] = 189;
                    rgba[idx + 2] = 248;
                    rgba[idx + 3] = 255;
                }
                if dist <= 0.17 {
                    rgba[idx] = 15;
                    rgba[idx + 1] = 23;
                    rgba[idx + 2] = 42;
                    rgba[idx + 3] = 255;
                }
                let glint = ((nx - 0.42).powi(2) + (ny - 0.42).powi(2)).sqrt();
                if glint <= 0.055 {
                    rgba[idx] = 255;
                    rgba[idx + 1] = 255;
                    rgba[idx + 2] = 255;
                    rgba[idx + 3] = 255;
                }
            }
        }
    }
    rgba
}

#[cfg(target_os = "linux")]
fn generate_tray_icon() -> Option<tray_icon::Icon> {
    let size = 32u32;
    let rgba = generate_tray_icon_rgba(size, false);
    tray_icon::Icon::from_rgba(rgba, size, size).ok()
}

/// Formats the HKCU Run command value for a given executable path.
#[allow(dead_code)]
pub fn format_autostart_command(exe_path: &Path) -> String {
    format!("\"{}\"", exe_path.display())
}

/// Selects the tray tooltip text based on which global hotkeys registered successfully.
#[allow(dead_code)]
pub fn tray_tooltip_for_hotkeys(prtscn_ok: bool, ctrl_alt_s_ok: bool) -> &'static str {
    if prtscn_ok {
        "LocalShot (PrtScn)"
    } else if ctrl_alt_s_ok {
        "LocalShot (Ctrl+Alt+S)"
    } else {
        "LocalShot (Tray)"
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::*;
    use global_hotkey::{
        hotkey::{Code, HotKey, Modifiers},
        GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    };
    use muda::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    const AUTOSTART_RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const AUTOSTART_VALUE_NAME: &str = "LocalShot";
    const PERSONALIZE_SUBKEY: &str =
        r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

    pub struct DaemonState {
        pub _hotkey_manager: Option<GlobalHotKeyManager>,
        pub _tray_icon: Option<TrayIcon>,
    }

    fn windows_tray_icon_size() -> u32 {
        extern "system" {
            fn GetSystemMetrics(nIndex: i32) -> i32;
        }
        const SM_CXSMICON: i32 = 49;
        let metric = unsafe { GetSystemMetrics(SM_CXSMICON) };
        if metric > 0 {
            (metric as u32).clamp(16, 64)
        } else {
            32
        }
    }

    fn is_windows_light_taskbar() -> bool {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(key) = hkcu.open_subkey(PERSONALIZE_SUBKEY) {
            if let Ok(val) = key.get_value::<u32, _>("SystemUsesLightTheme") {
                return val == 1;
            }
        }
        false
    }

    fn load_tray_icon() -> Option<Icon> {
        let size = windows_tray_icon_size();
        let light = is_windows_light_taskbar();
        let rgba = generate_tray_icon_rgba(size, light);
        Icon::from_rgba(rgba, size, size)
            .ok()
            .or_else(|| Icon::from_resource(1, Some((size, size))).ok())
    }

    fn is_autostart_enabled() -> bool {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let Ok(run_key) = hkcu.open_subkey(AUTOSTART_RUN_SUBKEY) else {
            return false;
        };
        run_key
            .get_value::<String, _>(AUTOSTART_VALUE_NAME)
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
    }

    fn set_autostart_enabled(enabled: bool) -> std::io::Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if enabled {
            let exe = std::env::current_exe()?;
            let (run_key, _) = hkcu.create_subkey(AUTOSTART_RUN_SUBKEY)?;
            run_key.set_value(AUTOSTART_VALUE_NAME, &format_autostart_command(&exe))?;
        } else if let Ok(run_key) =
            hkcu.open_subkey_with_flags(AUTOSTART_RUN_SUBKEY, winreg::enums::KEY_SET_VALUE)
        {
            match run_key.delete_value(AUTOSTART_VALUE_NAME) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn notify_printscreen_conflict_once() {
        let mut cfg = crate::config::load_user_config();
        if cfg.hotkey_conflict_warned {
            return;
        }
        cfg.hotkey_conflict_warned = true;
        let _ = crate::config::save_user_config(&cfg);

        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(500));
            let _ = slint::invoke_from_event_loop(|| {
                rfd::MessageDialog::new()
                    .set_title("LocalShot - PrintScreen Hotkey In Use")
                    .set_description(
                        "PrintScreen is already registered by another app (such as Windows Snipping Tool or Lightshot).\n\n\
                        Ctrl+Alt+S remains active for taking screenshots.\n\n\
                        Windows 11 hint: To free PrintScreen, open Settings > Accessibility > Keyboard and disable \"Use the Print screen key to open screen capture\".",
                    )
                    .set_buttons(rfd::MessageButtons::Ok)
                    .set_level(rfd::MessageLevel::Info)
                    .show();
            });
        });
    }

    pub fn init_daemon() -> DaemonState {
        // Register Global Hotkeys (PrintScreen and Ctrl+Alt+S) and check results
        let mut prtscn_ok = false;
        let mut ctrl_alt_s_ok = false;
        let hotkey_manager = if let Ok(manager) = GlobalHotKeyManager::new() {
            let prtscn = HotKey::new(None, Code::PrintScreen);
            let ctrl_alt_s = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyS);
            prtscn_ok = manager.register(prtscn).is_ok();
            ctrl_alt_s_ok = manager.register(ctrl_alt_s).is_ok();
            if !prtscn_ok {
                eprintln!(
                    "LocalShot: PrintScreen hotkey is already in use; keeping Ctrl+Alt+S active."
                );
            }
            if !ctrl_alt_s_ok {
                eprintln!("LocalShot: Failed to register Ctrl+Alt+S hotkey.");
            }
            Some(manager)
        } else {
            None
        };

        if !prtscn_ok {
            notify_printscreen_conflict_once();
        }

        // Handle hotkey press consistently and ignore key repeat via 300ms debounce + overlay guard.
        GlobalHotKeyEvent::set_event_handler(Some(|event: GlobalHotKeyEvent| {
            let _ = std::panic::catch_unwind(|| {
                if event.state == HotKeyState::Pressed {
                    trigger_capture();
                }
            });
        }));

        // Tray Menu & Icon
        let user_cfg = crate::config::load_user_config();
        let autostart_initial = is_autostart_enabled();
        let tray_menu = Menu::new();
        let capture_i = MenuItem::new("Take Screenshot (PrtScn)", true, None);
        let full_i = MenuItem::new("Capture Full Screen", true, None);
        let autostart_i = CheckMenuItem::new("Start with Windows", true, autostart_initial, None);
        let auto_update_i = CheckMenuItem::new(
            "Auto-check for Updates",
            true,
            user_cfg.auto_update_check,
            None,
        );
        let update_i = MenuItem::new("Check for Updates...", true, None);
        let quit_i = MenuItem::new("Quit LocalShot", true, None);
        let _ = tray_menu.append_items(&[
            &capture_i,
            &full_i,
            &PredefinedMenuItem::separator(),
            &autostart_i,
            &auto_update_i,
            &PredefinedMenuItem::separator(),
            &update_i,
            &PredefinedMenuItem::separator(),
            &quit_i,
        ]);

        let capture_id = capture_i.id().clone();
        let full_id = full_i.id().clone();
        let autostart_id = autostart_i.id().clone();
        let auto_update_id = auto_update_i.id().clone();
        let update_id = update_i.id().clone();
        let quit_id = quit_i.id().clone();

        let autostart_state = Arc::new(AtomicBool::new(autostart_initial));
        let auto_update_state = Arc::new(AtomicBool::new(user_cfg.auto_update_check));

        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if event.id == capture_id {
                    std::thread::spawn(|| {
                        std::thread::sleep(TRAY_HIDE_DELAY);
                        trigger_capture();
                    });
                } else if event.id == full_id {
                    std::thread::spawn(|| {
                        std::thread::sleep(TRAY_HIDE_DELAY);
                        trigger_full_capture();
                    });
                } else if event.id == autostart_id {
                    let want_enabled = !autostart_state.load(Ordering::SeqCst);
                    if let Err(e) = set_autostart_enabled(want_enabled) {
                        eprintln!("LocalShot: Failed to update Windows autostart: {}", e);
                    } else {
                        autostart_state.store(want_enabled, Ordering::SeqCst);
                    }
                } else if event.id == auto_update_id {
                    let want_enabled = !auto_update_state.load(Ordering::SeqCst);
                    auto_update_state.store(want_enabled, Ordering::SeqCst);
                    let mut cfg = crate::config::load_user_config();
                    cfg.auto_update_check = want_enabled;
                    let _ = crate::config::save_user_config(&cfg);
                } else if event.id == update_id {
                    trigger_update_check();
                } else if event.id == quit_id {
                    trigger_quit();
                }
            }));
        }));

        TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
            let _ = std::panic::catch_unwind(|| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    trigger_capture();
                }
            });
        }));

        let tooltip = tray_tooltip_for_hotkeys(prtscn_ok, ctrl_alt_s_ok);
        let mut builder = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_menu_on_left_click(false)
            .with_tooltip(tooltip);
        if let Some(icon) = load_tray_icon() {
            builder = builder.with_icon(icon);
        }
        let tray_icon = builder.build().ok();

        DaemonState {
            _hotkey_manager: hotkey_manager,
            _tray_icon: tray_icon,
        }
    }
}

#[cfg(target_os = "windows")]
pub use windows_impl::*;

#[cfg(target_os = "linux")]
mod linux_impl {
    use super::*;
    use crate::linux_desktop::{self, LinuxSession};
    use global_hotkey::{
        hotkey::{Code, HotKey, Modifiers},
        GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    };
    use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use std::sync::mpsc;
    use tray_icon::TrayIconBuilder;

    pub struct DaemonState {
        pub _hotkey_manager: Option<GlobalHotKeyManager>,
    }

    #[derive(Debug, Clone, Copy)]
    enum TrayAction {
        CaptureArea,
        CaptureFull,
        WaylandHelp,
        CheckUpdates,
        Quit,
    }

    fn spawn_linux_tray_thread() {
        let (tx, rx) = mpsc::channel::<TrayAction>();

        let _ = std::thread::Builder::new()
            .name("localshot-tray-dispatch".into())
            .spawn(move || {
                while let Ok(action) = rx.recv() {
                    match action {
                        TrayAction::CaptureArea => {
                            std::thread::sleep(std::time::Duration::from_millis(140));
                            trigger_capture();
                        }
                        TrayAction::CaptureFull => {
                            std::thread::sleep(std::time::Duration::from_millis(140));
                            trigger_full_capture();
                        }
                        TrayAction::WaylandHelp => {
                            std::thread::spawn(linux_desktop::show_wayland_help_dialog);
                        }
                        TrayAction::CheckUpdates => {
                            trigger_update_check();
                        }
                        TrayAction::Quit => {
                            trigger_quit();
                        }
                    }
                }
            });

        let _ = std::thread::Builder::new()
            .name("localshot-gtk-tray".into())
            .spawn(move || {
                if let Err(e) = gtk::init() {
                    eprintln!(
                        "LocalShot: GTK initialization failed ({}); continuing without tray icon.",
                        e
                    );
                    return;
                }

                let is_wayland = linux_desktop::is_wayland_session();
                let updater_enabled = !crate::updater::is_updater_disabled();

                let tray_menu = Menu::new();
                let capture_i = MenuItem::new("Take Screenshot (PrtScn)", true, None);
                let full_i = MenuItem::new("Capture Full Screen", true, None);
                let wayland_help_i = if is_wayland {
                    Some(MenuItem::new("Wayland Shortcuts Help...", true, None))
                } else {
                    None
                };
                let update_i = if updater_enabled {
                    Some(MenuItem::new("Check for Updates...", true, None))
                } else {
                    None
                };
                let quit_i = MenuItem::new("Quit LocalShot", true, None);

                let _ = tray_menu.append(&capture_i);
                let _ = tray_menu.append(&full_i);
                if let Some(ref item) = wayland_help_i {
                    let _ = tray_menu.append(&PredefinedMenuItem::separator());
                    let _ = tray_menu.append(item);
                }
                if let Some(ref item) = update_i {
                    let _ = tray_menu.append(&PredefinedMenuItem::separator());
                    let _ = tray_menu.append(item);
                }
                let _ = tray_menu.append(&PredefinedMenuItem::separator());
                let _ = tray_menu.append(&quit_i);

                let capture_id = capture_i.id().clone();
                let full_id = full_i.id().clone();
                let wayland_help_id = wayland_help_i.as_ref().map(|i| i.id().clone());
                let update_id = update_i.as_ref().map(|i| i.id().clone());
                let quit_id = quit_i.id().clone();

                MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                    let _ = std::panic::catch_unwind(|| {
                        if event.id == capture_id {
                            let _ = tx.send(TrayAction::CaptureArea);
                        } else if event.id == full_id {
                            let _ = tx.send(TrayAction::CaptureFull);
                        } else if wayland_help_id.as_ref() == Some(&event.id) {
                            let _ = tx.send(TrayAction::WaylandHelp);
                        } else if update_id.as_ref() == Some(&event.id) {
                            let _ = tx.send(TrayAction::CheckUpdates);
                        } else if event.id == quit_id {
                            let _ = tx.send(TrayAction::Quit);
                        }
                    });
                }));

                let mut builder = TrayIconBuilder::new()
                    .with_menu(Box::new(tray_menu))
                    .with_tooltip("LocalShot (PrtScn)");
                if let Some(icon) = generate_tray_icon() {
                    builder = builder.with_icon(icon);
                }

                match builder.build() {
                    Ok(_tray_icon) => {
                        gtk::main();
                    }
                    Err(e) => {
                        eprintln!(
                            "LocalShot: Tray icon unavailable ({}); continuing without tray.",
                            e
                        );
                    }
                }
            });
    }

    pub fn init_daemon() -> DaemonState {
        let session = linux_desktop::detect_session();
        let hotkey_manager = if session == LinuxSession::X11 {
            match GlobalHotKeyManager::new() {
                Ok(manager) => {
                    let prtscn = HotKey::new(None, Code::PrintScreen);
                    let ctrl_alt_s =
                        HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyS);
                    let shift_prtscn = HotKey::new(Some(Modifiers::SHIFT), Code::PrintScreen);
                    let full_id = shift_prtscn.id();

                    if let Err(e) = manager.register(prtscn) {
                        eprintln!("LocalShot: Failed to register PrintScreen hotkey: {}", e);
                    }
                    if let Err(e) = manager.register(ctrl_alt_s) {
                        eprintln!("LocalShot: Failed to register Ctrl+Alt+S hotkey: {}", e);
                    }
                    if let Err(e) = manager.register(shift_prtscn) {
                        eprintln!(
                            "LocalShot: Failed to register Shift+PrintScreen hotkey: {}",
                            e
                        );
                    }

                    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
                        let _ = std::panic::catch_unwind(|| {
                            if event.state == HotKeyState::Pressed {
                                if event.id == full_id {
                                    trigger_full_capture();
                                } else {
                                    trigger_capture();
                                }
                            }
                        });
                    }));
                    Some(manager)
                }
                Err(e) => {
                    eprintln!(
                        "LocalShot: X11 global hotkeys unavailable ({}); use CLI or tray.",
                        e
                    );
                    None
                }
            }
        } else {
            if session == LinuxSession::Wayland {
                #[cfg(feature = "portal-shortcuts")]
                linux_desktop::try_register_portal_shortcuts();
                linux_desktop::maybe_show_wayland_help_once();
            }
            None
        };

        spawn_linux_tray_thread();

        DaemonState {
            _hotkey_manager: hotkey_manager,
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux_impl::*;

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod non_windows_impl {
    pub struct DaemonState;

    pub fn init_daemon() -> DaemonState {
        DaemonState
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub use non_windows_impl::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn unique_test_endpoint(tag: &str) -> IpcEndpoint {
        let pid = std::process::id();
        #[cfg(target_os = "windows")]
        {
            IpcEndpoint {
                raw: format!(r"\\.\pipe\localshot-test-{}-{}", tag, pid),
                unix_path: None,
                verify_tmp_owner: false,
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let path = std::env::temp_dir().join(format!("localshot_test_{}_{}.sock", tag, pid));
            IpcEndpoint {
                raw: path.to_string_lossy().into_owned(),
                unix_path: Some(path),
                verify_tmp_owner: true,
            }
        }
    }

    #[test]
    fn test_parse_ipc_command_valid_and_invalid() {
        assert_eq!(
            parse_ipc_command("CAPTURE_AREA\n"),
            Ok(IpcCommand::CaptureArea)
        );
        assert_eq!(
            parse_ipc_command("CAPTURE_FULL\r\n"),
            Ok(IpcCommand::CaptureFull)
        );
        assert_eq!(parse_ipc_command("QUIT"), Ok(IpcCommand::Quit));
        assert_eq!(parse_ipc_command("PING\n"), Ok(IpcCommand::Ping));

        assert!(parse_ipc_command("").is_err());
        assert!(parse_ipc_command("SNAP").is_err());
        assert!(parse_ipc_command("UNKNOWN_CMD\n").is_err());
        let too_long = "A".repeat(65);
        assert!(parse_ipc_command(&too_long).is_err());
    }

    #[test]
    fn test_trigger_debounce() {
        let mut last = None;
        let t0 = Instant::now();
        assert!(should_accept_trigger_at(&mut last, t0, TRIGGER_DEBOUNCE));
        assert!(!should_accept_trigger_at(
            &mut last,
            t0 + Duration::from_millis(100),
            TRIGGER_DEBOUNCE
        ));
        assert!(should_accept_trigger_at(
            &mut last,
            t0 + Duration::from_millis(350),
            TRIGGER_DEBOUNCE
        ));
    }

    #[test]
    fn test_ipc_two_instances_and_bad_input() {
        let ep = unique_test_endpoint("two_inst");
        if let Some(p) = &ep.unix_path {
            let _ = std::fs::remove_file(p);
        }

        let (tx, rx) = mpsc::channel();
        let _handle = bind_ipc_listener_at(&ep, move |cmd| {
            let _ = tx.send(cmd);
        })
        .expect("bind test listener");

        assert_eq!(
            send_ipc_command_at(&ep, IpcCommand::Ping).expect("ping"),
            "OK PONG"
        );
        assert_eq!(
            send_ipc_command_at(&ep, IpcCommand::CaptureArea).expect("capture area"),
            "OK"
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(1)).expect("recv area"),
            IpcCommand::CaptureArea
        );
        assert_eq!(
            send_ipc_command_at(&ep, IpcCommand::CaptureFull).expect("capture full"),
            "OK"
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(1)).expect("recv full"),
            IpcCommand::CaptureFull
        );

        // Raw bad input must be rejected with ERR
        let name = ep.to_socket_name().expect("name");
        let mut stream = LocalSocketStream::connect(name).expect("connect raw");
        stream.write_all(b"BAD_COMMAND\n").expect("write bad");
        stream.flush().expect("flush");
        let mut resp = String::new();
        BufReader::new(stream)
            .read_line(&mut resp)
            .expect("read resp");
        assert_eq!(resp.trim(), "ERR");

        if let Some(p) = &ep.unix_path {
            let _ = std::fs::remove_file(p);
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_ipc_stale_socket_rebind() {
        let ep = unique_test_endpoint("stale");
        let path = ep.unix_path.as_ref().expect("unix path");
        let _ = std::fs::remove_file(path);

        // Create a stale socket file (ordinary dead file or unbound listener)
        std::fs::write(path, b"").expect("create stale file");
        assert!(path.exists());

        let _handle = bind_ipc_listener_at(&ep, |_| {}).expect("should remove stale and rebind");
        assert_eq!(
            send_ipc_command_at(&ep, IpcCommand::Ping).expect("ping after rebind"),
            "OK PONG"
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_tray_icon_sizes_and_theme_variants() {
        for size in [16u32, 20, 24, 32] {
            let dark = generate_tray_icon_rgba(size, false);
            let light = generate_tray_icon_rgba(size, true);
            assert_eq!(dark.len(), (size * size * 4) as usize);
            assert_eq!(light.len(), (size * size * 4) as usize);
            assert_ne!(dark, light);
            assert!(dark.iter().any(|&b| b != 0));
            assert!(light.iter().any(|&b| b != 0));
        }
    }

    #[test]
    fn test_autostart_command_and_hotkey_tooltip() {
        assert_eq!(
            format_autostart_command(Path::new(r"C:\Program Files\LocalShot\localshot.exe")),
            "\"C:\\Program Files\\LocalShot\\localshot.exe\""
        );
        assert_eq!(tray_tooltip_for_hotkeys(true, true), "LocalShot (PrtScn)");
        assert_eq!(
            tray_tooltip_for_hotkeys(false, true),
            "LocalShot (Ctrl+Alt+S)"
        );
        assert_eq!(tray_tooltip_for_hotkeys(false, false), "LocalShot (Tray)");
    }
}

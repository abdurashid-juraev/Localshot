use std::sync::Mutex;

#[derive(Debug, PartialEq, Eq)]
pub enum DaemonInitResult {
    PrimaryInstance,
    AlreadyRunningSignaled,
}

type DaemonCallback = Box<dyn Fn() + Send + Sync + 'static>;
static CAPTURE_CALLBACK: Mutex<Option<DaemonCallback>> = Mutex::new(None);
static QUIT_CALLBACK: Mutex<Option<DaemonCallback>> = Mutex::new(None);

pub fn set_capture_callback<F: Fn() + Send + Sync + 'static>(cb: F) {
    *CAPTURE_CALLBACK.lock().unwrap() = Some(Box::new(cb));
}

pub fn set_quit_callback<F: Fn() + Send + Sync + 'static>(cb: F) {
    *QUIT_CALLBACK.lock().unwrap() = Some(Box::new(cb));
}

#[allow(dead_code)]
pub fn trigger_capture() {
    if let Some(cb) = CAPTURE_CALLBACK.lock().unwrap().as_ref() {
        cb();
    }
}

#[allow(dead_code)]
pub fn trigger_quit() {
    if let Some(cb) = QUIT_CALLBACK.lock().unwrap().as_ref() {
        cb();
    }
}

const IPC_PORT: u16 = 42135;

pub fn check_or_signal_existing_instance() -> DaemonInitResult {
    match std::net::UdpSocket::bind(("127.0.0.1", IPC_PORT)) {
        Ok(socket) => {
            std::thread::spawn(move || {
                let mut buf = [0u8; 8];
                while let Ok((_, _)) = socket.recv_from(&mut buf) {
                    trigger_capture();
                }
            });
            DaemonInitResult::PrimaryInstance
        }
        Err(_) => {
            if let Ok(sender) = std::net::UdpSocket::bind("127.0.0.1:0") {
                let _ = sender.send_to(b"SNAP", ("127.0.0.1", IPC_PORT));
            }
            DaemonInitResult::AlreadyRunningSignaled
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::*;
    use global_hotkey::{
        hotkey::{Code, HotKey, Modifiers},
        GlobalHotKeyEvent, GlobalHotKeyManager,
    };
    use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    pub struct DaemonState {
        pub _hotkey_manager: Option<GlobalHotKeyManager>,
        pub _tray_icon: Option<TrayIcon>,
    }

    fn load_tray_icon() -> Icon {
        if let Ok(icon) = Icon::from_resource(1, Some((32, 32))) {
            return icon;
        }
        generate_tray_icon()
    }

    fn generate_tray_icon() -> Icon {
        let size = 32u32;
        let mut rgba = vec![0u8; (size * size * 4) as usize];
        for y in 0..size {
            for x in 0..size {
                let idx = ((y * size + x) * 4) as usize;
                let fx = x as f32;
                let fy = y as f32;
                let cx = 15.5f32;
                let cy = 15.5f32;
                let dist_sq = (fx - cx).powi(2) + (fy - cy).powi(2);

                let in_box = fx >= 2.0 && fx <= 29.0 && fy >= 2.0 && fy <= 29.0;
                let corner_dist = if fx < 6.0 && fy < 6.0 {
                    (fx - 6.0).powi(2) + (fy - 6.0).powi(2)
                } else if fx > 25.0 && fy < 6.0 {
                    (fx - 25.0).powi(2) + (fy - 6.0).powi(2)
                } else if fx < 6.0 && fy > 25.0 {
                    (fx - 6.0).powi(2) + (fy - 25.0).powi(2)
                } else if fx > 25.0 && fy > 25.0 {
                    (fx - 25.0).powi(2) + (fy - 25.0).powi(2)
                } else {
                    0.0
                };

                if in_box && corner_dist <= 16.0 {
                    let t = (fx + fy) / 60.0;
                    let r = (79.0 * (1.0 - t) + 219.0 * t) as u8;
                    let g = (70.0 * (1.0 - t) + 39.0 * t) as u8;
                    let b = (229.0 * (1.0 - t) + 119.0 * t) as u8;
                    rgba[idx] = r;
                    rgba[idx + 1] = g;
                    rgba[idx + 2] = b;
                    rgba[idx + 3] = 255;

                    if dist_sq <= 81.0 {
                        rgba[idx] = 56;
                        rgba[idx + 1] = 189;
                        rgba[idx + 2] = 248;
                        rgba[idx + 3] = 255;
                    }
                    if dist_sq <= 36.0 {
                        rgba[idx] = 15;
                        rgba[idx + 1] = 23;
                        rgba[idx + 2] = 42;
                        rgba[idx + 3] = 255;
                    }
                    if (fx - 13.0).powi(2) + (fy - 13.0).powi(2) <= 2.25 {
                        rgba[idx] = 255;
                        rgba[idx + 1] = 255;
                        rgba[idx + 2] = 255;
                        rgba[idx + 3] = 255;
                    }
                }
            }
        }
        Icon::from_rgba(rgba, size, size).unwrap()
    }

    pub fn init_daemon() -> DaemonState {
        // Register Global Hotkeys (PrintScreen and Ctrl+Alt+S)
        let hotkey_manager = if let Ok(manager) = GlobalHotKeyManager::new() {
            let prtscn = HotKey::new(None, Code::PrintScreen);
            let ctrl_alt_s = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyS);
            let _ = manager.register(prtscn);
            let _ = manager.register(ctrl_alt_s);
            Some(manager)
        } else {
            None
        };

        GlobalHotKeyEvent::set_event_handler(Some(|event: GlobalHotKeyEvent| {
            if event.state == global_hotkey::HotKeyState::Released {
                trigger_capture();
            }
        }));

        // Tray Menu & Icon
        let tray_menu = Menu::new();
        let capture_i = MenuItem::new("Take Screenshot (PrtScn)", true, None);
        let quit_i = MenuItem::new("Quit LocalShot", true, None);
        let _ = tray_menu.append_items(&[&capture_i, &PredefinedMenuItem::separator(), &quit_i]);

        let capture_id = capture_i.id().clone();
        let quit_id = quit_i.id().clone();

        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == capture_id {
                trigger_capture();
            } else if event.id == quit_id {
                trigger_quit();
            }
        }));

        let icon = load_tray_icon();
        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_tooltip("LocalShot (PrtScn)")
            .with_icon(icon)
            .build()
            .ok();

        DaemonState {
            _hotkey_manager: hotkey_manager,
            _tray_icon: tray_icon,
        }
    }
}

#[cfg(target_os = "windows")]
pub use windows_impl::*;

#[cfg(not(target_os = "windows"))]
mod non_windows_impl {
    pub struct DaemonState;

    pub fn init_daemon() -> DaemonState {
        DaemonState
    }
}

#[cfg(not(target_os = "windows"))]
pub use non_windows_impl::*;

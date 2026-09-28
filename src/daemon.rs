use global_hotkey::{hotkey::{HotKey, Modifiers, Code}, GlobalHotKeyEvent, GlobalHotKeyManager};
use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use single_instance::SingleInstance;
use std::sync::Mutex;
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

lazy_static::lazy_static! {
    static ref SINGLE_INSTANCE: Mutex<Option<SingleInstance>> = Mutex::new(None);
    static ref HOTKEY_MANAGER: Mutex<Option<GlobalHotKeyManager>> = Mutex::new(None);
    static ref TRAY_ICON: Mutex<Option<TrayIcon>> = Mutex::new(None);
}

pub fn init_daemon() -> bool {
    // 1. Single Instance check
    let instance = SingleInstance::new("localshot_daemon_unique_id").unwrap();
    if !instance.is_single() {
        return false;
    }
    *SINGLE_INSTANCE.lock().unwrap() = Some(instance);

    // 2. Global Hotkey (PrtScn and Ctrl+Alt+S)
    if let Ok(manager) = GlobalHotKeyManager::new() {
        let prtscn = HotKey::new(None, Code::PrintScreen);
        let ctrl_alt_s = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyS);
        let _ = manager.register(prtscn);
        let _ = manager.register(ctrl_alt_s);
        *HOTKEY_MANAGER.lock().unwrap() = Some(manager);
    }

    // 3. Tray Menu
    let tray_menu = Menu::new();
    let capture_i = MenuItem::new("Take Screenshot", true, None);
    let quit_i = MenuItem::new("Quit LocalShot", true, None);
    let _ = tray_menu.append_items(&[&capture_i, &PredefinedMenuItem::separator(), &quit_i]);

    // Simple 16x16 blue square icon for the tray
    let icon_rgba = vec![255; 16 * 16 * 4];
    let icon = Icon::from_rgba(icon_rgba, 16, 16).unwrap();

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("LocalShot (PrtScn)")
        .with_icon(icon)
        .build()
        .unwrap();

    *TRAY_ICON.lock().unwrap() = Some(tray);

    true
}

pub fn spawn_background_worker<F>(mut on_capture: F) 
where
    F: FnMut() + Send + 'static,
{
    std::thread::spawn(move || {
        loop {
            if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
                if event.state == global_hotkey::HotKeyState::Released {
                    on_capture();
                }
            }

            if let Ok(event) = MenuEvent::receiver().try_recv() {
                if event.id.0 == "Take Screenshot" {
                    on_capture();
                } else {
                    std::process::exit(0);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });
}

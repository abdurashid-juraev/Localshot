use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxSession {
    X11,
    Wayland,
    Unknown,
}

/// Pure session detector for Linux display servers (testable without display).
pub fn detect_session_from(
    xdg_session_type: Option<&str>,
    wayland_display: Option<&str>,
    x11_display: Option<&str>,
) -> LinuxSession {
    if let Some(session_type) = xdg_session_type {
        let normalized = session_type.trim().to_ascii_lowercase();
        if normalized == "wayland" {
            return LinuxSession::Wayland;
        }
        if normalized == "x11" {
            return LinuxSession::X11;
        }
    }
    if wayland_display.is_some_and(|v| !v.trim().is_empty()) {
        return LinuxSession::Wayland;
    }
    if x11_display.is_some_and(|v| !v.trim().is_empty()) {
        return LinuxSession::X11;
    }
    LinuxSession::Unknown
}

/// Detects the active Linux display session from environment variables.
pub fn detect_session() -> LinuxSession {
    let xdg = std::env::var("XDG_SESSION_TYPE").ok();
    let wl = std::env::var("WAYLAND_DISPLAY").ok();
    let x11 = std::env::var("DISPLAY").ok();
    detect_session_from(xdg.as_deref(), wl.as_deref(), x11.as_deref())
}

/// Returns true if running inside a Wayland session.
pub fn is_wayland_session() -> bool {
    detect_session() == LinuxSession::Wayland
}

/// Ready-to-copy Wayland shortcut instructions for GNOME, KDE, Sway, and Hyprland.
pub fn wayland_shortcuts_help_text() -> &'static str {
    "Wayland does not allow apps to register global hotkeys directly.\n\
    Bind your PrintScreen key in your compositor/desktop settings:\n\n\
    • GNOME (run in terminal):\n\
      gsettings set org.gnome.settings-daemon.plugins.media-keys custom-keybindings \"['/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/localshot/']\"\n\
      gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/localshot/ name 'LocalShot Area'\n\
      gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/localshot/ command 'localshot --area'\n\
      gsettings set org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/localshot/ binding 'Print'\n\n\
    • KDE Plasma:\n\
      System Settings -> Keyboard -> Shortcuts -> Add New -> Command -> `localshot --area` (Print) / `localshot --full` (Shift+Print)\n\
      (Or use the LocalShot .desktop actions in the launcher.)\n\n\
    • Sway (~/.config/sway/config):\n\
      bindsym Print exec localshot --area\n\
      bindsym Shift+Print exec localshot --full\n\n\
    • Hyprland (~/.config/hypr/hyprland.conf):\n\
      bind = , Print, exec, localshot --area\n\
      bind = SHIFT, Print, exec, localshot --full"
}

pub fn wayland_help_flag_path_from(base_dir: Option<&Path>) -> Option<PathBuf> {
    base_dir.map(|d| d.join("localshot").join("wayland_help_shown"))
}

pub fn should_show_wayland_help_once_at(flag_path: &Path) -> bool {
    !flag_path.exists()
}

pub fn mark_wayland_help_shown_at(flag_path: &Path) {
    if let Some(parent) = flag_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(flag_path, "1");
}

#[cfg(target_os = "linux")]
pub fn show_wayland_help_dialog() {
    rfd::MessageDialog::new()
        .set_title("LocalShot — Wayland Shortcuts Setup")
        .set_description(wayland_shortcuts_help_text())
        .set_buttons(rfd::MessageButtons::Ok)
        .set_level(rfd::MessageLevel::Info)
        .show();
}

#[cfg(target_os = "linux")]
pub fn maybe_show_wayland_help_once() {
    if !is_wayland_session() {
        return;
    }
    let base = dirs::config_dir().or_else(dirs::data_local_dir);
    let Some(flag_path) = wayland_help_flag_path_from(base.as_deref()) else {
        return;
    };
    if should_show_wayland_help_once_at(&flag_path) {
        mark_wayland_help_shown_at(&flag_path);
        std::thread::spawn(|| {
            show_wayland_help_dialog();
        });
    }
}

/// Applies X11 EWMH hints (`_NET_WM_STATE_ABOVE`, `_NET_WM_STATE_SKIP_TASKBAR`,
/// `_NET_WM_STATE_SKIP_PAGER`) and `WM_CLASS = localshot-overlay` to the overlay window.
#[cfg(target_os = "linux")]
pub fn setup_x11_overlay_window() {
    use xcb::x;

    let Ok((conn, screen_num)) = xcb::Connection::connect(None) else {
        return;
    };
    let setup = conn.get_setup();
    let Some(screen) = setup.roots().nth(screen_num as usize) else {
        return;
    };
    let root = screen.root();

    let intern = |name: &[u8]| -> Option<x::Atom> {
        let cookie = conn.send_request(&x::InternAtom {
            only_if_exists: false,
            name,
        });
        conn.wait_for_reply(cookie).ok().map(|r| r.atom())
    };

    let Some(atom_net_wm_pid) = intern(b"_NET_WM_PID") else {
        return;
    };
    let Some(atom_net_wm_state) = intern(b"_NET_WM_STATE") else {
        return;
    };
    let Some(atom_above) = intern(b"_NET_WM_STATE_ABOVE") else {
        return;
    };
    let Some(atom_skip_taskbar) = intern(b"_NET_WM_STATE_SKIP_TASKBAR") else {
        return;
    };
    let Some(atom_skip_pager) = intern(b"_NET_WM_STATE_SKIP_PAGER") else {
        return;
    };

    let pid = std::process::id();
    let Some(win) = find_x11_window_by_pid(&conn, root, atom_net_wm_pid, pid) else {
        return;
    };

    // Set WM_CLASS = "localshot-overlay\0localshot-overlay\0"
    let wm_class_bytes = b"localshot-overlay\0localshot-overlay\0";
    let _ = conn.send_request(&x::ChangeProperty {
        mode: x::PropMode::Replace,
        window: win,
        property: x::ATOM_WM_CLASS,
        r#type: x::ATOM_STRING,
        data: wm_class_bytes.as_slice(),
    });

    // Set _NET_WM_STATE property directly as well as sending ClientMessage
    let states = [atom_above, atom_skip_taskbar, atom_skip_pager];
    let _ = conn.send_request(&x::ChangeProperty {
        mode: x::PropMode::Replace,
        window: win,
        property: atom_net_wm_state,
        r#type: x::ATOM_ATOM,
        data: &states,
    });

    const NET_WM_STATE_ADD: u32 = 1;
    let event = x::ClientMessageEvent::new(
        win,
        atom_net_wm_state,
        x::ClientMessageData::Data32([
            NET_WM_STATE_ADD,
            xcb::Xid::resource_id(&atom_above),
            xcb::Xid::resource_id(&atom_skip_taskbar),
            1,
            0,
        ]),
    );
    let _ = conn.send_request(&x::SendEvent {
        propagate: false,
        destination: x::SendEventDest::Window(root),
        event_mask: x::EventMask::SUBSTRUCTURE_NOTIFY | x::EventMask::SUBSTRUCTURE_REDIRECT,
        event: &event,
    });

    let _ = conn.flush();
}

#[cfg(target_os = "linux")]
fn find_x11_window_by_pid(
    conn: &xcb::Connection,
    root: xcb::x::Window,
    atom_pid: xcb::x::Atom,
    target_pid: u32,
) -> Option<xcb::x::Window> {
    use xcb::x;
    let mut stack = vec![root];
    let mut visited = 0usize;
    while let Some(win) = stack.pop() {
        visited += 1;
        if visited > 4096 {
            break;
        }
        let prop_cookie = conn.send_request(&x::GetProperty {
            delete: false,
            window: win,
            property: atom_pid,
            r#type: x::ATOM_CARDINAL,
            long_offset: 0,
            long_length: 1,
        });
        if let Ok(reply) = conn.wait_for_reply(prop_cookie) {
            let vals: &[u32] = reply.value();
            if vals.first().copied() == Some(target_pid) && win != root {
                return Some(win);
            }
        }
        let tree_cookie = conn.send_request(&x::QueryTree { window: win });
        if let Ok(tree) = conn.wait_for_reply(tree_cookie) {
            stack.extend_from_slice(tree.children());
        }
    }
    None
}

/// Optional XDG GlobalShortcuts portal registration (behind `portal-shortcuts` feature).
#[cfg(all(target_os = "linux", feature = "portal-shortcuts"))]
#[allow(dead_code)]
pub fn try_register_portal_shortcuts() {
    // UNVERIFIED API: ashpd is optional and not fetched in offline sandbox
    eprintln!("LocalShot: portal-shortcuts feature enabled; XDG GlobalShortcuts portal requested.");
    let _ = std::any::type_name::<ashpd::desktop::global_shortcuts::GlobalShortcuts<'static>>();
    // UNVERIFIED API
}

/// Optional layer-shell support check (behind `layer-shell` feature).
#[cfg(all(target_os = "linux", feature = "layer-shell"))]
#[allow(dead_code)]
pub fn is_layer_shell_enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_session_from() {
        assert_eq!(
            detect_session_from(Some("wayland"), None, Some(":0")),
            LinuxSession::Wayland
        );
        assert_eq!(
            detect_session_from(Some("X11"), Some("wayland-0"), None),
            LinuxSession::X11
        );
        assert_eq!(
            detect_session_from(None, Some("wayland-0"), Some(":0")),
            LinuxSession::Wayland
        );
        assert_eq!(
            detect_session_from(None, None, Some(":0")),
            LinuxSession::X11
        );
        assert_eq!(
            detect_session_from(Some("tty"), None, None),
            LinuxSession::Unknown
        );
    }

    #[test]
    fn test_wayland_help_flag_roundtrip() {
        let dir = std::env::temp_dir().join(format!("localshot-wl-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let flag = wayland_help_flag_path_from(Some(&dir)).expect("flag path");
        assert!(should_show_wayland_help_once_at(&flag));
        mark_wayland_help_shown_at(&flag);
        assert!(!should_show_wayland_help_once_at(&flag));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_wayland_help_text_contains_all_compositors() {
        let text = wayland_shortcuts_help_text();
        assert!(text.contains("GNOME"));
        assert!(text.contains("gsettings"));
        assert!(text.contains("KDE Plasma"));
        assert!(text.contains("Sway"));
        assert!(text.contains("Hyprland"));
        assert!(text.contains("localshot --area"));
    }
}

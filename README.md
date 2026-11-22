# LocalShot

A fast, offline-first, privacy-focused Lightshot clone built in **Rust** and **Slint**.

---

## Security & Privacy Model

- **No Upload / No Telemetry**: Screenshots are never uploaded or sent anywhere. The only optional network request is an explicit or user-enabled GitHub Releases update check (`--check-updates` / tray menu), verified with **Minisign** (`minisign-verify`).
- **In-Memory Processing**: Captured frames, crops, and annotations live in volatile RAM and are scrubbed (`zeroize`, best-effort; OS clipboard and GPU/Slint textures remain managed by the OS/compositor) on copy or dismiss.
- **Zero Disk Residue**: Nothing is written to disk unless you explicitly **Save** (`Ctrl + S`).

---

## Features

- **Instant Fullscreen Freeze**: Hardware screen capture via `xcap` directly into RAM, supporting negative multi-monitor coordinates and fractional DPI scaling.
- **Interactive Cutout Overlay**: Dimmed backdrop with 8-point resize handles and live `W x H` dimension readout.
- **Dual Toolbars (1:1 Lightshot Layout)** with automatic screen-edge flipping:
  - **Drawing Tools**: Pen, Line, Arrow, Rectangle, Marker (highlighter), Redact (pixelate/solid), Text (with embedded `DejaVuSans` fallback), 6-color palette + custom picker, Undo/Redo.
  - **Action Controls**: Copy (`Ctrl + C`), Save (`Ctrl + S`, PNG/JPG/BMP), Close (`Esc`).
- **System Tray & Global Hotkeys**: `PrtScn` (Area) and `Ctrl + Alt + S` (fallback) or `Shift + PrtScn` (Full capture).

---

## Installation

### Linux

#### System Dependencies (Ubuntu / Debian)
```bash
sudo apt-get update
sudo apt-get install -y \
  libgtk-3-dev libayatana-appindicator3-dev \
  libx11-dev libxcb1-dev libxrandr-dev libxfixes-dev \
  libxcursor-dev libxi-dev libfontconfig1-dev \
  libxkbcommon-dev libwayland-dev
```

#### Build & Install
```bash
cargo build --release
./scripts/install.sh
```

### Windows

1. Download `LocalShot-Setup-<version>-x64.exe` (or `localshot-windows-x86_64.zip`) from GitHub Releases.
2. Or build from source:
```powershell
cargo build --release
powershell -ExecutionPolicy Bypass -File .\scripts\install.ps1
```

#### Cross-Compile Windows from Linux
```bash
cargo build --target x86_64-pc-windows-gnu --release
```

---

## CLI Flags

| Flag | Description |
| :--- | :--- |
| *(none)* / `--area` | Trigger area screenshot (connects to running daemon or runs oneshot) |
| `--full` | Capture full monitor under cursor, copy to clipboard, and save |
| `--daemon` / `--background` | Start silent background tray daemon |
| `--oneshot` | Run a single area capture without starting/contacting daemon |
| `--quit` | Signal the running daemon to exit cleanly |
| `--check-updates` / `--update` | Check GitHub Releases for updates |
| `--wayland-help` / `--shortcuts-help` | Show Wayland global shortcut setup instructions |

---

## Wayland Shortcut Setup

Wayland compositors do not allow applications to register global hotkeys directly. Bind your compositor's custom shortcut to `localshot --area` (and optionally `localshot --full`):

- **GNOME**: *Settings -> Keyboard -> Keyboard Shortcuts -> Custom Shortcuts* -> Command: `localshot --area`, Shortcut: `Print` (disable GNOME's built-in PrintScreen binding if needed).
- **KDE Plasma**: *System Settings -> Shortcuts -> Custom Shortcuts / Add Command* -> Command: `localshot --area`.
- **Sway**: Add to `~/.config/sway/config`:
  ```
  bindsym Print exec localshot --area
  bindsym Shift+Print exec localshot --full
  for_window [app_id="localshot"] floating enable, fullscreen enable
  ```
- **Hyprland**: Add to `~/.config/hypr/hyprland.conf`:
  ```
  bind = , Print, exec, localshot --area
  bind = SHIFT, Print, exec, localshot --full
  windowrulev2 = float, class:^(localshot)$
  windowrulev2 = fullscreen, class:^(localshot)$
  ```

---

## Troubleshooting

### Black Screenshots
- **DRM-protected content** (Netflix, banking apps, secure desktop/UAC) and **exclusive fullscreen games** block OS screen capture APIs and return black frames.
- **Wayland / Xwayland**: Ensure `xdg-desktop-portal` and your compositor's portal backend (`xdg-desktop-portal-gnome`, `xdg-desktop-portal-kde`, or `xdg-desktop-portal-wlr`) are installed and running.
- **VM / RDP / NVIDIA**: If OpenGL context creation fails, run with `SLINT_BACKEND=winit-software` as a software-renderer fallback.

### Missing System Tray Icon
- **GNOME**: GNOME Shell removed built-in AppIndicator support. Install the *AppIndicator and KStatusNotifierItem Support* extension (`gnome-shell-extension-appindicator`).
- **Windows Explorer Restart**: LocalShot recreates the tray or remains reachable via `localshot --area` / `localshot --quit`.

### Hotkey Conflicts (`PrintScreen` already taken)
- **Windows 11**: *Settings -> Accessibility -> Keyboard -> Use the Print screen key to open screen capture* -> turn **Off** if you want LocalShot to own `PrtScn`.
- **Other tools**: Close Snipping Tool, Lightshot, or Greenshot, or use the `Ctrl + Alt + S` fallback hotkey.

---

## Shortcuts Reference

| Shortcut | Action |
| :--- | :--- |
| **Mouse Drag** | Create selection area |
| **Corner / Edge Handles** | Resize selection area |
| **Ctrl + C** | Copy selection to Clipboard & Exit |
| **Ctrl + S** | Save selection to PNG/JPG/BMP & Exit |
| **Ctrl + Z / Ctrl + Y** | Undo / Redo last annotation |
| **Esc** | Cancel / Exit without saving |

---

## License

[MIT License](LICENSE)

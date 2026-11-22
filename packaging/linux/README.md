# LocalShot Linux Packaging & Compositor Notes

## Runtime Libraries

Required shared libraries on target Linux systems:
- `libc6`
- `libgtk-3-0` (or `libgtk-3-0t64` on Ubuntu 24.04+) — required by `tray-icon` and `rfd`
- `libayatana-appindicator3-1` (or `libappindicator3-1`) — required for system tray icon (if missing, LocalShot logs once and continues without a tray)
- `libx11-6`, `libxcb1`, `libxkbcommon0` — X11 / keyboard / cursor / EWMH overlay hints
- `libwayland-client0` — Wayland client support
- `libdbus-1-3` — XDG Desktop Portal communication on Wayland

Recommended runtime packages:
- `xdg-desktop-portal` + compositor portal backend (`xdg-desktop-portal-gnome`, `xdg-desktop-portal-kde`, `xdg-desktop-portal-wlr`, or `xdg-desktop-portal-hyprland`)
- `libnotify-bin` (`notify-send` for full-screen capture notifications)

## Window Manager / Compositor Float Rules (`localshot-overlay`)

LocalShot sets `WAYLAND_APP_ID=localshot-overlay` on Wayland and `WM_CLASS=localshot-overlay` on X11.

### i3 (`~/.config/i3/config`)
```text
for_window [class="localshot-overlay"] floating enable, border none, fullscreen enable
bindsym Print exec localshot --area
bindsym Shift+Print exec localshot --full
```

### Sway (`~/.config/sway/config`)
```text
for_window [app_id="localshot-overlay"] floating enable, border none, fullscreen enable
bindsym Print exec localshot --area
bindsym Shift+Print exec localshot --full
```

### Hyprland (`~/.config/hypr/hyprland.conf`)
```text
windowrulev2 = float, class:^(localshot-overlay)$
windowrulev2 = fullscreen, class:^(localshot-overlay)$
windowrulev2 = noborder, class:^(localshot-overlay)$
bind = , Print, exec, localshot --area
bind = SHIFT, Print, exec, localshot --full
```

## Optional Cargo Features
- `portal-shortcuts`: enables optional XDG GlobalShortcuts portal integration (`ashpd`).
- `layer-shell`: enables optional wlroots/KDE layer-shell feature flag.

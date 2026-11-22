# LocalShot Performance Targets & Measurements

This document tracks **idle daemon RSS** and **capture-to-visible latency** across supported platforms.
Numbers are measured using `scripts/bench.sh` (Linux) and `scripts/bench.ps1` (Windows) with `LOCALSHOT_PERF=1`.

> **Rule**: Never record estimated or guessed numbers. Untested platforms must be marked `Pending` until measured on real hardware.

## Targets

| Metric | Target (1080p) | Target (4K / Multi-Monitor) |
| :--- | :--- | :--- |
| Idle Daemon RSS | <= 25 MB | <= 30 MB |
| Capture-to-Visible Latency | <= 120 ms | <= 250 ms |

## Measured Results

| OS / Session | Resolution / DPI | Idle Daemon RSS | Capture Latency (`capture_ms`) | Overlay Ready (`overlay_ready_ms`) | Status |
| :--- | :--- | :--- | :--- | :--- | :--- |
| Windows 10 | 1920x1080 @ 100% | Pending | Pending | Pending | Pending hardware run |
| Windows 11 | 1920x1080 @ 100% | Pending | Pending | Pending | Pending hardware run |
| Windows 11 | 2560x1440 / 4K @ 150% | Pending | Pending | Pending | Pending hardware run |
| Ubuntu X11 | 1920x1080 @ 100% | Pending | Pending | Pending | Pending hardware run |
| GNOME Wayland | 1920x1080 @ 100% | Pending | Pending | Pending | Pending hardware run |
| KDE Wayland | 1920x1080 @ 125% | Pending | Pending | Pending hardware run | Pending hardware run |
| Sway (wlroots) | 1920x1080 @ 100% | Pending | Pending | Pending | Pending hardware run |

## How to Reproduce

### Linux
```bash
cargo build --release
chmod +x scripts/bench.sh
./scripts/bench.sh
```

### Windows (PowerShell)
```powershell
cargo build --release
powershell -ExecutionPolicy Bypass -File .\scripts\bench.ps1
```

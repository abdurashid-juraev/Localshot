# LocalShot 📸

A 100% offline, privacy-first, 1:1 Lightshot clone built in high-performance **Rust** and **Slint UI**.

---

## 🔒 Security & Privacy Non-Negotiables

1. **Strict Zero Network**: Zero network dependencies (`reqwest`, `tokio/net`, `hyper`, etc.). Not a single byte ever leaves your machine.
2. **Zero Disk Residue**: Captured desktop frames, crops, and live annotations exist strictly in volatile RAM. No unencrypted temp files in `/tmp` or `%TEMP%`.
3. **Safe Sanitization**: Frame buffers are dropped and scrubbed from memory immediately when copying to clipboard or dismissing the overlay.

---

## 🚀 Features

- **Instant Fullscreen Freeze**: Fast hardware screen capture via `xcap` directly into memory.
- **Interactive Cutout Overlay**: Dimmed semi-transparent background with sharp transparent viewport showing the selected crop area.
- **8-Point Resize Handles**: Resize your selection from any corner or edge with real-time dimension feedback (`W x H`).
- **1:1 Lightshot Dual Toolbars**:
  - **Vertical Toolbar (Drawing Engine)**:
    - ✏️ **Pen**: Smooth freehand drawing with anti-aliasing brush.
    - 📏 **Line**: Crisp straight vector line.
    - ➡️ **Arrow**: Straight shaft with calculated directional arrowhead.
    - 🔲 **Rectangle**: Vector stroke bounding box.
    - 🖍️ **Marker**: Semi-transparent highlighter with 35% alpha blend.
    - ⬛ **Redact / Blur**: Instant 8x8 average pixelation mosaic to censor sensitive credentials or personal info.
    - 🎨 **Palette**: 6 primary colors (Red, Blue, Green, Yellow, White, Black).
    - ↩️ **Undo**: Step-by-step undo stack.
  - **Horizontal Toolbar (Action Controls)**:
    - 📋 **Copy (`Ctrl + C`)**: Renders final composite image and pushes to system clipboard via `arboard`, then exits.
    - 💾 **Save (`Ctrl + S`)**: Saves PNG image with timestamp to local disk (`~/Pictures/Screenshots` or current directory).
    - ❌ **Close (`Esc`)**: Drops all buffers from RAM and exits immediately.
- **Smart Edge Flipping**: Toolbars automatically flip inside or above the selection when near the screen borders so buttons are never cut off.

---

## 🛠️ Building & Running

### Requirements
- Rust (1.80+ recommended)
- Linux: `libfontconfig`, `libxcb`, `libxkbcommon`
- Windows: Native or MinGW-w64 toolchain

### Linux (Native)

Run development build:
```bash
cargo run
```

Run optimized release build:
```bash
cargo run --release
```

Direct binary path:
```bash
./target/release/localshot
```

### Windows (Cross-compilation from Linux)

Compile standalone `.exe` using MinGW-w64:
```bash
cargo build --target x86_64-pc-windows-gnu --release
```

Output binary:
```
target/x86_64-pc-windows-gnu/release/localshot.exe
```

---

## ⌨️ Shortcuts Reference

| Shortcut | Action |
| :--- | :--- |
| **Mouse Drag** | Create selection area |
| **Corner / Edge Handles** | Resize selection area |
| **Ctrl + C** | Copy selection to Clipboard & Exit |
| **Ctrl + S** | Save selection to PNG & Exit |
| **Ctrl + Z** | Undo last annotation |
| **Esc** | Cancel / Exit without saving |

---

## 📄 License

This project is open-source software licensed under the [MIT License](LICENSE).

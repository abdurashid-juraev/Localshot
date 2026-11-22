# LocalShot Release Checklist

Use this checklist before tagging any `vX.Y.Z` release.

## 1. Automated CI Gates
- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] `cargo test --verbose` passes
- [ ] `cargo test --test headless --test golden_draw --verbose` passes
- [ ] `cargo check --target x86_64-pc-windows-gnu` and `cargo check --target x86_64-unknown-linux-gnu` pass
- [ ] `cargo audit` reports zero unmitigated advisories
- [ ] `cargo deny check advisories licenses sources` passes
- [ ] Zero `.unwrap()` / `.expect()` in non-test code

## 2. Version, Signing & Performance
- [ ] `Cargo.toml` version bumped to `X.Y.Z`
- [ ] `scripts/installer.iss` version matches `X.Y.Z`
- [ ] `UPDATER_MINISIGN_PUBKEY` in `src/updater.rs` matches production keypair (`docs/code-signing.md`)
- [ ] GitHub Secrets `MINISIGN_KEY` and `MINISIGN_PASSWORD` configured
- [ ] `PERF.md` updated with measured idle RSS and capture-to-visible latency from `scripts/bench.sh` / `scripts/bench.ps1`
- [ ] Release tag `vX.Y.Z` produces:
  - `LocalShot-Setup-X.Y.Z-x64.exe` (+ `.minisig`)
  - `localshot-windows-x86_64.zip` (+ `.minisig`)
  - `localshot-linux-x86_64.tar.gz` (+ `.minisig`)
  - `SHA256SUMS` (+ `.minisig`)

## 3. Manual QA Matrix (All Platforms)

| Platform / Session | Capture & Multi-Monitor | Fractional DPI (125%/150%/175%) | Overlay & Focus (No Flicker / Alt+Tab) | Hotkeys & Tray | Clipboard & Save | Drawing & Redact (Non-Recoverable) |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: |
| Windows 10 (100% DPI) | [ ] | [ ] | [ ] | [ ] | [ ] | [ ] |
| Windows 11 (100% DPI) | [ ] | [ ] | [ ] | [ ] | [ ] | [ ] |
| Windows 11 (150% DPI) | [ ] | [ ] | [ ] | [ ] | [ ] | [ ] |
| Ubuntu X11 | [ ] | [ ] | [ ] | [ ] | [ ] | [ ] |
| GNOME Wayland | [ ] | [ ] | [ ] | [ ] | [ ] | [ ] |
| KDE Wayland | [ ] | [ ] | [ ] | [ ] | [ ] | [ ] |
| Sway (wlroots) | [ ] | [ ] | [ ] | [ ] | [ ] | [ ] |

## 4. Edge-Case Verification
- [ ] Monitor hotplug / negative coordinates / vertical arrangement
- [ ] Black frame / DRM / UAC / RDP error surfaced clearly (never silently swallowed)
- [ ] Cyrillic / non-Latin keyboard layout (`Ctrl+C`, `Ctrl+Z`, `Ctrl+S` work)
- [ ] Windows Clipboard History (`Win+V`) privacy notice verified
- [ ] Corrupt or missing `config.json` falls back cleanly to defaults
- [ ] No screenshot pixels or sensitive file paths written to logs
- [ ] Lightshot 1:1 parity table reviewed and signed off

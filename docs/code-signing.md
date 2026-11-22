# LocalShot Windows Code-Signing & Release Verification Plan

## 1. Why LocalShot Triggers SmartScreen and Antivirus False Positives

LocalShot combines three behaviors that heuristic AV engines and Windows Defender SmartScreen treat as high-risk when an executable is unsigned:

1. **Global keyboard hooks / hotkeys** (`RegisterHotKey` for `PrintScreen` and `Ctrl+Alt+S`) — often flagged as keylogger-like behavior by static/behavioral heuristics.
2. **Screen capture APIs** (`xcap` / DXGI Desktop Duplication / GDI `BitBlt`) — flagged as spyware/infostealer behavior when combined with network access.
3. **Self-updating** (downloading and launching an `.exe` installer from GitHub Releases) — flagged as dropper behavior.

Without a valid Authenticode signature, Windows Defender SmartScreen displays *"Windows protected your PC"* on fresh installs, and consumer AVs may quarantine the binary or installer.

## 2. Authenticode Code-Signing Options

### Option A: Azure Trusted Signing (Recommended for OSS / Solo Maintainers)
- **Cost**: ~$9.99/month (Basic tier), no physical HSM USB token required.
- **Benefits**: Microsoft-rooted identity, integrates directly into GitHub Actions via `azure/trusted-signing-action`, builds SmartScreen reputation much faster than standard OV certificates.
- **Targets to sign**:
  1. `localshot.exe` (inside the installer and portable zip)
  2. `LocalShot-Setup-<version>-x64.exe` (NSIS/Inno Setup installer)

### Option B: Traditional OV / EV Code-Signing Certificate
- Requires an HSM-backed certificate (YubiKey / cloud HSM per CA/B Forum rules).
- **EV certificates** historically bypassed SmartScreen immediately, though Microsoft now evaluates reputation across both OV and EV; Azure Trusted Signing is preferred for CI automation.

### Timestamping
Always timestamp Authenticode signatures using an RFC 3161 server (e.g. `http://timestamp.digicert.com` or `http://timestamp.acs.microsoft.com`) so signatures remain valid after certificate expiration.

## 3. Updater Cryptographic Authenticity (`minisign`)

Even with Authenticode on the installer, LocalShot verifies a detached **Minisign** (Ed25519 + BLAKE2b prehashed) signature before executing any downloaded update:

- **Public key**: Embedded in `src/updater.rs` (`UPDATER_MINISIGN_PUBKEY`).
  - Before cutting production signed releases, generate a release keypair with `minisign -G -p localshot.pub -s localshot.key`, store `localshot.key` + passphrase in GitHub Actions Secrets (never commit the secret key), and replace `UPDATER_MINISIGN_PUBKEY` with the base64 public key line from `localshot.pub`.
- **Release assets**: Every Windows release must upload both:
  - `LocalShot-Setup-<version>-x64.exe`
  - `LocalShot-Setup-<version>-x64.exe.minisig`
- **Signing in CI**:
  ```bash
  minisign -S -s localshot.key -m "LocalShot-Setup-${VERSION}-x64.exe" \
    -t "LocalShot v${VERSION} release"
  ```
- **Client enforcement**:
  - HTTPS-only, `redirects(0)` with per-hop host allowlist (`github.com`, `objects.githubusercontent.com`, `release-assets.githubusercontent.com`).
  - Maximum download size enforced (200 MB for installer, 64 KB for `.minisig`).
  - Downloaded to `dirs::cache_dir()/localshot/updates/` with a randomized filename, streamed through `minisign_verify::PublicKey::verify_stream`, and deleted immediately if verification fails.

## 4. Antivirus Submission & False-Positive Mitigation

- Submit signed release installers to Microsoft Security Intelligence (`https://www.microsoft.com/en-us/wdsi/filesubmission`) as false positives if flagged during early releases.
- Keep `Cargo.toml` / `winres` metadata (`ProductName`, `FileDescription`, `OriginalFilename`, `LegalCopyright`) accurate and consistent with the Authenticode subject.
- Never pack or obfuscate the binary (no UPX), as packers severely worsen AV false positives.

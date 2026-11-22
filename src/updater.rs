use minisign_verify::{PublicKey, Signature};
use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use std::collections::hash_map::RandomState;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io::Read;
#[cfg(target_os = "windows")]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

const GITHUB_REPO: &str = "abdurashid-juraev/Localshot";
const USER_AGENT: &str = "LocalShot-DesktopApp";
const AUTO_CHECK_COOLDOWN_SECS: u64 = 86400; // 24 hours
#[cfg(target_os = "windows")]
const MAX_REDIRECT_HOPS: usize = 5;
#[cfg(target_os = "windows")]
const MAX_INSTALLER_BYTES: u64 = 200 * 1024 * 1024; // 200 MB
#[cfg(target_os = "windows")]
const MAX_SIGNATURE_BYTES: u64 = 64 * 1024; // 64 KB
#[cfg(any(target_os = "windows", test))]
const ALLOWED_DOWNLOAD_HOSTS: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

/// Embedded Minisign public key used to verify downloaded Windows installers.
/// Maintainers must replace this with their production `minisign.pub` key
/// before publishing signed releases (see `docs/code-signing.md`).
pub const UPDATER_MINISIGN_PUBKEY: &str =
    "RWRisCjLJ4GrDuQhZmTrxykjdhb8BpOkMyVkLiixv3xcqVX2ycFIfcvS";

/// Pure check for whether the self-updater should be disabled in package-managed
/// (`/usr/...`) or AppImage environments unless `LOCALSHOT_ALLOW_UPDATER` is set.
pub fn is_updater_disabled_with(
    exe_path: Option<&std::path::Path>,
    appimage_env: Option<&str>,
    allow_env: Option<&str>,
) -> bool {
    if let Some(allow) = allow_env {
        let v = allow.trim();
        if v == "1" || v.eq_ignore_ascii_case("true") {
            return false;
        }
    }
    if appimage_env.is_some_and(|v| !v.trim().is_empty()) {
        return true;
    }
    if let Some(path) = exe_path {
        if path.starts_with("/usr") {
            return true;
        }
    }
    false
}

/// Returns true if the updater is disabled for the current runtime environment.
pub fn is_updater_disabled() -> bool {
    let exe = std::env::current_exe().ok();
    let appimage = std::env::var("APPIMAGE").ok();
    let allow = std::env::var("LOCALSHOT_ALLOW_UPDATER").ok();
    is_updater_disabled_with(exe.as_deref(), appimage.as_deref(), allow.as_deref())
}

fn get_last_check_file() -> Option<PathBuf> {
    dirs::cache_dir()
        .or_else(dirs::data_local_dir)
        .map(|p| p.join("localshot").join("last_update_check"))
}

fn should_check_auto() -> bool {
    let check_file = match get_last_check_file() {
        Some(f) => f,
        None => return true,
    };

    if let Ok(metadata) = fs::metadata(&check_file) {
        if let Ok(modified) = metadata.modified() {
            if let Ok(elapsed) = modified.elapsed() {
                if elapsed.as_secs() < AUTO_CHECK_COOLDOWN_SECS {
                    return false;
                }
            }
        }
    }
    true
}

fn mark_check_timestamp() {
    if let Some(check_file) = get_last_check_file() {
        if let Some(parent) = check_file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let now_str = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_default();
        let _ = fs::write(&check_file, now_str);
    }
}

/// Extracts and formats up to `max_lines` or `max_chars` from release body
pub fn format_changelog(body: &str, max_lines: usize, max_chars: usize) -> String {
    let clean = body.trim();
    if clean.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let mut char_count = 0;
    let mut line_count = 0;

    for line in clean.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if line_count >= max_lines || char_count + trimmed.len() > max_chars {
            out.push_str("  ... (and more in release notes)\n");
            break;
        }
        out.push_str("  • ");
        let display_line = trimmed.trim_start_matches(['-', '*', '•']).trim();
        out.push_str(display_line);
        out.push('\n');
        char_count += display_line.len();
        line_count += 1;
    }
    out
}

/// Opens a web URL in the system's default browser
pub fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", "", url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

/// Parses semantic versions like "v0.1.4", "0.1.4", or "1.0.0-rc1" into
/// a comparable tuple `(major, minor, patch, is_prerelease)`.
/// A prerelease (has a `-` suffix on the patch component) sorts OLDER than the
/// corresponding release, so `1.0.0-rc1 < 1.0.0`.
fn parse_version_full(v: &str) -> Option<(u32, u32, u32, bool)> {
    let clean = v.trim().trim_start_matches('v');
    let mut parts = clean.splitn(3, '.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    let patch_str = parts.next()?;

    // Split on first non-digit to separate numeric patch from prerelease tag
    let dash = patch_str.find(|c: char| !c.is_ascii_digit());
    let (patch_num_str, prerelease_tag) = match dash {
        Some(pos) => (&patch_str[..pos], &patch_str[pos..]),
        None => (patch_str, ""),
    };
    let patch: u32 = patch_num_str.parse().ok()?;
    let is_prerelease = !prerelease_tag.is_empty();
    Some((major, minor, patch, is_prerelease))
}

/// Parses semantic versions like "v0.1.4" or "0.1.4" into (major, minor, patch).
/// Strips any pre-release suffix from the patch component.
#[allow(dead_code)]
pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let (major, minor, patch, _) = parse_version_full(v)?;
    Some((major, minor, patch))
}

/// Checks if `remote_ver` is strictly newer than `local_ver`.
/// Item 11: a prerelease (e.g. "1.0.0-rc1") is treated as OLDER than its release ("1.0.0").
pub fn is_newer_version(remote_ver: &str, local_ver: &str) -> bool {
    match (
        parse_version_full(remote_ver),
        parse_version_full(local_ver),
    ) {
        (Some((rma, rmi, rpa, rpre)), Some((lma, lmi, lpa, lpre))) => {
            // Compare (major, minor, patch) first; on tie, non-prerelease > prerelease.
            match (rma, rmi, rpa).cmp(&(lma, lmi, lpa)) {
                std::cmp::Ordering::Greater => true,
                std::cmp::Ordering::Less => false,
                std::cmp::Ordering::Equal => {
                    // Same numeric version: release (false) is newer than prerelease (true)
                    !rpre && lpre
                }
            }
        }
        _ => false,
    }
}

/// Returns the cache directory used for staged update downloads.
pub fn updates_cache_dir() -> Option<PathBuf> {
    dirs::cache_dir()
        .or_else(dirs::data_local_dir)
        .map(|p| p.join("localshot").join("updates"))
}

/// Generates an unpredictable filename for a downloaded installer.
pub fn random_update_filename() -> String {
    let h1 = RandomState::new().build_hasher().finish();
    let h2 = RandomState::new().build_hasher().finish();
    format!("localshot-update-{:016x}{:016x}.exe", h1, h2)
}

/// Computes the exact expected Windows installer asset name for a release tag.
pub fn expected_installer_asset_name(tag: &str) -> Option<String> {
    let version = tag.trim().trim_start_matches(['v', 'V']).trim();
    if version.is_empty()
        || version.contains('/')
        || version.contains('\\')
        || version.contains("..")
    {
        return None;
    }
    Some(format!("LocalShot-Setup-{}-x64.exe", version))
}

/// Selects the exact `(installer_url, signature_url)` pair from GitHub release assets.
/// Returns `None` if either the exact installer or its `.minisig` file is missing.
pub fn select_release_assets(assets: &[serde_json::Value], tag: &str) -> Option<(String, String)> {
    let exe_name = expected_installer_asset_name(tag)?;
    let sig_name = format!("{}.minisig", exe_name);

    let mut exe_url: Option<String> = None;
    let mut sig_url: Option<String> = None;

    for asset in assets {
        let name = asset.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let url = asset
            .get("browser_download_url")
            .and_then(|u| u.as_str())
            .unwrap_or("");
        if url.is_empty() {
            continue;
        }
        if name == exe_name {
            exe_url = Some(url.to_string());
        } else if name == sig_name {
            sig_url = Some(url.to_string());
        }
    }

    match (exe_url, sig_url) {
        (Some(exe), Some(sig)) => Some((exe, sig)),
        _ => None,
    }
}

/// Validates that `url` uses HTTPS and targets an allowlisted host.
pub fn validate_download_url(url: &str, allowed_hosts: &[&str]) -> Result<(), String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| "Only HTTPS download URLs are allowed".to_string())?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err("Invalid download URL authority".to_string());
    }
    let host = if let Some((h, port)) = authority.split_once(':') {
        if port != "443" {
            return Err("Non-standard HTTPS port is not allowed".to_string());
        }
        h
    } else {
        authority
    };
    let host_lower = host.to_ascii_lowercase();
    if allowed_hosts
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(&host_lower))
    {
        Ok(())
    } else {
        Err(format!(
            "Host '{}' is not in download allowlist",
            host_lower
        ))
    }
}

#[cfg(any(target_os = "windows", test))]
fn resolve_redirect_url(current_url: &str, location: &str) -> Result<String, String> {
    if location.starts_with("https://") {
        return Ok(location.to_string());
    }
    if location.starts_with('/') {
        let rest = current_url
            .strip_prefix("https://")
            .ok_or_else(|| "Current URL is not HTTPS".to_string())?;
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if authority.is_empty() {
            return Err("Invalid current URL authority".to_string());
        }
        return Ok(format!("https://{}{}", authority, location));
    }
    Err("Unsupported or insecure redirect location".to_string())
}

#[cfg(target_os = "windows")]
fn fetch_with_validated_redirects(
    initial_url: &str,
    allowed_hosts: &[&str],
    max_hops: usize,
) -> Result<ureq::Response, String> {
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(std::time::Duration::from_secs(30))
        .build();

    let mut current_url = initial_url.to_string();
    for _ in 0..=max_hops {
        validate_download_url(&current_url, allowed_hosts)?;
        let resp = agent
            .get(&current_url)
            .set("User-Agent", USER_AGENT)
            .call()
            .map_err(|e| format!("HTTP request failed: {}", e))?;

        let status = resp.status();
        if (200..=299).contains(&status) {
            return Ok(resp);
        }
        if (300..=399).contains(&status) {
            let loc = resp
                .header("Location")
                .ok_or_else(|| "Redirect missing Location header".to_string())?;
            current_url = resolve_redirect_url(&current_url, loc)?;
            continue;
        }
        return Err(format!("Unexpected HTTP status: {}", status));
    }

    Err("Too many redirects".to_string())
}

/// Verifies an in-memory payload against a Minisign public key and detached signature.
#[allow(dead_code)]
pub fn verify_minisign_bytes(pub_key_b64: &str, data: &[u8], sig_text: &str) -> Result<(), String> {
    let pk = PublicKey::from_base64(pub_key_b64.trim())
        .map_err(|e| format!("Invalid embedded public key: {}", e))?;
    let sig = Signature::decode(sig_text).map_err(|e| format!("Invalid signature: {}", e))?;
    pk.verify(data, &sig, false)
        .map_err(|e| format!("Signature verification failed: {}", e))
}

/// Streams a file and verifies its Minisign signature without loading the whole file into memory.
pub fn verify_minisign_file(
    pub_key_b64: &str,
    file_path: &Path,
    sig_text: &str,
) -> Result<(), String> {
    let pk = PublicKey::from_base64(pub_key_b64.trim())
        .map_err(|e| format!("Invalid embedded public key: {}", e))?;
    let sig = Signature::decode(sig_text).map_err(|e| format!("Invalid signature: {}", e))?;
    let mut verifier = pk
        .verify_stream(&sig)
        .map_err(|e| format!("Unable to initialize stream verifier: {}", e))?;

    let mut file = fs::File::open(file_path).map_err(|e| format!("Open file failed: {}", e))?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("Read file failed: {}", e))?;
        if n == 0 {
            break;
        }
        verifier.update(&buf[..n]);
    }
    verifier
        .finalize()
        .map_err(|e| format!("Signature verification failed: {}", e))
}

#[cfg(target_os = "windows")]
fn read_response_limited(resp: ureq::Response, max_bytes: u64) -> Result<Vec<u8>, String> {
    if let Some(len_str) = resp.header("Content-Length") {
        if let Ok(len) = len_str.trim().parse::<u64>() {
            if len > max_bytes {
                return Err(format!("Response exceeds {} byte limit", max_bytes));
            }
        }
    }
    let mut out = Vec::new();
    let mut reader = resp.into_reader().take(max_bytes + 1);
    reader
        .read_to_end(&mut out)
        .map_err(|e| format!("Failed reading response: {}", e))?;
    if out.len() as u64 > max_bytes {
        return Err(format!("Response exceeds {} byte limit", max_bytes));
    }
    Ok(out)
}

#[cfg(target_os = "windows")]
fn download_to_file_limited(
    resp: ureq::Response,
    dest: &Path,
    max_bytes: u64,
) -> Result<(), String> {
    if let Some(len_str) = resp.header("Content-Length") {
        if let Ok(len) = len_str.trim().parse::<u64>() {
            if len > max_bytes {
                return Err(format!("Installer exceeds {} byte limit", max_bytes));
            }
        }
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed creating cache dir: {}", e))?;
    }
    let mut file =
        fs::File::create(dest).map_err(|e| format!("Failed creating update file: {}", e))?;
    let mut reader = resp.into_reader();
    let mut buf = [0u8; 64 * 1024];
    let mut total: u64 = 0;

    let write_res: Result<(), String> = (|| {
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| format!("Download read error: {}", e))?;
            if n == 0 {
                break;
            }
            total = total.saturating_add(n as u64);
            if total > max_bytes {
                return Err(format!("Installer exceeds {} byte limit", max_bytes));
            }
            file.write_all(&buf[..n])
                .map_err(|e| format!("Download write error: {}", e))?;
        }
        file.flush()
            .map_err(|e| format!("Download flush error: {}", e))?;
        Ok(())
    })();

    if write_res.is_err() {
        drop(file);
        let _ = fs::remove_file(dest);
    }
    write_res
}

fn show_dialog(
    use_event_loop: bool,
    title: &str,
    description: &str,
    buttons: MessageButtons,
    level: MessageLevel,
) -> MessageDialogResult {
    let title = title.to_string();
    let description = description.to_string();

    if !use_event_loop {
        return MessageDialog::new()
            .set_title(&title)
            .set_description(&description)
            .set_buttons(buttons)
            .set_level(level)
            .show();
    }

    let (tx, rx) = mpsc::sync_channel(1);
    let dispatched = slint::invoke_from_event_loop(move || {
        let res = MessageDialog::new()
            .set_title(&title)
            .set_description(&description)
            .set_buttons(buttons)
            .set_level(level)
            .show();
        let _ = tx.send(res);
    });

    if dispatched.is_err() {
        return MessageDialogResult::Cancel;
    }

    rx.recv().unwrap_or(MessageDialogResult::Cancel)
}

/// Checks for updates against GitHub Releases API using the Slint UI thread for dialogs.
pub fn check_for_updates(interactive: bool) {
    check_for_updates_with_mode(interactive, true);
}

/// Checks for updates directly on the calling (main) thread before the Slint event loop starts.
pub fn check_for_updates_direct(interactive: bool) {
    check_for_updates_with_mode(interactive, false);
}

fn check_for_updates_with_mode(interactive: bool, use_event_loop: bool) {
    if is_updater_disabled() {
        if interactive {
            show_dialog(
                use_event_loop,
                "LocalShot Update",
                "Updates are disabled because LocalShot is running from a system package (/usr) or AppImage.\n\
                Use your package manager to update, or set LOCALSHOT_ALLOW_UPDATER=1 to override.",
                MessageButtons::Ok,
                MessageLevel::Info,
            );
        }
        return;
    }

    let current_version = env!("CARGO_PKG_VERSION");
    let api_url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        GITHUB_REPO
    );

    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(std::time::Duration::from_secs(10))
        .build();
    let response = agent
        .get(&api_url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github.v3+json")
        .call();

    let json: serde_json::Value = match response {
        Ok(resp) => match resp.into_json() {
            Ok(val) => val,
            Err(e) => {
                if interactive {
                    show_dialog(
                        use_event_loop,
                        "LocalShot Update",
                        &format!("Failed to parse release information:\n{}", e),
                        MessageButtons::Ok,
                        MessageLevel::Error,
                    );
                }
                return;
            }
        },
        Err(e) => {
            if interactive {
                show_dialog(
                    use_event_loop,
                    "LocalShot Update",
                    &format!(
                        "Could not connect to update server.\nPlease check your internet connection.\n\nError: {}",
                        e
                    ),
                    MessageButtons::Ok,
                    MessageLevel::Warning,
                );
            }
            return;
        }
    };

    let latest_tag = match json.get("tag_name").and_then(|v| v.as_str()) {
        Some(tag) => tag.to_string(),
        None => {
            if interactive {
                show_dialog(
                    use_event_loop,
                    "LocalShot Update",
                    "Could not find release version in server response.",
                    MessageButtons::Ok,
                    MessageLevel::Warning,
                );
            }
            return;
        }
    };

    let release_url = json
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://github.com/abdurashid-juraev/Localshot/releases")
        .to_string();

    mark_check_timestamp();

    if !is_newer_version(&latest_tag, current_version) {
        if interactive {
            show_dialog(
                use_event_loop,
                "LocalShot - Up to Date",
                &format!(
                    "You are already using the latest version of LocalShot (v{}).\nNo updates needed!",
                    current_version
                ),
                MessageButtons::Ok,
                MessageLevel::Info,
            );
        }
        return;
    }

    // A newer version is available! Extract changelog from release body
    let raw_notes = json.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let formatted_notes = format_changelog(raw_notes, 8, 500);

    let mut prompt_desc = format!(
        "A new version of LocalShot is available!\n\n\
        • Installed version: v{}\n\
        • Latest version:    {}\n\n",
        current_version, latest_tag
    );

    if !formatted_notes.is_empty() {
        prompt_desc.push_str("📝 What's New:\n");
        prompt_desc.push_str(&formatted_notes);
        prompt_desc.push('\n');
    }

    prompt_desc.push_str("Would you like to download and install the update now?");

    let user_choice = show_dialog(
        use_event_loop,
        "LocalShot Update Available",
        &prompt_desc,
        MessageButtons::YesNo,
        MessageLevel::Info,
    );

    if user_choice != MessageDialogResult::Yes {
        return;
    }

    #[cfg(target_os = "windows")]
    if let Some(assets_list) = json.get("assets").and_then(|a| a.as_array()) {
        if let Some((installer_url, sig_url)) = select_release_assets(assets_list, &latest_tag) {
            if let Some(cache_dir) = updates_cache_dir() {
                let staged_installer = cache_dir.join(random_update_filename());
                let update_result: Result<(), String> = (|| {
                    let sig_resp = fetch_with_validated_redirects(
                        &sig_url,
                        ALLOWED_DOWNLOAD_HOSTS,
                        MAX_REDIRECT_HOPS,
                    )?;
                    let sig_bytes = read_response_limited(sig_resp, MAX_SIGNATURE_BYTES)?;
                    let sig_text = String::from_utf8(sig_bytes)
                        .map_err(|_| "Signature file is not valid UTF-8".to_string())?;

                    let exe_resp = fetch_with_validated_redirects(
                        &installer_url,
                        ALLOWED_DOWNLOAD_HOSTS,
                        MAX_REDIRECT_HOPS,
                    )?;
                    download_to_file_limited(exe_resp, &staged_installer, MAX_INSTALLER_BYTES)?;

                    if let Err(err) =
                        verify_minisign_file(UPDATER_MINISIGN_PUBKEY, &staged_installer, &sig_text)
                    {
                        let _ = fs::remove_file(&staged_installer);
                        return Err(err);
                    }
                    Ok(())
                })();

                match update_result {
                    Ok(()) => {
                        if std::process::Command::new(&staged_installer)
                            .spawn()
                            .is_ok()
                        {
                            std::process::exit(0);
                        }
                    }
                    Err(err) => {
                        let _ = fs::remove_file(&staged_installer);
                        show_dialog(
                            use_event_loop,
                            "LocalShot Update Failed",
                            &format!(
                                "Update verification or download failed:\n{}\n\nOpening release page in browser instead.",
                                err
                            ),
                            MessageButtons::Ok,
                            MessageLevel::Error,
                        );
                    }
                }
            }
        }
    }

    // Fallback: Open release download page in web browser
    open_browser(&release_url);
}

/// Automatically checks for updates in background once every 24 hours.
pub fn check_for_updates_auto() {
    if !crate::config::load_user_config().auto_update_check {
        return;
    }
    if !should_check_auto() {
        return;
    }
    check_for_updates(false);
}

/// Spawns a non-blocking background thread with a 3-second startup delay
/// ensuring zero startup latency for the main application.
pub fn spawn_background_update_check() {
    if is_updater_disabled() || !crate::config::load_user_config().auto_update_check {
        return;
    }
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(3));
        check_for_updates_auto();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_parsing() {
        assert_eq!(parse_version("v0.1.4"), Some((0, 1, 4)));
        assert_eq!(parse_version("0.1.5"), Some((0, 1, 5)));
        assert_eq!(parse_version("v1.0.0-rc1"), Some((1, 0, 0)));
        assert!(is_newer_version("v0.1.5", "0.1.4"));
        assert!(!is_newer_version("v0.1.4", "0.1.4"));
        assert!(!is_newer_version("v0.1.3", "0.1.4"));
        assert!(is_newer_version("v0.2.0", "0.1.9"));
        assert!(is_newer_version("v1.0.0", "0.9.9"));
        // Item 11: prerelease is OLDER than the release
        assert!(
            !is_newer_version("v1.0.0-rc1", "1.0.0"),
            "1.0.0-rc1 must be treated as older than 1.0.0"
        );
        assert!(
            is_newer_version("1.0.0", "v1.0.0-rc1"),
            "1.0.0 must be treated as newer than 1.0.0-rc1"
        );
        assert!(
            !is_newer_version("v1.0.0-beta", "v1.0.0-alpha"),
            "two prereleases of same version: neither is newer"
        );
        assert!(
            is_newer_version("v1.0.1-rc1", "1.0.0"),
            "1.0.1-rc1 (prerelease of next) is still newer by numeric version"
        );
    }

    #[test]
    fn test_format_changelog() {
        let raw = "- Fix overlay drift\n- Add multi-monitor support\n* Improve drawing tools";
        let formatted = format_changelog(raw, 5, 200);
        assert!(formatted.contains("Fix overlay drift"));
        assert!(formatted.contains("Add multi-monitor support"));
        assert!(formatted.contains("Improve drawing tools"));
    }

    #[test]
    fn test_is_updater_disabled_with() {
        use std::path::Path;
        assert!(is_updater_disabled_with(
            Some(Path::new("/usr/bin/localshot")),
            None,
            None
        ));
        assert!(is_updater_disabled_with(
            Some(Path::new("/usr/local/bin/localshot")),
            None,
            None
        ));
        assert!(is_updater_disabled_with(
            Some(Path::new("/home/user/.local/bin/localshot")),
            Some("/tmp/LocalShot.AppImage"),
            None
        ));
        assert!(!is_updater_disabled_with(
            Some(Path::new("/home/user/.local/bin/localshot")),
            None,
            None
        ));
        assert!(!is_updater_disabled_with(
            Some(Path::new("/usr/bin/localshot")),
            Some("/tmp/LocalShot.AppImage"),
            Some("1")
        ));
        assert!(!is_updater_disabled_with(
            Some(Path::new("/usr/bin/localshot")),
            None,
            Some("true")
        ));
    }

    #[test]
    fn test_asset_selection_exact_pattern() {
        let assets = vec![
            serde_json::json!({
                "name": "LocalShot-Setup-other.exe",
                "browser_download_url": "https://github.com/org/repo/releases/download/v0.2.0/LocalShot-Setup-other.exe"
            }),
            serde_json::json!({
                "name": "LocalShot-Setup-0.2.0-x64.exe",
                "browser_download_url": "https://github.com/org/repo/releases/download/v0.2.0/LocalShot-Setup-0.2.0-x64.exe"
            }),
            serde_json::json!({
                "name": "LocalShot-Setup-0.2.0-x64.exe.minisig",
                "browser_download_url": "https://github.com/org/repo/releases/download/v0.2.0/LocalShot-Setup-0.2.0-x64.exe.minisig"
            }),
        ];

        let selected = select_release_assets(&assets, "v0.2.0").expect("exact assets must match");
        assert_eq!(
            selected.0,
            "https://github.com/org/repo/releases/download/v0.2.0/LocalShot-Setup-0.2.0-x64.exe"
        );
        assert_eq!(
            selected.1,
            "https://github.com/org/repo/releases/download/v0.2.0/LocalShot-Setup-0.2.0-x64.exe.minisig"
        );

        // Missing .minisig must be rejected
        let unsigned_only = vec![assets[1].clone()];
        assert_eq!(select_release_assets(&unsigned_only, "v0.2.0"), None);

        // Loose name without exact version must be rejected
        let loose_only = vec![assets[0].clone(), assets[2].clone()];
        assert_eq!(select_release_assets(&loose_only, "v0.2.0"), None);

        // Path traversal tag must be rejected
        assert_eq!(expected_installer_asset_name("../0.2.0"), None);
    }

    #[test]
    fn test_host_allowlist_and_https() {
        assert!(validate_download_url(
            "https://github.com/abdurashid-juraev/Localshot/releases/download/v0.2.0/LocalShot-Setup-0.2.0-x64.exe",
            ALLOWED_DOWNLOAD_HOSTS
        )
        .is_ok());
        assert!(validate_download_url(
            "https://objects.githubusercontent.com/github-production-release-asset-123/file.exe",
            ALLOWED_DOWNLOAD_HOSTS
        )
        .is_ok());
        assert!(validate_download_url(
            "https://release-assets.githubusercontent.com/some/path",
            ALLOWED_DOWNLOAD_HOSTS
        )
        .is_ok());

        // HTTP, spoofing, non-allowlisted hosts, and custom ports must fail
        assert!(
            validate_download_url("http://github.com/file.exe", ALLOWED_DOWNLOAD_HOSTS).is_err()
        );
        assert!(validate_download_url(
            "https://github.com@evil.example/file.exe",
            ALLOWED_DOWNLOAD_HOSTS
        )
        .is_err());
        assert!(
            validate_download_url("https://evil.github.com/file.exe", ALLOWED_DOWNLOAD_HOSTS)
                .is_err()
        );
        assert!(
            validate_download_url("https://github.com:8443/file.exe", ALLOWED_DOWNLOAD_HOSTS)
                .is_err()
        );
        assert!(resolve_redirect_url(
            "https://github.com/a",
            "http://objects.githubusercontent.com/b"
        )
        .is_err());
        assert_eq!(
            resolve_redirect_url("https://github.com/a", "/b/c").unwrap(),
            "https://github.com/b/c"
        );
    }

    #[test]
    fn test_minisign_verification_rejects_tampered_file() {
        const TEST_MINISIGN_PUBKEY: &str =
            "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
        let sig_text = "untrusted comment: signature from minisign secret key\n\
            RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\n\
            trusted comment: timestamp:1556193335\tfile:test\n\
            y/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg==";

        // Embedded production public key must be valid base64 Minisign format
        assert!(PublicKey::from_base64(UPDATER_MINISIGN_PUBKEY.trim()).is_ok());

        // Valid payload passes
        assert!(verify_minisign_bytes(TEST_MINISIGN_PUBKEY, b"test", sig_text).is_ok());

        // Tampered payload in file fails
        let tmp_dir =
            std::env::temp_dir().join(format!("localshot_upd_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp_dir);
        let valid_file = tmp_dir.join("valid.bin");
        let tampered_file = tmp_dir.join("tampered.bin");
        fs::write(&valid_file, b"test").unwrap();
        fs::write(&tampered_file, b"test-tampered").unwrap();

        assert!(verify_minisign_file(TEST_MINISIGN_PUBKEY, &valid_file, sig_text).is_ok());
        assert!(verify_minisign_file(TEST_MINISIGN_PUBKEY, &tampered_file, sig_text).is_err());
        // Production key must reject signatures from other keys
        assert!(verify_minisign_file(UPDATER_MINISIGN_PUBKEY, &valid_file, sig_text).is_err());

        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_random_update_filename_is_unpredictable() {
        let a = random_update_filename();
        let b = random_update_filename();
        assert!(a.starts_with("localshot-update-"));
        assert!(a.ends_with(".exe"));
        assert_ne!(a, b);
    }
}

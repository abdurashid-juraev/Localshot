use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use std::fs;
#[cfg(target_os = "windows")]
use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const GITHUB_REPO: &str = "abdurashid-juraev/Localshot";
const USER_AGENT: &str = "LocalShot-DesktopApp";
const AUTO_CHECK_COOLDOWN_SECS: u64 = 86400; // 24 hours

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
        let display_line = trimmed.trim_start_matches(|c| c == '-' || c == '*' || c == '•').trim();
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

/// Parses semantic versions like "v0.1.4" or "0.1.4" into (major, minor, patch)
pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let clean = v.trim().trim_start_matches('v');
    let mut parts = clean.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts
        .next()?
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    Some((major, minor, patch))
}

/// Checks if `remote_ver` is strictly newer than `local_ver`
pub fn is_newer_version(remote_ver: &str, local_ver: &str) -> bool {
    match (parse_version(remote_ver), parse_version(local_ver)) {
        (Some(remote), Some(local)) => remote > local,
        _ => false,
    }
}

/// Checks for updates against GitHub Releases API.
/// If `interactive` is true, shows feedback dialogs even if already up to date or if error occurs.
pub fn check_for_updates(interactive: bool) {
    let current_version = env!("CARGO_PKG_VERSION");
    let api_url = format!("https://api.github.com/repos/{}/releases/latest", GITHUB_REPO);

    let response = ureq::get(&api_url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github.v3+json")
        .timeout(std::time::Duration::from_secs(10))
        .call();

    let json: serde_json::Value = match response {
        Ok(resp) => match resp.into_json() {
            Ok(val) => val,
            Err(e) => {
                if interactive {
                    MessageDialog::new()
                        .set_title("LocalShot Update")
                        .set_description(&format!("Failed to parse release information:\n{}", e))
                        .set_buttons(MessageButtons::Ok)
                        .set_level(MessageLevel::Error)
                        .show();
                }
                return;
            }
        },
        Err(e) => {
            if interactive {
                MessageDialog::new()
                    .set_title("LocalShot Update")
                    .set_description(&format!(
                        "Could not connect to update server.\nPlease check your internet connection.\n\nError: {}",
                        e
                    ))
                    .set_buttons(MessageButtons::Ok)
                    .set_level(MessageLevel::Warning)
                    .show();
            }
            return;
        }
    };

    let latest_tag = match json.get("tag_name").and_then(|v| v.as_str()) {
        Some(tag) => tag.to_string(),
        None => {
            if interactive {
                MessageDialog::new()
                    .set_title("LocalShot Update")
                    .set_description("Could not find release version in server response.")
                    .set_buttons(MessageButtons::Ok)
                    .set_level(MessageLevel::Warning)
                    .show();
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
            MessageDialog::new()
                .set_title("LocalShot - Up to Date")
                .set_description(&format!(
                    "You are already using the latest version of LocalShot (v{}).\nNo updates needed!",
                    current_version
                ))
                .set_buttons(MessageButtons::Ok)
                .set_level(MessageLevel::Info)
                .show();
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

    let user_choice = MessageDialog::new()
        .set_title("LocalShot Update Available")
        .set_description(&prompt_desc)
        .set_buttons(MessageButtons::YesNo)
        .set_level(MessageLevel::Info)
        .show();

    if user_choice != MessageDialogResult::Yes {
        return;
    }

    // Check release assets for Windows installer
    #[cfg(target_os = "windows")]
    let mut installer_url: Option<String> = None;
    #[cfg(target_os = "windows")]
    if let Some(assets_list) = json.get("assets").and_then(|a| a.as_array()) {
        for asset in assets_list {
            let name = asset.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let url = asset
                .get("browser_download_url")
                .and_then(|u| u.as_str())
                .unwrap_or("");

            if name.ends_with(".exe")
                && (name.contains("Setup") || name.contains("installer") || name.contains("LocalShot"))
            {
                installer_url = Some(url.to_string());
                break;
            }
        }
    }

    // On Windows, if installer .exe is found, download and launch it
    #[cfg(target_os = "windows")]
    if let Some(download_url) = installer_url {
        let temp_installer = std::env::temp_dir().join("LocalShot-Setup-Update.exe");

        let download_result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let resp = ureq::get(&download_url)
                .set("User-Agent", USER_AGENT)
                .call()?;
            let mut reader = resp.into_reader();
            let mut file = fs::File::create(&temp_installer)?;
            io::copy(&mut reader, &mut file)?;
            Ok(())
        })();

        if download_result.is_ok() && temp_installer.exists() {
            let launched = std::process::Command::new(&temp_installer).spawn();
            if launched.is_ok() {
                std::process::exit(0);
            }
        }
    }

    // Fallback: Open release download page in web browser
    open_browser(&release_url);
}

/// Automatically checks for updates in background once every 24 hours.
pub fn check_for_updates_auto() {
    if !should_check_auto() {
        return;
    }
    check_for_updates(false);
}

/// Spawns a non-blocking background thread with a 3-second startup delay
/// ensuring zero startup latency for the main application.
pub fn spawn_background_update_check() {
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
    }

    #[test]
    fn test_format_changelog() {
        let raw = "- Fix overlay drift\n- Add multi-monitor support\n* Improve drawing tools";
        let formatted = format_changelog(raw, 5, 200);
        assert!(formatted.contains("Fix overlay drift"));
        assert!(formatted.contains("Add multi-monitor support"));
        assert!(formatted.contains("Improve drawing tools"));
    }
}

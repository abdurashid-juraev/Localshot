use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use std::fs;
use std::io;

const GITHUB_REPO: &str = "abdurashid-juraev/Localshot";
const USER_AGENT: &str = "LocalShot-DesktopApp";

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

    // A newer version is available!
    let prompt_desc = format!(
        "A new version of LocalShot is available!\n\n\
        • Installed version: v{}\n\
        • Latest version:    {}\n\n\
        Would you like to download and install the update now?",
        current_version, latest_tag
    );

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
    let mut installer_url: Option<String> = None;
    if let Some(assets_list) = json.get("assets").and_then(|a| a.as_array()) {
        for asset in assets_list {
            let name = asset.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let url = asset
                .get("browser_download_url")
                .and_then(|u| u.as_str())
                .unwrap_or("");

            #[cfg(target_os = "windows")]
            {
                if name.ends_with(".exe")
                    && (name.contains("Setup") || name.contains("installer") || name.contains("LocalShot"))
                {
                    installer_url = Some(url.to_string());
                    break;
                }
            }

            #[cfg(target_os = "linux")]
            {
                if name.ends_with(".tar.gz") {
                    installer_url = Some(url.to_string());
                    break;
                }
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
}

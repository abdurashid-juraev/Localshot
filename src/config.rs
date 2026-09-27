use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaletteColor {
    pub rgba: [u8; 4],
    pub hex: &'static str,
    pub name: &'static str,
}

pub const PALETTE: [PaletteColor; 6] = [
    PaletteColor {
        rgba: [229, 57, 53, 255],
        hex: "#e53935",
        name: "Red",
    },
    PaletteColor {
        rgba: [30, 136, 229, 255],
        hex: "#1e88e5",
        name: "Blue",
    },
    PaletteColor {
        rgba: [67, 160, 71, 255],
        hex: "#43a047",
        name: "Green",
    },
    PaletteColor {
        rgba: [253, 216, 53, 255],
        hex: "#fdd835",
        name: "Yellow",
    },
    PaletteColor {
        rgba: [255, 255, 255, 255],
        hex: "#ffffff",
        name: "White",
    },
    PaletteColor {
        rgba: [33, 33, 33, 255],
        hex: "#212121",
        name: "Black",
    },
];

// Drawing parameters
pub const PEN_THICKNESS: i32 = 3;
pub const LINE_THICKNESS: i32 = 3;
pub const ARROW_THICKNESS: i32 = 3;
pub const RECT_THICKNESS: i32 = 3;
pub const MARKER_THICKNESS: i32 = 12;
pub const MARKER_ALPHA: u8 = 90; // ~35% opacity
pub const REDACT_BLOCK_SIZE: i32 = 10;
pub const ARROW_HEAD_ANGLE: f32 = 0.50; // ~28 degrees
pub const ARROW_HEAD_BASE_LEN: f32 = 14.0;

/// Returns the default save directory respecting OS conventions, localized folders, and OneDrive.
pub fn default_save_dir() -> PathBuf {
    if let Some(pictures) = dirs::picture_dir() {
        let screenshots = pictures.join("Screenshots");
        if screenshots.is_dir() || std::fs::create_dir_all(&screenshots).is_ok() {
            return screenshots;
        }
        return pictures;
    }

    if let Some(desktop) = dirs::desktop_dir() {
        if desktop.is_dir() {
            return desktop;
        }
    }

    if let Some(home) = dirs::home_dir() {
        return home;
    }

    PathBuf::from(".")
}

/// Generates human-readable UTC timestamp filename: LocalShot_YYYY-MM-DD_HHMMSS.png
pub fn default_filename() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let formatted_time = format_timestamp_utc(now);
    format!("LocalShot_{}.png", formatted_time)
}

/// Resolves the destination filepath for saving a screenshot.
#[allow(dead_code)]
pub fn resolve_save_filepath() -> PathBuf {
    default_save_dir().join(default_filename())
}

/// Simple zero-dependency UTC timestamp formatter
fn format_timestamp_utc(epoch_secs: u64) -> String {
    let sec_in_day = 86400;
    let mut days = (epoch_secs / sec_in_day) as i64;
    let rem_secs = (epoch_secs % sec_in_day) as u32;

    let hour = rem_secs / 3600;
    let minute = (rem_secs % 3600) / 60;
    let second = rem_secs % 60;

    // Civil day from epoch algorithm
    days += 719468;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = (days - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}_{:02}{:02}{:02}",
        year, m, d, hour, minute, second
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_palette_validity() {
        assert_eq!(PALETTE.len(), 6);
        assert_eq!(PALETTE[0].name, "Red");
        assert_eq!(PALETTE[0].hex, "#e53935");
        assert_eq!(PALETTE[5].name, "Black");
        assert_eq!(PALETTE[5].hex, "#212121");
    }

    #[test]
    fn test_timestamp_formatting() {
        // Epoch 0: 1970-01-01 00:00:00
        assert_eq!(format_timestamp_utc(0), "1970-01-01_000000");
        // A known epoch: 1727471736 -> 2024-09-27 21:15:36
        assert_eq!(format_timestamp_utc(1727471736), "2024-09-27_211536");
    }

    #[test]
    fn test_save_filepath_resolution() {
        let path = resolve_save_filepath();
        assert!(path.to_str().unwrap().contains("LocalShot_"));
        assert!(path.extension().map_or(false, |ext| ext == "png"));
    }
}


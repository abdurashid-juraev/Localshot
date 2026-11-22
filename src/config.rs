use image::{DynamicImage, ImageFormat, RgbaImage};
use std::fs::{self, OpenOptions};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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
pub const TEXT_DEFAULT_SIZE: i32 = 18;
pub const TEXT_MIN_SIZE: i32 = 12;
pub const TEXT_MAX_SIZE: i32 = 48;
pub const MIN_THICKNESS: i32 = 1;
pub const MAX_THICKNESS: i32 = 24;
pub const MAX_HISTORY_LEN: usize = 100;
pub const MARKER_ALPHA: u8 = 90; // ~35% opacity
pub const MIN_REDACT_BLOCK_SIZE: i32 = 12;
#[allow(dead_code)]
pub const REDACT_BLOCK_SIZE: i32 = MIN_REDACT_BLOCK_SIZE;
pub const ARROW_HEAD_ANGLE: f32 = 0.50; // ~28 degrees
pub const ARROW_HEAD_BASE_LEN: f32 = 14.0;

/// Computes a pixelation block size scaled to the redaction rectangle (minimum 12 px).
#[inline]
pub fn redact_block_size(w: i32, h: i32) -> i32 {
    (w.min(h) / 8).max(MIN_REDACT_BLOCK_SIZE)
}

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

static TMP_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveFormat {
    Png,
    Jpeg,
    Bmp,
}

/// Determines the image format from a file path extension (defaults to PNG).
pub fn format_from_path(path: &Path) -> SaveFormat {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg" | "jpeg") => SaveFormat::Jpeg,
        Some("bmp") => SaveFormat::Bmp,
        _ => SaveFormat::Png,
    }
}

/// Ensures the path has a supported image extension (png, jpg, jpeg, bmp), appending `.png` if missing/unsupported.
pub fn ensure_supported_extension(path: &Path) -> PathBuf {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("png" | "jpg" | "jpeg" | "bmp") => path.to_path_buf(),
        _ => {
            let mut out = path.to_path_buf();
            let mut name = out
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            if name.is_empty() {
                name = default_filename();
            } else {
                name.push_str(".png");
            }
            out.set_file_name(name);
            out
        }
    }
}

/// Encodes an `RgbaImage` to bytes in the requested format (JPEG drops the alpha channel).
pub fn encode_image_bytes(img: &RgbaImage, fmt: SaveFormat) -> Result<Vec<u8>, image::ImageError> {
    let mut buf = Vec::new();
    let mut cursor = Cursor::new(&mut buf);
    match fmt {
        SaveFormat::Png => {
            img.write_to(&mut cursor, ImageFormat::Png)?;
        }
        SaveFormat::Jpeg => {
            let rgb = DynamicImage::ImageRgba8(img.clone()).into_rgb8();
            rgb.write_to(&mut cursor, ImageFormat::Jpeg)?;
        }
        SaveFormat::Bmp => {
            img.write_to(&mut cursor, ImageFormat::Bmp)?;
        }
    }
    Ok(buf)
}

fn create_temp_file_in_dir(dir: &Path) -> Result<(PathBuf, fs::File), std::io::Error> {
    let pid = std::process::id();
    for _ in 0..64 {
        let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_path = dir.join(format!(".localshot.tmp.{}.{}", pid, seq));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)
        {
            Ok(file) => return Ok((tmp_path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate temporary file",
    ))
}

/// Writes `img` atomically to `target_path` by writing to a temp file in the same directory and renaming.
pub fn save_image_atomic(
    img: &RgbaImage,
    target_path: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let fmt = format_from_path(target_path);
    let bytes = encode_image_bytes(img, fmt)?;
    let parent = target_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;

    let (tmp_path, mut file) = create_temp_file_in_dir(parent)?;
    let write_res = (|| -> std::io::Result<()> {
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp_path, target_path)?;
        Ok(())
    })();

    if let Err(e) = write_res {
        let _ = fs::remove_file(&tmp_path);
        return Err(Box::new(e));
    }
    Ok(())
}

/// Sanitizes a filename component (stem or extension) by removing path separators,
/// traversal dots, control characters, and Windows-forbidden filename characters.
pub fn sanitize_filename_part(input: &str, fallback: &str) -> String {
    let cleaned: String = input
        .chars()
        .filter(|&c| {
            !c.is_control() && !matches!(c, '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*')
        })
        .collect();
    let trimmed = cleaned.trim_matches(['.', ' ']);
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Reserves a non-colliding path in `dir` via `OpenOptions::create_new(true)` with a numeric suffix,
/// then atomically writes `img` into it.
pub fn save_image_unique_in_dir(
    img: &RgbaImage,
    dir: &Path,
    filename: &str,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    fs::create_dir_all(dir)?;
    let base_path = Path::new(filename);
    let stem = sanitize_filename_part(
        base_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("LocalShot"),
        "LocalShot",
    );
    let ext = sanitize_filename_part(
        base_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png"),
        "png",
    );

    let mut reserved_path = None;
    for idx in 0..10_000u32 {
        let candidate_name = if idx == 0 {
            format!("{}.{}", stem, ext)
        } else {
            format!("{}_{}.{}", stem, idx, ext)
        };
        let candidate = dir.join(candidate_name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                drop(file);
                reserved_path = Some(candidate);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(Box::new(e)),
        }
    }

    let target = reserved_path.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "exhausted numeric suffixes for screenshot file",
        )
    })?;

    if let Err(e) = save_image_atomic(img, &target) {
        let _ = fs::remove_file(&target);
        return Err(e);
    }
    Ok(target)
}

/// Generates human-readable local-time filename: LocalShot_YYYY-MM-DD_HHMMSS.png
pub fn default_filename() -> String {
    let now = chrono::Local::now();
    format!("LocalShot_{}.png", now.format("%Y-%m-%d_%H%M%S"))
}

/// Converts HSV (`h` in 0..360, `s` in 0..=1, `v` in 0..=1) to opaque `[R, G, B, 255]`.
pub fn hsv_to_rgba(h: f32, s: f32, v: f32) -> [u8; 4] {
    let h = h.rem_euclid(360.0);
    let s = s.clamp(0.0, 1.0);
    let v = v.clamp(0.0, 1.0);

    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;

    let (r1, g1, b1) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };

    [
        ((r1 + m) * 255.0).round() as u8,
        ((g1 + m) * 255.0).round() as u8,
        ((b1 + m) * 255.0).round() as u8,
        255,
    ]
}

/// Converts `[R, G, B, _]` to `(h: 0..360, s: 0..=1, v: 0..=1)`.
pub fn rgba_to_hsv(rgba: [u8; 4]) -> (f32, f32, f32) {
    let r = rgba[0] as f32 / 255.0;
    let g = rgba[1] as f32 / 255.0;
    let b = rgba[2] as f32 / 255.0;

    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let h = if delta < 1e-6 {
        0.0
    } else if (max - r).abs() < 1e-6 {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if (max - g).abs() < 1e-6 {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };

    let s = if max < 1e-6 { 0.0 } else { delta / max };
    (h.rem_euclid(360.0), s, max)
}

/// Parses `#RRGGBB` or `RRGGBB` (case-insensitive, trimmed) into `[R, G, B, 255]`.
pub fn parse_hex_color(input: &str) -> Option<[u8; 4]> {
    let trimmed = input.trim();
    let hex = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some([r, g, b, 255])
}

/// Formats `[R, G, B, _]` as lowercase `#rrggbb`.
pub fn format_hex_color(rgba: [u8; 4]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgba[0], rgba[1], rgba[2])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserConfig {
    pub auto_update_check: bool,
    pub hotkey_conflict_warned: bool,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            auto_update_check: true,
            hotkey_conflict_warned: false,
        }
    }
}

/// Returns the path to `config.json` in the user's config directory.
pub fn user_config_path() -> Option<PathBuf> {
    dirs::config_dir()
        .or_else(dirs::data_local_dir)
        .map(|p| p.join("localshot").join("config.json"))
}

/// Parses a `UserConfig` from JSON, falling back to defaults for missing or invalid fields.
pub fn parse_user_config(raw: &str) -> UserConfig {
    let mut cfg = UserConfig::default();
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(v) = val.get("auto_update_check").and_then(|v| v.as_bool()) {
            cfg.auto_update_check = v;
        }
        if let Some(v) = val.get("hotkey_conflict_warned").and_then(|v| v.as_bool()) {
            cfg.hotkey_conflict_warned = v;
        }
    }
    cfg
}

/// Serializes `UserConfig` as pretty JSON.
pub fn serialize_user_config(cfg: &UserConfig) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "auto_update_check": cfg.auto_update_check,
        "hotkey_conflict_warned": cfg.hotkey_conflict_warned,
    }))
    .unwrap_or_else(|_| "{}".to_string())
}

/// Loads `UserConfig` from disk, returning defaults if the file is absent or unreadable.
pub fn load_user_config() -> UserConfig {
    let Some(path) = user_config_path() else {
        return UserConfig::default();
    };
    match fs::read_to_string(&path) {
        Ok(raw) => parse_user_config(&raw),
        Err(_) => UserConfig::default(),
    }
}

/// Saves `UserConfig` to disk, creating parent directories if needed.
pub fn save_user_config(cfg: &UserConfig) -> std::io::Result<()> {
    let Some(path) = user_config_path() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "config directory unavailable",
        ));
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serialize_user_config(cfg))
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
    fn test_default_filename_local_format() {
        let name = default_filename();
        assert!(name.starts_with("LocalShot_"));
        assert!(name.ends_with(".png"));
        assert_eq!(name.len(), "LocalShot_2026-11-22_234000.png".len());
    }

    #[test]
    fn test_format_and_extension_selection() {
        assert_eq!(format_from_path(Path::new("a.png")), SaveFormat::Png);
        assert_eq!(format_from_path(Path::new("a.JPG")), SaveFormat::Jpeg);
        assert_eq!(format_from_path(Path::new("a.jpeg")), SaveFormat::Jpeg);
        assert_eq!(format_from_path(Path::new("a.bmp")), SaveFormat::Bmp);
        assert_eq!(format_from_path(Path::new("a.webp")), SaveFormat::Png);

        assert_eq!(
            ensure_supported_extension(Path::new("shot")),
            PathBuf::from("shot.png")
        );
        assert_eq!(
            ensure_supported_extension(Path::new("shot.jpg")),
            PathBuf::from("shot.jpg")
        );
        assert_eq!(
            ensure_supported_extension(Path::new("shot.BMP")),
            PathBuf::from("shot.BMP")
        );
    }

    #[test]
    fn test_encode_all_formats_and_atomic_unique_save() {
        let img = RgbaImage::from_pixel(4, 4, image::Rgba([10, 20, 30, 128]));
        assert!(!encode_image_bytes(&img, SaveFormat::Png)
            .unwrap()
            .is_empty());
        assert!(!encode_image_bytes(&img, SaveFormat::Jpeg)
            .unwrap()
            .is_empty());
        assert!(!encode_image_bytes(&img, SaveFormat::Bmp)
            .unwrap()
            .is_empty());

        let tmp_dir =
            std::env::temp_dir().join(format!("localshot_cfg_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp_dir);
        let p1 = save_image_unique_in_dir(&img, &tmp_dir, "shot.png").unwrap();
        let p2 = save_image_unique_in_dir(&img, &tmp_dir, "shot.png").unwrap();
        assert_ne!(p1, p2);
        assert!(p1.ends_with("shot.png"));
        assert!(p2.ends_with("shot_1.png"));
        assert!(p1.exists() && p2.exists());
        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_hsv_and_hex_color_roundtrip() {
        assert_eq!(hsv_to_rgba(0.0, 1.0, 1.0), [255, 0, 0, 255]);
        assert_eq!(hsv_to_rgba(120.0, 1.0, 1.0), [0, 255, 0, 255]);
        assert_eq!(hsv_to_rgba(240.0, 1.0, 1.0), [0, 0, 255, 255]);
        assert_eq!(hsv_to_rgba(0.0, 0.0, 1.0), [255, 255, 255, 255]);
        assert_eq!(hsv_to_rgba(0.0, 0.0, 0.0), [0, 0, 0, 255]);

        let (h, s, v) = rgba_to_hsv([229, 57, 53, 255]);
        let back = hsv_to_rgba(h, s, v);
        assert!((back[0] as i32 - 229).abs() <= 1);
        assert!((back[1] as i32 - 57).abs() <= 1);
        assert!((back[2] as i32 - 53).abs() <= 1);

        assert_eq!(parse_hex_color("#e53935"), Some([229, 57, 53, 255]));
        assert_eq!(parse_hex_color(" 1E88E5 "), Some([30, 136, 229, 255]));
        assert_eq!(parse_hex_color("#12345"), None);
        assert_eq!(parse_hex_color("#zzzzzz"), None);
        assert_eq!(format_hex_color([30, 136, 229, 255]), "#1e88e5");
    }

    #[test]
    fn test_user_config_roundtrip_and_defaults() {
        let def = UserConfig::default();
        assert!(def.auto_update_check);
        assert!(!def.hotkey_conflict_warned);

        let parsed_invalid = parse_user_config("not json");
        assert_eq!(parsed_invalid, def);

        let custom = UserConfig {
            auto_update_check: false,
            hotkey_conflict_warned: true,
        };
        let raw = serialize_user_config(&custom);
        assert_eq!(parse_user_config(&raw), custom);
    }
}

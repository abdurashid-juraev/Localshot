use image::RgbaImage;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use std::error::Error;
use xcap::Monitor;

pub struct CapturedFrame {
    pub raw_image: RgbaImage,
    pub slint_image: Image,
    pub width: u32,
    pub height: u32,
}

/// Abstract screen capturing service for SOLID Dependency Inversion & testability
pub trait ScreenCapturer {
    fn capture(&self) -> Result<CapturedFrame, Box<dyn Error>>;
}

pub struct HardwareScreenCapturer;

impl ScreenCapturer for HardwareScreenCapturer {
    fn capture(&self) -> Result<CapturedFrame, Box<dyn Error>> {
        capture_screen()
    }
}

/// Captures the primary or first active monitor entirely in RAM.
pub fn capture_screen() -> Result<CapturedFrame, Box<dyn Error>> {
    let monitors = Monitor::all()?;
    if monitors.is_empty() {
        return Err("No monitor detected on system for screen capture".into());
    }

    // Try primary monitor first; fallback to first available monitor if primary flag is not set
    let monitor = monitors
        .iter()
        .find(|m| m.is_primary())
        .or_else(|| monitors.first())
        .ok_or("Failed to select active monitor")?;

    let raw_image = monitor.capture_image()?;
    let (width, height) = raw_image.dimensions();

    let slint_image = rgba_to_slint_image(&raw_image);

    Ok(CapturedFrame {
        raw_image,
        slint_image,
        width,
        height,
    })
}

/// Converts an in-memory image::RgbaImage directly into slint::Image without disk I/O.
/// Uses fast SIMD memcpy via make_mut_bytes().
pub fn rgba_to_slint_image(rgba: &RgbaImage) -> Image {
    let (width, height) = rgba.dimensions();
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    buffer.make_mut_bytes().copy_from_slice(rgba.as_raw());
    Image::from_rgba8(buffer)
}

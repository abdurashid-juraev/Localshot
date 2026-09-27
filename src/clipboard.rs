use arboard::{Clipboard, ImageData};
use image::RgbaImage;
use std::borrow::Cow;
use std::error::Error;

/// Abstract clipboard service for SOLID Dependency Inversion & testability
pub trait ClipboardService {
    fn copy_image(&self, image: &RgbaImage) -> Result<(), Box<dyn Error>>;
}

pub struct SystemClipboard;

impl ClipboardService for SystemClipboard {
    fn copy_image(&self, image: &RgbaImage) -> Result<(), Box<dyn Error>> {
        copy_rgba_to_clipboard(image).map_err(|e| Box::new(e) as Box<dyn Error>)
    }
}

/// Copies an in-memory RgbaImage directly to the OS clipboard without any disk I/O.
pub fn copy_rgba_to_clipboard(image: &RgbaImage) -> Result<(), arboard::Error> {
    let mut clipboard = Clipboard::new()?;
    let (width, height) = image.dimensions();
    let img_data = ImageData {
        width: width as usize,
        height: height as usize,
        bytes: Cow::Borrowed(image.as_raw()),
    };
    clipboard.set_image(img_data)?;
    Ok(())
}

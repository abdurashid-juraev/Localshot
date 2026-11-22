use arboard::{Clipboard, ImageData};
use image::RgbaImage;
use std::borrow::Cow;
use std::error::Error;
#[cfg(target_os = "linux")]
use std::sync::mpsc;
#[cfg(target_os = "linux")]
use std::sync::OnceLock;
#[cfg(target_os = "linux")]
use zeroize::Zeroize;

/// Abstract clipboard service for SOLID Dependency Inversion & testability
pub trait ClipboardService {
    fn copy_image(&self, image: &RgbaImage) -> Result<(), Box<dyn Error>>;
}

pub struct SystemClipboard;

impl ClipboardService for SystemClipboard {
    fn copy_image(&self, image: &RgbaImage) -> Result<(), Box<dyn Error>> {
        copy_rgba_to_clipboard(image).map_err(Box::<dyn Error>::from)
    }
}

#[cfg(target_os = "linux")]
struct LinuxClipboardRequest {
    image: RgbaImage,
    reply: mpsc::SyncSender<Result<(), String>>,
}

#[cfg(target_os = "linux")]
static LINUX_CLIPBOARD_TX: OnceLock<mpsc::Sender<LinuxClipboardRequest>> = OnceLock::new();

#[cfg(target_os = "linux")]
fn linux_clipboard_sender() -> &'static mpsc::Sender<LinuxClipboardRequest> {
    LINUX_CLIPBOARD_TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<LinuxClipboardRequest>();
        let _ = std::thread::Builder::new()
            .name("localshot-clipboard".into())
            .spawn(move || {
                // Dedicated long-lived thread owns the arboard::Clipboard instance so
                // X11/Wayland selection contents survive after the overlay closes.
                let mut clipboard = Clipboard::new().ok();
                while let Ok(mut req) = rx.recv() {
                    if clipboard.is_none() {
                        clipboard = Clipboard::new().ok();
                    }
                    let res = match clipboard.as_mut() {
                        Some(cb) => {
                            let (width, height) = req.image.dimensions();
                            let img_data = ImageData {
                                width: width as usize,
                                height: height as usize,
                                bytes: Cow::Borrowed(req.image.as_raw()),
                            };
                            match cb.set_image(img_data) {
                                Ok(()) => Ok(()),
                                Err(e) => {
                                    // Re-initialize on next attempt in case the display connection reset
                                    clipboard = None;
                                    Err(e.to_string())
                                }
                            }
                        }
                        None => Err("Failed to initialize system clipboard".to_string()),
                    };
                    req.image.as_mut().zeroize();
                    let _ = req.reply.send(res);
                }
            });
        tx
    })
}

/// Copies an in-memory RgbaImage directly to the OS clipboard without any disk I/O.
/// On Linux in daemon mode, sends the image to a dedicated long-lived thread that owns
/// `arboard::Clipboard` so the clipboard remains valid after the overlay hides.
/// On Windows, sets the default bitmap format via `arboard` and also publishes registered `"PNG"`.
pub fn copy_rgba_to_clipboard(image: &RgbaImage) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let req = LinuxClipboardRequest {
            image: image.clone(),
            reply: reply_tx,
        };
        linux_clipboard_sender()
            .send(req)
            .map_err(|e| format!("Clipboard worker unavailable: {}", e))?;
        reply_rx
            .recv()
            .map_err(|e| format!("Clipboard worker failed to reply: {}", e))?
    }

    #[cfg(not(target_os = "linux"))]
    {
        let mut clipboard = Clipboard::new().map_err(|e| e.to_string())?;
        let (width, height) = image.dimensions();
        let img_data = ImageData {
            width: width as usize,
            height: height as usize,
            bytes: Cow::Borrowed(image.as_raw()),
        };
        clipboard.set_image(img_data).map_err(|e| e.to_string())?;

        #[cfg(target_os = "windows")]
        {
            if let Err(e) = publish_windows_png_format(image) {
                eprintln!(
                    "LocalShot: Warning - failed to publish PNG clipboard format: {}",
                    e
                );
            }
        }

        Ok(())
    }
}

/// Fallback for `--oneshot` mode without a running daemon:
/// On Linux, uses `SetExtLinux::wait()` so the process stays alive until another application
/// takes clipboard ownership.
#[allow(dead_code)]
pub fn copy_rgba_oneshot(image: &RgbaImage) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        use arboard::SetExtLinux;
        let mut clipboard = Clipboard::new().map_err(|e| e.to_string())?;
        let (width, height) = image.dimensions();
        let img_data = ImageData {
            width: width as usize,
            height: height as usize,
            bytes: Cow::Borrowed(image.as_raw()),
        };
        clipboard
            .set()
            .wait()
            .image(img_data)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    {
        copy_rgba_to_clipboard(image)
    }
}

#[cfg(target_os = "windows")]
fn publish_windows_png_format(image: &RgbaImage) -> Result<(), String> {
    use crate::config::{encode_image_bytes, SaveFormat};
    use std::ffi::c_void;

    let png_bytes = encode_image_bytes(image, SaveFormat::Png).map_err(|e| e.to_string())?;
    if png_bytes.is_empty() {
        return Err("Encoded PNG buffer is empty".to_string());
    }

    const GMEM_MOVEABLE: u32 = 0x0002;

    extern "system" {
        fn RegisterClipboardFormatW(lpszFormat: *const u16) -> u32;
        fn OpenClipboard(hWndNewOwner: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn SetClipboardData(uFormat: u32, hMem: *mut c_void) -> *mut c_void;
        fn GlobalAlloc(uFlags: u32, dwBytes: usize) -> *mut c_void;
        fn GlobalLock(hMem: *mut c_void) -> *mut u8;
        fn GlobalUnlock(hMem: *mut c_void) -> i32;
        fn GlobalFree(hMem: *mut c_void) -> *mut c_void;
    }

    let format_name: Vec<u16> = "PNG\0".encode_utf16().collect();
    let png_format = unsafe { RegisterClipboardFormatW(format_name.as_ptr()) };
    if png_format == 0 {
        return Err("RegisterClipboardFormatW(\"PNG\") failed".to_string());
    }

    let mut opened = false;
    for _ in 0..5 {
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            opened = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    if !opened {
        return Err("OpenClipboard failed".to_string());
    }

    let result = (|| -> Result<(), String> {
        let hmem = unsafe { GlobalAlloc(GMEM_MOVEABLE, png_bytes.len()) };
        if hmem.is_null() {
            return Err("GlobalAlloc failed for PNG clipboard data".to_string());
        }
        let ptr = unsafe { GlobalLock(hmem) };
        if ptr.is_null() {
            unsafe { GlobalFree(hmem) };
            return Err("GlobalLock failed".to_string());
        }
        unsafe {
            std::ptr::copy_nonoverlapping(png_bytes.as_ptr(), ptr, png_bytes.len());
            GlobalUnlock(hmem);
        }
        if unsafe { SetClipboardData(png_format, hmem) }.is_null() {
            unsafe { GlobalFree(hmem) };
            return Err("SetClipboardData(\"PNG\") failed".to_string());
        }
        Ok(())
    })();

    unsafe {
        CloseClipboard();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_png_encoding_for_clipboard_is_valid() {
        let img = RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]));
        let bytes = crate::config::encode_image_bytes(&img, crate::config::SaveFormat::Png)
            .expect("PNG encode");
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    }
}

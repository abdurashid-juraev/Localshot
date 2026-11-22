use image::RgbaImage;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use std::error::Error;
use xcap::Monitor;

#[allow(dead_code)]
pub struct CapturedFrame {
    pub raw_image: RgbaImage,
    pub slint_image: Image,
    pub x: i32,
    pub y: i32,
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

/// Queries the physical desktop coordinates (x, y) of the mouse cursor.
/// Returns None if querying the cursor fails (e.g. running in headless CI or Wayland restricted sandbox).
pub fn get_cursor_pos() -> Option<(i32, i32)> {
    #[cfg(target_os = "windows")]
    {
        #[repr(C)]
        struct POINT {
            x: i32,
            y: i32,
        }
        extern "system" {
            fn GetCursorPos(lpPoint: *mut POINT) -> i32;
        }
        let mut pt = POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut pt) } != 0 {
            Some((pt.x, pt.y))
        } else {
            None
        }
    }

    #[cfg(target_os = "linux")]
    {
        let (conn, screen_num) = xcb::Connection::connect(None).ok()?;
        let setup = conn.get_setup();
        let screen = setup.roots().nth(screen_num as usize)?;
        let cookie = conn.send_request(&xcb::x::QueryPointer {
            window: screen.root(),
        });
        let reply = conn.wait_for_reply(cookie).ok()?;
        Some((reply.root_x() as i32, reply.root_y() as i32))
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        None
    }
}

/// Captures the monitor under the cursor (falling back to primary or first available monitor).
/// Handles negative monitor origins (monitors positioned left of or above primary).
pub fn capture_screen() -> Result<CapturedFrame, Box<dyn Error>> {
    let monitors = Monitor::all()?;
    if monitors.is_empty() {
        return Err("No monitor detected on system for screen capture".into());
    }

    // Try monitor under the cursor first; fallback to primary monitor; then first available
    let cursor_pos = get_cursor_pos();
    let monitor = cursor_pos
        .and_then(|(cx, cy)| Monitor::from_point(cx, cy).ok())
        .or_else(|| monitors.iter().find(|m| m.is_primary()).cloned())
        .or_else(|| monitors.first().cloned())
        .ok_or("Failed to select active monitor")?;

    let x = monitor.x();
    let y = monitor.y();
    let raw_image = monitor.capture_image()?;
    let (width, height) = raw_image.dimensions();

    let slint_image = rgba_to_slint_image(&raw_image);

    Ok(CapturedFrame {
        raw_image,
        slint_image,
        x,
        y,
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

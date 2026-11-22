//! Coordinate conversion module.
//!
//! Centralizes ALL conversions between Slint logical pixels and frame physical pixels.
//! In LocalShot, ALL internal state (selection bounds, annotations, canvas dimensions)
//! lives strictly in PHYSICAL pixels of the captured desktop frame. Slint UI events and
//! properties represent logical pixels.
//!
//! To prevent scaling discrepancies across high-DPI displays (125%, 150%, 200%), no
//! other module is permitted to multiply or divide by the window scale factor directly.

/// Converts a logical coordinate or dimension to physical pixels (as float).
#[inline]
#[allow(dead_code)]
pub fn logical_to_physical(logical: f32, scale: f32) -> f32 {
    logical * scale
}

/// Converts a physical coordinate or dimension to logical pixels (as float).
#[inline]
pub fn physical_to_logical(physical: f32, scale: f32) -> f32 {
    if scale > 0.0 {
        physical / scale
    } else {
        physical
    }
}

/// Converts a logical coordinate to a physical pixel integer, using standard rounding (`.round()`).
#[inline]
pub fn logical_to_physical_round(logical: f32, scale: f32) -> i32 {
    (logical * scale).round() as i32
}

/// Converts a logical length/dimension to a physical pixel unsigned integer, clamped >= 0.
#[inline]
#[allow(dead_code)]
pub fn logical_to_physical_u32(logical: f32, scale: f32) -> u32 {
    (logical * scale).round().max(0.0) as u32
}

/// Converts a logical point (x, y) to physical pixel integers.
#[inline]
pub fn logical_to_physical_point(x: f32, y: f32, scale: f32) -> (i32, i32) {
    (
        logical_to_physical_round(x, scale),
        logical_to_physical_round(y, scale),
    )
}

/// Converts a physical point (x, y) to logical coordinates (as float).
#[inline]
#[allow(dead_code)]
pub fn physical_to_logical_point(x: i32, y: i32, scale: f32) -> (f32, f32) {
    (
        physical_to_logical(x as f32, scale),
        physical_to_logical(y as f32, scale),
    )
}

/// Converts a physical rectangle (x, y, w, h) to logical coordinates.
#[inline]
pub fn physical_to_logical_rect(
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    scale: f32,
) -> (f32, f32, f32, f32) {
    (
        physical_to_logical(x as f32, scale),
        physical_to_logical(y as f32, scale),
        physical_to_logical(w as f32, scale),
        physical_to_logical(h as f32, scale),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCALES: [f32; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];

    #[test]
    fn test_scales_basic_conversions() {
        for &scale in &SCALES {
            let logical_val = 100.0_f32;
            let phys = logical_to_physical(logical_val, scale);
            let expected_phys = logical_val * scale;
            assert!((phys - expected_phys).abs() < 1e-5);

            let back_logical = physical_to_logical(phys, scale);
            assert!((back_logical - logical_val).abs() < 1e-5);
        }
    }

    #[test]
    fn test_scale_100_percent() {
        let scale = 1.0_f32;
        assert_eq!(logical_to_physical_round(150.0, scale), 150);
        assert_eq!(logical_to_physical_u32(150.0, scale), 150);
        assert_eq!(logical_to_physical_point(10.0, 25.0, scale), (10, 25));
        assert_eq!(physical_to_logical_point(10, 25, scale), (10.0, 25.0));
        assert_eq!(
            physical_to_logical_rect(10, 20, 300, 200, scale),
            (10.0, 20.0, 300.0, 200.0)
        );
    }

    #[test]
    fn test_scale_125_percent() {
        let scale = 1.25_f32;
        // 100 * 1.25 = 125
        assert_eq!(logical_to_physical_round(100.0, scale), 125);
        assert_eq!(physical_to_logical(125.0, scale), 100.0);

        // Rounding: 101 * 1.25 = 126.25 -> 126
        assert_eq!(logical_to_physical_round(101.0, scale), 126);
        // Rounding: 102 * 1.25 = 127.5 -> 128
        assert_eq!(logical_to_physical_round(102.0, scale), 128);
    }

    #[test]
    fn test_scale_150_percent() {
        let scale = 1.5_f32;
        // 100 * 1.5 = 150
        assert_eq!(logical_to_physical_round(100.0, scale), 150);
        assert_eq!(physical_to_logical(150.0, scale), 100.0);

        // Rounding: 101 * 1.5 = 151.5 -> 152
        assert_eq!(logical_to_physical_round(101.0, scale), 152);
        // Rounding: 103 * 1.5 = 154.5 -> 155
        assert_eq!(logical_to_physical_round(103.0, scale), 155);

        let (x, y, w, h) = physical_to_logical_rect(150, 300, 600, 450, scale);
        assert_eq!((x, y, w, h), (100.0, 200.0, 400.0, 300.0));
    }

    #[test]
    fn test_scale_175_percent() {
        let scale = 1.75_f32;
        // 100 * 1.75 = 175
        assert_eq!(logical_to_physical_round(100.0, scale), 175);
        assert_eq!(physical_to_logical(175.0, scale), 100.0);

        // Rounding: 101 * 1.75 = 176.75 -> 177
        assert_eq!(logical_to_physical_round(101.0, scale), 177);
        // Rounding: 102 * 1.75 = 178.5 -> 179
        assert_eq!(logical_to_physical_round(102.0, scale), 179);
    }

    #[test]
    fn test_scale_200_percent() {
        let scale = 2.0_f32;
        // 100 * 2.0 = 200
        assert_eq!(logical_to_physical_round(100.0, scale), 200);
        assert_eq!(physical_to_logical(200.0, scale), 100.0);
        assert_eq!(logical_to_physical_point(50.0, 75.0, scale), (100, 150));
        assert_eq!(physical_to_logical_point(100, 150, scale), (50.0, 75.0));

        let (x, y, w, h) = physical_to_logical_rect(200, 400, 800, 600, scale);
        assert_eq!((x, y, w, h), (100.0, 200.0, 400.0, 300.0));
    }

    #[test]
    fn test_rounding_negative_coordinates() {
        let scale = 1.5_f32;
        // Negative coords occur on multi-monitor setups
        assert_eq!(logical_to_physical_round(-100.0, scale), -150);
        assert_eq!(logical_to_physical_round(-101.0, scale), -152);
        assert_eq!(physical_to_logical(-150.0, scale), -100.0);
    }

    #[test]
    fn test_zero_or_invalid_scale() {
        // Guard against division by zero
        assert_eq!(physical_to_logical(100.0, 0.0), 100.0);
        assert_eq!(physical_to_logical(100.0, -1.0), 100.0);
    }
}

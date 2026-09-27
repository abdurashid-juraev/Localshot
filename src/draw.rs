use crate::config;
use image::{Rgba, RgbaImage};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Select,
    Pen,
    Line,
    Arrow,
    Rectangle,
    Marker,
    Redact,
}

impl Tool {
    pub fn from_str_id(s: &str) -> Self {
        match s {
            "pen" => Self::Pen,
            "line" => Self::Line,
            "arrow" => Self::Arrow,
            "rect" => Self::Rectangle,
            "marker" => Self::Marker,
            "redact" => Self::Redact,
            _ => Self::Select,
        }
    }

    pub fn as_str_id(&self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Pen => "pen",
            Self::Line => "line",
            Self::Arrow => "arrow",
            Self::Rectangle => "rect",
            Self::Marker => "marker",
            Self::Redact => "redact",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Annotation {
    Stroke {
        points: Vec<(i32, i32)>,
        color: [u8; 4],
        thickness: i32,
    },
    Line {
        start: (i32, i32),
        end: (i32, i32),
        color: [u8; 4],
        thickness: i32,
    },
    Arrow {
        start: (i32, i32),
        end: (i32, i32),
        color: [u8; 4],
        thickness: i32,
    },
    Rectangle {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        color: [u8; 4],
        thickness: i32,
    },
    Marker {
        points: Vec<(i32, i32)>,
        color: [u8; 4],
        thickness: i32,
    },
    Redact {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        block_size: i32,
    },
}

impl Annotation {
    /// Applies this annotation onto the given canvas buffer.
    pub fn apply(&self, canvas: &mut RgbaImage) {
        match self {
            Annotation::Stroke {
                points,
                color,
                thickness,
            } => {
                for i in 1..points.len() {
                    draw_line(canvas, points[i - 1], points[i], *color, *thickness, false);
                }
            }
            Annotation::Line {
                start,
                end,
                color,
                thickness,
            } => {
                draw_line(canvas, *start, *end, *color, *thickness, false);
            }
            Annotation::Arrow {
                start,
                end,
                color,
                thickness,
            } => {
                draw_arrow(canvas, *start, *end, *color, *thickness);
            }
            Annotation::Rectangle {
                x,
                y,
                w,
                h,
                color,
                thickness,
            } => {
                draw_rect_outline(canvas, *x, *y, *w, *h, *color, *thickness);
            }
            Annotation::Marker {
                points,
                color,
                thickness,
            } => {
                draw_marker(canvas, points, *color, *thickness);
            }
            Annotation::Redact {
                x,
                y,
                w,
                h,
                block_size,
            } => {
                apply_pixelation(canvas, *x, *y, *w, *h, *block_size);
            }
        }
    }

    /// Computes dirty bounding box for this annotation clamped to canvas bounds.
    pub fn bounding_box(&self, canvas_w: u32, canvas_h: u32) -> (u32, u32, u32, u32) {
        if canvas_w == 0 || canvas_h == 0 {
            return (0, 0, 0, 0);
        }

        let (min_x, min_y, max_x, max_y) = match self {
            Annotation::Stroke {
                points, thickness, ..
            } => {
                let r = (*thickness / 2).max(1) + 2;
                let mut x0 = i32::MAX;
                let mut y0 = i32::MAX;
                let mut x1 = i32::MIN;
                let mut y1 = i32::MIN;
                for &(px, py) in points {
                    x0 = x0.min(px - r);
                    y0 = y0.min(py - r);
                    x1 = x1.max(px + r);
                    y1 = y1.max(py + r);
                }
                (x0, y0, x1, y1)
            }
            Annotation::Line {
                start,
                end,
                thickness,
                ..
            } => {
                let r = (*thickness / 2).max(1) + 2;
                (
                    start.0.min(end.0) - r,
                    start.1.min(end.1) - r,
                    start.0.max(end.0) + r,
                    start.1.max(end.1) + r,
                )
            }
            Annotation::Arrow {
                start,
                end,
                thickness,
                ..
            } => {
                let pad = (*thickness).max(20) + 6;
                (
                    start.0.min(end.0) - pad,
                    start.1.min(end.1) - pad,
                    start.0.max(end.0) + pad,
                    start.1.max(end.1) + pad,
                )
            }
            Annotation::Rectangle {
                x,
                y,
                w,
                h,
                thickness,
                ..
            } => {
                let pad = (*thickness).max(2) + 2;
                (*x - pad, *y - pad, *x + *w + pad, *y + *h + pad)
            }
            Annotation::Marker {
                points, thickness, ..
            } => {
                let r = (*thickness / 2).max(1) + 2;
                let mut x0 = i32::MAX;
                let mut y0 = i32::MAX;
                let mut x1 = i32::MIN;
                let mut y1 = i32::MIN;
                for &(px, py) in points {
                    x0 = x0.min(px - r);
                    y0 = y0.min(py - r);
                    x1 = x1.max(px + r);
                    y1 = y1.max(py + r);
                }
                (x0, y0, x1, y1)
            }
            Annotation::Redact { x, y, w, h, .. } => (*x, *y, *x + *w, *y + *h),
        };

        let x0 = min_x.clamp(0, canvas_w as i32 - 1) as u32;
        let y0 = min_y.clamp(0, canvas_h as i32 - 1) as u32;
        let x1 = max_x.clamp(0, canvas_w as i32 - 1) as u32;
        let y1 = max_y.clamp(0, canvas_h as i32 - 1) as u32;

        let w = (x1.saturating_sub(x0) + 1).min(canvas_w - x0);
        let h = (y1.saturating_sub(y0) + 1).min(canvas_h - y0);

        (x0, y0, w, h)
    }
}

/// Renders a list of annotations on top of a base image entirely in memory.
pub fn render_annotations(base: &RgbaImage, annotations: &[Annotation]) -> RgbaImage {
    let mut canvas = base.clone();
    for ann in annotations {
        ann.apply(&mut canvas);
    }
    canvas
}

/// Bresenham-based thick line drawing with optional alpha blending
fn draw_line(
    canvas: &mut RgbaImage,
    p0: (i32, i32),
    p1: (i32, i32),
    color: [u8; 4],
    thickness: i32,
    alpha_blend: bool,
) {
    let (mut x0, mut y0) = p0;
    let (x1, y1) = p1;

    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;

    let radius = (thickness / 2).max(1);

    loop {
        fill_circle(canvas, x0, y0, radius, color, alpha_blend);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

/// Draws an arrow with directional arrowhead
fn draw_arrow(
    canvas: &mut RgbaImage,
    start: (i32, i32),
    end: (i32, i32),
    color: [u8; 4],
    thickness: i32,
) {
    // 1. Draw arrow shaft
    draw_line(canvas, start, end, color, thickness, false);

    // 2. Draw arrow head
    let dx = (end.0 - start.0) as f32;
    let dy = (end.1 - start.1) as f32;
    let length = (dx * dx + dy * dy).sqrt();
    if length < 5.0 {
        return;
    }

    let head_length = (config::ARROW_HEAD_BASE_LEN + (thickness as f32) * 2.0).min(length * 0.4);
    let head_angle = config::ARROW_HEAD_ANGLE;

    let angle = dy.atan2(dx);
    let left_angle = angle + std::f32::consts::PI - head_angle;
    let right_angle = angle + std::f32::consts::PI + head_angle;

    let left_x = end.0 + (head_length * left_angle.cos()) as i32;
    let left_y = end.1 + (head_length * left_angle.sin()) as i32;

    let right_x = end.0 + (head_length * right_angle.cos()) as i32;
    let right_y = end.1 + (head_length * right_angle.sin()) as i32;

    draw_line(canvas, end, (left_x, left_y), color, thickness + 1, false);
    draw_line(canvas, end, (right_x, right_y), color, thickness + 1, false);
}

/// Draws hollow rectangle outline
fn draw_rect_outline(
    canvas: &mut RgbaImage,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: [u8; 4],
    thickness: i32,
) {
    let x2 = x + w;
    let y2 = y + h;

    draw_line(canvas, (x, y), (x2, y), color, thickness, false);
    draw_line(canvas, (x2, y), (x2, y2), color, thickness, false);
    draw_line(canvas, (x2, y2), (x, y2), color, thickness, false);
    draw_line(canvas, (x, y2), (x, y), color, thickness, false);
}

/// Fills circle for brush dab
fn fill_circle(
    canvas: &mut RgbaImage,
    cx: i32,
    cy: i32,
    radius: i32,
    color: [u8; 4],
    alpha_blend: bool,
) {
    let (width, height) = canvas.dimensions();
    let r2 = radius * radius;

    let min_x = (cx - radius).max(0) as u32;
    let max_x = (cx + radius).min(width as i32 - 1).max(0) as u32;
    let min_y = (cy - radius).max(0) as u32;
    let max_y = (cy + radius).min(height as i32 - 1).max(0) as u32;

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = x as i32 - cx;
            let dy = y as i32 - cy;
            if dx * dx + dy * dy <= r2 {
                if alpha_blend {
                    blend_pixel(canvas, x, y, color);
                } else {
                    canvas.put_pixel(x, y, Rgba(color));
                }
            }
        }
    }
}

/// Standard alpha blending onto canvas pixel
fn blend_pixel(canvas: &mut RgbaImage, x: u32, y: u32, src: [u8; 4]) {
    let dst = canvas.get_pixel(x, y);
    let alpha = src[3] as f32 / 255.0;
    let inv_alpha = 1.0 - alpha;

    let r = (src[0] as f32 * alpha + dst[0] as f32 * inv_alpha).min(255.0) as u8;
    let g = (src[1] as f32 * alpha + dst[1] as f32 * inv_alpha).min(255.0) as u8;
    let b = (src[2] as f32 * alpha + dst[2] as f32 * inv_alpha).min(255.0) as u8;

    canvas.put_pixel(x, y, Rgba([r, g, b, 255]));
}

/// Privacy Redact/Blur pixelation block (Zero disk residue)
fn apply_pixelation(
    canvas: &mut RgbaImage,
    rx: i32,
    ry: i32,
    rw: i32,
    rh: i32,
    block_size: i32,
) {
    let (img_w, img_h) = canvas.dimensions();
    let bs = block_size.max(4);

    let start_x = rx.max(0) as u32;
    let start_y = ry.max(0) as u32;
    let end_x = (rx + rw).min(img_w as i32).max(0) as u32;
    let end_y = (ry + rh).min(img_h as i32).max(0) as u32;

    for by in (start_y..end_y).step_by(bs as usize) {
        for bx in (start_x..end_x).step_by(bs as usize) {
            let cur_w = (bs as u32).min(end_x - bx);
            let cur_h = (bs as u32).min(end_y - by);

            let mut sum_r: u64 = 0;
            let mut sum_g: u64 = 0;
            let mut sum_b: u64 = 0;
            let mut count: u64 = 0;

            for y in by..(by + cur_h) {
                for x in bx..(bx + cur_w) {
                    let p = canvas.get_pixel(x, y);
                    sum_r += p[0] as u64;
                    sum_g += p[1] as u64;
                    sum_b += p[2] as u64;
                    count += 1;
                }
            }

            if count > 0 {
                let avg_r = (sum_r / count) as u8;
                let avg_g = (sum_g / count) as u8;
                let avg_b = (sum_b / count) as u8;
                let avg_color = Rgba([avg_r, avg_g, avg_b, 255]);

                for y in by..(by + cur_h) {
                    for x in bx..(bx + cur_w) {
                        canvas.put_pixel(x, y, avg_color);
                    }
                }
            }
        }
    }
}

/// Copies a sub-rectangle of pixels from src into dst without allocating new buffers.
pub fn copy_rect(src: &RgbaImage, dst: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32) {
    let max_x = (x + w).min(src.width()).min(dst.width());
    let max_y = (y + h).min(src.height()).min(dst.height());
    for py in y..max_y {
        for px in x..max_x {
            dst.put_pixel(px, py, *src.get_pixel(px, py));
        }
    }
}

/// Draws uniform semi-transparent highlighter stroke without overlapping dark blemishes
fn draw_marker(
    canvas: &mut RgbaImage,
    points: &[(i32, i32)],
    color: [u8; 4],
    thickness: i32,
) {
    if points.is_empty() {
        return;
    }

    let radius = (thickness / 2).max(1);
    let (img_w, img_h) = canvas.dimensions();

    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;

    for &(px, py) in points {
        min_x = min_x.min(px - radius);
        min_y = min_y.min(py - radius);
        max_x = max_x.max(px + radius);
        max_y = max_y.max(py + radius);
    }

    let x0 = min_x.clamp(0, img_w as i32 - 1) as u32;
    let y0 = min_y.clamp(0, img_h as i32 - 1) as u32;
    let x1 = max_x.clamp(0, img_w as i32 - 1) as u32;
    let y1 = max_y.clamp(0, img_h as i32 - 1) as u32;

    let bbox_w = (x1.saturating_sub(x0) + 1) as usize;
    let bbox_h = (y1.saturating_sub(y0) + 1) as usize;

    if bbox_w == 0 || bbox_h == 0 {
        return;
    }

    let mut mask = vec![false; bbox_w * bbox_h];

    if points.len() == 1 {
        mark_circle_mask(&mut mask, x0, y0, bbox_w, bbox_h, points[0].0, points[0].1, radius);
    } else {
        for i in 1..points.len() {
            mark_line_mask(&mut mask, x0, y0, bbox_w, bbox_h, points[i - 1], points[i], radius);
        }
    }

    let mut marker_color = color;
    marker_color[3] = config::MARKER_ALPHA;

    for ly in 0..bbox_h {
        for lx in 0..bbox_w {
            if mask[ly * bbox_w + lx] {
                let px = x0 + lx as u32;
                let py = y0 + ly as u32;
                blend_pixel(canvas, px, py, marker_color);
            }
        }
    }
}

fn mark_circle_mask(
    mask: &mut [bool],
    origin_x: u32,
    origin_y: u32,
    bbox_w: usize,
    bbox_h: usize,
    cx: i32,
    cy: i32,
    radius: i32,
) {
    let r2 = radius * radius;
    let min_x = (cx - radius - origin_x as i32).max(0) as usize;
    let max_x = (cx + radius - origin_x as i32).min(bbox_w as i32 - 1).max(0) as usize;
    let min_y = (cy - radius - origin_y as i32).max(0) as usize;
    let max_y = (cy + radius - origin_y as i32).min(bbox_h as i32 - 1).max(0) as usize;

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = (x as i32 + origin_x as i32) - cx;
            let dy = (y as i32 + origin_y as i32) - cy;
            if dx * dx + dy * dy <= r2 {
                mask[y * bbox_w + x] = true;
            }
        }
    }
}

fn mark_line_mask(
    mask: &mut [bool],
    origin_x: u32,
    origin_y: u32,
    bbox_w: usize,
    bbox_h: usize,
    p0: (i32, i32),
    p1: (i32, i32),
    radius: i32,
) {
    let (mut x0, mut y0) = p0;
    let (x1, y1) = p1;

    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;

    loop {
        mark_circle_mask(mask, origin_x, origin_y, bbox_w, bbox_h, x0, y0, radius);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_str_conversions() {
        let tools = [
            (Tool::Select, "select"),
            (Tool::Pen, "pen"),
            (Tool::Line, "line"),
            (Tool::Arrow, "arrow"),
            (Tool::Rectangle, "rect"),
            (Tool::Marker, "marker"),
            (Tool::Redact, "redact"),
        ];

        for (tool, str_id) in tools {
            assert_eq!(tool.as_str_id(), str_id);
            assert_eq!(Tool::from_str_id(str_id), tool);
        }
        assert_eq!(Tool::from_str_id("unknown"), Tool::Select);
    }

    #[test]
    fn test_annotation_rendering() {
        let canvas = RgbaImage::from_pixel(100, 100, Rgba([0, 0, 0, 255]));

        let annotations = vec![
            Annotation::Line {
                start: (10, 10),
                end: (90, 10),
                color: [255, 0, 0, 255],
                thickness: 2,
            },
            Annotation::Rectangle {
                x: 20,
                y: 20,
                w: 40,
                h: 40,
                color: [0, 255, 0, 255],
                thickness: 2,
            },
            Annotation::Redact {
                x: 30,
                y: 30,
                w: 20,
                h: 20,
                block_size: 4,
            },
        ];

        let result = render_annotations(&canvas, &annotations);
        assert_eq!(result.dimensions(), (100, 100));

        // The pixel on the red line should have been modified
        let pixel = result.get_pixel(50, 10);
        assert_eq!(pixel[0], 255); // Red component set
    }
}


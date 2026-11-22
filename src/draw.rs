use crate::config;
use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};
use image::{Rgba, RgbaImage};
use std::sync::OnceLock;
use tiny_skia::{
    BlendMode, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Stroke, StrokeDash,
    Transform,
};

const EMBEDDED_FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/DejaVuSans.ttf");

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
    Text,
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
            "text" => Self::Text,
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
            Self::Text => "text",
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
        solid_color: Option<[u8; 4]>,
    },
    Text {
        x: i32,
        y: i32,
        text: String,
        color: [u8; 4],
        font_size: f32,
    },
}

/// Computes dirty bounding coordinates for a slice of 2D points with radius padding
#[inline]
fn compute_points_bounds(points: &[(i32, i32)], radius: i32) -> (i32, i32, i32, i32) {
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
    (min_x, min_y, max_x, max_y)
}

impl Annotation {
    /// Applies this annotation onto the given canvas buffer using antialiased `tiny-skia` rasterization.
    pub fn apply(&self, canvas: &mut RgbaImage) {
        let (canvas_w, canvas_h) = canvas.dimensions();
        if canvas_w == 0 || canvas_h == 0 {
            return;
        }

        match self {
            Annotation::Redact {
                x,
                y,
                w,
                h,
                block_size,
                solid_color,
            } => {
                if let Some(color) = solid_color {
                    apply_solid_fill(canvas, *x, *y, *w, *h, *color);
                } else {
                    apply_pixelation(canvas, *x, *y, *w, *h, *block_size);
                }
            }
            _ => {
                let (bx, by, bw, bh) = self.bounding_box(canvas_w, canvas_h);
                if bw == 0 || bh == 0 {
                    return;
                }
                if let Some(pixmap) = self.render_to_pixmap(bx as i32, by as i32, bw, bh) {
                    composite_premultiplied_pixmap_onto_rgba(&pixmap, canvas, bx, by);
                }
            }
        }
    }

    /// Renders this vector annotation into a transparent premultiplied `tiny_skia::Pixmap`
    /// covering `(ox, oy, w, h)` in physical canvas coordinates.
    pub fn render_to_pixmap(&self, ox: i32, oy: i32, w: u32, h: u32) -> Option<Pixmap> {
        let mut pixmap = Pixmap::new(w, h)?;
        let tx = -(ox as f32);
        let ty = -(oy as f32);
        let transform = Transform::from_translate(tx, ty);

        match self {
            Annotation::Stroke {
                points,
                color,
                thickness,
            } => {
                draw_poly_stroke(
                    pixmap.as_mut(),
                    points,
                    *color,
                    (*thickness).max(1) as f32,
                    transform,
                );
            }
            Annotation::Line {
                start,
                end,
                color,
                thickness,
            } => {
                draw_segment_stroke(
                    pixmap.as_mut(),
                    *start,
                    *end,
                    *color,
                    (*thickness).max(1) as f32,
                    transform,
                );
            }
            Annotation::Arrow {
                start,
                end,
                color,
                thickness,
            } => {
                draw_arrow_skia(
                    pixmap.as_mut(),
                    *start,
                    *end,
                    *color,
                    (*thickness).max(1) as f32,
                    transform,
                );
            }
            Annotation::Rectangle {
                x,
                y,
                w: rw,
                h: rh,
                color,
                thickness,
            } => {
                draw_rect_skia(
                    pixmap.as_mut(),
                    *x,
                    *y,
                    *rw,
                    *rh,
                    *color,
                    (*thickness).max(1) as f32,
                    transform,
                );
            }
            Annotation::Marker {
                points,
                color,
                thickness,
            } => {
                let mut marker_color = *color;
                marker_color[3] = config::MARKER_ALPHA;
                draw_poly_stroke(
                    pixmap.as_mut(),
                    points,
                    marker_color,
                    (*thickness).max(1) as f32,
                    transform,
                );
            }
            Annotation::Redact {
                x,
                y,
                w: rw,
                h: rh,
                solid_color,
                ..
            } => {
                if let Some(color) = solid_color {
                    fill_rect_skia(pixmap.as_mut(), *x, *y, *rw, *rh, *color, transform);
                } else {
                    draw_dashed_rect_skia(pixmap.as_mut(), *x, *y, *rw, *rh, transform);
                }
            }
            Annotation::Text {
                x,
                y,
                text,
                color,
                font_size,
            } => {
                draw_text_onto_pixmap(&mut pixmap, *x - ox, *y - oy, text, *color, *font_size);
            }
        }

        Some(pixmap)
    }

    /// Computes dirty bounding box for this annotation clamped to canvas bounds.
    pub fn bounding_box(&self, canvas_w: u32, canvas_h: u32) -> (u32, u32, u32, u32) {
        if canvas_w == 0 || canvas_h == 0 {
            return (0, 0, 0, 0);
        }

        let (min_x, min_y, max_x, max_y) = match self {
            Annotation::Stroke {
                points, thickness, ..
            }
            | Annotation::Marker {
                points, thickness, ..
            } => {
                let r = (*thickness / 2).max(1) + 2;
                compute_points_bounds(points, r)
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
            Annotation::Redact { x, y, w, h, .. } => (*x - 2, *y - 2, *x + *w + 2, *y + *h + 2),
            Annotation::Text {
                x,
                y,
                text,
                font_size,
                ..
            } => {
                let (tw, th) = measure_text_bounds(text, *font_size);
                (*x - 2, *y - 2, *x + tw + 4, *y + th + 4)
            }
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

/// Converts straight RGBA `[u8; 4]` into a `tiny_skia::Paint` (which internally handles premultiplication).
#[inline]
fn paint_from_rgba(color: [u8; 4]) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
    paint.anti_alias = true;
    paint.blend_mode = BlendMode::SourceOver;
    paint
}

/// Builds a polyline `tiny_skia::Path` from integer points.
fn build_poly_path(points: &[(i32, i32)]) -> Option<Path> {
    if points.is_empty() {
        return None;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(points[0].0 as f32 + 0.5, points[0].1 as f32 + 0.5);
    if points.len() == 1 {
        // Zero-length move_to+line_to with Round cap produces an antialiased circular dot.
        pb.line_to(points[0].0 as f32 + 0.501, points[0].1 as f32 + 0.5);
    } else {
        for &(px, py) in &points[1..] {
            pb.line_to(px as f32 + 0.5, py as f32 + 0.5);
        }
    }
    pb.finish()
}

/// Draws a single antialiased polyline with round caps and round joins.
fn draw_poly_stroke(
    mut pixmap: tiny_skia::PixmapMut<'_>,
    points: &[(i32, i32)],
    color: [u8; 4],
    width: f32,
    transform: Transform,
) {
    let Some(path) = build_poly_path(points) else {
        return;
    };
    let paint = paint_from_rgba(color);
    let stroke = Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, transform, None);
}

/// Draws a single antialiased line segment with round caps.
fn draw_segment_stroke(
    mut pixmap: tiny_skia::PixmapMut<'_>,
    p0: (i32, i32),
    p1: (i32, i32),
    color: [u8; 4],
    width: f32,
    transform: Transform,
) {
    let mut pb = PathBuilder::new();
    pb.move_to(p0.0 as f32 + 0.5, p0.1 as f32 + 0.5);
    if p0 == p1 {
        pb.line_to(p1.0 as f32 + 0.501, p1.1 as f32 + 0.5);
    } else {
        pb.line_to(p1.0 as f32 + 0.5, p1.1 as f32 + 0.5);
    }
    let Some(path) = pb.finish() else {
        return;
    };
    let paint = paint_from_rgba(color);
    let stroke = Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, transform, None);
}

/// Draws an antialiased arrow: round-capped shaft + filled polygon arrowhead.
fn draw_arrow_skia(
    mut pixmap: tiny_skia::PixmapMut<'_>,
    start: (i32, i32),
    end: (i32, i32),
    color: [u8; 4],
    width: f32,
    transform: Transform,
) {
    let dx = (end.0 - start.0) as f32;
    let dy = (end.1 - start.1) as f32;
    let length = (dx * dx + dy * dy).sqrt();
    if length < 1.0 {
        return;
    }

    let head_length = (config::ARROW_HEAD_BASE_LEN + width * 2.0).min(length * 0.4);
    let angle = dy.atan2(dx);
    let tip_x = end.0 as f32 + 0.5;
    let tip_y = end.1 as f32 + 0.5;

    // Shorten shaft slightly so it terminates cleanly inside the filled arrowhead base.
    let shaft_end_x = tip_x - (head_length * 0.65) * angle.cos();
    let shaft_end_y = tip_y - (head_length * 0.65) * angle.sin();

    let mut shaft_pb = PathBuilder::new();
    shaft_pb.move_to(start.0 as f32 + 0.5, start.1 as f32 + 0.5);
    shaft_pb.line_to(shaft_end_x, shaft_end_y);
    if let Some(shaft_path) = shaft_pb.finish() {
        let paint = paint_from_rgba(color);
        let stroke = Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        pixmap.stroke_path(&shaft_path, &paint, &stroke, transform, None);
    }

    if length < 5.0 {
        return;
    }

    let head_angle = config::ARROW_HEAD_ANGLE;
    let left_angle = angle + std::f32::consts::PI - head_angle;
    let right_angle = angle + std::f32::consts::PI + head_angle;

    let left_x = tip_x + head_length * left_angle.cos();
    let left_y = tip_y + head_length * left_angle.sin();
    let right_x = tip_x + head_length * right_angle.cos();
    let right_y = tip_y + head_length * right_angle.sin();

    let mut head_pb = PathBuilder::new();
    head_pb.move_to(tip_x, tip_y);
    head_pb.line_to(left_x, left_y);
    head_pb.line_to(right_x, right_y);
    head_pb.close();

    if let Some(head_path) = head_pb.finish() {
        let paint = paint_from_rgba(color);
        pixmap.fill_path(&head_path, &paint, FillRule::Winding, transform, None);
    }
}

/// Draws an antialiased rectangle outline with miter joins.
#[allow(clippy::too_many_arguments)]
fn draw_rect_skia(
    mut pixmap: tiny_skia::PixmapMut<'_>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: [u8; 4],
    width: f32,
    transform: Transform,
) {
    if w <= 0 || h <= 0 {
        return;
    }
    let Some(rect) = tiny_skia::Rect::from_xywh(x as f32 + 0.5, y as f32 + 0.5, w as f32, h as f32)
    else {
        return;
    };
    let path = PathBuilder::from_rect(rect);
    let paint = paint_from_rgba(color);
    let stroke = Stroke {
        width,
        line_cap: LineCap::Square,
        line_join: LineJoin::Miter,
        miter_limit: 4.0,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, transform, None);
}

/// Draws a solid-filled rectangle into a `Pixmap`.
fn fill_rect_skia(
    mut pixmap: tiny_skia::PixmapMut<'_>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: [u8; 4],
    transform: Transform,
) {
    if w <= 0 || h <= 0 {
        return;
    }
    let Some(rect) = tiny_skia::Rect::from_xywh(x as f32, y as f32, w as f32, h as f32) else {
        return;
    };
    let path = PathBuilder::from_rect(rect);
    let paint = paint_from_rgba(color);
    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
}

/// Draws a dashed rectangle outline for live Redact preview (pixelation applied only on mouse-up).
fn draw_dashed_rect_skia(
    mut pixmap: tiny_skia::PixmapMut<'_>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    transform: Transform,
) {
    if w <= 0 || h <= 0 {
        return;
    }
    let Some(rect) = tiny_skia::Rect::from_xywh(x as f32 + 0.5, y as f32 + 0.5, w as f32, h as f32)
    else {
        return;
    };
    let path = PathBuilder::from_rect(rect);

    // Subtle dark backing + bright dashed foreground for visibility on any desktop wallpaper.
    let mut bg_paint = paint_from_rgba([15, 23, 42, 180]);
    bg_paint.anti_alias = true;
    let bg_stroke = Stroke {
        width: 2.0,
        line_join: LineJoin::Miter,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &bg_paint, &bg_stroke, transform, None);

    let fg_paint = paint_from_rgba([248, 250, 252, 240]);
    let dash = StrokeDash::new(vec![6.0, 4.0], 0.0);
    let fg_stroke = Stroke {
        width: 1.5,
        line_join: LineJoin::Miter,
        dash,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &fg_paint, &fg_stroke, transform, None);
}

/// Composites a premultiplied `tiny_skia::Pixmap` onto a straight-alpha `RgbaImage` at `(ox, oy)`.
/// Handles premultiplied -> straight alpha conversion explicitly using Porter-Duff SourceOver.
pub fn composite_premultiplied_pixmap_onto_rgba(
    pixmap: &Pixmap,
    canvas: &mut RgbaImage,
    ox: u32,
    oy: u32,
) {
    let (cw, ch) = canvas.dimensions();
    let pw = pixmap.width();
    let ph = pixmap.height();
    let max_x = pw.min(cw.saturating_sub(ox));
    let max_y = ph.min(ch.saturating_sub(oy));
    if max_x == 0 || max_y == 0 {
        return;
    }

    let pm_data = pixmap.data();
    let pm_stride = (pw * 4) as usize;
    let dst_stride = (cw * 4) as usize;
    let dst_raw = canvas.as_mut();

    for py in 0..max_y {
        let pm_row = py as usize * pm_stride;
        let dst_row = (oy + py) as usize * dst_stride + (ox * 4) as usize;
        for px in 0..max_x {
            let si = pm_row + (px * 4) as usize;
            let sa_u8 = pm_data[si + 3];
            if sa_u8 == 0 {
                continue;
            }
            let di = dst_row + (px * 4) as usize;
            if sa_u8 == 255 {
                // Fully opaque premultiplied == straight RGB
                dst_raw[di] = pm_data[si];
                dst_raw[di + 1] = pm_data[si + 1];
                dst_raw[di + 2] = pm_data[si + 2];
                dst_raw[di + 3] = 255;
                continue;
            }

            let sa = sa_u8 as f32 / 255.0;
            let inv_sa = 1.0 - sa;
            let da_u8 = dst_raw[di + 3];

            if da_u8 == 255 {
                // Fast path for opaque desktop canvas: out_a == 1.0, so no division needed
                let r = pm_data[si] as f32 + dst_raw[di] as f32 * inv_sa;
                let g = pm_data[si + 1] as f32 + dst_raw[di + 1] as f32 * inv_sa;
                let b = pm_data[si + 2] as f32 + dst_raw[di + 2] as f32 * inv_sa;
                dst_raw[di] = r.round().clamp(0.0, 255.0) as u8;
                dst_raw[di + 1] = g.round().clamp(0.0, 255.0) as u8;
                dst_raw[di + 2] = b.round().clamp(0.0, 255.0) as u8;
                dst_raw[di + 3] = 255;
            } else {
                let da = da_u8 as f32 / 255.0;
                let out_a = sa + da * inv_sa;
                if out_a <= 0.0 {
                    dst_raw[di..di + 4].fill(0);
                } else {
                    let out_pm_r = pm_data[si] as f32 + (dst_raw[di] as f32 * da) * inv_sa;
                    let out_pm_g = pm_data[si + 1] as f32 + (dst_raw[di + 1] as f32 * da) * inv_sa;
                    let out_pm_b = pm_data[si + 2] as f32 + (dst_raw[di + 2] as f32 * da) * inv_sa;
                    dst_raw[di] = (out_pm_r / out_a).round().clamp(0.0, 255.0) as u8;
                    dst_raw[di + 1] = (out_pm_g / out_a).round().clamp(0.0, 255.0) as u8;
                    dst_raw[di + 2] = (out_pm_b / out_a).round().clamp(0.0, 255.0) as u8;
                    dst_raw[di + 3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}

/// Converts a premultiplied `tiny_skia::Pixmap` to straight RGBA bytes in `dst`,
/// optionally scaling alpha by `alpha_scale` (e.g. `MARKER_ALPHA as f32 / 255.0`).
pub fn pixmap_to_straight_rgba_bytes(pixmap: &Pixmap, dst: &mut [u8], alpha_scale: f32) {
    let src = pixmap.data();
    let len = src.len().min(dst.len());
    for (s, d) in src[..len]
        .as_chunks::<4>()
        .0
        .iter()
        .zip(dst[..len].as_chunks_mut::<4>().0.iter_mut())
    {
        let a = s[3];
        if a == 0 {
            d[0] = 0;
            d[1] = 0;
            d[2] = 0;
            d[3] = 0;
        } else if a == 255 {
            d[0] = s[0];
            d[1] = s[1];
            d[2] = s[2];
            d[3] = if alpha_scale >= 1.0 {
                255
            } else {
                (255.0 * alpha_scale).round().clamp(0.0, 255.0) as u8
            };
        } else {
            let a_u16 = a as u16;
            let half = a_u16 / 2;
            d[0] = (((s[0] as u16 * 255) + half) / a_u16).min(255) as u8;
            d[1] = (((s[1] as u16 * 255) + half) / a_u16).min(255) as u8;
            d[2] = (((s[2] as u16 * 255) + half) / a_u16).min(255) as u8;
            d[3] = if alpha_scale >= 1.0 {
                a
            } else {
                (a as f32 * alpha_scale).round().clamp(0.0, 255.0) as u8
            };
        }
    }
}

/// Incremental stroke buffer for O(1) per-move Pen and Marker preview rendering.
/// Avoids O(N^2) full-path re-rasterization and avoids cloning `current_points` per move.
pub struct IncrementalStrokePreview {
    pixmap: Pixmap,
    origin_x: i32,
    origin_y: i32,
    last_pt: (i32, i32),
    dirty_min_x: i32,
    dirty_min_y: i32,
    dirty_max_x: i32,
    dirty_max_y: i32,
    thickness: i32,
    color: [u8; 4],
    is_marker: bool,
}

impl IncrementalStrokePreview {
    const GROW_MARGIN: i32 = 64;

    pub fn new(start: (i32, i32), color: [u8; 4], thickness: i32, is_marker: bool) -> Option<Self> {
        let pad = (thickness / 2).max(1) + 2;
        let half = pad + Self::GROW_MARGIN;
        let size = (half * 2 + 1).max(1) as u32;
        let origin_x = start.0 - half;
        let origin_y = start.1 - half;
        let mut pixmap = Pixmap::new(size, size)?;

        // For Marker, draw opaque into the scratch pixmap and apply MARKER_ALPHA once on export
        // so overlapping round caps at segment joints never darken.
        let draw_color = if is_marker {
            [color[0], color[1], color[2], 255]
        } else {
            color
        };

        let transform = Transform::from_translate(-(origin_x as f32), -(origin_y as f32));
        draw_segment_stroke(
            pixmap.as_mut(),
            start,
            start,
            draw_color,
            thickness.max(1) as f32,
            transform,
        );

        Some(Self {
            pixmap,
            origin_x,
            origin_y,
            last_pt: start,
            dirty_min_x: start.0 - pad,
            dirty_min_y: start.1 - pad,
            dirty_max_x: start.0 + pad,
            dirty_max_y: start.1 + pad,
            thickness,
            color,
            is_marker,
        })
    }

    /// Appends a single new point and rasterizes ONLY the new segment `last_pt -> pt`.
    pub fn push_point(&mut self, pt: (i32, i32)) {
        let pad = (self.thickness / 2).max(1) + 2;
        let p_min_x = pt.0 - pad;
        let p_min_y = pt.1 - pad;
        let p_max_x = pt.0 + pad;
        let p_max_y = pt.1 + pad;

        self.dirty_min_x = self.dirty_min_x.min(p_min_x);
        self.dirty_min_y = self.dirty_min_y.min(p_min_y);
        self.dirty_max_x = self.dirty_max_x.max(p_max_x);
        self.dirty_max_y = self.dirty_max_y.max(p_max_y);

        let cur_max_x = self.origin_x + self.pixmap.width() as i32 - 1;
        let cur_max_y = self.origin_y + self.pixmap.height() as i32 - 1;

        if p_min_x < self.origin_x
            || p_min_y < self.origin_y
            || p_max_x > cur_max_x
            || p_max_y > cur_max_y
        {
            let new_ox = self.origin_x.min(p_min_x - Self::GROW_MARGIN);
            let new_oy = self.origin_y.min(p_min_y - Self::GROW_MARGIN);
            let new_end_x = cur_max_x.max(p_max_x + Self::GROW_MARGIN);
            let new_end_y = cur_max_y.max(p_max_y + Self::GROW_MARGIN);
            let new_w = (new_end_x - new_ox + 1).max(1) as u32;
            let new_h = (new_end_y - new_oy + 1).max(1) as u32;

            if let Some(mut grown) = Pixmap::new(new_w, new_h) {
                let copy_x = (self.origin_x - new_ox) as u32;
                let copy_y = (self.origin_y - new_oy) as u32;
                let old_w = self.pixmap.width();
                let old_h = self.pixmap.height();
                let old_data = self.pixmap.data();
                let new_data = grown.data_mut();
                let old_stride = (old_w * 4) as usize;
                let new_stride = (new_w * 4) as usize;
                for row in 0..old_h as usize {
                    let s = row * old_stride;
                    let d = (copy_y as usize + row) * new_stride + (copy_x as usize * 4);
                    new_data[d..d + old_stride].copy_from_slice(&old_data[s..s + old_stride]);
                }
                self.pixmap = grown;
                self.origin_x = new_ox;
                self.origin_y = new_oy;
            }
        }

        let draw_color = if self.is_marker {
            [self.color[0], self.color[1], self.color[2], 255]
        } else {
            self.color
        };

        let transform = Transform::from_translate(-(self.origin_x as f32), -(self.origin_y as f32));
        draw_segment_stroke(
            self.pixmap.as_mut(),
            self.last_pt,
            pt,
            draw_color,
            self.thickness.max(1) as f32,
            transform,
        );
        self.last_pt = pt;
    }

    /// Exports only the tight clamped dirty bounding box as straight RGBA bytes for Slint preview.
    pub fn export_preview_rgba(
        &self,
        canvas_w: u32,
        canvas_h: u32,
    ) -> Option<(u32, u32, u32, u32, Vec<u8>)> {
        if canvas_w == 0 || canvas_h == 0 {
            return None;
        }
        let x0 = self.dirty_min_x.clamp(0, canvas_w as i32 - 1) as u32;
        let y0 = self.dirty_min_y.clamp(0, canvas_h as i32 - 1) as u32;
        let x1 = self.dirty_max_x.clamp(0, canvas_w as i32 - 1) as u32;
        let y1 = self.dirty_max_y.clamp(0, canvas_h as i32 - 1) as u32;
        let bw = (x1.saturating_sub(x0) + 1).min(canvas_w - x0);
        let bh = (y1.saturating_sub(y0) + 1).min(canvas_h - y0);
        if bw == 0 || bh == 0 {
            return None;
        }

        let alpha_scale = if self.is_marker {
            config::MARKER_ALPHA as f32 / 255.0
        } else {
            1.0
        };

        let mut out = vec![0u8; (bw * bh * 4) as usize];
        let pm_data = self.pixmap.data();
        let pm_w = self.pixmap.width() as i32;
        let pm_h = self.pixmap.height() as i32;
        let pm_stride = (self.pixmap.width() * 4) as usize;
        let out_stride = (bw * 4) as usize;

        for ry in 0..bh {
            let py = (y0 as i32 + ry as i32) - self.origin_y;
            if py < 0 || py >= pm_h {
                continue;
            }
            let pm_row = py as usize * pm_stride;
            let out_row = ry as usize * out_stride;
            for rx in 0..bw {
                let px = (x0 as i32 + rx as i32) - self.origin_x;
                if px < 0 || px >= pm_w {
                    continue;
                }
                let si = pm_row + (px as usize * 4);
                let di = out_row + (rx as usize * 4);
                let a = pm_data[si + 3];
                if a == 0 {
                    continue;
                } else if a == 255 {
                    out[di] = pm_data[si];
                    out[di + 1] = pm_data[si + 1];
                    out[di + 2] = pm_data[si + 2];
                    out[di + 3] = if alpha_scale >= 1.0 {
                        255
                    } else {
                        (255.0 * alpha_scale).round().clamp(0.0, 255.0) as u8
                    };
                } else {
                    let a_u16 = a as u16;
                    let half = a_u16 / 2;
                    out[di] = (((pm_data[si] as u16 * 255) + half) / a_u16).min(255) as u8;
                    out[di + 1] = (((pm_data[si + 1] as u16 * 255) + half) / a_u16).min(255) as u8;
                    out[di + 2] = (((pm_data[si + 2] as u16 * 255) + half) / a_u16).min(255) as u8;
                    out[di + 3] = if alpha_scale >= 1.0 {
                        a
                    } else {
                        (a as f32 * alpha_scale).round().clamp(0.0, 255.0) as u8
                    };
                }
            }
        }

        Some((x0, y0, bw, bh, out))
    }
}

/// Applies solid-color redaction directly onto `canvas`.
fn apply_solid_fill(canvas: &mut RgbaImage, rx: i32, ry: i32, rw: i32, rh: i32, color: [u8; 4]) {
    let (img_w, img_h) = canvas.dimensions();
    let start_x = rx.max(0) as u32;
    let start_y = ry.max(0) as u32;
    let end_x = (rx + rw).min(img_w as i32).max(0) as u32;
    let end_y = (ry + rh).min(img_h as i32).max(0) as u32;
    let fill = Rgba([color[0], color[1], color[2], 255]);

    for y in start_y..end_y {
        for x in start_x..end_x {
            canvas.put_pixel(x, y, fill);
        }
    }
}

/// Privacy Redact/Blur pixelation block (Zero disk residue).
///
/// SECURITY NOTE: Pixelation is NOT cryptographically safe for text or high-contrast
/// content—block averages can leak character shapes under reconstruction attacks.
/// Use solid-fill redaction (`solid_color = Some(...)`) when irreversible obfuscation
/// is required.
fn apply_pixelation(canvas: &mut RgbaImage, rx: i32, ry: i32, rw: i32, rh: i32, block_size: i32) {
    let (img_w, img_h) = canvas.dimensions();
    let bs = block_size.max(config::MIN_REDACT_BLOCK_SIZE);

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

            if let Some(nonzero_count) = std::num::NonZeroU64::new(count) {
                let div = nonzero_count.get();
                let avg_r = (sum_r / div) as u8;
                let avg_g = (sum_g / div) as u8;
                let avg_b = (sum_b / div) as u8;
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

/// Copies a sub-rectangle of pixels from src into dst using fast SIMD row-wise slice copy without allocating.
#[allow(dead_code)]
pub fn copy_rect(src: &RgbaImage, dst: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32) {
    let max_x = (x + w).min(src.width()).min(dst.width());
    let max_y = (y + h).min(src.height()).min(dst.height());
    if x >= max_x || y >= max_y {
        return;
    }
    let row_bytes = ((max_x - x) * 4) as usize;
    let src_stride = (src.width() * 4) as usize;
    let dst_stride = (dst.width() * 4) as usize;
    let src_raw = src.as_raw();
    let dst_raw = dst.as_mut();

    for py in y..max_y {
        let src_idx = py as usize * src_stride + (x * 4) as usize;
        let dst_idx = py as usize * dst_stride + (x * 4) as usize;
        dst_raw[dst_idx..dst_idx + row_bytes]
            .copy_from_slice(&src_raw[src_idx..src_idx + row_bytes]);
    }
}

fn embedded_font() -> Option<&'static FontRef<'static>> {
    static FONT: OnceLock<Option<FontRef<'static>>> = OnceLock::new();
    FONT.get_or_init(|| FontRef::try_from_slice(EMBEDDED_FONT_BYTES).ok())
        .as_ref()
}

fn system_font_db() -> &'static fontdb::Database {
    static DB: OnceLock<fontdb::Database> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        db
    })
}

fn with_fallback_font<R>(
    ch: char,
    primary: &FontRef<'_>,
    mut f: impl FnMut(&FontRef<'_>) -> R,
) -> R {
    if primary.glyph_id(ch).0 != 0 || ch.is_control() {
        return f(primary);
    }
    let db = system_font_db();
    for face in db.faces() {
        let mut found: Option<R> = None;
        let _ = db.with_face_data(face.id, |data, face_idx| {
            if let Ok(candidate) = FontRef::try_from_slice_and_index(data, face_idx) {
                if candidate.glyph_id(ch).0 != 0 {
                    found = Some(f(&candidate));
                }
            }
        });
        if let Some(res) = found {
            return res;
        }
    }
    f(primary)
}

/// Measures the pixel `(width, height)` required to render `text` at `font_size`.
pub fn measure_text_bounds(text: &str, font_size: f32) -> (i32, i32) {
    let Some(primary) = embedded_font() else {
        return (1, 1);
    };
    let scale = PxScale::from(font_size.max(8.0));
    let scaled = primary.as_scaled(scale);
    let line_height = (scaled.ascent() - scaled.descent() + scaled.line_gap()).max(font_size);

    let mut max_w: f32 = 1.0;
    let mut line_count: usize = 0;

    for line in text.split('\n') {
        line_count += 1;
        let mut pen_x: f32 = 0.0;
        let mut prev_glyph = None;
        for ch in line.chars() {
            let adv = with_fallback_font(ch, primary, |font| {
                let sf = font.as_scaled(scale);
                let gid = font.glyph_id(ch);
                let kern = prev_glyph.map(|p| sf.kern(p, gid)).unwrap_or(0.0);
                prev_glyph = Some(gid);
                kern + sf.h_advance(gid)
            });
            pen_x += adv;
        }
        max_w = max_w.max(pen_x);
    }

    let total_h = (line_count.max(1) as f32) * line_height;
    (max_w.ceil() as i32 + 4, total_h.ceil() as i32 + 4)
}

/// Rasterizes multi-line `text` onto a premultiplied `tiny_skia::Pixmap` at `(origin_x, origin_y)`.
fn draw_text_onto_pixmap(
    pixmap: &mut Pixmap,
    origin_x: i32,
    origin_y: i32,
    text: &str,
    color: [u8; 4],
    font_size: f32,
) {
    let Some(primary) = embedded_font() else {
        return;
    };
    let scale = PxScale::from(font_size.max(8.0));
    let primary_scaled = primary.as_scaled(scale);
    let ascent = primary_scaled.ascent();
    let line_height = (primary_scaled.ascent() - primary_scaled.descent()
        + primary_scaled.line_gap())
    .max(font_size);

    let pix_w = pixmap.width() as i32;
    let pix_h = pixmap.height() as i32;
    let data = pixmap.data_mut();

    let src_a = color[3] as f32 / 255.0;
    let src_r = color[0] as f32;
    let src_g = color[1] as f32;
    let src_b = color[2] as f32;

    for (line_idx, line) in text.split('\n').enumerate() {
        let mut pen_x = origin_x as f32;
        let baseline_y = origin_y as f32 + ascent + (line_idx as f32) * line_height;
        let mut prev_glyph = None;

        for ch in line.chars() {
            let adv = with_fallback_font(ch, primary, |font| {
                let sf = font.as_scaled(scale);
                let gid = font.glyph_id(ch);
                let kern = prev_glyph.map(|p| sf.kern(p, gid)).unwrap_or(0.0);
                pen_x += kern;
                prev_glyph = Some(gid);

                let glyph = gid.with_scale_and_position(scale, point(pen_x, baseline_y));
                if let Some(outlined) = font.outline_glyph(glyph) {
                    let bounds = outlined.px_bounds();
                    let min_x = bounds.min.x.floor() as i32;
                    let min_y = bounds.min.y.floor() as i32;
                    outlined.draw(|gx, gy, cov| {
                        let px = min_x + gx as i32;
                        let py = min_y + gy as i32;
                        if px < 0 || py < 0 || px >= pix_w || py >= pix_h {
                            return;
                        }
                        let alpha = (cov * src_a).clamp(0.0, 1.0);
                        if alpha <= 0.001 {
                            return;
                        }
                        let idx = ((py * pix_w + px) * 4) as usize;
                        let inv_a = 1.0 - alpha;
                        let dst_r = data[idx] as f32;
                        let dst_g = data[idx + 1] as f32;
                        let dst_b = data[idx + 2] as f32;
                        let dst_a = data[idx + 3] as f32;

                        data[idx] = (src_r * alpha + dst_r * inv_a).round().clamp(0.0, 255.0) as u8;
                        data[idx + 1] =
                            (src_g * alpha + dst_g * inv_a).round().clamp(0.0, 255.0) as u8;
                        data[idx + 2] =
                            (src_b * alpha + dst_b * inv_a).round().clamp(0.0, 255.0) as u8;
                        data[idx + 3] =
                            (255.0 * alpha + dst_a * inv_a).round().clamp(0.0, 255.0) as u8;
                    });
                }
                sf.h_advance(gid)
            });
            pen_x += adv;
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
            (Tool::Text, "text"),
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
                block_size: 12,
                solid_color: None,
            },
        ];

        let result = render_annotations(&canvas, &annotations);
        assert_eq!(result.dimensions(), (100, 100));

        // The pixel on the red line should have been modified
        let pixel = result.get_pixel(50, 10);
        assert_eq!(pixel[0], 255); // Red component set
    }

    fn encode_png_in_memory(img: &RgbaImage) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        img.write_to(&mut cursor, image::ImageFormat::Png)
            .expect("png encode");
        buf
    }

    fn assert_images_within_tolerance(actual: &RgbaImage, expected: &RgbaImage, max_diff: u8) {
        assert_eq!(actual.dimensions(), expected.dimensions());
        for (a, e) in actual.as_raw().iter().zip(expected.as_raw().iter()) {
            let diff = a.abs_diff(*e);
            assert!(
                diff <= max_diff,
                "pixel channel diff {} exceeds tolerance {}",
                diff,
                max_diff
            );
        }
    }

    #[test]
    fn test_golden_image_png_all_tools() {
        let base = RgbaImage::from_pixel(120, 120, Rgba([240, 240, 240, 255]));

        let cases: Vec<(&str, Annotation)> = vec![
            (
                "stroke",
                Annotation::Stroke {
                    points: vec![(15, 20), (45, 55), (85, 30)],
                    color: [229, 57, 53, 255],
                    thickness: 4,
                },
            ),
            (
                "line",
                Annotation::Line {
                    start: (10, 15),
                    end: (105, 95),
                    color: [30, 136, 229, 255],
                    thickness: 3,
                },
            ),
            (
                "arrow",
                Annotation::Arrow {
                    start: (20, 90),
                    end: (100, 25),
                    color: [67, 160, 71, 255],
                    thickness: 3,
                },
            ),
            (
                "rect",
                Annotation::Rectangle {
                    x: 20,
                    y: 25,
                    w: 70,
                    h: 50,
                    color: [253, 216, 53, 255],
                    thickness: 3,
                },
            ),
            (
                "marker",
                Annotation::Marker {
                    points: vec![(15, 60), (95, 60), (55, 20), (55, 100)],
                    color: [253, 216, 53, 255],
                    thickness: 12,
                },
            ),
            (
                "redact_pixelate",
                Annotation::Redact {
                    x: 20,
                    y: 20,
                    w: 48,
                    h: 48,
                    block_size: 12,
                    solid_color: None,
                },
            ),
            (
                "redact_solid",
                Annotation::Redact {
                    x: 20,
                    y: 20,
                    w: 40,
                    h: 40,
                    block_size: 12,
                    solid_color: Some([33, 33, 33, 255]),
                },
            ),
        ];

        for (name, ann) in cases {
            let rendered = render_annotations(&base, std::slice::from_ref(&ann));
            let png_bytes = encode_png_in_memory(&rendered);
            let decoded = image::load_from_memory_with_format(&png_bytes, image::ImageFormat::Png)
                .expect("png decode")
                .to_rgba8();
            assert_images_within_tolerance(&decoded, &rendered, 1);

            match &ann {
                Annotation::Line { .. } => {
                    // Diagonal line should produce antialiased edge pixels between background and stroke.
                    let has_aa = rendered
                        .pixels()
                        .any(|p| p[0] > 30 && p[0] < 240 && p[2] > 229 && p[2] < 240);
                    assert!(has_aa, "{name}: expected antialiased edge pixels");
                }
                Annotation::Marker { .. } => {
                    // Crossing marker at (55, 60) must have the same alpha blend as non-crossing (25, 60).
                    assert_eq!(
                        rendered.get_pixel(55, 60),
                        rendered.get_pixel(25, 60),
                        "{name}: overlapping marker stroke darkened"
                    );
                }
                Annotation::Redact {
                    solid_color: Some(c),
                    ..
                } => {
                    assert_eq!(rendered.get_pixel(35, 35), &Rgba([c[0], c[1], c[2], 255]));
                }
                _ => {}
            }
        }
    }

    #[test]
    fn test_incremental_stroke_matches_batch_and_scales() {
        let mut inc = IncrementalStrokePreview::new((10, 10), [229, 57, 53, 255], 4, false)
            .expect("incremental preview");
        inc.push_point((50, 40));
        inc.push_point((120, 90));
        let exported = inc.export_preview_rgba(200, 200).expect("export");
        assert!(exported.2 > 0 && exported.3 > 0);
        assert_eq!(exported.4.len(), (exported.2 * exported.3 * 4) as usize);
    }

    #[test]
    fn test_redact_block_size_min_12() {
        let mut img = RgbaImage::from_pixel(40, 40, Rgba([0, 0, 0, 255]));
        img.put_pixel(0, 0, Rgba([255, 255, 255, 255]));
        // Even if block_size = 4 is passed, apply_pixelation enforces min 12 px.
        apply_pixelation(&mut img, 0, 0, 24, 24, 4);
        assert_eq!(img.get_pixel(0, 0), img.get_pixel(11, 11));
        assert_ne!(img.get_pixel(0, 0), img.get_pixel(12, 12));
    }

    #[test]
    fn test_text_annotation_latin_cyrillic_uzbek_multiline() {
        let base = RgbaImage::from_pixel(240, 120, Rgba([16, 20, 28, 255]));
        let ann = Annotation::Text {
            x: 10,
            y: 10,
            text: "Hello\nЎзбек қғҳ".to_string(),
            color: [255, 255, 255, 255],
            font_size: 20.0,
        };
        let rendered = render_annotations(&base, &[ann]);
        let lit_pixels = rendered
            .pixels()
            .filter(|p| p[0] > 100 && p[1] > 100 && p[2] > 100)
            .count();
        assert!(
            lit_pixels > 20,
            "expected rasterized Latin + Uzbek Cyrillic glyphs, got {lit_pixels} lit pixels"
        );
    }
}

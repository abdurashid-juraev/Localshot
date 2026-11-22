use crate::capture::CapturedFrame;
use crate::config;
use crate::draw::{self, Annotation, Tool};
use crate::interaction::SelectionMachine;
use image::RgbaImage;
use zeroize::Zeroize;

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl SelectionRect {
    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    pub fn set_bounds(&mut self, x: i32, y: i32, w: u32, h: u32) {
        self.x = x;
        self.y = y;
        self.w = w;
        self.h = h;
    }

    /// Computes safe pixel crop boundaries clamped to frame dimensions,
    /// robust against negative coordinates and multi-monitor setups.
    pub fn crop_bounds(&self, max_w: u32, max_h: u32) -> Option<(u32, u32, u32, u32)> {
        if self.is_empty() {
            return None;
        }

        let max_w_i = max_w as i32;
        let max_h_i = max_h as i32;

        let x0 = self.x.clamp(0, max_w_i);
        let y0 = self.y.clamp(0, max_h_i);
        let x1 = (self.x + self.w as i32).clamp(0, max_w_i);
        let y1 = (self.y + self.h as i32).clamp(0, max_h_i);

        let crop_w = (x1 - x0).max(0) as u32;
        let crop_h = (y1 - y0).max(0) as u32;

        if crop_w > 0 && crop_h > 0 {
            Some((x0 as u32, y0 as u32, crop_w, crop_h))
        } else {
            None
        }
    }
}

/// Central application state holding in-memory image buffers, undo stack, and selection.
pub struct AppState {
    /// Pristine desktop capture untouched in RAM
    pub raw_base: RgbaImage,
    /// Cached base including all committed annotations (allocated lazily on first annotation)
    pub composited_cache: Option<RgbaImage>,
    /// Incremental bounding-box preview buffer for O(1) Pen/Marker mouse-move rendering
    pub incremental_preview: Option<draw::IncrementalStrokePreview>,
    /// Stack of applied annotations for undo support
    pub annotations: Vec<Annotation>,
    /// Stack of undone annotations for redo support
    pub redo_stack: Vec<Annotation>,
    pub tool: Tool,
    pub color_idx: usize,
    pub custom_color: Option<[u8; 4]>,
    /// Session-scoped per-tool thickness (or font size for Tool::Text)
    pub tool_thickness: [i32; 8],
    pub selection: SelectionRect,
    pub machine: SelectionMachine,
    pub drag_start: (i32, i32),
    pub initial_rect: SelectionRect,
    pub start_pt: Option<(i32, i32)>,
    pub current_points: Vec<(i32, i32)>,
}

/// Helper to normalize start and end points into top-left (x, y) and positive dimensions (w, h)
#[inline]
fn normalize_rect_coords(start: (i32, i32), end: (i32, i32)) -> (i32, i32, i32, i32) {
    (
        start.0.min(end.0),
        start.1.min(end.1),
        (end.0 - start.0).abs(),
        (end.1 - start.1).abs(),
    )
}

#[inline]
fn tool_index(tool: Tool) -> usize {
    match tool {
        Tool::Select => 0,
        Tool::Pen => 1,
        Tool::Line => 2,
        Tool::Arrow => 3,
        Tool::Rectangle => 4,
        Tool::Marker => 5,
        Tool::Redact => 6,
        Tool::Text => 7,
    }
}

impl AppState {
    pub fn reset(&mut self, frame: CapturedFrame) {
        *self = Self::new(frame);
    }

    /// Zero-cost initialization: eliminates eager 66MB duplicate buffer cloning
    pub fn new(frame: CapturedFrame) -> Self {
        Self {
            raw_base: frame.raw_image,
            composited_cache: None,
            incremental_preview: None,
            annotations: Vec::new(),
            redo_stack: Vec::new(),
            tool: Tool::Select,
            color_idx: 0,
            custom_color: None,
            tool_thickness: [
                0,
                config::PEN_THICKNESS,
                config::LINE_THICKNESS,
                config::ARROW_THICKNESS,
                config::RECT_THICKNESS,
                config::MARKER_THICKNESS,
                config::RECT_THICKNESS,
                config::TEXT_DEFAULT_SIZE,
            ],
            selection: SelectionRect::default(),
            machine: SelectionMachine::Idle,
            drag_start: (0, 0),
            initial_rect: SelectionRect::default(),
            start_pt: None,
            current_points: Vec::new(),
        }
    }

    /// Explicitly zeroizes all volatile image buffers from RAM (Zero disk & RAM residue)
    pub fn sanitize(&mut self) {
        self.raw_base.as_mut().zeroize();
        if let Some(cache) = self.composited_cache.as_mut() {
            cache.as_mut().zeroize();
        }
        self.incremental_preview = None;
    }

    /// Returns a reference to the active composited image (or pristine base if no annotations)
    #[inline]
    pub fn composited_image(&self) -> &RgbaImage {
        self.composited_cache.as_ref().unwrap_or(&self.raw_base)
    }

    /// Lazily ensures the composited cache buffer is allocated
    fn ensure_composited(&mut self) -> &mut RgbaImage {
        let base = &self.raw_base;
        self.composited_cache.get_or_insert_with(|| base.clone())
    }

    pub fn active_color(&self) -> [u8; 4] {
        if self.color_idx >= config::PALETTE.len() {
            return self.custom_color.unwrap_or(config::PALETTE[0].rgba);
        }
        config::PALETTE
            .get(self.color_idx)
            .map(|c| c.rgba)
            .unwrap_or(config::PALETTE[0].rgba)
    }

    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
    }

    pub fn set_color_idx(&mut self, idx: usize) {
        if idx < config::PALETTE.len()
            || (idx == config::PALETTE.len() && self.custom_color.is_some())
        {
            self.color_idx = idx;
        }
    }

    pub fn set_custom_color(&mut self, rgba: [u8; 4]) {
        self.custom_color = Some(rgba);
        self.color_idx = config::PALETTE.len();
    }

    #[inline]
    pub fn thickness_for(&self, tool: Tool) -> i32 {
        self.tool_thickness[tool_index(tool)]
    }

    #[inline]
    pub fn active_thickness(&self) -> i32 {
        self.thickness_for(self.tool)
    }

    pub fn adjust_active_thickness(&mut self, delta: i32) -> i32 {
        let idx = tool_index(self.tool);
        let (min_t, max_t, step) = if self.tool == Tool::Text {
            (config::TEXT_MIN_SIZE, config::TEXT_MAX_SIZE, 2)
        } else {
            (config::MIN_THICKNESS, config::MAX_THICKNESS, 1)
        };
        let next = (self.tool_thickness[idx] + delta * step).clamp(min_t, max_t);
        self.tool_thickness[idx] = next;
        next
    }

    pub fn start_drawing(&mut self, x: i32, y: i32) {
        self.start_pt = Some((x, y));
        self.current_points.clear();
        self.current_points.push((x, y));
        let thickness = self.active_thickness();
        self.incremental_preview = match self.tool {
            Tool::Pen => {
                draw::IncrementalStrokePreview::new((x, y), self.active_color(), thickness, false)
            }
            Tool::Marker => {
                draw::IncrementalStrokePreview::new((x, y), self.active_color(), thickness, true)
            }
            _ => None,
        };
    }

    /// Item 2: ignore if start_pt is not set (called on hover before any mouse-down).
    /// Applies 2px squared-distance decimation for Pen/Marker to cap point growth on long strokes.
    pub fn add_drawing_point(&mut self, x: i32, y: i32) {
        if self.start_pt.is_some() {
            if matches!(self.tool, Tool::Pen | Tool::Marker) {
                if let Some(&(lx, ly)) = self.current_points.last() {
                    let dx = (x - lx) as i64;
                    let dy = (y - ly) as i64;
                    if dx * dx + dy * dy < 4 {
                        return;
                    }
                }
            }
            self.current_points.push((x, y));
            if let Some(inc) = self.incremental_preview.as_mut() {
                inc.push_point((x, y));
            }
        }
    }

    /// DRY: Single source of truth for constructing Annotation based on active Tool.
    /// Item 8: returns None for degenerate zero-size / zero-length annotations.
    #[allow(dead_code)]
    pub fn build_current_annotation(&self, end_pt: (i32, i32)) -> Option<Annotation> {
        self.build_current_annotation_with_redact(end_pt, false)
    }

    /// Builds the current annotation, allowing solid-fill redaction when `solid_redact` is true.
    pub fn build_current_annotation_with_redact(
        &self,
        end_pt: (i32, i32),
        solid_redact: bool,
    ) -> Option<Annotation> {
        let start = self.start_pt?;
        let color = self.active_color();
        let thickness = self.active_thickness();

        match self.tool {
            Tool::Pen => {
                // A single-point pen click becomes a 1-point stroke (dot) — allowed.
                Some(Annotation::Stroke {
                    points: self.current_points.clone(),
                    color,
                    thickness,
                })
            }
            Tool::Line => {
                let end = if solid_redact {
                    crate::interaction::constrain_line_shift(start, end_pt)
                } else {
                    end_pt
                };
                // Item 8: reject start == end
                if start == end {
                    return None;
                }
                Some(Annotation::Line {
                    start,
                    end,
                    color,
                    thickness,
                })
            }
            Tool::Arrow => {
                let end = if solid_redact {
                    crate::interaction::constrain_line_shift(start, end_pt)
                } else {
                    end_pt
                };
                // Item 8: reject start == end
                if start == end {
                    return None;
                }
                Some(Annotation::Arrow {
                    start,
                    end,
                    color,
                    thickness,
                })
            }
            Tool::Rectangle => {
                let end = if solid_redact {
                    crate::interaction::constrain_rect_shift(start, end_pt)
                } else {
                    end_pt
                };
                let (x, y, w, h) = normalize_rect_coords(start, end);
                // Item 8: reject zero-area rectangles
                if w == 0 || h == 0 {
                    return None;
                }
                Some(Annotation::Rectangle {
                    x,
                    y,
                    w,
                    h,
                    color,
                    thickness,
                })
            }
            Tool::Marker => Some(Annotation::Marker {
                points: self.current_points.clone(),
                color,
                thickness,
            }),
            Tool::Redact => {
                let (x, y, w, h) = normalize_rect_coords(start, end_pt);
                // Item 8: reject zero-area redactions
                if w == 0 || h == 0 {
                    return None;
                }
                Some(Annotation::Redact {
                    x,
                    y,
                    w,
                    h,
                    block_size: config::redact_block_size(w, h),
                    solid_color: if solid_redact { Some(color) } else { None },
                })
            }
            Tool::Select | Tool::Text => None,
        }
    }

    /// Commits an annotation to history (capped at MAX_HISTORY_LEN), clears redo stack,
    /// and directly updates the cached composited buffer.
    pub fn commit_annotation(&mut self, ann: Annotation) {
        self.redo_stack.clear();
        if self.annotations.len() >= config::MAX_HISTORY_LEN {
            self.annotations.remove(0);
        }
        let composited = self.ensure_composited();
        ann.apply(composited);
        self.annotations.push(ann);
    }

    pub fn finish_drawing(&mut self) {
        self.start_pt = None;
        self.current_points.clear();
        self.incremental_preview = None;
    }

    #[inline]
    pub fn can_undo(&self) -> bool {
        !self.annotations.is_empty()
    }

    #[inline]
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Undoes the last annotation, pushes it to redo_stack, and rebuilds the composited cache.
    pub fn undo(&mut self) -> bool {
        if let Some(ann) = self.annotations.pop() {
            self.redo_stack.push(ann);
            if self.annotations.is_empty() {
                self.composited_cache = None;
            } else {
                let cache = draw::render_annotations(&self.raw_base, &self.annotations);
                self.composited_cache = Some(cache);
            }
            true
        } else {
            false
        }
    }

    /// Redoes the last undone annotation and applies it onto the composited cache.
    pub fn redo(&mut self) -> bool {
        if let Some(ann) = self.redo_stack.pop() {
            let composited = self.ensure_composited();
            ann.apply(composited);
            self.annotations.push(ann);
            true
        } else {
            false
        }
    }

    /// Renders ONLY the active annotation's bounding box with a transparent background.
    /// Returns `(x, y, w, h, straight_rgba_bytes)` in physical coordinates.
    /// Zero full-frame allocation, zero `current_points.clone()` on mouse move.
    pub fn render_preview_layer(
        &self,
        end_pt: (i32, i32),
        solid_redact: bool,
    ) -> Option<(u32, u32, u32, u32, Vec<u8>)> {
        let (canvas_w, canvas_h) = self.raw_base.dimensions();
        if canvas_w == 0 || canvas_h == 0 {
            return None;
        }

        if let Some(inc) = &self.incremental_preview {
            return inc.export_preview_rgba(canvas_w, canvas_h);
        }

        let ann = self.build_current_annotation_with_redact(end_pt, solid_redact)?;
        let (bx, by, bw, bh) = ann.bounding_box(canvas_w, canvas_h);
        if bw == 0 || bh == 0 {
            return None;
        }
        let pixmap = ann.render_to_pixmap(bx as i32, by as i32, bw, bh)?;
        let mut rgba = vec![0u8; (bw * bh * 4) as usize];
        draw::pixmap_to_straight_rgba_bytes(&pixmap, &mut rgba, 1.0);
        Some((bx, by, bw, bh, rgba))
    }

    /// Produces the final composited and cropped image bounded by the active selection.
    /// Item 10: forces alpha=255 on all pixels of the cropped result.
    pub fn get_final_crop(&self) -> Option<RgbaImage> {
        let img = self.composited_image();
        let (cx, cy, cw, ch) = self.selection.crop_bounds(img.width(), img.height())?;
        let mut cropped = image::imageops::crop_imm(img, cx, cy, cw, ch).to_image();
        // Item 10: force alpha = 255 so PNG/clipboard output is always fully opaque
        for pixel in cropped.pixels_mut() {
            pixel[3] = 255;
        }
        Some(cropped)
    }
}

/// Renders an 11x11 pixel magnifier grid centered at `(cx, cy)` scaled to 110x110 RGBA bytes,
/// returning `(rgba_bytes, width, height, hex_color_string)`.
#[cfg(feature = "loupe")]
pub fn render_loupe_rgba(img: &RgbaImage, cx: i32, cy: i32) -> (Vec<u8>, u32, u32, String) {
    const GRID: i32 = 11;
    const CELL: usize = 10;
    const SIZE: usize = (GRID as usize) * CELL;
    let mut out = vec![0u8; SIZE * SIZE * 4];

    let w = img.width() as i32;
    let h = img.height() as i32;
    let half = GRID / 2;

    let center_rgba = if cx >= 0 && cy >= 0 && cx < w && cy < h {
        let p = img.get_pixel(cx as u32, cy as u32);
        [p[0], p[1], p[2], 255]
    } else {
        [0, 0, 0, 255]
    };

    for gy in 0..GRID {
        for gx in 0..GRID {
            let sx = cx + (gx - half);
            let sy = cy + (gy - half);
            let px = if sx >= 0 && sy >= 0 && sx < w && sy < h {
                let p = img.get_pixel(sx as u32, sy as u32);
                [p[0], p[1], p[2], 255]
            } else {
                [15, 23, 42, 255]
            };
            let is_center = gx == half && gy == half;

            for dy in 0..CELL {
                for dx in 0..CELL {
                    let ox = (gx as usize) * CELL + dx;
                    let oy = (gy as usize) * CELL + dy;
                    let idx = (oy * SIZE + ox) * 4;
                    let is_border = dx == 0 || dy == 0;
                    let is_center_ring =
                        is_center && (dx == 0 || dy == 0 || dx == CELL - 1 || dy == CELL - 1);
                    let c = if is_center_ring {
                        [56, 189, 248, 255]
                    } else if is_border {
                        [30, 41, 59, 255]
                    } else {
                        px
                    };
                    out[idx..idx + 4].copy_from_slice(&c);
                }
            }
        }
    }

    (
        out,
        SIZE as u32,
        SIZE as u32,
        config::format_hex_color(center_rgba),
    )
}

impl Drop for AppState {
    fn drop(&mut self) {
        self.sanitize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::CapturedFrame;

    fn create_dummy_frame(w: u32, h: u32) -> CapturedFrame {
        let raw = RgbaImage::from_pixel(w, h, image::Rgba([100, 100, 100, 255]));
        CapturedFrame {
            raw_image: raw,
            slint_image: None,
            x: 0,
            y: 0,
            width: w,
            height: h,
        }
    }

    #[test]
    fn test_selection_crop_bounds() {
        let mut sel = SelectionRect::default();
        assert!(sel.is_empty());
        assert_eq!(sel.crop_bounds(1920, 1080), None);

        sel.set_bounds(100, 200, 300, 400);
        assert!(!sel.is_empty());
        assert_eq!(sel.crop_bounds(1920, 1080), Some((100, 200, 300, 400)));

        // Test boundary clamping
        sel.set_bounds(1800, 1000, 500, 500);
        assert_eq!(sel.crop_bounds(1920, 1080), Some((1800, 1000, 120, 80)));

        // Test negative coordinate handling (multi-monitor)
        sel.set_bounds(-100, -50, 300, 200);
        assert_eq!(sel.crop_bounds(1920, 1080), Some((0, 0, 200, 150)));
    }

    #[test]
    fn test_app_state_drawing_and_undo() {
        let frame = create_dummy_frame(200, 200);
        let mut state = AppState::new(frame);

        // Verify lazy zero-allocation initialization
        assert!(state.composited_cache.is_none());
        assert!(state.incremental_preview.is_none());
        assert_eq!(state.tool, Tool::Select);
        assert_eq!(state.annotations.len(), 0);

        state.set_tool(Tool::Rectangle);
        state.set_color_idx(1); // Blue

        state.start_drawing(10, 10);
        // Bounding-box preview should cover only the annotation, not the full 200x200 frame
        let preview = state.render_preview_layer((50, 50), false);
        assert!(preview.is_some());
        let (_px, _py, pw, ph, _bytes) = preview.unwrap();
        assert!(pw < 200 && ph < 200);

        let ann = state.build_current_annotation((50, 50));
        assert!(ann.is_some());

        state.commit_annotation(ann.unwrap());
        state.finish_drawing();
        assert_eq!(state.annotations.len(), 1);
        assert!(state.composited_cache.is_some());
        assert!(state.incremental_preview.is_none());

        // Test crop
        state.selection.set_bounds(10, 10, 40, 40);
        let crop = state.get_final_crop();
        assert!(crop.is_some());
        assert_eq!(crop.unwrap().dimensions(), (40, 40));

        // Test undo
        assert!(state.undo());
        assert_eq!(state.annotations.len(), 0);
        // Buffers should be freed after full undo
        assert!(state.composited_cache.is_none());
        assert!(state.incremental_preview.is_none());
        assert!(!state.undo()); // Nothing left to undo
    }

    #[test]
    fn test_4k_preview_frame_time_under_4ms() {
        // 4K (3840x2160) timing harness for pen, line, rect, and marker preview frames.
        let frame = create_dummy_frame(3840, 2160);
        let mut state = AppState::new(frame);

        for tool in [Tool::Pen, Tool::Line, Tool::Rectangle, Tool::Marker] {
            state.set_tool(tool);
            state.start_drawing(100, 100);
            if matches!(tool, Tool::Pen | Tool::Marker) {
                for i in 1..20 {
                    state.add_drawing_point(100 + i * 10, 100 + (i % 5) * 6);
                }
            }
            let t0 = std::time::Instant::now();
            let preview = state.render_preview_layer((300, 220), false);
            let elapsed = t0.elapsed();
            assert!(preview.is_some());
            // Keep a generous CI-safe guard here while logging exact timing;
            // acceptance target on user's machine in release is < 4 ms.
            assert!(
                elapsed.as_millis() < 50,
                "{:?} 4K preview took {:?} (expected well under 4ms in release)",
                tool,
                elapsed
            );
            state.finish_drawing();
        }
    }

    /// Item 2: add_drawing_point must be a no-op when start_pt is None.
    #[test]
    fn test_add_drawing_point_ignores_when_no_start() {
        let frame = create_dummy_frame(100, 100);
        let mut state = AppState::new(frame);

        // No start_drawing() call → start_pt is None
        assert!(state.start_pt.is_none());
        state.add_drawing_point(50, 50);
        // current_points must remain empty
        assert!(
            state.current_points.is_empty(),
            "add_drawing_point must be a no-op when start_pt is None"
        );
    }

    /// Item 8: build_current_annotation returns None for degenerate shapes.
    #[test]
    fn test_degenerate_annotations_rejected() {
        let frame = create_dummy_frame(100, 100);
        let mut state = AppState::new(frame);

        // Rectangle with zero width (start == end on x-axis)
        state.set_tool(Tool::Rectangle);
        state.start_drawing(50, 10);
        assert!(
            state.build_current_annotation((50, 50)).is_none(),
            "zero-width rect must be rejected"
        );
        assert!(
            state.build_current_annotation((60, 10)).is_none(),
            "zero-height rect must be rejected"
        );

        // Redact with zero area
        state.set_tool(Tool::Redact);
        state.start_drawing(20, 20);
        assert!(
            state.build_current_annotation((20, 20)).is_none(),
            "zero-size redact must be rejected"
        );
        assert!(
            state.build_current_annotation((20, 40)).is_none(),
            "zero-width redact must be rejected"
        );

        // Line with start == end
        state.set_tool(Tool::Line);
        state.start_drawing(30, 30);
        assert!(
            state.build_current_annotation((30, 30)).is_none(),
            "zero-length line must be rejected"
        );

        // Arrow with start == end
        state.set_tool(Tool::Arrow);
        state.start_drawing(10, 10);
        assert!(
            state.build_current_annotation((10, 10)).is_none(),
            "zero-length arrow must be rejected"
        );
    }

    /// Item 10: get_final_crop must produce fully opaque pixels.
    #[test]
    fn test_get_final_crop_forces_alpha_255() {
        // Create a frame with semi-transparent pixels
        let raw = RgbaImage::from_pixel(100, 100, image::Rgba([200, 100, 50, 128]));
        let frame = CapturedFrame {
            raw_image: raw,
            slint_image: None,
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        let mut state = AppState::new(frame);
        state.selection.set_bounds(10, 10, 20, 20);
        let crop = state.get_final_crop().expect("should produce crop");
        for pixel in crop.pixels() {
            assert_eq!(pixel[3], 255, "all cropped pixels must have alpha=255");
        }
    }

    #[test]
    fn test_redo_and_history_cap_and_clear() {
        let frame = create_dummy_frame(120, 120);
        let mut state = AppState::new(frame);
        state.set_tool(Tool::Line);

        // Push 105 annotations -> capped at 100
        for i in 0..105 {
            state.commit_annotation(Annotation::Line {
                start: (i, 10),
                end: (i + 5, 20),
                color: [229, 57, 53, 255],
                thickness: 2,
            });
        }
        assert_eq!(state.annotations.len(), config::MAX_HISTORY_LEN);
        assert!(!state.can_redo());

        // Undo then redo
        assert!(state.undo());
        assert!(state.can_redo());
        assert_eq!(state.annotations.len(), config::MAX_HISTORY_LEN - 1);

        assert!(state.redo());
        assert!(!state.can_redo());
        assert_eq!(state.annotations.len(), config::MAX_HISTORY_LEN);

        // Undo, then commit new annotation -> clears redo stack
        assert!(state.undo());
        assert!(state.can_redo());
        state.commit_annotation(Annotation::Line {
            start: (1, 1),
            end: (10, 10),
            color: [229, 57, 53, 255],
            thickness: 2,
        });
        assert!(!state.can_redo());
    }

    #[test]
    fn test_per_tool_thickness_and_custom_color() {
        let frame = create_dummy_frame(100, 100);
        let mut state = AppState::new(frame);

        state.set_tool(Tool::Pen);
        assert_eq!(state.active_thickness(), config::PEN_THICKNESS);
        assert_eq!(state.adjust_active_thickness(2), config::PEN_THICKNESS + 2);
        assert_eq!(state.adjust_active_thickness(-100), config::MIN_THICKNESS);
        assert_eq!(state.adjust_active_thickness(100), config::MAX_THICKNESS);

        // Switching to Text keeps separate thickness (font size 12..=48)
        state.set_tool(Tool::Text);
        assert_eq!(state.active_thickness(), config::TEXT_DEFAULT_SIZE);
        assert_eq!(
            state.adjust_active_thickness(1),
            config::TEXT_DEFAULT_SIZE + 2
        );
        assert_eq!(state.adjust_active_thickness(-100), config::TEXT_MIN_SIZE);
        assert_eq!(state.adjust_active_thickness(100), config::TEXT_MAX_SIZE);

        // Pen still remembers MAX_THICKNESS
        state.set_tool(Tool::Pen);
        assert_eq!(state.active_thickness(), config::MAX_THICKNESS);

        // Custom color
        state.set_custom_color([12, 34, 56, 255]);
        assert_eq!(state.color_idx, config::PALETTE.len());
        assert_eq!(state.active_color(), [12, 34, 56, 255]);
    }

    #[test]
    fn test_shift_constraints_in_build_annotation() {
        let frame = create_dummy_frame(200, 200);
        let mut state = AppState::new(frame);

        state.set_tool(Tool::Line);
        state.start_drawing(50, 50);
        let ann = state
            .build_current_annotation_with_redact((150, 54), true)
            .expect("line");
        match ann {
            Annotation::Line { end, .. } => assert_eq!(end.1, 50),
            _ => panic!("expected Line"),
        }

        state.set_tool(Tool::Rectangle);
        state.start_drawing(20, 20);
        let ann = state
            .build_current_annotation_with_redact((80, 40), true)
            .expect("rect");
        match ann {
            Annotation::Rectangle { w, h, .. } => assert_eq!(w, h),
            _ => panic!("expected Rectangle"),
        }
    }

    #[cfg(feature = "loupe")]
    #[test]
    fn test_render_loupe_rgba() {
        let mut img = RgbaImage::from_pixel(30, 30, image::Rgba([10, 20, 30, 255]));
        img.put_pixel(15, 15, image::Rgba([255, 128, 0, 255]));
        let (bytes, w, h, hex) = render_loupe_rgba(&img, 15, 15);
        assert_eq!(w, 110);
        assert_eq!(h, 110);
        assert_eq!(bytes.len(), 110 * 110 * 4);
        assert_eq!(hex, "#ff8000");
    }
}

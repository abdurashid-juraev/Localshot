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
    /// Reusable live preview buffer to eliminate O(N) allocation churn (allocated lazily on first drawing)
    pub live_preview: Option<RgbaImage>,
    /// Dirty bounding box of the active live preview annotation
    pub live_dirty_rect: Option<(u32, u32, u32, u32)>,
    /// Stack of applied annotations for undo support
    pub annotations: Vec<Annotation>,
    pub tool: Tool,
    pub color_idx: usize,
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

impl AppState {
    pub fn reset(&mut self, frame: CapturedFrame) {
        *self = Self::new(frame);
    }

    /// Zero-cost initialization: eliminates eager 66MB duplicate buffer cloning
    pub fn new(frame: CapturedFrame) -> Self {
        Self {
            raw_base: frame.raw_image,
            composited_cache: None,
            live_preview: None,
            live_dirty_rect: None,
            annotations: Vec::new(),
            tool: Tool::Select,
            color_idx: 0,
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
        if let Some(preview) = self.live_preview.as_mut() {
            preview.as_mut().zeroize();
        }
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

    /// Lazily ensures the live preview buffer is allocated
    fn ensure_live_preview(&mut self) -> &mut RgbaImage {
        if self.live_preview.is_none() {
            let base = self.composited_image().clone();
            self.live_preview = Some(base);
        }
        self.live_preview.as_mut().unwrap_or(&mut self.raw_base)
    }

    pub fn active_color(&self) -> [u8; 4] {
        config::PALETTE
            .get(self.color_idx)
            .map(|c| c.rgba)
            .unwrap_or(config::PALETTE[0].rgba)
    }

    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
    }

    pub fn set_color_idx(&mut self, idx: usize) {
        if idx < config::PALETTE.len() {
            self.color_idx = idx;
        }
    }

    pub fn start_drawing(&mut self, x: i32, y: i32) {
        self.start_pt = Some((x, y));
        self.current_points = vec![(x, y)];
        self.live_dirty_rect = None;
    }

    /// Item 2: ignore if start_pt is not set (called on hover before any mouse-down)
    pub fn add_drawing_point(&mut self, x: i32, y: i32) {
        if self.start_pt.is_some() {
            self.current_points.push((x, y));
        }
    }

    /// DRY: Single source of truth for constructing Annotation based on active Tool.
    /// Item 8: returns None for degenerate zero-size / zero-length annotations.
    pub fn build_current_annotation(&self, end_pt: (i32, i32)) -> Option<Annotation> {
        let start = self.start_pt?;
        let color = self.active_color();

        match self.tool {
            Tool::Pen => {
                // A single-point pen click becomes a 1-point stroke (dot) — allowed.
                Some(Annotation::Stroke {
                    points: self.current_points.clone(),
                    color,
                    thickness: config::PEN_THICKNESS,
                })
            }
            Tool::Line => {
                // Item 8: reject start == end
                if start == end_pt {
                    return None;
                }
                Some(Annotation::Line {
                    start,
                    end: end_pt,
                    color,
                    thickness: config::LINE_THICKNESS,
                })
            }
            Tool::Arrow => {
                // Item 8: reject start == end
                if start == end_pt {
                    return None;
                }
                Some(Annotation::Arrow {
                    start,
                    end: end_pt,
                    color,
                    thickness: config::ARROW_THICKNESS,
                })
            }
            Tool::Rectangle => {
                let (x, y, w, h) = normalize_rect_coords(start, end_pt);
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
                    thickness: config::RECT_THICKNESS,
                })
            }
            Tool::Marker => Some(Annotation::Marker {
                points: self.current_points.clone(),
                color,
                thickness: config::MARKER_THICKNESS,
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
                    block_size: config::REDACT_BLOCK_SIZE,
                })
            }
            Tool::Select => None,
        }
    }

    /// Commits an annotation to history and directly updates the cached composited buffer.
    /// Item 1: restore live_preview over the UNION of old live_dirty_rect and the annotation's
    /// bounding_box so that when the final mouse-up point differs from the last move point,
    /// earlier annotations are not erased from the preview.
    pub fn commit_annotation(&mut self, ann: Annotation) {
        // Compute the annotation's bounding box BEFORE applying it to the cache.
        // We need canvas dimensions from either cache or raw_base.
        let (canvas_w, canvas_h) = if let Some(c) = &self.composited_cache {
            c.dimensions()
        } else {
            self.raw_base.dimensions()
        };
        let ann_bbox = ann.bounding_box(canvas_w, canvas_h);

        let composited = self.ensure_composited();
        ann.apply(composited);
        self.annotations.push(ann);

        // Item 1: union old dirty rect with the committed annotation's bounding box so
        // that any area painted in the final mouse-up pass (outside the last move's dirty
        // rect) is also restored in live_preview from the freshly-updated composited_cache.
        let union_rect = match self.live_dirty_rect.take() {
            None => ann_bbox,
            Some((rx, ry, rw, rh)) => {
                let (ax, ay, aw, ah) = ann_bbox;
                let ux0 = rx.min(ax);
                let uy0 = ry.min(ay);
                let ux1 = (rx + rw).max(ax + aw);
                let uy1 = (ry + rh).max(ay + ah);
                (ux0, uy0, ux1.saturating_sub(ux0), uy1.saturating_sub(uy0))
            }
        };

        // Sync live_preview by restoring the union rect from the now-updated composited_cache.
        if let (Some(cache), Some(preview)) = (&self.composited_cache, &mut self.live_preview) {
            let (ux, uy, uw, uh) = union_rect;
            draw::copy_rect(cache, preview, ux, uy, uw, uh);
        }
    }

    pub fn finish_drawing(&mut self) {
        self.start_pt = None;
        self.current_points.clear();
        self.live_dirty_rect = None;
    }

    /// Undoes the last annotation and rebuilds the composited cache from the pristine raw base.
    pub fn undo(&mut self) -> bool {
        if self.annotations.pop().is_some() {
            if self.annotations.is_empty() {
                self.composited_cache = None;
                self.live_preview = None;
            } else {
                let cache = draw::render_annotations(&self.raw_base, &self.annotations);
                self.live_preview = Some(cache.clone());
                self.composited_cache = Some(cache);
            }
            self.live_dirty_rect = None;
            true
        } else {
            false
        }
    }

    /// Renders a fast preview by restoring the dirty rectangle and applying the live annotation.
    /// Eliminates full 4K frame cloning on every mouse move.
    pub fn render_preview(&mut self, live_ann: Option<&Annotation>) -> &RgbaImage {
        self.ensure_composited();
        self.ensure_live_preview();

        if let Some((rx, ry, rw, rh)) = self.live_dirty_rect.take() {
            if let (Some(cache), Some(preview)) = (&self.composited_cache, &mut self.live_preview) {
                draw::copy_rect(cache, preview, rx, ry, rw, rh);
            }
        }

        if let Some(ann) = live_ann {
            if let Some(ref mut preview) = self.live_preview {
                ann.apply(preview);
                let bbox = ann.bounding_box(preview.width(), preview.height());
                self.live_dirty_rect = Some(bbox);
            }
        }

        self.live_preview.as_ref().unwrap_or(&self.raw_base)
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

impl Drop for AppState {
    fn drop(&mut self) {
        self.sanitize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::CapturedFrame;
    use slint::Image;

    fn create_dummy_frame(w: u32, h: u32) -> CapturedFrame {
        let raw = RgbaImage::from_pixel(w, h, image::Rgba([100, 100, 100, 255]));
        CapturedFrame {
            raw_image: raw,
            slint_image: Image::default(),
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
        assert!(state.live_preview.is_none());
        assert_eq!(state.tool, Tool::Select);
        assert_eq!(state.annotations.len(), 0);

        state.set_tool(Tool::Rectangle);
        state.set_color_idx(1); // Blue

        state.start_drawing(10, 10);
        let ann = state.build_current_annotation((50, 50));
        assert!(ann.is_some());

        state.commit_annotation(ann.unwrap());
        state.finish_drawing();
        assert_eq!(state.annotations.len(), 1);
        assert!(state.composited_cache.is_some());

        // Preview should work without allocation
        let preview = state.render_preview(None);
        assert_eq!(preview.dimensions(), (200, 200));

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
        assert!(state.live_preview.is_none());
        assert!(!state.undo()); // Nothing left to undo
    }

    /// Item 1: commit a rectangle whose end point lies outside the last preview rect;
    /// assert live_preview equals composited_image() after commit.
    #[test]
    fn test_commit_annotation_union_dirty_rect() {
        let frame = create_dummy_frame(200, 200);
        let mut state = AppState::new(frame);
        state.set_tool(Tool::Rectangle);
        state.set_color_idx(0); // Red

        // Simulate draw: start at (10,10), preview moves to (30,30) …
        state.start_drawing(10, 10);

        // Build and render a preview up to (30,30) — sets live_dirty_rect
        let preview_ann = state
            .build_current_annotation((30, 30))
            .expect("should build rect");
        state.render_preview(Some(&preview_ann));
        assert!(state.live_dirty_rect.is_some(), "dirty rect should be set");

        // … but the final mouse-up is at (80,80), outside the preview dirty rect.
        let final_ann = state
            .build_current_annotation((80, 80))
            .expect("should build rect");
        state.commit_annotation(final_ann);
        state.finish_drawing();

        // After commit, live_preview must match composited_image pixel-for-pixel.
        let preview_pixels = state
            .live_preview
            .as_ref()
            .expect("live_preview must exist")
            .as_raw()
            .to_vec();
        let composited_pixels = state.composited_image().as_raw().to_vec();
        assert_eq!(
            preview_pixels, composited_pixels,
            "live_preview must equal composited_image after commit"
        );
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
            slint_image: Image::default(),
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
}

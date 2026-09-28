use crate::capture::CapturedFrame;
use crate::config;
use crate::draw::{self, Annotation, Tool};
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
    /// Cached base including all committed annotations for O(1) live preview rendering
    pub composited_cache: RgbaImage,
    /// Reusable live preview buffer to eliminate O(N) allocation churn during mouse dragging
    pub live_preview: RgbaImage,
    /// Dirty bounding box of the active live preview annotation
    pub live_dirty_rect: Option<(u32, u32, u32, u32)>,
    /// Stack of applied annotations for undo support
    pub annotations: Vec<Annotation>,
    pub tool: Tool,
    pub color_idx: usize,
    pub selection: SelectionRect,
    pub start_pt: Option<(i32, i32)>,
    pub current_points: Vec<(i32, i32)>,
}

impl AppState {
    pub fn reset(&mut self, frame: CapturedFrame) {
        self.original_frame = frame;
        self.composited_cache = self.original_frame.img.clone();
        self.selection = SelectionRect::default();
        self.annotations.clear();
        self.redo_stack.clear();
        self.is_drawing = false;
    }

    pub fn new(frame: CapturedFrame) -> Self {
        let composited = frame.raw_image.clone();
        let preview = composited.clone();
        Self {
            raw_base: frame.raw_image,
            composited_cache: composited,
            live_preview: preview,
            live_dirty_rect: None,
            annotations: Vec::new(),
            tool: Tool::Select,
            color_idx: 0,
            selection: SelectionRect::default(),
            start_pt: None,
            current_points: Vec::new(),
        }
    }

    /// Explicitly zeroizes all volatile image buffers from RAM (Zero disk & RAM residue)
    pub fn sanitize(&mut self) {
        self.raw_base.as_mut().zeroize();
        self.composited_cache.as_mut().zeroize();
        self.live_preview.as_mut().zeroize();
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

    pub fn add_drawing_point(&mut self, x: i32, y: i32) {
        self.current_points.push((x, y));
    }

    /// DRY: Single source of truth for constructing Annotation based on active Tool
    pub fn build_current_annotation(&self, end_pt: (i32, i32)) -> Option<Annotation> {
        let start = self.start_pt?;
        let color = self.active_color();

        match self.tool {
            Tool::Pen => Some(Annotation::Stroke {
                points: self.current_points.clone(),
                color,
                thickness: config::PEN_THICKNESS,
            }),
            Tool::Line => Some(Annotation::Line {
                start,
                end: end_pt,
                color,
                thickness: config::LINE_THICKNESS,
            }),
            Tool::Arrow => Some(Annotation::Arrow {
                start,
                end: end_pt,
                color,
                thickness: config::ARROW_THICKNESS,
            }),
            Tool::Rectangle => {
                let x = start.0.min(end_pt.0);
                let y = start.1.min(end_pt.1);
                let w = (end_pt.0 - start.0).abs();
                let h = (end_pt.1 - start.1).abs();
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
                let x = start.0.min(end_pt.0);
                let y = start.1.min(end_pt.1);
                let w = (end_pt.0 - start.0).abs();
                let h = (end_pt.1 - start.1).abs();
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
    pub fn commit_annotation(&mut self, ann: Annotation) {
        ann.apply(&mut self.composited_cache);
        self.annotations.push(ann);
        // Sync live_preview by restoring dirty rect
        if let Some((rx, ry, rw, rh)) = self.live_dirty_rect.take() {
            draw::copy_rect(&self.composited_cache, &mut self.live_preview, rx, ry, rw, rh);
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
            self.composited_cache = draw::render_annotations(&self.raw_base, &self.annotations);
            self.live_preview = self.composited_cache.clone();
            self.live_dirty_rect = None;
            true
        } else {
            false
        }
    }

    /// Renders a fast preview by restoring the dirty rectangle and applying the live annotation.
    /// Eliminates full 4K frame cloning on every mouse move.
    pub fn render_preview(&mut self, live_ann: Option<&Annotation>) -> &RgbaImage {
        if let Some((rx, ry, rw, rh)) = self.live_dirty_rect.take() {
            draw::copy_rect(&self.composited_cache, &mut self.live_preview, rx, ry, rw, rh);
        }

        if let Some(ann) = live_ann {
            ann.apply(&mut self.live_preview);
            let bbox = ann.bounding_box(self.live_preview.width(), self.live_preview.height());
            self.live_dirty_rect = Some(bbox);
        }

        &self.live_preview
    }

    /// Produces the final composited and cropped image bounded by the active selection.
    pub fn get_final_crop(&self) -> Option<RgbaImage> {
        let (cx, cy, cw, ch) = self.selection.crop_bounds(
            self.composited_cache.width(),
            self.composited_cache.height(),
        )?;
        Some(image::imageops::crop_imm(&self.composited_cache, cx, cy, cw, ch).to_image())
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
        assert!(!state.undo()); // Nothing left to undo
    }
}

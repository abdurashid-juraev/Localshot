//! Pure interaction logic, selection state machine, and geometry functions.
//!
//! Owns the SelectionMachine, handle hit-testing, resize math with clamping,
//! move math with clamping, and dual toolbar placement.
//! Every function in this module is pure and display-independent.

use crate::state::SelectionRect;

/// Minimum width/height (in physical pixels) for a finalized selection.
pub const MIN_SELECTION_SIZE: u32 = 5;

/// Clamps a physical point `(x, y)` to lie inside `sel` (inclusive of `[x, x + w - 1]`).
#[inline]
pub fn clamp_point_to_selection(pt: (i32, i32), sel: &SelectionRect) -> (i32, i32) {
    if sel.is_empty() {
        return pt;
    }
    let max_x = sel.x + sel.w as i32 - 1;
    let max_y = sel.y + sel.h as i32 - 1;
    (pt.0.clamp(sel.x, max_x), pt.1.clamp(sel.y, max_y))
}

/// Snaps `end` relative to `start` to the nearest 0°, 45°, or 90° angle (for Line / Arrow with Shift).
pub fn constrain_line_shift(start: (i32, i32), end: (i32, i32)) -> (i32, i32) {
    let dx = (end.0 - start.0) as f64;
    let dy = (end.1 - start.1) as f64;
    let len = dx.hypot(dy);
    if len < 1e-3 {
        return end;
    }
    let angle = dy.atan2(dx);
    let step = std::f64::consts::FRAC_PI_4;
    let snapped = (angle / step).round() * step;
    (
        start.0 + (len * snapped.cos()).round() as i32,
        start.1 + (len * snapped.sin()).round() as i32,
    )
}

/// Constrains `end` relative to `start` to form a square (equal |dx| and |dy|, for Rectangle with Shift).
pub fn constrain_rect_shift(start: (i32, i32), end: (i32, i32)) -> (i32, i32) {
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    let side = dx.abs().max(dy.abs());
    let sx = if dx >= 0 { side } else { -side };
    let sy = if dy >= 0 { side } else { -side };
    (start.0 + sx, start.1 + sy)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionMachine {
    #[default]
    Idle,
    Selecting,
    Selected,
    Moving,
    Resizing(Handle),
    Drawing,
}

/// Normalizes two arbitrary points into a top-left origin and positive dimensions.
#[inline]
pub fn normalize_rect(p1: (i32, i32), p2: (i32, i32)) -> SelectionRect {
    let x = p1.0.min(p2.0);
    let y = p1.1.min(p2.1);
    let w = (p1.0 - p2.0).unsigned_abs();
    let h = (p1.1 - p2.1).unsigned_abs();
    SelectionRect { x, y, w, h }
}

/// Tests if a point lies within the tolerance distance of any of the 8 selection handles.
/// Tolerance is given in physical pixels.
pub fn hit_test_handle(rect: &SelectionRect, point: (i32, i32), tolerance: i32) -> Option<Handle> {
    if rect.is_empty() {
        return None;
    }

    let left = rect.x;
    let right = rect.x + rect.w as i32;
    let mid_x = rect.x + (rect.w as i32) / 2;

    let top = rect.y;
    let bottom = rect.y + rect.h as i32;
    let mid_y = rect.y + (rect.h as i32) / 2;

    let handle_coords: [(Handle, (i32, i32)); 8] = [
        // Test corners first so corners take precedence over edges on small rects
        (Handle::TopLeft, (left, top)),
        (Handle::TopRight, (right, top)),
        (Handle::BottomRight, (right, bottom)),
        (Handle::BottomLeft, (left, bottom)),
        (Handle::Top, (mid_x, top)),
        (Handle::Right, (right, mid_y)),
        (Handle::Bottom, (mid_x, bottom)),
        (Handle::Left, (left, mid_y)),
    ];

    let mut closest_handle = None;
    let mut min_dist_sq = i64::MAX;
    let tol_sq = (tolerance as i64) * (tolerance as i64);

    for (handle, (hx, hy)) in handle_coords {
        let dx = (point.0 - hx) as i64;
        let dy = (point.1 - hy) as i64;
        let dist_sq = dx * dx + dy * dy;

        // Check Chebyshev distance (box hit) and keep closest Euclidean distance
        if (point.0 - hx).abs() <= tolerance
            && (point.1 - hy).abs() <= tolerance
            && dist_sq <= tol_sq
            && dist_sq < min_dist_sq
        {
            min_dist_sq = dist_sq;
            closest_handle = Some(handle);
        }
    }

    closest_handle
}

/// Checks if a point lies strictly inside the selection rectangle.
#[inline]
pub fn hit_test_selection(rect: &SelectionRect, point: (i32, i32)) -> bool {
    if rect.is_empty() {
        return false;
    }
    point.0 >= rect.x
        && point.0 < rect.x + rect.w as i32
        && point.1 >= rect.y
        && point.1 < rect.y + rect.h as i32
}

/// Applies a resize delta to an initial selection rectangle with strict min_size and screen clamping.
pub fn apply_resize(
    initial: &SelectionRect,
    handle: Handle,
    delta: (i32, i32),
    screen_w: u32,
    screen_h: u32,
    min_size: u32,
) -> SelectionRect {
    let mut left = initial.x;
    let mut right = initial.x + initial.w as i32;
    let mut top = initial.y;
    let mut bottom = initial.y + initial.h as i32;

    let max_x = screen_w as i32;
    let max_y = screen_h as i32;
    let m = min_size as i32;

    match handle {
        Handle::TopLeft => {
            left = (initial.x + delta.0).clamp(0, right - m);
            top = (initial.y + delta.1).clamp(0, bottom - m);
        }
        Handle::Top => {
            top = (initial.y + delta.1).clamp(0, bottom - m);
        }
        Handle::TopRight => {
            right = (initial.x + initial.w as i32 + delta.0).clamp(left + m, max_x);
            top = (initial.y + delta.1).clamp(0, bottom - m);
        }
        Handle::Right => {
            right = (initial.x + initial.w as i32 + delta.0).clamp(left + m, max_x);
        }
        Handle::BottomRight => {
            right = (initial.x + initial.w as i32 + delta.0).clamp(left + m, max_x);
            bottom = (initial.y + initial.h as i32 + delta.1).clamp(top + m, max_y);
        }
        Handle::Bottom => {
            bottom = (initial.y + initial.h as i32 + delta.1).clamp(top + m, max_y);
        }
        Handle::BottomLeft => {
            left = (initial.x + delta.0).clamp(0, right - m);
            bottom = (initial.y + initial.h as i32 + delta.1).clamp(top + m, max_y);
        }
        Handle::Left => {
            left = (initial.x + delta.0).clamp(0, right - m);
        }
    }

    SelectionRect {
        x: left,
        y: top,
        w: (right - left).max(0) as u32,
        h: (bottom - top).max(0) as u32,
    }
}

/// Applies a translation delta to an initial selection rectangle with strict screen clamping.
pub fn apply_move(
    initial: &SelectionRect,
    delta: (i32, i32),
    screen_w: u32,
    screen_h: u32,
) -> SelectionRect {
    let max_x = (screen_w as i32 - initial.w as i32).max(0);
    let max_y = (screen_h as i32 - initial.h as i32).max(0);

    let new_x = (initial.x + delta.0).clamp(0, max_x);
    let new_y = (initial.y + delta.1).clamp(0, max_y);

    SelectionRect {
        x: new_x,
        y: new_y,
        w: initial.w,
        h: initial.h,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectF {
    #[inline]
    pub fn overlaps(&self, other: &RectF) -> bool {
        self.x < other.x + other.w
            && self.x + self.w > other.x
            && self.y < other.y + other.h
            && self.y + self.h > other.y
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToolbarPositions {
    pub vert: (f32, f32),
    pub horiz: (f32, f32),
}

/// Computes the positions of the vertical and horizontal toolbars.
///
/// Rules:
/// 1. Prefer right of selection for vertical toolbar, bottom of selection for horizontal toolbar.
/// 2. Flip to left/top when outside does not fit.
/// 3. Then place inside the selection corner when outside does not fit on either side.
/// 4. The two toolbars must NEVER overlap each other and must NEVER leave the screen.
pub fn calculate_toolbar_positions(
    selection: RectF,
    screen: (f32, f32),
    vert_size: (f32, f32),
    horiz_size: (f32, f32),
) -> ToolbarPositions {
    let gap = 8.0_f32;
    let margin = 8.0_f32;

    let (screen_w, screen_h) = screen;
    let (vw, vh) = vert_size;
    let (hw, hh) = horiz_size;

    let sel_left = selection.x;
    let sel_top = selection.y;
    let sel_right = selection.x + selection.w;
    let sel_bottom = selection.y + selection.h;

    // --- Vertical Toolbar Placement ---
    // Prefer right outside
    let right_outside = sel_right + gap;
    let left_outside = sel_left - gap - vw;
    let right_inside = sel_right - margin - vw;

    let mut vx = if right_outside + vw <= screen_w - margin {
        right_outside
    } else if left_outside >= margin {
        left_outside
    } else if right_inside >= sel_left && right_inside + vw <= screen_w - margin {
        right_inside
    } else {
        (screen_w - vw - margin).clamp(margin, screen_w - vw)
    };

    let mut vy = sel_top.clamp(margin, (screen_h - vh - margin).max(margin));

    // --- Horizontal Toolbar Placement ---
    // Prefer bottom outside
    let bottom_outside = sel_bottom + gap;
    let top_outside = sel_top - gap - hh;
    let bottom_inside = sel_bottom - margin - hh;

    let mut hy = if bottom_outside + hh <= screen_h - margin {
        bottom_outside
    } else if top_outside >= margin {
        top_outside
    } else if bottom_inside >= sel_top && bottom_inside + hh <= screen_h - margin {
        bottom_inside
    } else {
        (screen_h - hh - margin).clamp(margin, screen_h - hh)
    };

    // Right-align horizontal toolbar with selection
    let mut hx = (sel_right - hw).clamp(margin, (screen_w - hw - margin).max(margin));

    // Ensure within screen bounds
    vx = vx.clamp(margin, (screen_w - vw - margin).max(margin));
    vy = vy.clamp(margin, (screen_h - vh - margin).max(margin));
    hx = hx.clamp(margin, (screen_w - hw - margin).max(margin));
    hy = hy.clamp(margin, (screen_h - hh - margin).max(margin));

    // --- Overlap Resolution ---
    let mut vert_rect = RectF {
        x: vx,
        y: vy,
        w: vw,
        h: vh,
    };
    let mut horiz_rect = RectF {
        x: hx,
        y: hy,
        w: hw,
        h: hh,
    };

    if vert_rect.overlaps(&horiz_rect) {
        // Try shifting horizontal toolbar to the left of the vertical toolbar
        let hx_cand_left = vx - gap - hw;
        let hx_cand_right = vx + vw + gap;
        let hy_cand_above = vy - gap - hh;
        let hy_cand_below = vy + vh + gap;

        if hx_cand_left >= margin {
            hx = hx_cand_left;
        } else if hx_cand_right + hw <= screen_w - margin {
            hx = hx_cand_right;
        } else if hy_cand_above >= margin {
            hy = hy_cand_above;
        } else if hy_cand_below + hh <= screen_h - margin {
            hy = hy_cand_below;
        } else {
            // Extreme cramped screen: place horizontal toolbar at top-left, vert at right
            hx = margin;
            hy = margin;
            vx = (screen_w - vw - margin).max(margin);
            vy = (screen_h - vh - margin).max(margin);
        }
    }

    // Final boundary clamp
    vx = vx.clamp(0.0, (screen_w - vw).max(0.0));
    vy = vy.clamp(0.0, (screen_h - vh).max(0.0));
    hx = hx.clamp(0.0, (screen_w - hw).max(0.0));
    hy = hy.clamp(0.0, (screen_h - hh).max(0.0));

    vert_rect = RectF {
        x: vx,
        y: vy,
        w: vw,
        h: vh,
    };
    horiz_rect = RectF {
        x: hx,
        y: hy,
        w: hw,
        h: hh,
    };

    // If still overlapping (due to screen smaller than both toolbars combined),
    // separate vertically or horizontally as best as possible
    if vert_rect.overlaps(&horiz_rect) {
        if vx + vw + hw <= screen_w {
            hx = vx + vw;
        } else if vy + vh + hh <= screen_h {
            hy = vy + vh;
        } else {
            hx = (vx - hw).max(0.0);
        }
    }

    ToolbarPositions {
        vert: (vx, vy),
        horiz: (hx, hy),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_rect() {
        assert_eq!(
            normalize_rect((10, 20), (50, 70)),
            SelectionRect {
                x: 10,
                y: 20,
                w: 40,
                h: 50
            }
        );
        // Inverted points
        assert_eq!(
            normalize_rect((50, 70), (10, 20)),
            SelectionRect {
                x: 10,
                y: 20,
                w: 40,
                h: 50
            }
        );
        assert_eq!(
            normalize_rect((10, 70), (50, 20)),
            SelectionRect {
                x: 10,
                y: 20,
                w: 40,
                h: 50
            }
        );
    }

    #[test]
    fn test_hit_test_all_8_handles() {
        let rect = SelectionRect {
            x: 100,
            y: 100,
            w: 200,
            h: 200,
        };
        let tol = 8;

        // TopLeft at (100, 100)
        assert_eq!(
            hit_test_handle(&rect, (100, 100), tol),
            Some(Handle::TopLeft)
        );
        assert_eq!(
            hit_test_handle(&rect, (105, 95), tol),
            Some(Handle::TopLeft)
        );

        // Top at (200, 100)
        assert_eq!(hit_test_handle(&rect, (200, 100), tol), Some(Handle::Top));

        // TopRight at (300, 100)
        assert_eq!(
            hit_test_handle(&rect, (300, 100), tol),
            Some(Handle::TopRight)
        );

        // Right at (300, 200)
        assert_eq!(hit_test_handle(&rect, (300, 200), tol), Some(Handle::Right));

        // BottomRight at (300, 300)
        assert_eq!(
            hit_test_handle(&rect, (300, 300), tol),
            Some(Handle::BottomRight)
        );

        // Bottom at (200, 300)
        assert_eq!(
            hit_test_handle(&rect, (200, 300), tol),
            Some(Handle::Bottom)
        );

        // BottomLeft at (100, 300)
        assert_eq!(
            hit_test_handle(&rect, (100, 300), tol),
            Some(Handle::BottomLeft)
        );

        // Left at (100, 200)
        assert_eq!(hit_test_handle(&rect, (100, 200), tol), Some(Handle::Left));

        // Outside tolerance
        assert_eq!(hit_test_handle(&rect, (150, 150), tol), None);
        assert_eq!(hit_test_handle(&rect, (80, 80), tol), None);
    }

    #[test]
    fn test_apply_move_clamping() {
        let initial = SelectionRect {
            x: 100,
            y: 100,
            w: 200,
            h: 200,
        };
        let screen_w = 1000;
        let screen_h = 800;

        // Normal delta
        let m1 = apply_move(&initial, (50, 30), screen_w, screen_h);
        assert_eq!(
            m1,
            SelectionRect {
                x: 150,
                y: 130,
                w: 200,
                h: 200
            }
        );

        // Clamp to top-left (negative)
        let m2 = apply_move(&initial, (-200, -300), screen_w, screen_h);
        assert_eq!(
            m2,
            SelectionRect {
                x: 0,
                y: 0,
                w: 200,
                h: 200
            }
        );

        // Clamp to bottom-right
        let m3 = apply_move(&initial, (1000, 1000), screen_w, screen_h);
        assert_eq!(
            m3,
            SelectionRect {
                x: 800,
                y: 600,
                w: 200,
                h: 200
            }
        );
    }

    #[test]
    fn test_apply_resize_clamping_and_min_size() {
        let initial = SelectionRect {
            x: 100,
            y: 100,
            w: 200,
            h: 200,
        };
        let screen_w = 1000;
        let screen_h = 800;
        let min_size = 16;

        // Resize Right expanding
        let r1 = apply_resize(
            &initial,
            Handle::Right,
            (50, 0),
            screen_w,
            screen_h,
            min_size,
        );
        assert_eq!(
            r1,
            SelectionRect {
                x: 100,
                y: 100,
                w: 250,
                h: 200
            }
        );

        // Resize Right contracting beyond min_size
        let r2 = apply_resize(
            &initial,
            Handle::Right,
            (-250, 0),
            screen_w,
            screen_h,
            min_size,
        );
        assert_eq!(
            r2,
            SelectionRect {
                x: 100,
                y: 100,
                w: min_size,
                h: 200
            }
        );

        // Resize Left expanding left past 0
        let r3 = apply_resize(
            &initial,
            Handle::Left,
            (-200, 0),
            screen_w,
            screen_h,
            min_size,
        );
        assert_eq!(
            r3,
            SelectionRect {
                x: 0,
                y: 100,
                w: 300,
                h: 200
            }
        );

        // Resize Bottom expanding past screen height
        let r4 = apply_resize(
            &initial,
            Handle::Bottom,
            (0, 900),
            screen_w,
            screen_h,
            min_size,
        );
        assert_eq!(
            r4,
            SelectionRect {
                x: 100,
                y: 100,
                w: 200,
                h: 700
            }
        );

        // Resize Top contracting beyond min_size
        let r5 = apply_resize(
            &initial,
            Handle::Top,
            (0, 250),
            screen_w,
            screen_h,
            min_size,
        );
        assert_eq!(
            r5,
            SelectionRect {
                x: 100,
                y: 300 - min_size as i32,
                w: 200,
                h: min_size
            }
        );
    }

    #[test]
    fn test_toolbar_placement_never_overlaps_and_never_leaves_screen() {
        let screen = (1920.0, 1080.0);
        let vert_size = (48.0, 368.0);
        let horiz_size = (156.0, 46.0);

        let test_cases = [
            // Center normal
            RectF {
                x: 400.0,
                y: 300.0,
                w: 500.0,
                h: 400.0,
            },
            // Whole screen
            RectF {
                x: 0.0,
                y: 0.0,
                w: 1920.0,
                h: 1080.0,
            },
            // Touching left edge
            RectF {
                x: 0.0,
                y: 100.0,
                w: 400.0,
                h: 300.0,
            },
            // Touching right edge
            RectF {
                x: 1520.0,
                y: 100.0,
                w: 400.0,
                h: 300.0,
            },
            // Touching top edge
            RectF {
                x: 400.0,
                y: 0.0,
                w: 400.0,
                h: 300.0,
            },
            // Touching bottom edge
            RectF {
                x: 400.0,
                y: 780.0,
                w: 400.0,
                h: 300.0,
            },
            // Top-left corner
            RectF {
                x: 0.0,
                y: 0.0,
                w: 200.0,
                h: 200.0,
            },
            // Top-right corner
            RectF {
                x: 1720.0,
                y: 0.0,
                w: 200.0,
                h: 200.0,
            },
            // Bottom-left corner
            RectF {
                x: 0.0,
                y: 880.0,
                w: 200.0,
                h: 200.0,
            },
            // Bottom-right corner
            RectF {
                x: 1720.0,
                y: 880.0,
                w: 200.0,
                h: 200.0,
            },
            // Very small selection
            RectF {
                x: 500.0,
                y: 500.0,
                w: 20.0,
                h: 20.0,
            },
            // Very small selection at bottom-right edge
            RectF {
                x: 1900.0,
                y: 1060.0,
                w: 20.0,
                h: 20.0,
            },
        ];

        for (idx, &sel) in test_cases.iter().enumerate() {
            let pos = calculate_toolbar_positions(sel, screen, vert_size, horiz_size);

            let vert_rect = RectF {
                x: pos.vert.0,
                y: pos.vert.1,
                w: vert_size.0,
                h: vert_size.1,
            };
            let horiz_rect = RectF {
                x: pos.horiz.0,
                y: pos.horiz.1,
                w: horiz_size.0,
                h: horiz_size.1,
            };

            // 1. Must never leave screen
            assert!(
                vert_rect.x >= 0.0 && vert_rect.x + vert_rect.w <= screen.0,
                "Case {}: vert toolbar x out of bounds: {:?}",
                idx,
                vert_rect
            );
            assert!(
                vert_rect.y >= 0.0 && vert_rect.y + vert_rect.h <= screen.1,
                "Case {}: vert toolbar y out of bounds: {:?}",
                idx,
                vert_rect
            );
            assert!(
                horiz_rect.x >= 0.0 && horiz_rect.x + horiz_rect.w <= screen.0,
                "Case {}: horiz toolbar x out of bounds: {:?}",
                idx,
                horiz_rect
            );
            assert!(
                horiz_rect.y >= 0.0 && horiz_rect.y + horiz_rect.h <= screen.1,
                "Case {}: horiz toolbar y out of bounds: {:?}",
                idx,
                horiz_rect
            );

            // 2. Must never overlap each other
            assert!(
                !vert_rect.overlaps(&horiz_rect),
                "Case {}: toolbars overlap! vert={:?}, horiz={:?}",
                idx,
                vert_rect,
                horiz_rect
            );
        }
    }

    #[test]
    fn test_clamp_point_to_selection() {
        let sel = SelectionRect {
            x: 100,
            y: 200,
            w: 50,
            h: 40,
        };
        assert_eq!(clamp_point_to_selection((120, 210), &sel), (120, 210));
        assert_eq!(clamp_point_to_selection((0, 0), &sel), (100, 200));
        assert_eq!(clamp_point_to_selection((500, 500), &sel), (149, 239));
    }

    #[test]
    fn test_constrain_line_shift() {
        // Nearly horizontal -> 0 deg
        let h = constrain_line_shift((100, 100), (200, 108));
        assert_eq!(h.1, 100);
        assert!(h.0 > 190);

        // Nearly vertical -> 90 deg
        let v = constrain_line_shift((100, 100), (106, 200));
        assert_eq!(v.0, 100);
        assert!(v.1 > 190);

        // Nearly 45 deg -> |dx| == |dy|
        let d = constrain_line_shift((100, 100), (180, 175));
        assert_eq!((d.0 - 100).abs(), (d.1 - 100).abs());
    }

    #[test]
    fn test_constrain_rect_shift_square() {
        assert_eq!(constrain_rect_shift((100, 100), (150, 120)), (150, 150));
        assert_eq!(constrain_rect_shift((100, 100), (60, 80)), (60, 60));
        assert_eq!(constrain_rect_shift((100, 100), (120, 40)), (160, 40));
    }
}

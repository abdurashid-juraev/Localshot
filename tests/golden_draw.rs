use image::{Rgba, RgbaImage};
use localshot::draw::Annotation;
use std::path::Path;

fn make_solid_canvas(w: u32, h: u32, color: [u8; 4]) -> RgbaImage {
    RgbaImage::from_pixel(w, h, Rgba(color))
}

fn make_checkerboard_canvas(w: u32, h: u32, cell: u32) -> RgbaImage {
    let mut img = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let even = ((x / cell) + (y / cell)).is_multiple_of(2);
            let px = if even {
                Rgba([240, 240, 240, 255])
            } else {
                Rgba([30, 30, 30, 255])
            };
            img.put_pixel(x, y, px);
        }
    }
    img
}

fn count_diff_pixels(a: &RgbaImage, b: &RgbaImage) -> usize {
    assert_eq!(a.dimensions(), b.dimensions());
    a.pixels()
        .zip(b.pixels())
        .filter(|(pa, pb)| pa != pb)
        .count()
}

fn assert_or_update_golden(name: &str, actual: &RgbaImage) {
    let fixture_path = Path::new("tests/fixtures").join(format!("{name}.png"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        if let Some(parent) = fixture_path.parent() {
            std::fs::create_dir_all(parent).expect("create fixtures dir");
        }
        actual.save(&fixture_path).expect("save golden fixture");
        return;
    }

    if fixture_path.exists() {
        let expected = image::open(&fixture_path)
            .expect("open golden fixture")
            .to_rgba8();
        assert_eq!(
            actual.dimensions(),
            expected.dimensions(),
            "dimension mismatch for {name}"
        );
        let max_delta = actual
            .as_raw()
            .iter()
            .zip(expected.as_raw().iter())
            .map(|(&a, &b)| a.abs_diff(b))
            .max()
            .unwrap_or(0);
        assert!(
            max_delta <= 2,
            "golden image mismatch for {name}: max channel delta {max_delta} > 2"
        );
    }
}

#[test]
fn golden_pen_line_arrow() {
    let bg = make_solid_canvas(128, 128, [255, 255, 255, 255]);
    let mut canvas = bg.clone();

    Annotation::Stroke {
        points: vec![(12, 18), (28, 30), (48, 22), (64, 40)],
        color: [220, 30, 30, 255],
        thickness: 4,
    }
    .apply(&mut canvas);

    Annotation::Line {
        start: (16, 70),
        end: (110, 70),
        color: [20, 90, 220, 255],
        thickness: 4,
    }
    .apply(&mut canvas);

    Annotation::Arrow {
        start: (20, 108),
        end: (112, 92),
        color: [20, 160, 60, 255],
        thickness: 4,
    }
    .apply(&mut canvas);

    assert!(count_diff_pixels(&bg, &canvas) > 250);
    // Arrow tip vicinity should be painted green.
    let tip = canvas.get_pixel(110, 92);
    assert!(tip[1] > tip[0] && tip[1] > tip[2]);

    assert_or_update_golden("golden_pen_line_arrow", &canvas);
}

#[test]
fn golden_rect_marker_redact() {
    let bg = make_checkerboard_canvas(128, 128, 4);
    let mut canvas = bg.clone();

    Annotation::Rectangle {
        x: 10,
        y: 10,
        w: 44,
        h: 36,
        color: [255, 0, 0, 255],
        thickness: 3,
    }
    .apply(&mut canvas);

    Annotation::Marker {
        points: vec![(64, 24), (112, 24)],
        color: [255, 235, 59, 255],
        thickness: 14,
    }
    .apply(&mut canvas);

    Annotation::Redact {
        x: 20,
        y: 64,
        w: 88,
        h: 48,
        block_size: 8,
        solid_color: None,
    }
    .apply(&mut canvas);

    assert!(count_diff_pixels(&bg, &canvas) > 500);

    // Inside a single 8x8 redact block aligned at (20, 64), pixels should be uniform.
    let p0 = canvas.get_pixel(21, 65);
    let p1 = canvas.get_pixel(26, 70);
    assert_eq!(p0, p1);

    assert_or_update_golden("golden_rect_marker_redact", &canvas);
}

#[test]
fn golden_text_annotation() {
    let bg = make_solid_canvas(160, 64, [24, 24, 28, 255]);
    let mut canvas = bg.clone();

    Annotation::Text {
        x: 12,
        y: 16,
        text: "LocalShot QA".to_string(),
        color: [255, 255, 255, 255],
        font_size: 20.0,
    }
    .apply(&mut canvas);

    assert!(count_diff_pixels(&bg, &canvas) > 80);
    assert_or_update_golden("golden_text_annotation", &canvas);
}

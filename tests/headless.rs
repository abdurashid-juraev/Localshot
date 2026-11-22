use localshot::config::sanitize_filename_part;
use localshot::coord::{
    logical_to_physical_point, logical_to_physical_round, physical_to_logical,
    physical_to_logical_rect,
};
use localshot::daemon::{parse_ipc_command, tray_tooltip_for_hotkeys, IpcCommand};
use localshot::interaction::{
    apply_move, apply_resize, calculate_toolbar_positions, clamp_point_to_selection,
    constrain_line_shift, constrain_rect_shift, hit_test_handle, hit_test_selection,
    normalize_rect, Handle, RectF, SelectionMachine, MIN_SELECTION_SIZE,
};
use localshot::state::SelectionRect;
use localshot::updater::{
    expected_installer_asset_name, format_changelog, is_newer_version, is_updater_disabled_with,
    parse_version, select_release_assets, validate_download_url,
};
use serde_json::json;
use std::path::Path;

#[test]
fn headless_coord_fractional_scales_and_guards() {
    for scale in [1.0_f32, 1.25, 1.5, 1.75, 2.0] {
        let (px, py) = logical_to_physical_point(100.0, 200.0, scale);
        assert_eq!(px, (100.0 * scale).round() as i32);
        assert_eq!(py, (200.0 * scale).round() as i32);

        let (lx, ly, lw, lh) = physical_to_logical_rect(-100, -50, 400, 300, scale);
        assert!((lx - (-100.0 / scale)).abs() < 1e-4);
        assert!((ly - (-50.0 / scale)).abs() < 1e-4);
        assert!((lw - (400.0 / scale)).abs() < 1e-4);
        assert!((lh - (300.0 / scale)).abs() < 1e-4);
    }

    assert_eq!(logical_to_physical_round(-120.0, 1.5), -180);
    assert_eq!(physical_to_logical(42.0, 0.0), 42.0);
    assert_eq!(physical_to_logical(42.0, -1.0), 42.0);
}

#[test]
fn headless_selection_machine_and_geometry() {
    let mut machine = SelectionMachine::default();
    assert_eq!(machine, SelectionMachine::Idle);
    machine = SelectionMachine::Selecting;
    assert_eq!(machine, SelectionMachine::Selecting);

    let rect = normalize_rect((300, 400), (100, 150));
    assert_eq!(
        rect,
        SelectionRect {
            x: 100,
            y: 150,
            w: 200,
            h: 250
        }
    );

    assert!(hit_test_selection(&rect, (150, 200)));
    assert!(!hit_test_selection(&rect, (99, 200)));
    assert_eq!(hit_test_handle(&rect, (100, 150), 8), Some(Handle::TopLeft));
    assert_eq!(
        hit_test_handle(&rect, (300, 400), 8),
        Some(Handle::BottomRight)
    );

    let resized = apply_resize(
        &rect,
        Handle::BottomRight,
        (-500, -500),
        1920,
        1080,
        MIN_SELECTION_SIZE,
    );
    assert!(resized.w >= MIN_SELECTION_SIZE);
    assert!(resized.h >= MIN_SELECTION_SIZE);

    let moved = apply_move(&rect, (2000, 2000), 1920, 1080);
    assert_eq!(moved.x + moved.w as i32, 1920);
    assert_eq!(moved.y + moved.h as i32, 1080);

    assert_eq!(clamp_point_to_selection((0, 0), &rect), (100, 150));
    assert_eq!(clamp_point_to_selection((999, 999), &rect), (299, 399));

    let snapped_line = constrain_line_shift((100, 100), (200, 105));
    assert_eq!(snapped_line.1, 100);
    let snapped_square = constrain_rect_shift((100, 100), (180, 130));
    assert_eq!(snapped_square, (180, 180));
}

#[test]
fn headless_toolbar_placement_never_overlaps_or_leaves_screen() {
    let cases = [
        RectF {
            x: 200.0,
            y: 200.0,
            w: 400.0,
            h: 300.0,
        },
        RectF {
            x: 1850.0,
            y: 1020.0,
            w: 60.0,
            h: 50.0,
        },
        RectF {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        },
        RectF {
            x: 10.0,
            y: 10.0,
            w: 20.0,
            h: 20.0,
        },
    ];

    let screen = (1920.0, 1080.0);
    let vert_sz = (40.0, 320.0);
    let horiz_sz = (280.0, 40.0);

    for sel in cases {
        let pos = calculate_toolbar_positions(sel, screen, vert_sz, horiz_sz);
        let vert_rect = RectF {
            x: pos.vert.0,
            y: pos.vert.1,
            w: vert_sz.0,
            h: vert_sz.1,
        };
        let horiz_rect = RectF {
            x: pos.horiz.0,
            y: pos.horiz.1,
            w: horiz_sz.0,
            h: horiz_sz.1,
        };

        assert!(!vert_rect.overlaps(&horiz_rect));
        assert!(vert_rect.x >= 0.0 && vert_rect.x + vert_rect.w <= screen.0 + 0.5);
        assert!(vert_rect.y >= 0.0 && vert_rect.y + vert_rect.h <= screen.1 + 0.5);
        assert!(horiz_rect.x >= 0.0 && horiz_rect.x + horiz_rect.w <= screen.0 + 0.5);
        assert!(horiz_rect.y >= 0.0 && horiz_rect.y + horiz_rect.h <= screen.1 + 0.5);
    }
}

#[test]
fn headless_updater_version_and_asset_logic() {
    assert_eq!(parse_version("v0.1.6"), Some((0, 1, 6)));
    assert_eq!(parse_version("1.2.3-rc1"), Some((1, 2, 3)));
    assert!(is_newer_version("v0.2.0", "0.1.6"));
    assert!(!is_newer_version("v0.1.6", "0.1.6"));
    assert!(!is_newer_version("v0.1.5", "0.1.6"));

    assert_eq!(
        expected_installer_asset_name("v0.1.6"),
        Some("LocalShot-Setup-0.1.6-x64.exe".to_string())
    );

    let assets = vec![
        json!({
            "name": "LocalShot-Setup-0.1.6-x64.exe",
            "browser_download_url": "https://github.com/abdurashid-juraev/Localshot/releases/download/v0.1.6/LocalShot-Setup-0.1.6-x64.exe"
        }),
        json!({
            "name": "LocalShot-Setup-0.1.6-x64.exe.minisig",
            "browser_download_url": "https://github.com/abdurashid-juraev/Localshot/releases/download/v0.1.6/LocalShot-Setup-0.1.6-x64.exe.minisig"
        }),
    ];
    let selected = select_release_assets(&assets, "v0.1.6");
    assert!(selected.is_some());

    assert!(validate_download_url(
        "https://github.com/abdurashid-juraev/Localshot/releases/download/v0.1.6/LocalShot-Setup-0.1.6-x64.exe",
        &["github.com"]
    )
    .is_ok());
    assert!(validate_download_url("http://github.com/file.exe", &["github.com"]).is_err());
    assert!(validate_download_url("https://evil.example/file.exe", &["github.com"]).is_err());

    assert!(is_updater_disabled_with(
        Some(Path::new("/usr/bin/localshot")),
        None,
        None
    ));
    assert!(is_updater_disabled_with(
        Some(Path::new("/home/user/localshot")),
        Some("/tmp/LocalShot.AppImage"),
        None
    ));
    assert!(!is_updater_disabled_with(
        Some(Path::new("/usr/bin/localshot")),
        None,
        Some("1")
    ));

    let notes = format_changelog("- Fix A\n- Fix B\n- Fix C", 2, 200);
    assert!(notes.contains("Fix A"));
    assert!(notes.contains("Fix B"));
    assert!(notes.contains("more in release notes"));
}

#[test]
fn headless_daemon_ipc_command_and_tooltip_logic() {
    assert_eq!(parse_ipc_command("AREA"), Ok(IpcCommand::CaptureArea));
    assert_eq!(parse_ipc_command("  area \n"), Ok(IpcCommand::CaptureArea));
    assert_eq!(parse_ipc_command("FULL"), Ok(IpcCommand::CaptureFull));
    assert_eq!(parse_ipc_command("QUIT"), Ok(IpcCommand::Quit));
    assert_eq!(parse_ipc_command("PING"), Ok(IpcCommand::Ping));
    assert!(parse_ipc_command("UNKNOWN").is_err());

    let both = tray_tooltip_for_hotkeys(true, true);
    let fallback = tray_tooltip_for_hotkeys(false, true);
    let none = tray_tooltip_for_hotkeys(false, false);
    assert_ne!(both, fallback);
    assert_ne!(fallback, none);
}

#[test]
fn headless_config_filename_sanitization() {
    assert_eq!(
        sanitize_filename_part("..//bad:name*?.png", "LocalShot"),
        "badname.png"
    );
    assert_eq!(sanitize_filename_part("...   ", "LocalShot"), "LocalShot");
    assert_eq!(sanitize_filename_part("shot_01", "LocalShot"), "shot_01");
}

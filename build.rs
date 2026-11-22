fn main() {
    // Rerun when UI or assets change (item 6)
    println!("cargo:rerun-if-changed=ui/overlay.slint");
    println!("cargo:rerun-if-changed=assets/");

    slint_build::compile("ui/overlay.slint").unwrap();

    // Item 6: use std::env::var("CARGO_CFG_TARGET_OS") – build scripts run on the host,
    // so #[cfg(target_os = "...")] tests the HOST, not the cross-compilation target.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_os == "windows" {
        // winres is a [target.'cfg(windows)'.build-dependencies] entry, so this branch
        // is only reachable when cross-compiling or building natively on Windows.
        // On Linux hosts this branch is never taken.
        windows_resources();
    }

    if target_os == "linux" {
        if let Ok(home) = std::env::var("HOME") {
            let local_lib = std::path::Path::new(&home).join(".local").join("lib");
            if local_lib.is_dir() {
                println!("cargo:rustc-link-search=native={}", local_lib.display());
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn windows_resources() {
    let mut res = winres::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    res.compile().unwrap();
}

#[cfg(not(target_os = "windows"))]
fn windows_resources() {}

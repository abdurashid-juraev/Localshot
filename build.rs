fn main() {
    slint_build::compile("ui/overlay.slint").unwrap();

    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.compile().unwrap();
    }

    #[cfg(target_os = "linux")]
    if let Ok(home) = std::env::var("HOME") {
        let local_lib = std::path::Path::new(&home).join(".local").join("lib");
        if local_lib.is_dir() {
            println!("cargo:rustc-link-search=native={}", local_lib.display());
        }
    }
}

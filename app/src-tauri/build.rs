fn main() {
    // Compile the native macOS Settings window. Rust calls it through
    // the C interface in src/settings_window.h.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/settings_window.m")
            .file("src/overlay_panel.m")
            .flag("-fobjc-arc")
            .compile("reamesh_native");
        println!("cargo:rustc-link-lib=framework=Cocoa");
        // CGWindowListCopyWindowInfo, for following REAPER's window.
        println!("cargo:rustc-link-lib=framework=CoreGraphics");
        println!("cargo:rerun-if-changed=src/settings_window.m");
        println!("cargo:rerun-if-changed=src/settings_window.h");
        println!("cargo:rerun-if-changed=src/overlay_panel.m");
        println!("cargo:rerun-if-changed=src/overlay_panel.h");
    }

    tauri_build::build()
}

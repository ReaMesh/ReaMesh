fn main() {
    // Compile the native macOS Settings window. Rust calls it through
    // the C interface in src/settings_window.h.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/settings_window.m")
            .flag("-fobjc-arc")
            .compile("settings_window");
        println!("cargo:rustc-link-lib=framework=Cocoa");
        println!("cargo:rerun-if-changed=src/settings_window.m");
        println!("cargo:rerun-if-changed=src/settings_window.h");
    }

    tauri_build::build()
}

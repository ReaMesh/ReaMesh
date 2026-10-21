fn main() {
    // On macOS the presence HUD (a non-activating panel), the About window and
    // the Bluetooth radio are native AppKit code, compiled here; Rust talks to
    // them over tiny C ABIs (overlay_panel.h, about_window.h, ble.h).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/overlay_panel.m")
            .file("src/about_window.m")
            .file("src/ble.m")
            .flag("-fobjc-arc")
            .compile("reamesh_native");
        println!("cargo:rustc-link-lib=framework=Cocoa");
        // CGWindowListCopyWindowInfo, for following REAPER's window.
        println!("cargo:rustc-link-lib=framework=CoreGraphics");
        // CADisplayLink, driving the overlay's follow tick.
        println!("cargo:rustc-link-lib=framework=QuartzCore");
        // Nearby-session hints over Bluetooth LE (src/ble.m).
        println!("cargo:rustc-link-lib=framework=CoreBluetooth");
        println!("cargo:rerun-if-changed=src/overlay_panel.m");
        println!("cargo:rerun-if-changed=src/overlay_panel.h");
        println!("cargo:rerun-if-changed=src/about_window.m");
        println!("cargo:rerun-if-changed=src/about_window.h");
        println!("cargo:rerun-if-changed=src/ble.m");
        println!("cargo:rerun-if-changed=src/ble.h");
    }

    build_info();

    tauri_build::build()
}

/// The commit and build time the About window shows. Reruns whenever the
/// checked-out commit moves or the crate's sources change, so the stamp
/// follows the binary rather than the first build of the day.
fn build_info() {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let commit = git(&["rev-parse", "--short=7", "HEAD"])
        .or_else(|| std::env::var("GITHUB_SHA").ok().map(|s| s.chars().take(7).collect()))
        .unwrap_or_default();
    println!("cargo:rustc-env=REAMESH_COMMIT={commit}");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=REAMESH_BUILD_TIME={now}");

    for path in ["HEAD", "packed-refs"] {
        if let Some(p) = git(&["rev-parse", "--path-format=absolute", "--git-path", path]) {
            println!("cargo:rerun-if-changed={p}");
        }
    }
    if let Some(head_ref) = git(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(p) = git(&["rev-parse", "--path-format=absolute", "--git-path", &head_ref]) {
            println!("cargo:rerun-if-changed={p}");
        }
    }
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=build.rs");
}

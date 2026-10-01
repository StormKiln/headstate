fn main() {
    tauri_build::build();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/notifications.m")
            .flag("-fobjc-arc")
            .flag("-fblocks")
            .flag(format!(
                "-mmacosx-version-min={}",
                std::env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| "10.13".into())
            ))
            .compile("headstate_notifications");
        println!("cargo:rustc-link-lib=framework=AppKit");
        // UserNotifications does not exist on the supported macOS 10.13 baseline.
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,UserNotifications");
        println!("cargo:rerun-if-changed=src/notifications.m");
    }
}

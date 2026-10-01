fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/notifications.m")
            .flag("-fobjc-arc")
            .flag("-fblocks")
            .compile("headstate_notifications");
        println!("cargo:rustc-link-lib=framework=AppKit");
        println!("cargo:rustc-link-lib=framework=UserNotifications");
        println!("cargo:rerun-if-changed=src/notifications.m");
    }
    tauri_build::build()
}

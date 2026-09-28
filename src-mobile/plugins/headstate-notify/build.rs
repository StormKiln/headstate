//! Standard Tauri plugin build script. `COMMANDS` names the native
//! commands so the permission files exist in the shape Tauri expects;
//! nothing in the webview is ever granted them, because every call into
//! this plugin comes from Rust (`run_mobile_plugin`), which does not
//! pass through the ACL. The frontend cannot ask for notification
//! permission or post a notification on its own.
//!
//! No `android_path`. This plugin is iOS-only by decision, not by
//! omission: #789 is an iOS feature, Android notifications are a
//! different API with a different permission model (a runtime
//! `POST_NOTIFICATIONS` grant since API 33 plus a channel), and shipping
//! a half-built Android path would be worse than shipping none -- a
//! silent no-op is the failure mode that takes longest to notice. The
//! Rust side reports `Unavailable` on every non-iOS target, which the
//! caller logs, so an Android build says so in the log rather than
//! quietly dropping notifications.
//!
//! That absence also keeps `scripts/check-plugin-commands.py` honest:
//! it compares Kotlin `@Command` names against Rust's `mod cmd`
//! constants and skips any plugin with no `android/` directory. A plugin
//! with an Android folder and no Kotlin would fail it, correctly.

const COMMANDS: &[&str] = &["permission", "request_permission", "post", "take_tapped"];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).ios_path("ios").build();
}

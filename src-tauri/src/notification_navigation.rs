//! Notification clicks are retained until the app shell is ready. An event is
//! only a wakeup: consuming the pending identity makes duplicate wakeups safe.
//! Native click routing needs macOS 10.14+. macOS 10.13 keeps legacy plugin
//! delivery and its existing click behavior; no routing claim applies there.
use crate::identity::PrIdentity;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Where a click goes: the pull request, or -- for a Jira drift notice
/// (#2043) -- the issue it names, in My Jira issues. Flattened, so a
/// notification delivered by an older build, whose payload was a bare
/// `PrIdentity`, still reads as one with no issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    #[serde(flatten)]
    pub pr: PrIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jira_key: Option<String>,
}

static PENDING: Mutex<Option<Target>> = Mutex::new(None);

#[tauri::command]
pub fn take_notification_pr() -> Option<Target> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).take()
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::{
        ffi::{c_char, CStr, CString},
        sync::OnceLock,
    };
    use tauri::Emitter;
    static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
    unsafe extern "C" {
        fn headstate_notifications_init(callback: extern "C" fn(*const c_char));
        fn headstate_notify(title: *const c_char, body: *const c_char, identity: *const c_char);
        fn headstate_notifications_supported() -> bool;
        pub fn headstate_about();
    }
    extern "C" fn clicked(payload: *const c_char) {
        // The bridge calls on the main queue and keeps the NSString alive
        // through this call. Copy immediately; nothing borrowed escapes FFI.
        let target = if payload.is_null() {
            None
        } else {
            serde_json::from_slice::<Target>(unsafe { CStr::from_ptr(payload) }.to_bytes()).ok()
        };
        if let Some(app) = APP.get() {
            let has_target = target.is_some();
            *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = target;
            if has_target {
                let _ = app.emit("notification-pr-pending", ());
            }
            crate::show_main_window(app);
        }
    }
    pub fn setup(app: &tauri::AppHandle) {
        let _ = APP.set(app.clone());
        unsafe {
            headstate_notifications_init(clicked);
        }
    }
    pub fn supported() -> bool {
        unsafe { headstate_notifications_supported() }
    }
    pub fn notify(title: &str, body: &str, identity: &Target) {
        let strings = (
            CString::new(title),
            CString::new(body),
            CString::new(serde_json::to_string(identity).unwrap_or_default()),
        );
        if let (Ok(title), Ok(body), Ok(identity)) = strings {
            unsafe {
                headstate_notify(title.as_ptr(), body.as_ptr(), identity.as_ptr());
            }
        }
    }
}
#[cfg(target_os = "macos")]
pub use macos::{headstate_about, notify, setup, supported};

#[cfg(test)]
mod tests {
    use super::*;
    // Cargo's native test executable is not an application bundle. The
    // supported gate must leave dev notifications on the plugin path,
    // which supplies com.apple.Terminal as their bundle identity.
    #[cfg(target_os = "macos")]
    #[test]
    fn unbundled_process_keeps_legacy_notification_delivery() {
        assert!(!supported());
    }
    #[test]
    fn click_handoff_keeps_provider_identity_and_consumes_only_once() {
        let target = Target {
            pr: PrIdentity {
                source: crate::identity::Source {
                    provider: crate::identity::Provider::Gitlab,
                    host: "gitlab.example".into(),
                },
                repo: "team/sub/project".into(),
                number: 7,
            },
            jira_key: None,
        };
        *PENDING.lock().unwrap() = Some(target.clone());
        assert_eq!(take_notification_pr(), Some(target));
        assert_eq!(take_notification_pr(), None);
    }

    /// The payload crosses the native bridge as JSON. An older build's bare
    /// `PrIdentity` still reads, and a drift notice carries its issue.
    #[test]
    fn the_payload_reads_old_and_new_shapes() {
        let old = r#"{"source":{"provider":"github","host":"github.com"},"repo":"octocat/hello-world","number":7}"#;
        let t: Target = serde_json::from_str(old).unwrap();
        assert_eq!((t.pr.number, t.jira_key.as_deref()), (7, None));
        assert_eq!(
            serde_json::to_string(&t).unwrap(),
            old,
            "no key: the old shape"
        );
        let with = Target {
            jira_key: Some("ABC-1".into()),
            ..t
        };
        let back: Target = serde_json::from_str(&serde_json::to_string(&with).unwrap()).unwrap();
        assert_eq!(back, with);
    }
}

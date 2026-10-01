//! Notification clicks are retained until the app shell is ready. An event is
//! only a wakeup: consuming the pending identity makes duplicate wakeups safe.
use crate::identity::PrIdentity;
use std::sync::Mutex;

static PENDING: Mutex<Option<PrIdentity>> = Mutex::new(None);

#[tauri::command]
pub fn take_notification_pr() -> Option<PrIdentity> {
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
        pub fn headstate_about();
    }
    extern "C" fn clicked(payload: *const c_char) {
        // The bridge calls on the main queue and keeps the NSString alive
        // through this call. Copy immediately; nothing borrowed escapes FFI.
        let target = if payload.is_null() {
            None
        } else {
            serde_json::from_slice::<PrIdentity>(unsafe { CStr::from_ptr(payload) }.to_bytes()).ok()
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
    pub fn notify(title: &str, body: &str, identity: &PrIdentity) {
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
pub use macos::{headstate_about, notify, setup};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn click_handoff_keeps_provider_identity_and_consumes_only_once() {
        let target = PrIdentity {
            source: crate::identity::Source {
                provider: crate::identity::Provider::Gitlab,
                host: "gitlab.example".into(),
            },
            repo: "team/sub/project".into(),
            number: 7,
        };
        *PENDING.lock().unwrap() = Some(target.clone());
        assert_eq!(take_notification_pr(), Some(target));
        assert_eq!(take_notification_pr(), None);
    }
}

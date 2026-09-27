//! `tauri-plugin-headstate-notify`: the phone's LOCAL user
//! notifications (#789).
//!
//! `UNUserNotificationCenter`, posted from inside the background refresh
//! window `tauri-plugin-headstate-refresh` is granted. That is the
//! entire mechanism, and #789's scope comment settles it: **no APNs, no
//! certificates, no push server.**
//!
//! # The tradeoff, stated rather than buried
//!
//! Delivery is BEST-EFFORT. iOS decides when to grant a background
//! refresh window -- opportunistically, at most every fifteen minutes
//! or so, and never for an app the user has force-quit -- so a new pull
//! request surfaces "within the hour", not instantly. That is the honest
//! description of this feature and it belongs in the UI copy as well as
//! here.
//!
//! The alternative was APNs push from the desktop, which is timely and
//! much more work: credentials, a push path from a desktop on a
//! Tailscale-style link rather than a public server, and per-device
//! targeting built on the pairing identity. #789 keeps it as a possible
//! follow-up if an hour proves too slow in practice. It is not a thing
//! this plugin can grow into incrementally -- it is a different
//! delivery path -- which is why the scope decision was made before any
//! code.
//!
//! # Why this is a separate plugin from `headstate-refresh`
//!
//! The refresh plugin owns an OS SCHEDULER and a channel Rust is called
//! back on. This one owns a post-and-return API with a permission gate.
//! Folding them together would put a user-facing permission prompt
//! inside the lifecycle object that registers background tasks before
//! `UIApplicationMain`, and the ask-once discipline below is precisely
//! about WHEN the prompt appears.
//!
//! It also keeps `scripts/check-plugin-commands.py` satisfied for free:
//! the refresh plugin has Kotlin, so every name in its `mod cmd` must
//! have a matching `@Command`. This plugin is iOS-only and has no
//! `android/` directory, so the check skips it -- see `build.rs` on why
//! Android is absent by decision rather than by omission.
//!
//! # Ask ONCE, before the first use
//!
//! [`HeadstateNotify::allowed`] mirrors the desktop's
//! `poll::notification_allowed`: read the state, and only if it is
//! undetermined ask for it -- then remember the answer for the process.
//! Prompting at launch was rejected there and is rejected here for the
//! same reason. On a phone it is worse: the first launch of a companion
//! app is the pairing flow, a QR scan the user is in the middle of, and
//! a permission sheet in front of it is a sheet dismissed without being
//! read. A denial is then permanent and silent.
//!
//! So the prompt appears the first time the app has something to say.
//! A user who sees "Allow notifications?" immediately after a pull
//! request appeared has the context to answer it.
//!
//! # A tap on a SESSION notification is remembered, not pushed (#1486)
//!
//! #789 left taps unhandled: tapping opened the app, and that was the
//! whole of what a pull-request notification needed. A session
//! notification is different -- "waiting for your input" is only useful
//! if the tap lands on THAT session's transcript -- so the Swift side now
//! installs a `UNUserNotificationCenterDelegate` when the plugin is
//! registered, during app setup.
//!
//! The delegate does not call into Rust or the webview. It stores the
//! tapped notification's `session` in one slot, and
//! [`HeadstateNotify::take_tapped`] reads and clears it. The webview asks
//! whenever it becomes visible. That shape is deliberate: a cold launch
//! from a tap delivers the response before the webview exists, and a
//! pushed event with no listener yet would be lost. A slot that waits to
//! be asked cannot be.
//!
//! The same delegate answers `willPresent` with NO presentation options,
//! which is what iOS did before it existed: while the app is in the
//! foreground the app shows its own in-app toasts instead of a banner.
//! Pull-request notifications carry no `session` and a tap on one still
//! just opens the app.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

mod bridge;

use bridge::Bridge;

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_headstate_notify);

/// The native command names.
///
/// Swift dispatches on the Objective-C selector `<name>:`, so these are
/// METHOD names rather than snake_case -- the same split
/// `headstate-keys` documents, and the same one `build.rs` names in
/// snake_case for the ACL files. There is no Kotlin to disagree with
/// (see `build.rs`), but the constants live here anyway so the literals
/// are pinned by a test rather than spelled at each call site.
pub mod cmd {
    /// Read the current authorization state without prompting.
    pub const PERMISSION: &str = "permission";
    /// Prompt, if and only if the state is undetermined.
    pub const REQUEST_PERMISSION: &str = "requestPermission";
    /// Post one notification now.
    pub const POST: &str = "post";
    /// Read and clear the session of the notification last tapped.
    pub const TAKE_TAPPED: &str = "takeTapped";
}

/// What iOS says about notification authorization.
///
/// Three states, not a bool, for the reason `poll::notification_allowed`
/// needs three: "not yet asked" and "asked and refused" lead to opposite
/// actions, and collapsing them would either re-prompt a user who said
/// no or never prompt a user who has not been asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    /// Notifications may be posted.
    Granted,
    /// The user has not been asked yet.
    Prompt,
    /// The user said no. Only Settings can change this, so the app must
    /// never ask again.
    Denied,
}

/// What the native side answers with.
#[derive(Debug, Deserialize)]
struct PermissionReply {
    permission: Permission,
}

/// One notification to post.
///
/// No category and no badge. A category is only useful with action
/// buttons; a badge count is state the app would have to maintain
/// correctly across a suspension to avoid showing a stale number, which
/// is a worse failure than showing none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub title: String,
    pub body: String,
    /// The Claude Code session a tap should open (#1486), carried in the
    /// notification's `userInfo`. An id, never text. `None` for a
    /// notification a tap should simply open the app for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

/// What `takeTapped` answers.
#[derive(Debug, Deserialize)]
struct TappedReply {
    session: Option<String>,
}

/// Why a notification was not posted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// No native side on this platform. Every non-iOS target, including
    /// the desktop host the tests run on.
    #[error("local notifications are not available on this platform")]
    Unavailable,
    /// The user has refused, or the prompt was dismissed.
    #[error("notifications are not permitted")]
    Denied,
    /// The native call failed.
    #[error("{0}")]
    Native(String),
}

/// The plugin's state: the bridge, and what the permission gate has
/// already learnt.
pub struct HeadstateNotify {
    bridge: Box<dyn Bridge>,
    /// The answer, once iOS has given one.
    ///
    /// Cached for the life of the process so the gate costs one native
    /// round trip rather than one per notification -- a window that
    /// posts five would otherwise make five. Not persisted: iOS is the
    /// authority and the user can change it in Settings between
    /// launches, so a stored "denied" would outlive the decision and
    /// silence an app the user had since allowed.
    ///
    /// `Prompt` is never cached. It is the one state that is not an
    /// answer.
    known: Mutex<Option<Permission>>,
}

impl HeadstateNotify {
    pub fn new(bridge: Box<dyn Bridge>) -> Self {
        Self {
            bridge,
            known: Mutex::new(None),
        }
    }

    /// Whether a notification may be posted, asking ONCE if iOS has not
    /// been asked yet.
    ///
    /// Mirrors `poll::notification_allowed` on the desktop, including
    /// what it logs: a denial is INFO, not a warning, because the user
    /// said no and that is a choice rather than a fault. Silence there
    /// made "why do I get no notifications?" unanswerable from the log,
    /// and it would be worse here -- on a phone there is no window to
    /// check a setting in while the background window is open.
    pub fn allowed(&self) -> bool {
        match self.permission() {
            Ok(Permission::Granted) => true,
            Ok(Permission::Denied) => {
                log::info!("headstate-notify: notifications are denied; not notifying");
                false
            }
            Ok(Permission::Prompt) => match self.request() {
                Ok(Permission::Granted) => true,
                Ok(other) => {
                    log::info!("headstate-notify: permission request answered {other:?}");
                    false
                }
                Err(e) => {
                    log::warn!("headstate-notify: could not request permission: {e}");
                    false
                }
            },
            Err(Error::Unavailable) => {
                // Debug, not warn: on the desktop host this is every
                // test run and the expected state, so a warning here
                // would be noise that trains people to ignore the log.
                log::debug!("headstate-notify: no native side; nothing to notify with");
                false
            }
            Err(e) => {
                log::warn!("headstate-notify: could not read permission: {e}");
                false
            }
        }
    }

    /// The current state, from the cache where there is one.
    pub fn permission(&self) -> Result<Permission, Error> {
        if let Some(known) = *self.known.lock().unwrap_or_else(|e| e.into_inner()) {
            return Ok(known);
        }
        let state = self.ask(cmd::PERMISSION)?;
        self.remember(state);
        Ok(state)
    }

    /// Prompt. Only called from [`allowed`] and only when the state is
    /// `Prompt`, which is what makes this ask-once rather than
    /// ask-every-time.
    ///
    /// [`allowed`]: HeadstateNotify::allowed
    fn request(&self) -> Result<Permission, Error> {
        let state = self.ask(cmd::REQUEST_PERMISSION)?;
        self.remember(state);
        Ok(state)
    }

    fn remember(&self, state: Permission) {
        // `Prompt` is not an answer and must not be cached: caching it
        // would mean the gate never asks again, which is exactly the
        // bug the three-state enum exists to avoid.
        if state != Permission::Prompt {
            *self.known.lock().unwrap_or_else(|e| e.into_inner()) = Some(state);
        }
    }

    fn ask(&self, command: &str) -> Result<Permission, Error> {
        let reply: PermissionReply = self.bridge.call(command, serde_json::json!({}))?;
        Ok(reply.permission)
    }

    /// Post one notification, if permitted.
    ///
    /// Returns `Err` rather than swallowing, so the caller decides --
    /// the callers in `src-mobile/src/notify.rs` log and continue,
    /// matching "a notification is an affordance and losing one must
    /// never take down the thing that produced it", but a silent `Ok`
    /// here would make that choice for every future caller and hide a
    /// broken bridge completely.
    pub fn post(&self, notification: &Notification) -> Result<(), Error> {
        if !self.allowed() {
            return Err(Error::Denied);
        }
        let args = serde_json::to_value(notification)
            .map_err(|e| Error::Native(format!("could not encode the notification: {e}")))?;
        let _: serde_json::Value = self.bridge.call(cmd::POST, args)?;
        Ok(())
    }

    /// The session of the notification the user last tapped, cleared as
    /// it is read, so one tap opens the session once (#1486).
    ///
    /// No permission gate: reading what was tapped posts nothing, and a
    /// notification that was tapped was by definition allowed.
    pub fn take_tapped(&self) -> Result<Option<String>, Error> {
        let reply: TappedReply = self.bridge.call(cmd::TAKE_TAPPED, serde_json::json!({}))?;
        Ok(reply.session)
    }
}

/// Reach the plugin from an app handle.
pub trait HeadstateNotifyExt<R: Runtime> {
    fn headstate_notify(&self) -> tauri::State<'_, HeadstateNotify>;
}

impl<R: Runtime, T: Manager<R>> HeadstateNotifyExt<R> for T {
    fn headstate_notify(&self) -> tauri::State<'_, HeadstateNotify> {
        self.state::<HeadstateNotify>()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("headstate-notify")
        .setup(|app, _api| {
            #[cfg(target_os = "ios")]
            let bridge: Box<dyn Bridge> = Box::new(bridge::Native(
                _api.register_ios_plugin(init_plugin_headstate_notify)?,
            ));
            // Android included: see `build.rs`. `Unavailable` REPORTS
            // the absence rather than accepting and dropping, so a
            // platform with no native side says so in the log.
            #[cfg(not(target_os = "ios"))]
            let bridge: Box<dyn Bridge> = Box::new(bridge::Unavailable);
            app.manage(HeadstateNotify::new(bridge));
            Ok(())
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;

    /// A bridge that answers with a scripted sequence of permission
    /// states and records every call.
    ///
    /// Shared through an `Arc` so a test can read the call log after
    /// driving the plugin: the plugin takes its bridge boxed, and
    /// `Arc<Fake>` implementing [`Bridge`] is what lets one `Fake` be
    /// both the plugin's bridge and the test's recorder.
    #[derive(Default)]
    struct Fake {
        /// Answers to `permission` / `requestPermission`, in order. The
        /// LAST one repeats, so a one-element script is a constant
        /// answer.
        states: StdMutex<Vec<Permission>>,
        calls: StdMutex<Vec<String>>,
        posts: StdMutex<Vec<Notification>>,
        /// What the Swift delegate's slot holds.
        tapped: StdMutex<Option<String>>,
    }

    impl Fake {
        fn new(states: &[Permission]) -> Arc<Self> {
            Arc::new(Self {
                states: StdMutex::new(states.to_vec()),
                ..Default::default()
            })
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
        fn posts(&self) -> Vec<Notification> {
            self.posts.lock().unwrap().clone()
        }
    }

    impl Bridge for Arc<Fake> {
        fn call_json(&self, command: &str, args: serde_json::Value) -> Result<String, Error> {
            self.calls.lock().unwrap().push(command.to_string());
            if command == cmd::POST {
                self.posts.lock().unwrap().push(Notification {
                    title: args["title"].as_str().unwrap_or_default().to_string(),
                    body: args["body"].as_str().unwrap_or_default().to_string(),
                    session: args["session"].as_str().map(str::to_string),
                });
                return Ok("{}".into());
            }
            if command == cmd::TAKE_TAPPED {
                let tapped = self.tapped.lock().unwrap().take();
                return Ok(serde_json::json!({ "session": tapped }).to_string());
            }
            let mut states = self.states.lock().unwrap();
            let state = if states.len() > 1 {
                states.remove(0)
            } else {
                *states.first().expect("the fake needs a state")
            };
            Ok(serde_json::json!({ "permission": state }).to_string())
        }
    }

    fn plugin(fake: &Arc<Fake>) -> HeadstateNotify {
        HeadstateNotify::new(Box::new(fake.clone()))
    }

    #[test]
    fn granted_permission_needs_no_prompt() {
        let fake = Fake::new(&[Permission::Granted]);
        let n = plugin(&fake);
        assert!(n.allowed());
        assert_eq!(n.permission(), Ok(Permission::Granted));
        assert_eq!(
            fake.calls(),
            vec![cmd::PERMISSION.to_string()],
            "read once, never prompted, and the answer cached"
        );
    }

    #[test]
    fn a_denial_is_not_a_prompt() {
        let fake = Fake::new(&[Permission::Denied]);
        assert!(!plugin(&fake).allowed());
        assert_eq!(
            fake.calls(),
            vec![cmd::PERMISSION.to_string()],
            "a user who said no is never asked again"
        );
    }

    /// **The ask-once test, and the whole point of the permission
    /// gate.** An undetermined state prompts exactly once, and the
    /// answer is remembered -- so a window that posts five
    /// notifications does not show five sheets.
    #[test]
    fn an_undetermined_state_prompts_exactly_once() {
        let fake = Fake::new(&[Permission::Prompt, Permission::Granted]);
        let n = plugin(&fake);
        assert!(n.allowed());
        assert!(n.allowed());
        assert!(n.allowed());
        assert_eq!(
            fake.calls(),
            vec![
                cmd::PERMISSION.to_string(),
                cmd::REQUEST_PERMISSION.to_string()
            ],
            "read, then prompt, then nothing"
        );
    }

    /// A dismissed prompt answers `Denied`, which IS cached -- so the
    /// user is not asked again on the next window. iOS would refuse to
    /// show the sheet a second time anyway; caching means the app does
    /// not spend a native round trip per notification discovering that.
    #[test]
    fn a_refused_prompt_is_not_retried() {
        let fake = Fake::new(&[Permission::Prompt, Permission::Denied]);
        let n = plugin(&fake);
        assert!(!n.allowed());
        assert!(!n.allowed());
        assert_eq!(n.permission(), Ok(Permission::Denied));
        assert_eq!(
            fake.calls(),
            vec![
                cmd::PERMISSION.to_string(),
                cmd::REQUEST_PERMISSION.to_string()
            ]
        );
    }

    /// The desktop host, and every non-iOS target: reports the absence
    /// rather than accepting and dropping.
    #[test]
    fn no_native_side_is_an_error_not_a_silent_success() {
        let n = HeadstateNotify::new(Box::new(bridge::Unavailable));
        assert_eq!(n.permission(), Err(Error::Unavailable));
        assert!(!n.allowed());
        assert_eq!(
            n.post(&Notification {
                title: "x".into(),
                body: "y".into(),
                session: None,
            }),
            Err(Error::Denied),
            "the gate refuses before the bridge is reached"
        );
    }

    /// Posting carries the title and body through as the Swift
    /// `Decodable` expects them.
    #[test]
    fn a_post_carries_the_title_and_body() {
        let fake = Fake::new(&[Permission::Granted]);
        let n = plugin(&fake);
        assert_eq!(
            n.post(&Notification {
                title: "A pull request appeared".into(),
                body: "octocat/hello-world#7".into(),
                session: None,
            }),
            Ok(())
        );
        let posts = fake.posts();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].title, "A pull request appeared");
        assert_eq!(posts[0].body, "octocat/hello-world#7");
    }

    /// A denied user's notification is refused rather than posted, and
    /// the bridge never sees a `post` at all. The point of the gate.
    #[test]
    fn a_denied_user_gets_no_notification() {
        let fake = Fake::new(&[Permission::Denied]);
        let n = plugin(&fake);
        assert_eq!(
            n.post(&Notification {
                title: "x".into(),
                body: "y".into(),
                session: None,
            }),
            Err(Error::Denied)
        );
        assert!(fake.posts().is_empty());
        assert!(!fake.calls().contains(&cmd::POST.to_string()));
    }

    /// The wire names are METHOD names, because Swift dispatches on the
    /// selector. Pinned as literals so a rename on one side is a
    /// failing test rather than a notification that never appears on a
    /// device.
    #[test]
    fn the_wire_names_are_selector_names() {
        assert_eq!(cmd::PERMISSION, "permission");
        assert_eq!(cmd::REQUEST_PERMISSION, "requestPermission");
        assert_eq!(cmd::POST, "post");
        assert_eq!(cmd::TAKE_TAPPED, "takeTapped");
    }

    /// The Swift side implements every command Rust invokes.
    ///
    /// `mobile-ios` compiles Swift, so a selector typo is a build
    /// failure there -- but only on a job that takes minutes and only
    /// when something mobile changed. This costs nothing and names the
    /// mismatch directly.
    #[test]
    fn the_swift_side_implements_every_command() {
        let swift = include_str!("../ios/Sources/HeadstateNotifyPlugin.swift");
        for name in [
            cmd::PERMISSION,
            cmd::REQUEST_PERMISSION,
            cmd::POST,
            cmd::TAKE_TAPPED,
        ] {
            assert!(
                swift.contains(&format!("func {name}(")),
                "Swift has no `func {name}(`"
            );
        }
    }

    /// And the ACL list in `build.rs` names them in snake_case, the
    /// shape Tauri's permission files need. A command missing from it
    /// has no generated permission file, which `tauri ios build`
    /// notices and nothing else does.
    #[test]
    fn the_acl_list_names_every_command() {
        let build = include_str!("../build.rs");
        for name in [
            "\"permission\"",
            "\"request_permission\"",
            "\"post\"",
            "\"take_tapped\"",
        ] {
            assert!(build.contains(name), "build.rs COMMANDS is missing {name}");
        }
    }

    /// A session notification carries the session id to the Swift side
    /// for its `userInfo`, and a pull-request one carries no key at all
    /// -- so a tap on it still just opens the app (#1486).
    #[test]
    fn a_session_notification_carries_its_session_and_others_carry_none() {
        let fake = Fake::new(&[Permission::Granted]);
        let n = plugin(&fake);
        n.post(&Notification {
            title: "hello-world".into(),
            body: "Waiting for your input".into(),
            session: Some("s-1".into()),
        })
        .unwrap();
        n.post(&Notification {
            title: "Add a spoon".into(),
            body: "octocat/hello-world#7 just appeared".into(),
            session: None,
        })
        .unwrap();
        let posts = fake.posts();
        assert_eq!(posts[0].session.as_deref(), Some("s-1"));
        assert_eq!(posts[1].session, None);
        let wire = serde_json::to_value(&posts[1]).unwrap();
        assert!(wire.get("session").is_none(), "{wire}");
    }

    /// A tap is read ONCE: the slot clears, so returning to the app a
    /// second time does not re-open the same session over whatever the
    /// user moved on to.
    #[test]
    fn a_tapped_session_is_taken_once() {
        let fake = Fake::new(&[Permission::Granted]);
        *fake.tapped.lock().unwrap() = Some("s-1".into());
        let n = plugin(&fake);
        assert_eq!(n.take_tapped(), Ok(Some("s-1".into())));
        assert_eq!(n.take_tapped(), Ok(None));
    }

    #[test]
    fn no_native_side_has_no_tap_to_take() {
        let n = HeadstateNotify::new(Box::new(bridge::Unavailable));
        assert_eq!(n.take_tapped(), Err(Error::Unavailable));
    }
}

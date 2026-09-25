//! What the phone does when it is in the background: a single refresh
//! per OS-granted window, and nothing else.
//!
//! The spec's decision ("What the phone does when it is in the
//! background") is that the phone does nothing while suspended and
//! catches up on resume, which is the event subscriber's reconnect
//! (`subscribe_events`, `companion.rs`) and is not duplicated here. The
//! one cheap improvement it allows is [`BackgroundRefresh`]: when
//! `tauri-plugin-headstate-refresh` is granted a window, `GET /v1/hello`
//! and then the cached list, exactly what a connect does before the
//! stream, store the list as the snapshot, and return. On any error give
//! up quietly; nothing retries inside the window.
//!
//! **Never `/v1/events`.** The snapshot the subscriber gets is the first
//! frame of the stream; the background path must not open the stream,
//! so it asks for the same list through `get_cached` instead (the
//! desktop's `remote/events.rs` documents them as the same data). The
//! [`Desktop`] seam has no way to open the stream at all, and the tests
//! pin the request sequence -- against the fake, and against the
//! loopback server with the real client.
//!
//! [`Desktop`] and [`SnapshotSink`] are implemented by [`Companion`]:
//! `hello` is `Client::hello`, `get_cached` is
//! `Client::call("get_cached", {}, None)` on the live client, `save` is
//! `events::save_snapshot` plus `Connection::mark_poll`. A window does
//! not move the connection state: the subscriber owns that, and a
//! desktop that is away is the expected case for a phone in a pocket.
//!
//! # The notification pass (#789)
//!
//! A window now does one more thing, AFTER the snapshot is stored: it
//! compares the list it just fetched against what the last pass saw and
//! posts a local notification for anything that appeared, then asks the
//! desktop for its health conditions and posts the transitions. This is
//! the entire delivery path for #789 -- local notifications off the back
//! of the window iOS grants -- and its cost is that delivery is
//! BEST-EFFORT: a new pull request surfaces within the hour rather than
//! instantly.
//!
//! Three rules it follows, each the subject of a test below:
//!
//! 1. **The snapshot is stored first, and a notification failure never
//!    fails the window.** The snapshot is the thing the app opens with;
//!    notifications are an affordance. Ordering them the other way would
//!    let a denied permission cost the user a fresh list.
//! 2. **A third request, not a wider seam.** `health_alerts` is added to
//!    [`Desktop`] deliberately, and `the_seam_offers_only_what_a_window_needs`
//!    is updated with it rather than relaxed -- the seam's job is to make
//!    "this path cannot open the stream" structural, and it still has no
//!    method that could.
//! 3. **Health is asked for only when it would be used.** A phone whose
//!    health categories are all off makes two requests, not three. A
//!    background window is a few seconds of granted time and a request
//!    whose answer is discarded spends some of it.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::json;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_headstate_refresh::{RefreshFuture, Refresher};

use crate::companion::Companion;

/// A boxed request to the desktop.
pub type DesktopFuture<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// The three requests the background path may make. Deliberately
/// nothing else: no stream, no write commands.
///
/// `health_alerts` joined `hello` and `get_cached` for #789. The seam
/// grew by exactly one READ, and the test below was updated to name it
/// rather than loosened -- the seam exists so "this path cannot open the
/// stream" is a structural fact, and a third read does not weaken that.
pub trait Desktop: Send + Sync {
    /// `GET /v1/hello`. Proves the desktop is there and still knows us.
    fn hello(&self) -> DesktopFuture<()>;
    /// The cached PR list as `get_cached` returns it, JSON verbatim.
    fn get_cached(&self) -> DesktopFuture<String>;
    /// The desktop's current health conditions, as `health_alerts`
    /// returns them: verdicts the desktop evaluated, never the raw
    /// series. The phone holds no copy of any threshold -- see
    /// `notify.rs` on why that is the point rather than a convenience.
    fn health_alerts(&self) -> DesktopFuture<String>;
}

/// Where the list goes: the snapshot store, so the app opens fresh.
pub trait SnapshotSink: Send + Sync {
    fn save(&self, prs_json: &str) -> Result<(), String>;
}

/// What the phone knows about itself, and how it tells the user.
///
/// A seam rather than direct calls into the plugin and the store, for
/// the same reason [`Desktop`] is one: the notification rules are about
/// ordering and suppression ("a first sync announces nothing", "the
/// snapshot is stored even when notifying fails"), and those are only
/// testable if the posting and the remembering can be observed. On a
/// desktop host the real implementation posts nothing -- the plugin has
/// no native side there -- so without this seam the tests would be
/// asserting the absence of a platform rather than the presence of a
/// rule.
pub trait Notifier: Send + Sync {
    /// Which notifications the user wants.
    fn prefs(&self) -> crate::notify::PhoneNotifyPrefs;
    /// What the previous pass saw. [`crate::notify::Previous::First`] on
    /// a fresh install, which announces nothing.
    fn seen(&self) -> crate::notify::Previous;
    /// Remember this pass's list as the next pass's "previous".
    fn remember(&self, prs: &[crate::notify::Pr]) -> Result<(), String>;
    /// The paired desktop's name, for copy that must say whose machine a
    /// health alert is about. `None` while unpaired.
    fn machine(&self) -> Option<String>;
    /// Post one notification. Failure is the caller's to log.
    fn post(&self, title: &str, body: &str) -> Result<(), String>;
    /// The phone's record of which health conditions it has already
    /// announced, so a standing condition is said once.
    fn health_fired(&self) -> Arc<std::sync::Mutex<crate::notify::Fired>>;
}

impl Desktop for Companion {
    fn hello(&self) -> DesktopFuture<()> {
        let client = self.client();
        Box::pin(async move { client?.hello().await.map(|_| ()).map_err(|e| e.to_string()) })
    }

    fn get_cached(&self) -> DesktopFuture<String> {
        let client = self.client();
        Box::pin(async move {
            let list = client?
                .call("get_cached", &json!({}), None)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_string(&list).map_err(|e| e.to_string())
        })
    }

    fn health_alerts(&self) -> DesktopFuture<String> {
        let client = self.client();
        Box::pin(async move {
            let alerts = client?
                .call("health_alerts", &json!({}), None)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_string(&alerts).map_err(|e| e.to_string())
        })
    }
}

impl SnapshotSink for Companion {
    fn save(&self, prs_json: &str) -> Result<(), String> {
        self.record_snapshot(prs_json)
    }
}

/// The [`Refresher`] the plugin runs in a window.
pub struct BackgroundRefresh {
    desktop: Arc<dyn Desktop>,
    sink: Arc<dyn SnapshotSink>,
    /// `None` where nothing can notify -- a desktop host, or a build
    /// with no notification plugin. The window then does exactly what it
    /// did before #789, which is what keeps the existing tests
    /// meaningful rather than merely passing.
    notifier: Option<Arc<dyn Notifier>>,
}

impl BackgroundRefresh {
    pub fn new(desktop: Arc<dyn Desktop>, sink: Arc<dyn SnapshotSink>) -> Self {
        Self {
            desktop,
            sink,
            notifier: None,
        }
    }

    /// With notifications (#789).
    pub fn notifying(
        desktop: Arc<dyn Desktop>,
        sink: Arc<dyn SnapshotSink>,
        notifier: Arc<dyn Notifier>,
    ) -> Self {
        Self {
            desktop,
            sink,
            notifier: Some(notifier),
        }
    }
}

/// The notification half of a window, after the snapshot is safely
/// stored.
///
/// Returns nothing and fails at nothing: every error is logged and the
/// pass continues. A notification is an affordance and losing one must
/// never cost the user the fresh list the window's whole purpose was to
/// fetch -- the same rule `poll::notify_breakage` and `notify_battery`
/// follow on the desktop.
async fn notify_pass(desktop: &Arc<dyn Desktop>, notifier: &Arc<dyn Notifier>, prs_json: &str) {
    use crate::notify;

    let prefs = notifier.prefs();

    // New pull requests. The ORDER here matters: the seen set is
    // recorded whether or not anything was announced and whether or not
    // posting worked.
    //
    // Recorded on a failed post on purpose. The alternative -- retry
    // next window -- sounds kinder and is worse: a user who has denied
    // permission would accumulate an ever-growing backlog of "new" pull
    // requests, and the day they allowed notifications they would get
    // every one of them at once. An unposted notification is lost, which
    // is the honest outcome for a best-effort channel.
    //
    // Recorded with notifications OFF for the same reason: switching
    // them on must announce what appears NEXT, not everything that
    // appeared while they were off.
    match notify::decode_prs(prs_json) {
        notify::Decoded::List(current) => {
            if prefs.wants_new_pr() {
                for pr in notify::newly_appeared(&notifier.seen(), &current) {
                    let (title, body) = notify::appeared_notification(&pr);
                    if let Err(e) = notifier.post(&title, &body) {
                        log::info!("notify: could not announce a new pull request: {e}");
                    }
                }
            }
            if let Err(e) = notifier.remember(&current) {
                // Worth a warning: the next pass will treat a failed
                // write as a first sync and announce nothing, so a
                // persistently failing store means the feature is
                // silently dead.
                log::warn!("notify: could not record what this pass saw: {e}");
            }
        }
        // The payload could not be read -- a desktop whose list shape
        // this build does not recognise. The seen set is LEFT ALONE.
        //
        // Overwriting it with the empty list this case used to produce
        // was a burst waiting to happen: the record would be wiped, and
        // the next window that COULD read the payload would see every
        // pull request as new and fire one notification each. That is
        // precisely the failure first-sync suppression exists to
        // prevent, arriving by the back door.
        //
        // Leaving the record costs at most one round of missed
        // appearances, which is the right direction for a best-effort
        // channel.
        notify::Decoded::Unreadable => {
            log::info!("notify: the cached list could not be read; leaving the seen set alone");
        }
    }

    // Health. Not asked for at all when nothing would be posted: a
    // background window is a few seconds of granted time, and a request
    // whose answer is discarded spends some of it.
    let wants_health = prefs.enabled && (prefs.health_battery || prefs.health_cpu);
    if !wants_health {
        return;
    }
    let Some(machine) = notifier.machine() else {
        // Unpaired. Nothing to ask and, more to the point, no name to
        // put in front of the title -- and an unnamed health alert on a
        // phone reads as a claim about the phone.
        return;
    };
    let report = match desktop.health_alerts().await {
        Ok(json) => json,
        Err(e) => {
            // Info, not warn: a desktop that is away or too old to have
            // this command is the ordinary case for a phone in a pocket,
            // and the window already proved reachability with `hello`.
            log::info!("notify: could not read the desktop's health: {e}");
            return;
        }
    };
    let present: Vec<notify::HealthAlert> = notify::decode_health(&report)
        .into_iter()
        .filter(|a| prefs.wants_health(&a.key))
        .collect();
    let fired = notifier.health_fired();
    let transitions = {
        let mut fired = fired.lock().unwrap_or_else(|e| e.into_inner());
        notify::health_transitions(&mut fired, &present, &machine)
    };
    for (title, body) in transitions {
        if let Err(e) = notifier.post(&title, &body) {
            log::info!("notify: could not announce a health change: {e}");
        }
    }
}

impl Refresher for BackgroundRefresh {
    fn refresh(&self) -> RefreshFuture {
        let desktop = self.desktop.clone();
        let sink = self.sink.clone();
        let notifier = self.notifier.clone();
        Box::pin(async move {
            desktop.hello().await?;
            let prs = desktop.get_cached().await?;
            // The snapshot FIRST, and its failure is the window's
            // failure: it is the thing the app opens with. Notifications
            // are an affordance and come after, so a denied permission
            // can never cost the user a fresh list.
            sink.save(&prs)?;
            if let Some(notifier) = notifier {
                notify_pass(&desktop, &notifier, &prs).await;
            }
            Ok(())
        })
    }
}

/// Put the app's refresher in Tauri state for the plugin to find: a
/// [`BackgroundRefresh`] over the managed [`Companion`], so it must run
/// after `setup` has managed one.
pub fn install<R: Runtime>(app: &AppHandle<R>) {
    let companion = app.state::<Arc<Companion>>().inner().clone();
    let notifier: Arc<dyn Notifier> = Arc::new(crate::notify::PhoneNotifier::new(
        companion.clone(),
        app.clone(),
    ));
    let refresher: Arc<dyn Refresher> = Arc::new(BackgroundRefresh::notifying(
        companion.clone(),
        companion,
        notifier,
    ));
    app.manage(refresher);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::tests::Recorder;
    use crate::connection::State;
    use crate::events;
    use crate::keys::SoftwareKeys;
    use crate::store::MemoryStore;
    use crate::testing::{Reply, TestServer};
    use base64::Engine;
    use chrono::Utc;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Every request the fake desktop saw, by kind. Three variants
    /// because the seam has three methods; a fourth request kind cannot
    /// be recorded because it cannot be made -- which is the point of
    /// `the_seam_offers_only_what_a_window_needs` below.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Request {
        Hello,
        GetCached,
        HealthAlerts,
    }

    struct FakeDesktop {
        log: Mutex<Vec<Request>>,
        hello: Result<(), String>,
        cached: Result<String, String>,
        health: Result<String, String>,
    }

    impl FakeDesktop {
        fn new(hello: Result<(), String>, cached: Result<String, String>) -> Arc<Self> {
            Arc::new(Self {
                log: Mutex::new(vec![]),
                hello,
                cached,
                health: Ok("[]".into()),
            })
        }
        /// A desktop that also has health conditions to report.
        fn with_health(cached: &str, health: &str) -> Arc<Self> {
            Arc::new(Self {
                log: Mutex::new(vec![]),
                hello: Ok(()),
                cached: Ok(cached.into()),
                health: Ok(health.into()),
            })
        }
        fn requests(&self) -> Vec<Request> {
            self.log.lock().unwrap().clone()
        }
    }

    impl Desktop for FakeDesktop {
        fn hello(&self) -> DesktopFuture<()> {
            self.log.lock().unwrap().push(Request::Hello);
            let r = self.hello.clone();
            Box::pin(async move { r })
        }
        fn get_cached(&self) -> DesktopFuture<String> {
            self.log.lock().unwrap().push(Request::GetCached);
            let r = self.cached.clone();
            Box::pin(async move { r })
        }
        fn health_alerts(&self) -> DesktopFuture<String> {
            self.log.lock().unwrap().push(Request::HealthAlerts);
            let r = self.health.clone();
            Box::pin(async move { r })
        }
    }

    #[derive(Default)]
    struct FakeSink {
        saved: Mutex<Vec<String>>,
    }

    impl SnapshotSink for FakeSink {
        fn save(&self, prs_json: &str) -> Result<(), String> {
            self.saved.lock().unwrap().push(prs_json.to_string());
            Ok(())
        }
    }

    /// A notifier whose store, permission answer and posted
    /// notifications are all inspectable.
    struct FakeNotifier {
        prefs: crate::notify::PhoneNotifyPrefs,
        seen: Mutex<crate::notify::Previous>,
        machine: Option<String>,
        /// What `post` returns. `Err` stands in for a denied permission,
        /// which is the case the ordering rules are about.
        post_result: Result<(), String>,
        posted: Mutex<Vec<(String, String)>>,
        remembered: Mutex<Vec<Vec<crate::notify::Pr>>>,
        fired: Arc<Mutex<crate::notify::Fired>>,
    }

    impl FakeNotifier {
        fn build(
            seen: crate::notify::Previous,
            prefs: crate::notify::PhoneNotifyPrefs,
            machine: Option<String>,
            post_result: Result<(), String>,
        ) -> Arc<Self> {
            Arc::new(Self {
                prefs,
                seen: Mutex::new(seen),
                machine,
                post_result,
                posted: Mutex::new(vec![]),
                remembered: Mutex::new(vec![]),
                fired: Arc::new(Mutex::new(crate::notify::Fired::default())),
            })
        }
        /// Everything on, paired, posting succeeds.
        fn new(seen: crate::notify::Previous) -> Arc<Self> {
            Self::build(
                seen,
                crate::notify::PhoneNotifyPrefs::default(),
                Some("Mac mini".into()),
                Ok(()),
            )
        }
        fn with_prefs(
            seen: crate::notify::Previous,
            prefs: crate::notify::PhoneNotifyPrefs,
        ) -> Arc<Self> {
            Self::build(seen, prefs, Some("Mac mini".into()), Ok(()))
        }
        fn posted(&self) -> Vec<(String, String)> {
            self.posted.lock().unwrap().clone()
        }
        fn titles(&self) -> Vec<String> {
            self.posted().into_iter().map(|(t, _)| t).collect()
        }
    }

    impl Notifier for FakeNotifier {
        fn prefs(&self) -> crate::notify::PhoneNotifyPrefs {
            self.prefs
        }
        fn seen(&self) -> crate::notify::Previous {
            self.seen.lock().unwrap().clone()
        }
        fn remember(&self, prs: &[crate::notify::Pr]) -> Result<(), String> {
            self.remembered.lock().unwrap().push(prs.to_vec());
            // Also advance the in-memory "previous", so a test can run
            // two windows back to back the way the phone does.
            *self.seen.lock().unwrap() = crate::notify::Previous::Known(prs.to_vec());
            Ok(())
        }
        fn machine(&self) -> Option<String> {
            self.machine.clone()
        }
        fn post(&self, title: &str, body: &str) -> Result<(), String> {
            self.posted
                .lock()
                .unwrap()
                .push((title.to_string(), body.to_string()));
            self.post_result.clone()
        }
        fn health_fired(&self) -> Arc<Mutex<crate::notify::Fired>> {
            self.fired.clone()
        }
    }

    const LIST: &str = r#"[{"number":1347,"title":"Add spoon"}]"#;

    /// A list in the shape `notify::Pr` decodes, which `LIST` is
    /// deliberately NOT -- it has no `repo`, so the existing tests prove
    /// that a payload the notifier cannot read still stores fine.
    const NOTIFIABLE: &str =
        r#"[{"repo":"octocat/hello-world","number":1347,"title":"Add a spoon"}]"#;

    fn run(desktop: &Arc<FakeDesktop>) -> (Result<(), String>, Arc<FakeSink>) {
        let sink = Arc::new(FakeSink::default());
        let refresh = BackgroundRefresh::new(desktop.clone(), sink.clone());
        let result = tauri::async_runtime::block_on(refresh.refresh());
        (result, sink)
    }

    /// One window with notifications wired.
    fn run_notifying(
        desktop: &Arc<FakeDesktop>,
        notifier: &Arc<FakeNotifier>,
    ) -> (Result<(), String>, Arc<FakeSink>) {
        let sink = Arc::new(FakeSink::default());
        let refresh = BackgroundRefresh::notifying(
            desktop.clone(),
            sink.clone(),
            notifier.clone() as Arc<dyn Notifier>,
        );
        let result = tauri::async_runtime::block_on(refresh.refresh());
        (result, sink)
    }

    /// The whole background path: hello, the list, the store. Exactly
    /// two requests, in that order, and never the stream.
    #[test]
    fn a_window_is_hello_then_the_cached_list_then_the_store() {
        let desktop = FakeDesktop::new(Ok(()), Ok(LIST.into()));
        let (result, sink) = run(&desktop);
        assert_eq!(result, Ok(()));
        assert_eq!(desktop.requests(), vec![Request::Hello, Request::GetCached]);
        assert_eq!(*sink.saved.lock().unwrap(), vec![LIST.to_string()]);
    }

    #[test]
    fn an_unreachable_desktop_is_given_up_on_after_hello() {
        let desktop = FakeDesktop::new(Err("desktop unreachable".into()), Ok(LIST.into()));
        let (result, sink) = run(&desktop);
        assert_eq!(result, Err("desktop unreachable".into()));
        assert_eq!(
            desktop.requests(),
            vec![Request::Hello],
            "no list fetch, no retry"
        );
        assert!(sink.saved.lock().unwrap().is_empty());
    }

    #[test]
    fn a_failed_list_fetch_stores_nothing() {
        let desktop = FakeDesktop::new(Ok(()), Err("desktop answered HTTP 500".into()));
        let (result, sink) = run(&desktop);
        assert_eq!(result, Err("desktop answered HTTP 500".into()));
        assert_eq!(desktop.requests(), vec![Request::Hello, Request::GetCached]);
        assert!(sink.saved.lock().unwrap().is_empty());
    }

    /// The seam is the proof: `Desktop` has no method that could open
    /// the stream, so the background path cannot. Pinned here so a
    /// later "convenience" method is a deliberate change to this test.
    ///
    /// #789 added `health_alerts` -- a READ -- and this list was updated
    /// rather than relaxed. That is the intended way to widen the seam:
    /// the enumeration is what makes "cannot open the stream" structural,
    /// and it only keeps working if every addition has to be written here
    /// by someone who has read this comment.
    ///
    /// Scanned LINE BY LINE rather than by searching for `"\n}\n"`.
    /// `include_str!` preserves whatever line endings the checkout has,
    /// so on a Windows checkout with `core.autocrlf` a byte-pattern
    /// containing a bare `\n` finds nothing and the test panics on its
    /// own `unwrap`. `str::lines` splits on both. This crate has no
    /// Windows CI job, so nothing here would have caught it -- the
    /// identical pattern DID fail on the desktop's `platform
    /// (windows-latest)` job, and this is the same bug fixed in the same
    /// change rather than left for whoever adds that job.
    #[test]
    fn the_seam_offers_only_what_a_window_needs() {
        let src = include_str!("background.rs");
        let lines: Vec<&str> = src.lines().collect();
        let start = lines
            .iter()
            .position(|l| l.starts_with("pub trait Desktop"))
            .expect("the seam exists");
        let len = lines[start..]
            .iter()
            .position(|l| *l == "}")
            .expect("the seam closes");
        let body = &lines[start..start + len];
        let methods: Vec<&str> = body
            .iter()
            .filter_map(|l| l.trim().strip_prefix("fn "))
            .map(|l| l.split('(').next().unwrap())
            .collect();
        assert_eq!(methods, vec!["hello", "get_cached", "health_alerts"]);
        assert!(
            !body.iter().any(|l| l.contains("events")),
            "no stream on the seam"
        );
    }

    // ---- Notifications (#789) ---------------------------------------

    /// **The first-sync suppression test, at the window level.**
    ///
    /// A fresh install's first window fetches a list and must announce
    /// NOTHING, however many pull requests are in it -- and must still
    /// record them, so the SECOND window has something to compare
    /// against.
    #[test]
    fn a_first_window_stores_the_list_and_announces_nothing() {
        let desktop = FakeDesktop::new(Ok(()), Ok(NOTIFIABLE.into()));
        let notifier = FakeNotifier::new(crate::notify::Previous::First);
        let (result, sink) = run_notifying(&desktop, &notifier);
        assert_eq!(result, Ok(()));
        assert_eq!(*sink.saved.lock().unwrap(), vec![NOTIFIABLE.to_string()]);
        assert!(
            notifier.posted().is_empty(),
            "a first sync must not fire a burst: {:?}",
            notifier.posted()
        );
        assert_eq!(
            notifier.remembered.lock().unwrap().len(),
            1,
            "and it must record what it saw, or the next window is also a first sync"
        );
    }

    /// Two windows back to back: the first is silent, the second
    /// announces only what appeared between them.
    #[test]
    fn the_second_window_announces_only_what_appeared() {
        let notifier = FakeNotifier::new(crate::notify::Previous::First);
        let first = FakeDesktop::new(Ok(()), Ok(NOTIFIABLE.into()));
        let _ = run_notifying(&first, &notifier);
        assert!(notifier.posted().is_empty());

        let two = r#"[{"repo":"octocat/hello-world","number":1347,"title":"Add a spoon"},
                      {"repo":"octocat/hello-world","number":1348,"title":"Remove a fork"}]"#;
        let second = FakeDesktop::new(Ok(()), Ok(two.into()));
        let _ = run_notifying(&second, &notifier);
        assert_eq!(
            notifier.titles(),
            vec!["Remove a fork"],
            "only the one that appeared"
        );
    }

    /// **The ordering rule.** The snapshot is stored BEFORE anything is
    /// notified, and a failed post does not fail the window -- a denied
    /// permission must never cost the user the fresh list the window
    /// existed to fetch.
    #[test]
    fn a_failed_notification_does_not_fail_the_window() {
        let desktop = FakeDesktop::new(Ok(()), Ok(NOTIFIABLE.into()));
        let notifier = FakeNotifier::build(
            crate::notify::Previous::Known(vec![]),
            crate::notify::PhoneNotifyPrefs::default(),
            Some("Mac mini".into()),
            Err("notifications are not permitted".into()),
        );
        let (result, sink) = run_notifying(&desktop, &notifier);
        assert_eq!(result, Ok(()), "the window succeeded");
        assert_eq!(*sink.saved.lock().unwrap(), vec![NOTIFIABLE.to_string()]);
        assert_eq!(notifier.posted().len(), 1, "it was attempted");
        assert_eq!(
            notifier.remembered.lock().unwrap().len(),
            1,
            "and recorded anyway -- otherwise a denied user accumulates a \
             backlog that all arrives the day they allow notifications"
        );
    }

    /// A window with a failed SNAPSHOT notifies nothing: the save is the
    /// window's job and its failure ends the pass.
    #[test]
    fn nothing_is_notified_when_the_list_could_not_be_fetched() {
        let desktop = FakeDesktop::new(Ok(()), Err("HTTP 500".into()));
        let notifier = FakeNotifier::new(crate::notify::Previous::Known(vec![]));
        let (result, _) = run_notifying(&desktop, &notifier);
        assert_eq!(result, Err("HTTP 500".into()));
        assert!(notifier.posted().is_empty());
        assert_eq!(desktop.requests(), vec![Request::Hello, Request::GetCached]);
    }

    /// **The copy requirement at the window level.** A health
    /// notification on the phone says whose machine it is about.
    #[test]
    fn a_health_notification_names_the_desktop() {
        let health = r#"[{"key":"low","title":"Battery at 18%","body":"below 25%."}]"#;
        let desktop = FakeDesktop::with_health("[]", health);
        let notifier = FakeNotifier::new(crate::notify::Previous::Known(vec![]));
        let (result, _) = run_notifying(&desktop, &notifier);
        assert_eq!(result, Ok(()));
        assert_eq!(notifier.titles(), vec!["Mac mini: Battery at 18%"]);
    }

    /// A standing health condition is announced once across windows --
    /// the phone's `Fired` set lives in the notifier, so it survives
    /// from one window to the next.
    #[test]
    fn a_standing_health_condition_is_announced_once_across_windows() {
        let health = r#"[{"key":"low","title":"Battery at 18%","body":"b"}]"#;
        let notifier = FakeNotifier::new(crate::notify::Previous::Known(vec![]));
        for _ in 0..3 {
            let _ = run_notifying(&FakeDesktop::with_health("[]", health), &notifier);
        }
        assert_eq!(notifier.titles().len(), 1, "{:?}", notifier.titles());
    }

    /// Health is not even ASKED for when nothing would be posted. A
    /// background window is a few seconds of granted time, and a request
    /// whose answer is discarded spends some of it.
    #[test]
    fn health_is_not_requested_when_no_category_wants_it() {
        let desktop = FakeDesktop::with_health(
            "[]",
            r#"[{"key":"low","title":"Battery at 18%","body":"b"}]"#,
        );
        let notifier = FakeNotifier::with_prefs(
            crate::notify::Previous::Known(vec![]),
            crate::notify::PhoneNotifyPrefs {
                health_battery: false,
                health_cpu: false,
                ..Default::default()
            },
        );
        let _ = run_notifying(&desktop, &notifier);
        assert_eq!(
            desktop.requests(),
            vec![Request::Hello, Request::GetCached],
            "two requests, not three"
        );
        assert!(notifier.posted().is_empty());
    }

    /// The master switch silences both halves and saves the third
    /// request.
    #[test]
    fn the_master_switch_silences_the_whole_pass() {
        let desktop = FakeDesktop::with_health(
            NOTIFIABLE,
            r#"[{"key":"low","title":"Battery at 18%","body":"b"}]"#,
        );
        let notifier = FakeNotifier::with_prefs(
            crate::notify::Previous::Known(vec![]),
            crate::notify::PhoneNotifyPrefs {
                enabled: false,
                ..Default::default()
            },
        );
        let (result, sink) = run_notifying(&desktop, &notifier);
        assert_eq!(result, Ok(()));
        assert_eq!(
            *sink.saved.lock().unwrap(),
            vec![NOTIFIABLE.to_string()],
            "the snapshot is not a notification and is stored regardless"
        );
        assert!(notifier.posted().is_empty());
        assert_eq!(desktop.requests(), vec![Request::Hello, Request::GetCached]);
        assert_eq!(
            notifier.remembered.lock().unwrap().len(),
            1,
            "still recorded, so switching notifications ON announces what \
             appears NEXT rather than everything that appeared while they were off"
        );
    }

    /// Only the wanted category is posted. Turning CPU alerts off must
    /// not silence battery ones.
    #[test]
    fn an_unwanted_health_category_is_filtered_out() {
        let health = r#"[{"key":"low","title":"Battery at 18%","body":"b"},
                         {"key":"diffuse_cpu","title":"CPU is busy","body":"b"}]"#;
        let desktop = FakeDesktop::with_health("[]", health);
        let notifier = FakeNotifier::with_prefs(
            crate::notify::Previous::Known(vec![]),
            crate::notify::PhoneNotifyPrefs {
                health_cpu: false,
                ..Default::default()
            },
        );
        let _ = run_notifying(&desktop, &notifier);
        assert_eq!(notifier.titles(), vec!["Mac mini: Battery at 18%"]);
    }

    /// An unpaired phone has no machine to name, so it posts no health
    /// notification at all rather than an unqualified one that would
    /// read as a claim about the phone.
    #[test]
    fn an_unpaired_phone_posts_no_unnamed_health_alert() {
        let desktop = FakeDesktop::with_health(
            "[]",
            r#"[{"key":"low","title":"Battery at 18%","body":"b"}]"#,
        );
        let notifier = FakeNotifier::build(
            crate::notify::Previous::Known(vec![]),
            crate::notify::PhoneNotifyPrefs::default(),
            None,
            Ok(()),
        );
        let _ = run_notifying(&desktop, &notifier);
        assert!(notifier.posted().is_empty());
        assert_eq!(desktop.requests(), vec![Request::Hello, Request::GetCached]);
    }

    /// A desktop too old to answer `health_alerts` costs the health
    /// notification and nothing else: the window still succeeds and the
    /// new-PR half already ran.
    #[test]
    fn an_unreadable_health_report_does_not_fail_the_window() {
        let desktop = Arc::new(FakeDesktop {
            log: Mutex::new(vec![]),
            hello: Ok(()),
            cached: Ok(NOTIFIABLE.into()),
            health: Err("`health_alerts` is not a Headstate command".into()),
        });
        let notifier = FakeNotifier::new(crate::notify::Previous::Known(vec![]));
        let (result, sink) = run_notifying(&desktop, &notifier);
        assert_eq!(result, Ok(()));
        assert_eq!(*sink.saved.lock().unwrap(), vec![NOTIFIABLE.to_string()]);
        assert_eq!(
            notifier.titles(),
            vec!["Add a spoon"],
            "the pull request was still announced"
        );
    }

    /// **The regression test for the back-door burst.**
    ///
    /// A window whose list this build cannot read must LEAVE THE SEEN SET
    /// ALONE. Recording it as empty -- which an `Unreadable` collapsed
    /// into an empty `Vec` used to do -- wipes the record, so the next
    /// window that CAN read the payload sees every pull request as new
    /// and fires one notification each. That is the first-sync burst
    /// arriving by the back door, past the suppression built to stop it.
    ///
    /// Driven as two windows, because one cannot show it: the damage is
    /// done in the first and only visible in the second.
    #[test]
    fn an_unreadable_list_does_not_wipe_the_seen_set_and_cause_a_burst() {
        let notifier = FakeNotifier::new(crate::notify::Previous::First);

        // Window one: a readable list, recorded. Nothing announced, as a
        // first sync.
        let many = r#"[{"repo":"o/r","number":1,"title":"a"},
                       {"repo":"o/r","number":2,"title":"b"},
                       {"repo":"o/r","number":3,"title":"c"}]"#;
        let _ = run_notifying(&FakeDesktop::new(Ok(()), Ok(many.into())), &notifier);
        assert!(notifier.posted().is_empty());
        let recorded = notifier.remembered.lock().unwrap().len();
        assert_eq!(recorded, 1);

        // Window two: the desktop answers with a shape this build cannot
        // read. `LIST` has no `repo`, so every entry is skipped.
        let _ = run_notifying(&FakeDesktop::new(Ok(()), Ok(LIST.into())), &notifier);
        assert!(notifier.posted().is_empty(), "nothing readable to announce");
        assert_eq!(
            notifier.remembered.lock().unwrap().len(),
            recorded,
            "the seen set must NOT be rewritten from a list we could not read"
        );

        // Window three: readable again, and the SAME three pull requests.
        // None of them is new, so none is announced -- which is only true
        // because window two left the record alone.
        let _ = run_notifying(&FakeDesktop::new(Ok(()), Ok(many.into())), &notifier);
        assert!(
            notifier.posted().is_empty(),
            "a burst of three: the unreadable window wiped the seen set: {:?}",
            notifier.titles()
        );
    }

    /// And the other side of that distinction: a GENUINELY empty list is
    /// a real answer and IS recorded, so a pull request opened afterwards
    /// is news.
    #[test]
    fn a_genuinely_empty_list_is_recorded_and_the_next_arrival_is_news() {
        let notifier = FakeNotifier::new(crate::notify::Previous::First);
        let _ = run_notifying(&FakeDesktop::new(Ok(()), Ok("[]".into())), &notifier);
        assert_eq!(
            notifier.remembered.lock().unwrap().len(),
            1,
            "an empty list is a real list"
        );

        let _ = run_notifying(&FakeDesktop::new(Ok(()), Ok(NOTIFIABLE.into())), &notifier);
        assert_eq!(
            notifier.titles(),
            vec!["Add a spoon"],
            "the first pull request after an empty list is news"
        );
    }

    /// A window with NO notifier does exactly what it did before #789 --
    /// which is what keeps the tests above this line meaningful rather
    /// than merely passing.
    #[test]
    fn a_window_without_a_notifier_makes_two_requests() {
        let desktop = FakeDesktop::with_health(NOTIFIABLE, "[]");
        let (result, sink) = run(&desktop);
        assert_eq!(result, Ok(()));
        assert_eq!(desktop.requests(), vec![Request::Hello, Request::GetCached]);
        assert_eq!(*sink.saved.lock().unwrap(), vec![NOTIFIABLE.to_string()]);
    }

    // ---- The real client, against the loopback server ---------------

    /// Polls until `cond` holds, or fails the test.
    ///
    /// The budget is deliberately generous and overridable: these tests
    /// pass in milliseconds on a developer machine and have taken over
    /// five minutes on a loaded CI runner, where the suite itself ran
    /// for 316s. A timeout that fits a laptop turns a slow runner into a
    /// red build for no reason (#569, and again on mobile-v0.1.10).
    async fn until(mut cond: impl FnMut() -> bool) {
        let secs = std::env::var("HEADSTATE_TEST_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(120);
        tokio::time::timeout(Duration::from_secs(secs), async {
            while !cond() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("condition within {secs}s"));
    }

    /// A companion paired with the loopback server, its subscriber
    /// connected and holding the stream open, as on a phone.
    async fn paired() -> (TestServer, Arc<MemoryStore>, Arc<Recorder>, Arc<Companion>) {
        let store = Arc::new(MemoryStore::default());
        let rec = Arc::new(Recorder::default());
        let c = Arc::new(Companion::new(
            store.clone(),
            Arc::new(SoftwareKeys::new(store.clone())),
            rec.clone(),
            Arc::new(|f| {
                tokio::spawn(f);
            }),
        ));
        let server = TestServer::start().await;
        server.open_window(true);
        server.reply(
            "/v1/events",
            Reply::sse(&[("prs-updated", r#"[{"number":1}]"#)], true),
        );
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([9u8; 32]);
        let qr = server.qr(&token, Utc::now().timestamp() + 120);
        c.pair(&qr, None).await.unwrap();
        let fp = server.requests()[0].peer_fp.clone();
        server.pair(&fp);
        server.open_window(false);
        until(|| {
            let r = c.connection_state();
            r.state == State::Connected && r.last_poll.is_some()
        })
        .await;
        (server, store, rec, c)
    }

    /// The wired refresher, end to end: one window against the loopback
    /// server makes exactly `GET /v1/hello` and `POST /v1/call/get_cached`
    /// on the real client, stores what came back as the snapshot with a
    /// fresh poll time, and opens no second stream.
    #[tokio::test]
    async fn the_wired_refresher_goes_through_the_seam_and_never_opens_the_stream() {
        let (server, store, _rec, c) = paired().await;
        server.reply(
            "/v1/call/get_cached",
            Reply::json(200, serde_json::from_str(LIST).unwrap()),
        );
        let before = server.requests().len();
        let streams = |s: &TestServer| {
            s.requests()
                .iter()
                .filter(|r| r.path == "/v1/events")
                .count()
        };
        assert_eq!(streams(&server), 1, "the subscriber's stream, held open");
        let stale = c.connection_state().last_poll.unwrap();
        tokio::time::sleep(Duration::from_millis(1100)).await;

        let refresh = BackgroundRefresh::new(c.clone(), c.clone());
        refresh.refresh().await.unwrap();

        let made: Vec<(String, String)> = server.requests()[before..]
            .iter()
            .map(|r| (r.method.clone(), r.path.clone()))
            .collect();
        assert_eq!(
            made,
            vec![
                ("GET".to_string(), "/v1/hello".to_string()),
                ("POST".to_string(), "/v1/call/get_cached".to_string()),
            ]
        );
        assert_eq!(streams(&server), 1, "no stream opened by the window");
        let snap = events::cached_snapshot(store.as_ref()).unwrap().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(snap.prs.get()).unwrap(),
            serde_json::from_str::<serde_json::Value>(LIST).unwrap()
        );
        let fresh = c.connection_state().last_poll.unwrap();
        assert!(fresh > stale, "mark_poll: {stale} -> {fresh}");
        assert_eq!(c.connection_state().state, State::Connected);
    }

    /// A window while the desktop is away: hello fails, nothing else is
    /// tried, the snapshot and the connection state are left alone.
    #[tokio::test]
    async fn a_window_while_the_desktop_is_away_touches_nothing() {
        let (server, store, _rec, c) = paired().await;
        let snapshot_before = events::cached_snapshot(store.as_ref())
            .unwrap()
            .unwrap()
            .received_at;
        server.go_away();
        // The subscriber notices the dead stream on its own; the window
        // must not be what tells it.
        until(|| c.connection_state().state == State::Unreachable).await;

        let refresh = BackgroundRefresh::new(c.clone(), c.clone());
        let err = refresh.refresh().await.unwrap_err();
        assert!(err.contains("unreachable"), "{err}");
        assert_eq!(
            events::cached_snapshot(store.as_ref())
                .unwrap()
                .unwrap()
                .received_at,
            snapshot_before
        );
    }

    #[tokio::test]
    async fn an_unpaired_companion_has_nothing_to_refresh() {
        let store = Arc::new(MemoryStore::default());
        let c = Arc::new(Companion::new(
            store.clone(),
            Arc::new(SoftwareKeys::new(store)),
            Arc::new(Recorder::default()),
            Arc::new(|f| {
                tokio::spawn(f);
            }),
        ));
        let refresh = BackgroundRefresh::new(c.clone(), c);
        assert_eq!(
            refresh.refresh().await.unwrap_err(),
            "not paired with a desktop"
        );
    }
}

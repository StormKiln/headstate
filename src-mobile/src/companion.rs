//! The client commands' logic, behind the Tauri wrappers in `lib.rs`:
//! one [`Companion`] holds the store, the keys, the connection state,
//! and -- while paired -- the client and the running event subscriber.
//!
//! Separate from `lib.rs` so every command can be driven in a test
//! against the loopback server with no `AppHandle`: the sink and the
//! task spawner are injected.

use chrono::{DateTime, Utc};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::client::{Client, ClientError};
use crate::connection::{Connection, EventSink, Report, State};
use crate::events;
use crate::keys::{DeviceKeys, KeyError};
use crate::notify;
use crate::pairing::{self, Desktop};
use crate::stepup;
use crate::store::Store;
use crate::surface::{self, Class};

/// How a background task is started: `tauri::async_runtime::spawn` in
/// the app, `tokio::spawn` in tests.
pub type Spawner = Arc<dyn Fn(Pin<Box<dyn Future<Output = ()> + Send>>) + Send + Sync>;

struct Live {
    desktop: Desktop,
    client: Arc<Client>,
    events: events::Handle,
}

/// What `call` rejects with when the user dismisses the biometric
/// prompt.
///
/// A cancel is a decision, not a failure, and the UI should say nothing
/// at all -- so it needs to be recognisable rather than merely worded
/// differently from a real error. A Tauri command's error crosses the
/// IPC boundary as a `String`, which is why this is a marker and not a
/// variant; the frontend matches it exactly and swallows it.
///
/// The frontend's copy is `CANCELLED` in `src/lib/cancelled.ts`, and the
/// two are asserted equal by `src/lib/mirroredConstants.test.ts`, which
/// reads this literal via Vite's `?raw`. Until #850 the string existed in
/// THREE hand-written copies -- this one, `cancelled.ts`', and a third
/// inside `cancelled.test.ts` that compared copies two and three -- so
/// changing this one left every test on both sides passing while each
/// dismissed Face ID prompt showed the user a toast reading
/// `headstate:cancelled`: their own decision reported back as an error.
pub const CANCELLED: &str = "headstate:cancelled";

pub struct Companion {
    store: Arc<dyn Store>,
    keys: Arc<dyn DeviceKeys>,
    sink: Arc<dyn EventSink>,
    conn: Arc<Connection>,
    spawn: Spawner,
    live: Mutex<Option<Live>>,
}

/// The cached snapshot's timestamp, or `None`, saying so out loud when
/// the cache is unreadable.
///
/// This was `.ok().flatten()`, which threw away `StoreError::Corrupt` --
/// a variant `store.rs` went to deliberate trouble to produce rather
/// than a silent `None` that would read as "not paired". The effect is
/// bounded (the banner loses its timestamp), but on a phone there is no
/// console, so a corrupt cache was undiagnosable.
fn last_poll_from_cache(store: &dyn Store) -> Option<DateTime<Utc>> {
    match events::cached_snapshot(store) {
        Ok(snapshot) => snapshot.and_then(|s| s.received_at()),
        Err(e) => {
            log::warn!("companion: the cached snapshot could not be read: {e}");
            None
        }
    }
}

impl Companion {
    pub fn new(
        store: Arc<dyn Store>,
        keys: Arc<dyn DeviceKeys>,
        sink: Arc<dyn EventSink>,
        spawn: Spawner,
    ) -> Self {
        Self {
            conn: Arc::new(Connection::new(sink.clone())),
            store,
            keys,
            sink,
            spawn,
            live: Mutex::new(None),
        }
    }

    /// Restore a pairing from the store at startup and start the
    /// subscriber. A record without usable keys is reported and left
    /// in place rather than deleted: the name still belongs in the
    /// banner, and re-pairing replaces it.
    pub fn load(&self) -> Result<(), String> {
        let list = pairing::load_desktops(self.store.as_ref()).map_err(|e| e.to_string())?;
        let Some(desktop) = list.into_iter().next() else {
            return Ok(());
        };
        let last_poll = last_poll_from_cache(self.store.as_ref());
        self.conn.set_desktop(Some(desktop.name.clone()), last_poll);
        let identity = match self.keys.session_identity() {
            Ok(id) => id,
            // The keys are GONE: `generate` was never called, or
            // `destroy` was. Nothing here will fix itself, and the
            // pairing is over.
            Err(KeyError::NoKeys) => {
                log::warn!(
                    "companion: paired with {} but the device keys are gone",
                    desktop.name
                );
                self.conn.set_state(State::Revoked);
                return Ok(());
            }
            // The keys exist and could not be READ right now. On iOS
            // that is routine rather than exceptional: the session
            // items are `kSecAttrAccessibleWhenUnlockedThisDeviceOnly`,
            // so a cold start from a background launch -- a
            // `BGAppRefreshTask` wake, a notification -- reads them
            // while the device is still locked and gets
            // `errSecInteractionNotAllowed`.
            //
            // Reporting that as `Revoked` told the user their desktop
            // had removed this phone, on a healthy pairing, and the
            // banner's advice ("pair again") would have made them
            // discard a perfectly good one. `load` runs once from
            // `setup`, so nothing ever retried and it persisted until
            // the process was killed and cold-started unlocked.
            //
            // `Unreachable` is the honest state: we are paired, we
            // cannot talk to the desktop yet, and `retry_load` below
            // gets another go on the next foreground.
            Err(e) => {
                log::warn!(
                    "companion: paired with {} but the keys are not readable yet: {e}",
                    desktop.name
                );
                self.conn.set_state(State::Unreachable);
                return Ok(());
            }
        };
        let client = Client::new(&identity, &desktop.fp, desktop.addrs.clone(), desktop.port)
            .map_err(|e| e.to_string())?;
        self.attach(desktop, Arc::new(client));
        Ok(())
    }

    /// `pair_from_qr`: the flow in `pairing.rs`, then the subscriber.
    /// Returns the desktop's name.
    pub async fn pair(&self, payload: &str, device_name: Option<String>) -> Result<String, String> {
        self.detach();
        let name = device_name
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| pairing::default_device_name().to_string());
        let (desktop, client) = pairing::pair(
            self.store.as_ref(),
            self.keys.as_ref(),
            payload,
            &name,
            Utc::now(),
        )
        .await
        .map_err(|e| e.to_string())?;
        let name = desktop.name.clone();
        self.attach(desktop, client);
        Ok(name)
    }

    /// `unpair`: forget every desktop and the snapshot, destroy the
    /// keys, stop the subscriber.
    pub fn unpair(&self) -> Result<(), String> {
        self.detach();
        let mut problems = vec![];
        if let Err(e) = pairing::forget_all(self.store.as_ref()) {
            problems.push(e.to_string());
        }
        if let Err(e) = events::forget_snapshot(self.store.as_ref()) {
            problems.push(e.to_string());
        }
        if let Err(e) = self.keys.destroy() {
            problems.push(e.to_string());
        }
        self.conn.set_desktop(None, None);
        self.conn.set_state(State::Unpaired);
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    /// Persist the address that just worked, so the next COLD START
    /// begins with it.
    ///
    /// `Client` keeps `preferred` and `discovered` in memory only, and
    /// `load` rebuilds it from `Desktop::addrs` verbatim on every
    /// launch. So a phone that had found its desktop at a new address --
    /// after a DHCP renewal, over mDNS -- threw that away when the
    /// process died and paid the whole walk, plus a three-second
    /// browse, all over again on the next open.
    ///
    /// Writes only when the address has actually CHANGED. A store write
    /// behind every successful command would put disk I/O on the hot
    /// path for a value that changes about as often as the desktop's
    /// lease does.
    ///
    /// Best effort throughout: a failure here costs one slow start, and
    /// is not worth failing the user's command over.
    fn remember_address(&self, client: &Client) {
        let Some(best) = client.best_address() else {
            return;
        };
        let mut list = match pairing::load_desktops(self.store.as_ref()) {
            Ok(l) => l,
            Err(e) => {
                log::warn!("companion: could not read the paired desktops to update them: {e}");
                return;
            }
        };
        let Some(d) = list.first_mut() else {
            return;
        };
        if d.addrs.first().is_some_and(|a| *a == best) {
            return;
        }
        // Moved to the front rather than made the only one: the others
        // are still how this desktop is reached from another network,
        // and the QR is not shown again.
        d.addrs.retain(|a| *a != best);
        d.addrs.insert(0, best);
        if let Err(e) = pairing::save_desktops(self.store.as_ref(), &list) {
            log::warn!("companion: could not save the desktop's address order: {e}");
        }
    }

    /// `connection_state`.
    pub fn connection_state(&self) -> Report {
        let mut report = self.conn.report();
        // Whether this phone signs with a post-quantum key, answered
        // for the device the user is holding.
        //
        // The desktop has always shown this in its paired-devices list,
        // but from the phone it was unanswerable -- and the phone is
        // where someone is standing when they wonder what their own
        // hardware does. #670.
        //
        // `None` on a key error rather than `Some(false)`: a Keychain
        // that would not open is not evidence of a classical-only
        // device, and reporting it as one would be exactly the
        // absent-is-not-false mistake this codebase avoids elsewhere.
        report.has_mldsa = self.keys.public_keys().ok().map(|k| k.mldsa_65.is_some());
        report
    }

    /// `subscribe_events`: start the subscriber if it is not running,
    /// wake it if it is. The frontend calls this on first listen and on
    /// every return to the foreground.
    pub fn subscribe(&self) -> Result<(), String> {
        // A paired phone with no live client is the transient-key case
        // in `load`: the pairing is on disk, but the Keychain would not
        // hand over the session identity when the app started, most
        // likely because the device was still locked. `load` runs once
        // from `setup`, so without this it never ran again and the
        // phone stayed dead until the process was cold-started
        // unlocked. The foreground is exactly the moment the device is
        // known to be unlocked, so it is the right place to try again.
        if self
            .live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_none()
            && self.conn.state() != State::Unpaired
            && self.conn.state() != State::Revoked
        {
            if let Err(e) = self.load() {
                log::warn!("companion: retrying the paired desktop failed: {e}");
            }
        }
        let live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        match live.as_ref() {
            None => Err("not paired with a desktop".into()),
            Some(l) => {
                if l.events.is_stopped() && self.conn.state() != State::Revoked {
                    // Stopped by a handshake failure on a call; a resume
                    // is a request to find out whether that still holds.
                    self.start_events(l);
                }
                l.events.resume();
                Ok(())
            }
        }
    }

    /// `remote_call`.
    pub async fn call(&self, command: &str, args: Value) -> Result<Value, String> {
        let class = surface::admit(command).map_err(|e| e.to_string())?;
        let (client, events, desktop_name) = {
            let live = self.live.lock().unwrap_or_else(|e| e.into_inner());
            let l = live.as_ref().ok_or("not paired with a desktop")?;
            (l.client.clone(), l.events.clone(), l.desktop.name.clone())
        };
        // A desktop too old to speak this protocol is refused for EVERY
        // command, reads included (#734). Checked before the reachability
        // gate below because it is the one blocked condition retrying
        // cannot resolve: polling a desktop that speaks an older protocol
        // does not upgrade it, it just returns answers this app will
        // misread -- which surfaces as a broken page rather than as the
        // version mismatch it actually is.
        if let Some(why) = self.conn.version_blocked() {
            return Err(format!("{desktop_name} {why}"));
        }
        // Otherwise reads go through whatever the state -- the attempt is
        // how the phone finds out the desktop is back, and `get_cached`
        // has the snapshot to fall back on. Anything that changes
        // something is refused while the desktop is away or has revoked
        // this phone.
        if matches!(class, Class::Write | Class::Destructive) {
            if let Some(why) = self.conn.actions_blocked() {
                return Err(format!("{desktop_name} {why}"));
            }
        }
        let signature = if class == Class::Destructive {
            // On the blocking pool, not this worker. `sign_request`
            // reaches the hardware keys, and on a phone that is where
            // Face ID or the Android BiometricPrompt is SHOWN and
            // waited on -- a sheet that can sit for the full system
            // timeout while the user decides. Inline, that parked a
            // tokio worker for the duration, and the runtime here is
            // small enough that it could starve the event subscriber
            // and any concurrent command.
            let keys = self.keys.clone();
            let cmd = command.to_string();
            let args_for_sig = args.clone();
            let signed = tauri::async_runtime::spawn_blocking(move || {
                stepup::sign_request(keys.as_ref(), &cmd, &args_for_sig, Utc::now().timestamp())
            })
            .await
            .map_err(|e| format!("the confirmation could not run: {e}"))?;
            match signed {
                Ok(sig) => Some(sig),
                // The user declined. Reported with a STABLE marker
                // rather than an empty string or prose: `remote.ts`
                // matches on it to stay silent, and an empty error
                // would be indistinguishable from a bug that lost its
                // message. See `CANCELLED` for the contract.
                Err(e) if e.is_cancelled() => return Err(CANCELLED.to_string()),
                Err(e) => return Err(e.to_string()),
            }
        } else {
            None
        };
        match client.call(command, &args, signature.as_deref()).await {
            Ok(value) => {
                if self.conn.state() == State::Unreachable {
                    // Back, evidently; let the subscriber confirm and
                    // fill in the protocol version.
                    events.resume();
                }
                self.remember_address(&client);
                Ok(value)
            }
            Err(e) if e.is_handshake() => {
                log::warn!("companion: the desktop refused this phone on {command}: {e}");
                events.stop();
                self.conn.set_state(State::Revoked);
                Err(format!(
                    "{desktop_name} no longer recognises this phone; pair again"
                ))
            }
            Err(ClientError::Unreachable(m)) => {
                self.conn.set_state(State::Unreachable);
                events.resume();
                if command == "get_cached" {
                    // An `Err` here is a corrupt cache, not an absent
                    // one, and it decides whether the phone shows a list
                    // at all -- worth a line rather than a silent empty
                    // screen.
                    let cached = events::cached_snapshot(self.store.as_ref()).unwrap_or_else(|e| {
                        log::warn!("companion: the cached snapshot could not be read: {e}");
                        None
                    });
                    if let Some(snap) = cached {
                        log::info!(
                            "companion: {desktop_name} unreachable; serving the cached list"
                        );
                        return serde_json::from_str(snap.prs.get()).map_err(|e| e.to_string());
                    }
                }
                Err(format!("{desktop_name} is unreachable: {m}"))
            }
            // ONE command was slow; the connection is fine (#1466). The
            // state and the event stream are left exactly as they are:
            // marking the desktop unreachable flashed the banner, and
            // `events.resume()` dropped the stream -- and with it every
            // other call's in-flight frames (`worktree-size`, ...) -- over
            // evidence about this call alone. Other calls may be
            // succeeding at this moment. The desktop may also still be
            // working on it, which is worth saying: a retry is a second
            // copy of the same work, not a reconnect.
            Err(ClientError::TimedOut(m)) => {
                log::info!("companion: {command} timed out: {m}");
                Err(format!(
                    "{desktop_name} took too long to answer {command}; it may still be working on it"
                ))
            }
            Err(e) => Err(e.to_string()),
        }
    }

    fn attach(&self, desktop: Desktop, client: Arc<Client>) {
        self.conn.set_desktop(
            Some(desktop.name.clone()),
            last_poll_from_cache(self.store.as_ref()),
        );
        self.conn.set_state(State::Connecting);
        let live = Live {
            desktop,
            client,
            events: events::Handle::new(),
        };
        self.start_events(&live);
        *self.live.lock().unwrap_or_else(|e| e.into_inner()) = Some(live);
    }

    fn start_events(&self, live: &Live) {
        let sub = events::Subscriber {
            client: live.client.clone(),
            sink: self.sink.clone(),
            store: self.store.clone(),
            conn: self.conn.clone(),
            desktop_fp: live.desktop.fp.clone(),
        };
        (self.spawn)(Box::pin(events::run(sub, live.events.clone())));
    }

    fn detach(&self) {
        if let Some(l) = self.live.lock().unwrap_or_else(|e| e.into_inner()).take() {
            l.events.stop();
        }
    }

    /// The live client, for the background window (`background.rs`),
    /// which talks to the desktop directly rather than through
    /// [`Companion::call`]: that path moves the connection state and
    /// wakes the subscriber, and a window in the background does
    /// neither.
    pub(crate) fn client(&self) -> Result<Arc<Client>, String> {
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|l| l.client.clone())
            .ok_or_else(|| "not paired with a desktop".to_string())
    }

    /// Keep a list the background window fetched, exactly as the
    /// subscriber keeps a `prs-updated` frame: the snapshot, and the
    /// poll time the banner shows.
    pub(crate) fn record_snapshot(&self, prs_json: &str) -> Result<(), String> {
        let now = Utc::now();
        events::save_snapshot(self.store.as_ref(), prs_json, now).map_err(|e| e.to_string())?;
        self.conn.mark_poll(now);
        Ok(())
    }

    /// Which notifications this phone should send (#789).
    ///
    /// An unreadable preference falls back to the default -- everything
    /// on -- rather than to silence, matching `poll::read_notify_prefs`
    /// on the desktop. A database problem must not quietly turn off a
    /// feature the user is relying on, because the failure would be
    /// indistinguishable from "nothing happened".
    pub(crate) fn notify_prefs(&self) -> notify::PhoneNotifyPrefs {
        match crate::store::get_json(self.store.as_ref(), notify::PREFS_KEY) {
            Ok(Some(prefs)) => prefs,
            Ok(None) => notify::PhoneNotifyPrefs::default(),
            Err(e) => {
                log::warn!("notify: preferences unreadable, using the defaults: {e}");
                notify::PhoneNotifyPrefs::default()
            }
        }
    }

    pub(crate) fn set_notify_prefs(&self, prefs: &notify::PhoneNotifyPrefs) -> Result<(), String> {
        crate::store::put_json(self.store.as_ref(), notify::PREFS_KEY, prefs)
            .map_err(|e| e.to_string())
    }

    /// The pull requests the last notification pass compared against.
    ///
    /// An unreadable record is [`notify::Previous::First`], NOT an empty
    /// list. That is the whole of the first-sync suppression: an empty
    /// list would make every pull request on the machine look new and
    /// fire a burst, which is exactly the failure a corrupt store must
    /// not cause. The cost of getting it wrong in this direction is one
    /// missed round of notifications.
    pub(crate) fn notify_seen(&self) -> notify::Previous {
        match crate::store::get_json::<notify::Seen>(self.store.as_ref(), notify::SEEN_KEY) {
            Ok(Some(seen)) => seen.as_previous(),
            Ok(None) => notify::Previous::First,
            Err(e) => {
                log::warn!("notify: the seen set is unreadable; suppressing this pass: {e}");
                notify::Previous::First
            }
        }
    }

    pub(crate) fn record_notify_seen(&self, prs: &[notify::Pr]) -> Result<(), String> {
        crate::store::put_json(
            self.store.as_ref(),
            notify::SEEN_KEY,
            &notify::Seen::of(prs),
        )
        .map_err(|e| e.to_string())
    }

    /// The paired desktop's name, for the copy that must say whose
    /// machine a health alert is about.
    pub(crate) fn desktop_name(&self) -> Option<String> {
        let live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        live.as_ref().map(|l| l.desktop.name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::tests::Recorder;
    use crate::keys::{KeyError, PublicKeys, SessionIdentity, Signatures, SoftwareKeys};
    use crate::store::MemoryStore;
    use crate::testing::{Reply, TestServer};
    use base64::Engine;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    fn companion(store: Arc<MemoryStore>, rec: Arc<Recorder>) -> Companion {
        Companion::new(
            store.clone(),
            Arc::new(SoftwareKeys::new(store)),
            rec,
            Arc::new(|f| {
                tokio::spawn(f);
            }),
        )
    }

    /// The phone can say whether it holds a post-quantum key (#670).
    ///
    /// `SoftwareKeys` always generates ML-DSA, so a generated set
    /// reports true. The desktop has always shown this in its
    /// paired-devices list; this is the phone answering for itself,
    /// which is the device someone is holding when they wonder.
    #[tokio::test]
    async fn the_report_says_whether_this_phone_has_a_post_quantum_key() {
        let store = Arc::new(MemoryStore::default());
        let c = companion(store.clone(), Arc::new(Recorder::default()));
        SoftwareKeys::new(store).generate().unwrap();
        assert_eq!(c.connection_state().has_mldsa, Some(true));
    }

    /// A keychain that will not open is NOT a classical-only device.
    ///
    /// Reporting `Some(false)` there would tell the user their hardware
    /// lacks a capability it has -- the absent-is-not-false mistake
    /// this codebase avoids everywhere else. It reports `None`, and the
    /// UI renders nothing rather than a claim.
    ///
    /// A dedicated fixture rather than `LockedKeys`, which gates only
    /// `session_identity` (it models a cold start on a locked device,
    /// a different scenario) and would leave `public_keys` readable --
    /// so the test would pass on the wrong path.
    #[tokio::test]
    async fn an_unreadable_keychain_is_unknown_not_a_no() {
        struct Unreadable;
        impl DeviceKeys for Unreadable {
            fn generate(&self) -> Result<PublicKeys, KeyError> {
                Err(KeyError::Unavailable("locked".into()))
            }
            fn destroy(&self) -> Result<(), KeyError> {
                Ok(())
            }
            fn public_keys(&self) -> Result<PublicKeys, KeyError> {
                Err(KeyError::Unavailable("the device is locked".into()))
            }
            fn sign(&self, _bytes: &[u8]) -> Result<Signatures, KeyError> {
                Err(KeyError::Unavailable("locked".into()))
            }
            fn session_identity(&self) -> Result<SessionIdentity, KeyError> {
                Err(KeyError::Unavailable("locked".into()))
            }
        }

        let c = Companion::new(
            Arc::new(MemoryStore::default()),
            Arc::new(Unreadable),
            Arc::new(Recorder::default()),
            Arc::new(|f| {
                tokio::spawn(f);
            }),
        );
        assert_eq!(
            c.connection_state().has_mldsa,
            None,
            "a locked keychain must not be reported as a device without ML-DSA"
        );
    }

    /// Keys that refuse to be read until told otherwise, wrapping a
    /// real `SoftwareKeys` for everything else.
    ///
    /// Models the iOS Keychain on a locked device: the items exist, and
    /// `SecItemCopyMatching` answers `errSecInteractionNotAllowed`,
    /// which reaches Rust as `KeyError::Unavailable`. Nothing is
    /// missing and nothing is broken; it is simply not readable yet.
    struct LockedKeys {
        inner: SoftwareKeys,
        locked: AtomicBool,
    }

    impl LockedKeys {
        fn new(store: Arc<MemoryStore>) -> Self {
            Self {
                inner: SoftwareKeys::new(store),
                locked: AtomicBool::new(false),
            }
        }
        fn lock(&self) {
            self.locked.store(true, Ordering::SeqCst);
        }
        fn unlock(&self) {
            self.locked.store(false, Ordering::SeqCst);
        }
        fn blocked(&self) -> bool {
            self.locked.load(Ordering::SeqCst)
        }
    }

    impl DeviceKeys for LockedKeys {
        fn generate(&self) -> Result<PublicKeys, KeyError> {
            self.inner.generate()
        }
        fn destroy(&self) -> Result<(), KeyError> {
            self.inner.destroy()
        }
        fn public_keys(&self) -> Result<PublicKeys, KeyError> {
            self.inner.public_keys()
        }
        fn sign(&self, bytes: &[u8]) -> Result<Signatures, KeyError> {
            self.inner.sign(bytes)
        }
        fn session_identity(&self) -> Result<SessionIdentity, KeyError> {
            if self.blocked() {
                return Err(KeyError::Unavailable("the device is locked".into()));
            }
            self.inner.session_identity()
        }
    }

    /// A phone whose keys cannot be READ at startup is not a phone whose
    /// desktop revoked it.
    ///
    /// `load` reported every `session_identity` failure as `Revoked`,
    /// and on iOS the common cause is entirely benign: the session items
    /// are `WhenUnlockedThisDeviceOnly`, so a cold start from a
    /// background launch reads them while the device is still locked.
    /// The user was told their desktop had removed this phone, and the
    /// banner told them to pair again -- discarding a healthy pairing.
    #[tokio::test]
    async fn a_locked_keychain_is_not_a_revocation() {
        let store = Arc::new(MemoryStore::default());
        // One keys object across both companions: the store is shared,
        // so the session identity -- and therefore the fingerprint the
        // desktop paired with -- is the same one either way. Locking it
        // models the Keychain refusing to hand that identity over, not
        // the identity changing.
        let keys = Arc::new(LockedKeys::new(store.clone()));
        let spawn: Spawner = Arc::new(|f| {
            tokio::spawn(f);
        });
        let c = Companion::new(
            store.clone(),
            keys.clone(),
            Arc::new(Recorder::default()),
            spawn.clone(),
        );
        let server = TestServer::start().await;
        server.open_window(true);
        server.reply("/v1/events", Reply::sse(&[("prs-updated", "[]")], true));
        let qr = server.qr(&token(), Utc::now().timestamp() + 120);
        c.pair(&qr, None).await.unwrap();
        // The harness admits a fingerprint only once told to, the way
        // the desktop does after a person approves the request.
        let fp = server.requests()[0].peer_fp.clone();
        server.pair(&fp);
        server.open_window(false);
        // Stop the first companion's subscriber so only the reloaded
        // one is talking to the server.
        c.detach();

        // Restart with the device locked, as a background launch does.
        keys.lock();
        let again = Companion::new(
            store.clone(),
            keys.clone(),
            Arc::new(Recorder::default()),
            spawn.clone(),
        );
        again.load().unwrap();
        assert_ne!(
            again.connection_state().state,
            State::Revoked,
            "a locked keychain must not read as the desktop removing this phone"
        );
        assert_eq!(again.connection_state().state, State::Unreachable);
        // The desktop's name still belongs in the banner.
        assert!(again.connection_state().desktop.is_some());

        // Unlocked, the next foreground recovers on its own -- no
        // re-pairing, and no cold start.
        keys.unlock();
        again.subscribe().unwrap();
        until(|| again.connection_state().state == State::Connected).await;
    }

    /// Keys that are genuinely GONE still end the pairing: nothing will
    /// bring them back, and pretending otherwise leaves the phone
    /// retrying forever.
    #[tokio::test]
    async fn destroyed_keys_are_still_a_revocation() {
        let store = Arc::new(MemoryStore::default());
        let c = companion(store.clone(), Arc::new(Recorder::default()));
        let server = TestServer::start().await;
        server.open_window(true);
        server.reply("/v1/events", Reply::sse(&[("prs-updated", "[]")], true));
        let qr = server.qr(&token(), Utc::now().timestamp() + 120);
        c.pair(&qr, None).await.unwrap();

        SoftwareKeys::new(store.clone()).destroy().unwrap();
        let again = companion(store.clone(), Arc::new(Recorder::default()));
        again.load().unwrap();
        assert_eq!(again.connection_state().state, State::Revoked);
    }

    /// The address that worked survives a cold start.
    ///
    /// `Client` keeps `preferred` and `discovered` in memory, and
    /// `load` rebuilds it from `Desktop::addrs` verbatim -- so a phone
    /// that had walked past a dead address paid that walk again on
    /// every launch, and a desktop found at a NEW address over mDNS was
    /// forgotten entirely when the process died.
    #[tokio::test]
    async fn the_address_that_answered_is_remembered_across_a_restart() {
        let store = Arc::new(MemoryStore::default());
        let c = companion(store.clone(), Arc::new(Recorder::default()));
        let server = TestServer::start().await;
        server.open_window(true);
        server.reply("/v1/events", Reply::sse(&[("prs-updated", "[]")], true));
        server.reply("/v1/call/get_cached", Reply::json(200, json!([])));
        let qr = server.qr(&token(), Utc::now().timestamp() + 120);
        c.pair(&qr, None).await.unwrap();
        let fp = server.requests()[0].peer_fp.clone();
        server.pair(&fp);
        server.open_window(false);

        // Put a dead address in front, as a stale QR entry would be.
        let mut list = pairing::load_desktops(store.as_ref()).unwrap();
        let live = list[0].addrs[0].clone();
        list[0].addrs = vec!["192.0.2.1".into(), live.clone()];
        pairing::save_desktops(store.as_ref(), &list).unwrap();

        // A restart picks that order up, and one successful call is
        // enough to correct it on disk.
        let again = companion(store.clone(), Arc::new(Recorder::default()));
        again.load().unwrap();
        again.call("get_cached", json!({})).await.unwrap();

        let saved = pairing::load_desktops(store.as_ref()).unwrap();
        assert_eq!(
            saved[0].addrs.first().map(String::as_str),
            Some(live.as_str()),
            "the address that answered should be tried first next launch"
        );
        // The others are kept: they are still how this desktop is
        // reached from another network, and the QR is not shown again.
        assert!(saved[0].addrs.iter().any(|a| a == "192.0.2.1"));
    }

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

    fn token() -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([9u8; 32])
    }

    async fn paired() -> (TestServer, Arc<MemoryStore>, Arc<Recorder>, Companion) {
        let store = Arc::new(MemoryStore::default());
        let rec = Arc::new(Recorder::default());
        let c = companion(store.clone(), rec.clone());
        let server = TestServer::start().await;
        server.open_window(true);
        server.reply(
            "/v1/events",
            Reply::sse(&[("prs-updated", r#"[{"number":1347}]"#)], true),
        );
        let qr = server.qr(&token(), Utc::now().timestamp() + 120);
        assert_eq!(c.pair(&qr, None).await.unwrap(), "octocat's laptop");
        // The test server pairs whoever the window admitted.
        let fp = server.requests()[0].peer_fp.clone();
        server.pair(&fp);
        server.open_window(false);
        // Connected is set after `hello`; the snapshot frame that fills in
        // `last_poll` arrives a moment later on the stream. Wait for both, or
        // a busy runner reads the report between the two.
        until(|| {
            let r = c.connection_state();
            r.state == State::Connected && r.last_poll.is_some()
        })
        .await;
        (server, store, rec, c)
    }

    #[tokio::test]
    async fn unpaired_commands_say_so() {
        let c = companion(
            Arc::new(MemoryStore::default()),
            Arc::new(Recorder::default()),
        );
        assert_eq!(c.connection_state().state, State::Unpaired);
        assert_eq!(c.subscribe().unwrap_err(), "not paired with a desktop");
        assert_eq!(
            c.call("get_cached", json!({})).await.unwrap_err(),
            "not paired with a desktop"
        );
        assert_eq!(
            c.call("reveal_log", json!({})).await.unwrap_err(),
            "`reveal_log` is only available on the desktop"
        );
        assert_eq!(
            c.call("drop_database", json!({})).await.unwrap_err(),
            "`drop_database` is not a Headstate command"
        );
        assert!(c.unpair().is_ok(), "unpair is idempotent");
    }

    #[tokio::test]
    async fn pairing_connects_and_reports_the_desktop() {
        let (server, store, rec, c) = paired().await;
        let report = c.connection_state();
        assert_eq!(report.desktop.as_deref(), Some("octocat's laptop"));
        assert_eq!(report.protocol_version, Some(2));
        assert!(report.last_poll.is_some());
        assert_eq!(rec.last("prs-updated").unwrap(), r#"[{"number":1347}]"#);
        assert_eq!(
            server.requests()[0].header("content-type"),
            Some("application/json")
        );
        assert_eq!(pairing::load_desktops(store.as_ref()).unwrap().len(), 1);
        assert!(c.subscribe().is_ok());
    }

    #[tokio::test]
    async fn calls_are_forwarded_and_destructive_ones_carry_the_signature() {
        let (server, _, _, c) = paired().await;
        server.reply(
            "/v1/call/get_stats",
            Reply::json(200, json!({"merged_week": 3})),
        );
        server.reply("/v1/call/remove_orphan", Reply::json(200, json!(null)));
        assert_eq!(
            c.call("get_stats", json!({})).await.unwrap(),
            json!({"merged_week": 3})
        );
        assert_eq!(
            c.call("remove_orphan", json!({"path": "/srv/x"}))
                .await
                .unwrap(),
            json!(null)
        );
        let reqs = server.requests();
        let stats = reqs
            .iter()
            .find(|r| r.path == "/v1/call/get_stats")
            .unwrap();
        assert_eq!(stats.header("x-headstate-signature"), None);
        let rm = reqs
            .iter()
            .find(|r| r.path == "/v1/call/remove_orphan")
            .unwrap();
        let sig = rm.header("x-headstate-signature").unwrap();
        assert!(sig.starts_with("v1;ts="));
        assert!(sig.contains(";ecdsa=") && sig.contains(";mldsa="));
        assert_eq!(
            serde_json::from_str::<Value>(&rm.body).unwrap(),
            json!({"path": "/srv/x"})
        );
    }

    #[tokio::test]
    async fn a_desktop_error_is_passed_through_verbatim() {
        let (server, _, _, c) = paired().await;
        server.reply(
            "/v1/call/refresh_now",
            Reply::text(500, "gh auth status failed"),
        );
        assert_eq!(
            c.call("refresh_now", json!({})).await.unwrap_err(),
            "gh auth status failed"
        );
    }

    #[tokio::test]
    async fn while_unreachable_the_list_comes_from_the_cache_and_actions_are_refused() {
        let (server, _, _, c) = paired().await;
        server.go_away();
        until(|| c.connection_state().state == State::Unreachable).await;
        assert_eq!(
            c.call("get_cached", json!({})).await.unwrap(),
            json!([{"number": 1347}]),
            "the cached snapshot, marked by the connection state"
        );
        assert_eq!(c.connection_state().protocol_version, None);
        let err = c
            .call("act_on_pr", json!({"id": "PR_1"}))
            .await
            .unwrap_err();
        assert_eq!(
            err,
            "octocat's laptop is unreachable; actions are disabled until it is back"
        );
        let err = c
            .call("remove_orphan", json!({"path": "/x"}))
            .await
            .unwrap_err();
        assert!(err.contains("actions are disabled"));
        // Other reads are attempted and fail honestly.
        let err = c.call("get_stats", json!({})).await.unwrap_err();
        assert!(err.starts_with("octocat's laptop is unreachable:"), "{err}");
    }

    #[tokio::test]
    async fn a_handshake_refusal_on_a_call_is_revocation() {
        let (server, store, _, c) = paired().await;
        let fp = server.requests()[0].peer_fp.clone();
        server.revoke(&fp);
        server.end_streams();
        // Whichever notices first -- the subscriber's next hello or this
        // call -- the outcome is the same state.
        let err = loop {
            match c.call("get_stats", json!({})).await {
                Err(e) => break e,
                Ok(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        };
        until(|| c.connection_state().state == State::Revoked).await;
        assert!(
            err.contains("no longer recognises this phone")
                || err.contains("no longer paired with this phone"),
            "{err}"
        );
        assert_eq!(
            c.call("get_stats", json!({})).await.unwrap_err(),
            "octocat's laptop no longer recognises this phone; pair again"
        );
        // The record and the name stay for the banner until re-pairing.
        assert_eq!(
            c.connection_state().desktop.as_deref(),
            Some("octocat's laptop")
        );
        assert_eq!(pairing::load_desktops(store.as_ref()).unwrap().len(), 1);
    }

    /// One slow command is not an unreachable desktop (#1466).
    ///
    /// A call that runs out its timeout fails on its own, with a message
    /// that says the desktop was slow -- and leaves the connection state
    /// and the event stream exactly as they were. Before, it marked the
    /// desktop unreachable and restarted the stream, dropping every other
    /// call's in-flight frames.
    #[tokio::test]
    async fn a_slow_call_leaves_the_connection_and_the_stream_alone() {
        let (server, _, _, c) = paired().await;
        server.reply("/v1/call/size_worktrees", Reply::Stall);
        // The same desktop, with a call timeout a test can wait out. Only
        // `call` uses this client; the subscriber keeps its own.
        {
            let identity = c.keys.session_identity().unwrap();
            let mut live = c.live.lock().unwrap();
            let l = live.as_mut().unwrap();
            l.client = Arc::new(
                Client::with_call_timeout(
                    &identity,
                    &l.desktop.fp,
                    l.desktop.addrs.clone(),
                    l.desktop.port,
                    Duration::from_millis(500),
                )
                .unwrap(),
            );
        }
        // One call first, so the fresh client has its address and the
        // slow call below sends no `hello` of its own to be miscounted.
        server.reply("/v1/call/get_stats", Reply::json(200, json!({"ok": 1})));
        c.call("get_stats", json!({})).await.unwrap();
        let streams = |s: &TestServer| {
            s.requests()
                .iter()
                .filter(|r| r.path == "/v1/events" || r.path == "/v1/hello")
                .count()
        };
        let before = streams(&server);

        let err = c
            .call("size_worktrees", json!({"paths": ["/srv/r"]}))
            .await
            .unwrap_err();
        assert_eq!(
            err,
            "octocat's laptop took too long to answer size_worktrees; it may still be working on it"
        );
        assert_eq!(c.connection_state().state, State::Connected);

        // A restarted stream shows up as a fresh hello and events request
        // on the server. Give one time to arrive, then check none did.
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(c.connection_state().state, State::Connected);
        assert_eq!(
            streams(&server),
            before,
            "the event stream must not be restarted over one slow call"
        );
        // And the connection still serves other calls.
        assert_eq!(
            c.call("get_stats", json!({})).await.unwrap(),
            json!({"ok": 1})
        );
    }

    #[tokio::test]
    async fn unpair_forgets_everything() {
        let (_server, store, _, c) = paired().await;
        c.unpair().unwrap();
        let report = c.connection_state();
        assert_eq!(report.state, State::Unpaired);
        assert_eq!(report.desktop, None);
        assert!(pairing::load_desktops(store.as_ref()).unwrap().is_empty());
        assert!(events::cached_snapshot(store.as_ref()).unwrap().is_none());
        assert_eq!(
            SoftwareKeys::new(store.clone()).public_keys().unwrap_err(),
            KeyError::NoKeys
        );
        assert_eq!(c.subscribe().unwrap_err(), "not paired with a desktop");
    }

    /// #734: an older desktop is refused for EVERYTHING, not just
    /// writes.
    ///
    /// This replaces `an_older_desktop_is_read_only`, which asserted the
    /// opposite because that was the old behaviour. Letting reads through
    /// meant the phone rendered pages from a desktop whose answers it
    /// could misread, and the resulting breakage looked like a phone bug
    /// rather than a version mismatch.
    #[tokio::test]
    async fn an_older_desktop_is_refused_entirely() {
        let store = Arc::new(MemoryStore::default());
        let rec = Arc::new(Recorder::default());
        let c = companion(store, rec);
        let server = TestServer::start().await;
        server.open_window(true);
        server.reply(
            "/v1/hello",
            Reply::json(
                200,
                json!({"desktop_version": "4.0.0", "protocol_version": 0, "viewer_login": null}),
            ),
        );
        server.reply("/v1/events", Reply::sse(&[("prs-updated", "[]")], true));
        server.reply(
            "/v1/call/get_stats",
            Reply::json(200, json!({"merged_week": 1})),
        );
        let qr = server.qr(&token(), Utc::now().timestamp() + 120);
        c.pair(&qr, None).await.unwrap();
        let fp = server.requests()[0].peer_fp.clone();
        server.pair(&fp);
        server.open_window(false);
        until(|| c.connection_state().state == State::Connected).await;
        let report = c.connection_state();
        assert_eq!(report.protocol_version, Some(0));
        assert!(report.stale, "old desktop: shown as stale");

        // The read is refused, and names the desktop and the reason.
        let err = c.call("get_stats", json!({})).await.unwrap_err();
        assert!(
            err.starts_with("octocat's laptop runs an older Headstate"),
            "a read against an old desktop must be refused: {err}"
        );

        // And the write, as before.
        let err = c
            .call("act_on_pr", json!({"id": "PR_1"}))
            .await
            .unwrap_err();
        assert!(
            err.starts_with("octocat's laptop runs an older Headstate"),
            "{err}"
        );

        // The refusal happens on the PHONE: nothing was sent to a
        // desktop that cannot answer it correctly. Asserting on the
        // request log rather than only on the error keeps this honest --
        // an error raised after a round trip would pass the checks above
        // while still driving the old desktop.
        assert!(
            !server
                .requests()
                .iter()
                .any(|r| r.path.starts_with("/v1/call/")),
            "no command may reach an unsupported desktop: {:?}",
            server
                .requests()
                .iter()
                .map(|r| r.path.clone())
                .collect::<Vec<_>>()
        );
    }

    /// The other half: a CURRENT desktop is unaffected. Without this the
    /// strict gate could pass by refusing everything.
    #[tokio::test]
    async fn a_current_desktop_still_serves_reads() {
        let (server, _store, _rec, c) = paired().await;
        server.reply(
            "/v1/call/get_stats",
            Reply::json(200, json!({"merged_week": 3})),
        );
        assert_eq!(
            c.call("get_stats", json!({})).await.unwrap(),
            json!({"merged_week": 3}),
            "a desktop on the current protocol must still answer reads"
        );
    }

    #[tokio::test]
    async fn a_pairing_is_restored_from_the_store_at_startup() {
        let (server, store, _, c) = paired().await;
        c.detach();
        let rec = Arc::new(Recorder::default());
        let again = companion(store, rec);
        again.load().unwrap();
        assert_eq!(
            again.connection_state().desktop.as_deref(),
            Some("octocat's laptop")
        );
        assert!(
            again.connection_state().last_poll.is_some(),
            "from the snapshot"
        );
        // Wait for the RESUBSCRIBE itself, not merely for the state to
        // read Connected. Those are different moments: the restarted
        // client can report Connected while its `/v1/events` request is
        // still in flight, so asserting the count immediately after is a
        // race that only loses on a slow runner -- which is how
        // mobile-v0.1.10 went red with 85 of 86 tests passing.
        until(|| {
            server
                .requests()
                .iter()
                .filter(|r| r.path == "/v1/events")
                .count()
                >= 2
        })
        .await;
        assert_eq!(again.connection_state().state, State::Connected);
    }
}

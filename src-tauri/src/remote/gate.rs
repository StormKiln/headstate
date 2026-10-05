//! The feature gate: **Allow phone connections**.
//!
//! Off by default. The setting is read once at startup to decide whether
//! the listener comes up with the app, and flipped by the Settings
//! toggle, which starts or stops the listener on the spot. The desktop
//! identity is loaded from the keychain -- and created, the first time
//! -- when the listener first starts, never earlier, so a user who never
//! enables the feature never gets a keychain item.
//!
//! Managed as Tauri state so pairing can reach the running listener and
//! the identity's fingerprint for the QR code.
//!
//! This is also where the listener's seams meet the rest of the app:
//! its `PairedCerts` is the pairing state's copy of `paired_devices`,
//! its `CommandHost` is `surface::dispatch` on the live `AppHandle`,
//! and pairing's `IdentityInfo` is the identity the listener loaded
//! plus this machine's name.
//!
//! While the listener is up the desktop also advertises itself on the
//! LAN (`remote/discovery.rs`) under that same machine name. That is
//! best effort: it starts after the listener and its failure is a
//! warning, never a failed toggle.

use crate::commands::db_path;
use crate::remote::discovery::Advertisement;
use crate::remote::events::{Hub, SnapshotSource};
use crate::remote::identity::{self, FileStore, Identity, PlatformStore};
use crate::remote::listener::{
    self, CommandHost, Handle, ListenerConfig, PairedCerts, ViewerLookup, PORT,
};
use crate::remote::pairing::{self, DesktopIdentity, IdentityInfo, PairingState};
use crate::remote::stepup;
use crate::remote::surface::{self, RemoteError};
use crate::store::{self, settings};
use serde_json::Value;
use std::future::Future;
use std::net::{Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Manager, State};

/// The verifier's view of `paired_devices`: the pairing state's
/// in-memory copy, which every approve and revoke refreshes, and its
/// live token for the window. Nothing here touches SQLite, which is
/// what the trait demands of a handshake-path callee.
impl PairedCerts for PairingState {
    fn is_paired(&self, sha256_fp_hex: &str) -> bool {
        self.is_device_paired(sha256_fp_hex)
    }
    fn pairing_window_open(&self) -> bool {
        self.pairing_open()
    }
    fn authorized_device(&self, fp: &str) -> Option<super::context::AuthorizedDevice> {
        self.authorized_device(fp)
    }
    fn device(&self, sha256_fp_hex: &str) -> Option<crate::store::devices::PairedDevice> {
        self.paired_device(sha256_fp_hex)
    }
}

/// `/v1/call` on the live app: the allowlist dispatch and the
/// destructive-command notification, each on the real `AppHandle`.
struct AppHost(AppHandle);

impl CommandHost for AppHost {
    fn dispatch<'a>(
        &'a self,
        command: &'a str,
        args: Value,
        context: &'a super::context::DispatchContext,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RemoteError>> + Send + 'a>> {
        Box::pin(surface::dispatch(&self.0, command, args, context))
    }
    fn notify_destructive(&self, device_name: &str, command: &str) {
        stepup::notify_destructive(&self.0, device_name, command);
    }
}

/// Pairing's view of this desktop: the loaded identity's fingerprint
/// and the machine's name. Refuses until the listener has loaded the
/// identity -- `issue_pairing_token` starts it first, so in practice
/// this only refuses when that start failed.
struct DesktopIdentityInfo(AppHandle);

impl IdentityInfo for DesktopIdentityInfo {
    fn identity(&self) -> Result<pairing::Identity, String> {
        let fingerprint =
            self.0.state::<Remote>().fingerprint().ok_or_else(|| {
                "phone connections are off; turn them on to pair a phone".to_string()
            })?;
        Ok(pairing::Identity {
            fingerprint,
            display_name: desktop_name(),
        })
    }
}

/// What the pairing QR and the mDNS record call this desktop, e.g.
/// "octocat's laptop". Read once per process: it shells out on macOS.
fn desktop_name() -> String {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| display_name_from(computer_name(), host_name()))
        .clone()
}

/// The name in System Settings > General > About, which is the one a
/// user recognises as "my laptop"; the hostname is derived from it
/// and mangled (`Octocats-MacBook-Pro.local`).
#[cfg(target_os = "macos")]
fn computer_name() -> Option<String> {
    let out = std::process::Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(not(target_os = "macos"))]
fn computer_name() -> Option<String> {
    None
}

/// The kernel's hostname, from where each platform keeps it, with no
/// hostname crate: Windows sets `COMPUTERNAME` for every process, and
/// Linux exposes it as a file.
fn host_name() -> Option<String> {
    if cfg!(windows) {
        return std::env::var("COMPUTERNAME").ok();
    }
    ["/proc/sys/kernel/hostname", "/etc/hostname"]
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
}

/// The first usable name of the two, trimmed and without the mDNS
/// domain a hostname often carries; the product name when neither is.
fn display_name_from(computer: Option<String>, host: Option<String>) -> String {
    computer
        .into_iter()
        .chain(host)
        .map(|n| {
            n.trim()
                .trim_end_matches(".local")
                .trim_end_matches(".lan")
                .to_string()
        })
        .find(|n| !n.is_empty())
        .unwrap_or_else(|| "Headstate desktop".to_string())
}

/// The remote feature's live state.
pub struct Remote {
    listener: tokio::sync::Mutex<Option<Handle>>,
    /// The mDNS record, while the listener is up and the platform let
    /// us advertise. A std mutex: it is only ever touched under the
    /// listener lock and never held across an await.
    advertisement: Mutex<Option<Advertisement>>,
    paired: Arc<dyn PairedCerts>,
    /// Loaded once per process; the keychain may prompt on macOS, and
    /// asking on every enable would prompt on every enable.
    identity: OnceLock<Identity>,
    /// The event fan-out for `/v1/events`. Outlives any one listener so
    /// toggling the setting off and on does not re-tap the app's events.
    events: Arc<Hub>,
}

impl Remote {
    pub fn new(paired: Arc<dyn PairedCerts>, events: Arc<Hub>) -> Self {
        Self {
            listener: tokio::sync::Mutex::new(None),
            advertisement: Mutex::new(None),
            paired,
            identity: OnceLock::new(),
            events,
        }
    }

    #[cfg(test)]
    fn is_advertising(&self) -> bool {
        self.advertisement.lock().unwrap().is_some()
    }

    /// The desktop certificate's fingerprint, once the identity has been
    /// loaded -- which happens on the first start. `None` before that.
    pub fn fingerprint(&self) -> Option<String> {
        self.identity.get().map(Identity::fingerprint)
    }

    /// Whether the listener is up right now. The live answer, not the
    /// stored setting: the two differ when the port was taken at
    /// startup, and "phones can connect" is the question being asked.
    pub async fn is_running(&self) -> bool {
        self.listener.lock().await.is_some()
    }
}

/// The process-level rustls default. Called first thing in `run`.
///
/// Two providers are compiled in -- ring for octocrab and reqwest,
/// aws-lc-rs for the listener -- and with two, rustls refuses to guess:
/// any `ClientConfig::builder()` without an installed default panics.
/// `ring`, so the GitHub client keeps the exact provider it has used
/// since it was added; the listener never consults the default and
/// picks aws-lc-rs per config (`listener::provider`).
///
/// `Err` from `install_default` means one is already installed, which
/// is the end state wanted either way.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Manage the state and bring the listener up if the setting says so.
/// Called from Tauri's `setup`, which is synchronous and outside the
/// runtime, hence `block_on`.
pub fn setup(app: &AppHandle) {
    // Tapped whether or not the listener is on: the events are cheap
    // to forward to nobody, and attaching lazily would mean a poll
    // that fired between "enable" and "attached" reached no phone.
    let events = Arc::new(Hub::new(snapshot_source(app)));
    events.attach(app);
    // Managed in `lib.rs` before this runs. Its copy of the table is
    // empty until loaded; every later change goes through it.
    let pairing = app.state::<Arc<PairingState>>().inner().clone();
    match store::open_db(&db_path(app)).and_then(|c| pairing.reload_devices(&c)) {
        Ok(()) => {}
        // No phone can connect, which the user will notice as "it will
        // not connect"; the log says why.
        Err(e) => log::warn!("phone connections: could not load the paired devices: {e}"),
    }
    app.manage(Remote::new(pairing, events));
    app.manage(DesktopIdentity(Arc::new(DesktopIdentityInfo(app.clone()))));
    // Off the async workers, and before anything asks for it.
    let _ = desktop_name();
    if !read_enabled(app) {
        return;
    }
    match tauri::async_runtime::block_on(start(app)) {
        Ok(addr) => log::info!("phone connections: on ({addr})"),
        // The toggle will read as off, which is the truth; the setting
        // stays on so the next launch tries again.
        Err(e) => log::warn!("phone connections: enabled but could not start: {e}"),
    }
}

fn read_enabled(app: &AppHandle) -> bool {
    store::open_db(&db_path(app))
        .ok()
        .and_then(|c| settings::get::<bool>(&c, settings::keys::ALLOW_PHONE_CONNECTIONS).ok())
        .flatten()
        .unwrap_or(false)
}

fn persist_enabled(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let conn = store::open_db(&db_path(app)).map_err(|e| e.to_string())?;
    settings::set(&conn, settings::keys::ALLOW_PHONE_CONNECTIONS, &enabled)
        .map_err(|e| e.to_string())
}

/// `/v1/hello`'s login, through the GitHub client the app already
/// holds. `None` when not signed in.
fn viewer_lookup(app: &AppHandle) -> ViewerLookup {
    let client = app
        .try_state::<crate::commands::GhClient>()
        .and_then(|s| s.0.clone());
    Arc::new(move || {
        let client = client.clone();
        Box::pin(async move {
            match client {
                Some(c) => c.fetch_viewer().await.ok(),
                None => None,
            }
        })
    })
}

/// The opening `prs-updated` frame of `/v1/events`: the list the
/// webview's `get_cached` returns, through the same function, so the
/// phone's first paint is the desktop's first paint. rusqlite is
/// synchronous, so it runs off the async workers.
fn snapshot_source(app: &AppHandle) -> SnapshotSource {
    let app = app.clone();
    Arc::new(move || {
        let app = app.clone();
        Box::pin(async move {
            let loaded =
                tauri::async_runtime::spawn_blocking(move || crate::commands::get_cached(app))
                    .await;
            match loaded {
                Ok(Ok(prs)) => serde_json::to_string(&prs).ok(),
                Ok(Err(e)) => {
                    log::warn!("remote: could not load the snapshot for a phone: {e}");
                    None
                }
                Err(e) => {
                    log::warn!("remote: the snapshot task failed: {e}");
                    None
                }
            }
        })
    })
}

/// Where the Linux fallback file for the seed goes; see `PlatformStore`.
fn fallback_path(app: &AppHandle) -> std::path::PathBuf {
    db_path(app).with_file_name("remote-identity.json")
}

/// Where the certificate goes, on every platform: it is public, and too
/// large for a Windows credential. See `remote/identity.rs`.
fn certificate_path(app: &AppHandle) -> std::path::PathBuf {
    db_path(app).with_file_name("remote-identity.crt")
}

async fn load_identity(app: &AppHandle) -> Result<Identity, String> {
    let remote = app.state::<Remote>();
    if let Some(id) = remote.identity.get() {
        return Ok(id.clone());
    }
    let store = PlatformStore::new(fallback_path(app));
    let certificate = FileStore::new(certificate_path(app));
    // Keychain access is synchronous and may wait on a user prompt;
    // keep it off the async workers.
    let id = tauri::async_runtime::spawn_blocking(move || {
        identity::load_or_create(&store, &certificate)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    // A concurrent load may have won; either copy is the same identity.
    Ok(remote.identity.get_or_init(|| id).clone())
}

/// Start the listener if it is not already running. Returns where it is
/// listening. Public so the pairing task can make sure the listener is
/// up before showing a QR code.
pub async fn start(app: &AppHandle) -> Result<SocketAddr, String> {
    let remote = app.state::<Remote>();
    let mut guard = remote.listener.lock().await;
    if let Some(h) = guard.as_ref() {
        return Ok(h.local_addr());
    }
    let identity = load_identity(app).await?;
    // Managed in `lib.rs` before this module is set up.
    let pairing = app.state::<Arc<PairingState>>().inner().clone();
    let fingerprint = identity.fingerprint();
    let handle = listener::start(ListenerConfig {
        // The IPv6 wildcard, dual-stack: see `ListenerConfig::bind`.
        bind: SocketAddr::from((Ipv6Addr::UNSPECIFIED, PORT)),
        identity,
        paired: remote.paired.clone(),
        revocations: pairing.subscribe_revocations(),
        pairing,
        host: Arc::new(AppHost(app.clone())),
        desktop_version: app.package_info().version.to_string(),
        viewer_login: viewer_lookup(app),
        events: remote.events.clone(),
    })
    .await
    .map_err(|e| e.to_string())?;
    let addr = handle.local_addr();
    *guard = Some(handle);
    advertise(&remote, addr.port(), &fingerprint);
    Ok(addr)
}

/// Put the mDNS record up for a listener that just started, named as
/// the pairing QR names this desktop (`desktop_name`), so a Bonjour
/// browser and the phone agree on what it is. Best effort by design
/// (see `remote/discovery.rs`): a platform that cannot multicast logs a
/// warning and the phone falls back to the addresses it stored at
/// pairing.
fn advertise(remote: &Remote, port: u16, fingerprint: &str) {
    match Advertisement::start(&desktop_name(), port, fingerprint) {
        Ok(a) => *remote.advertisement.lock().unwrap() = Some(a),
        Err(e) => log::warn!("phone connections: on, but not advertised on the LAN: {e}"),
    }
}

/// Stop the listener, dropping every open connection. A no-op when it
/// is not running.
pub async fn stop(app: &AppHandle) {
    let remote = app.state::<Remote>();
    let mut guard = remote.listener.lock().await;
    let advertisement = remote.advertisement.lock().unwrap().take();
    if let Some(a) = advertisement {
        // Sends the goodbye and waits for it, briefly; off the workers.
        let _ = tokio::task::spawn_blocking(move || a.stop()).await;
    }
    if let Some(h) = guard.take() {
        h.stop().await;
    }
}

/// Whether phones can connect right now.
#[tauri::command]
pub async fn get_remote_enabled(remote: State<'_, Remote>) -> Result<bool, String> {
    Ok(remote.is_running().await)
}

/// Turn phone connections on or off, and remember the choice.
///
/// The change happens BEFORE it is persisted: a port that cannot be
/// bound or a keychain that refuses is reported and nothing is saved,
/// so the toggle never claims a state the app is not in.
#[tauri::command]
pub async fn set_remote_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    if enabled {
        let addr = start(&app).await?;
        log::info!("phone connections: on ({addr})");
    } else {
        stop(&app).await;
        log::info!("phone connections: off");
    }
    persist_enabled(&app, enabled)
}

/// Feature-only construction: same AppHost, snapshot and authorization as production.
#[cfg(feature = "enterprise-harness")]
pub(crate) async fn start_synthetic(
    app: &AppHandle,
    identity: super::identity::Identity,
    pairing: Arc<PairingState>,
) -> Result<(super::listener::Handle, Arc<Hub>), String> {
    let events = Arc::new(Hub::new(snapshot_source(app)));
    events.attach(app);
    let handle = super::listener::start(ListenerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        identity,
        paired: pairing.clone(),
        revocations: pairing.subscribe_revocations(),
        pairing,
        host: Arc::new(AppHost(app.clone())),
        desktop_version: "9.0.0".into(),
        viewer_login: viewer_lookup(app),
        events: events.clone(),
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok((handle, events))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pairing state IS the verifier's source: nothing paired and
    /// no window until a token is issued; the window closes when the
    /// token is spent.
    #[test]
    fn the_pairing_state_answers_the_verifier() {
        let pairing = PairingState::new(|_| {});
        let certs: &dyn PairedCerts = &pairing;
        assert!(!certs.is_paired("ab"));
        assert!(!certs.is_paired(""));
        assert!(certs.device("ab").is_none());
        assert!(!certs.pairing_window_open());
        pairing.issue_token();
        assert!(certs.pairing_window_open());
    }

    #[tokio::test]
    async fn remote_starts_with_no_listener_and_no_identity() {
        let remote = Remote::new(
            Arc::new(PairingState::new(|_| {})),
            Arc::new(Hub::new(Arc::new(|| Box::pin(async { None })))),
        );
        assert!(!remote.is_running().await);
        assert!(!remote.is_advertising());
        assert_eq!(remote.fingerprint(), None);
    }

    #[test]
    fn the_display_name_prefers_the_computer_name_and_strips_the_domain() {
        let name = |c: Option<&str>, h: Option<&str>| {
            display_name_from(c.map(String::from), h.map(String::from))
        };
        assert_eq!(
            name(Some("Octocat's laptop\n"), Some("octocats-laptop.local\n")),
            "Octocat's laptop"
        );
        assert_eq!(
            name(None, Some("octocats-laptop.local\n")),
            "octocats-laptop"
        );
        assert_eq!(name(None, Some("build-box.lan")), "build-box");
        assert_eq!(name(Some("  "), Some("build-box")), "build-box");
        assert_eq!(name(None, None), "Headstate desktop");
        assert_eq!(name(Some(""), Some("\n")), "Headstate desktop");
    }

    /// Whatever this machine is called, the name is usable and stable.
    #[test]
    fn the_desktop_name_is_non_empty_and_cached() {
        let a = desktop_name();
        assert!(!a.trim().is_empty());
        assert_eq!(a, desktop_name());
    }

    /// The setting is off by default: an absent key reads as `false`.
    #[test]
    fn allow_phone_connections_is_off_by_default() {
        let dir = tempfile::TempDir::new().unwrap();
        let conn = store::open_db(&dir.path().join("t.db")).unwrap();
        let v: Option<bool> =
            settings::get(&conn, settings::keys::ALLOW_PHONE_CONNECTIONS).unwrap();
        assert!(!v.unwrap_or(false));
    }

    /// Installing twice must not panic and must not error out loud:
    /// `registry.rs` installs the same provider lazily and either may
    /// run first.
    #[test]
    fn installing_the_crypto_provider_is_idempotent() {
        install_crypto_provider();
        install_crypto_provider();
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }
}

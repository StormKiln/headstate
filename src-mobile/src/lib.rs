//! Headstate Companion: the phone half of the mobile companion design
//! (docs/superpowers/specs/2026-09-05-mobile-companion-design.md).
//!
//! This crate is a thin client. It owns the TLS session and the device
//! keys and forwards every command to a paired desktop; it never holds a
//! GitHub token and never talks to GitHub.
//!
//! - `keys`: the session identity and step-up keys behind one trait,
//!   with the software implementation; the hardware one is #513.
//! - `client`: reqwest on rustls/aws-lc-rs, pinned to the desktop's
//!   certificate fingerprint, presenting the session certificate.
//! - `pairing`: the QR, the proof, `POST /v1/pair`, the stored record.
//! - `events`: the `/v1/events` subscriber re-emitting Tauri events.
//! - `companion`: the five client commands' logic; `store`,
//!   `connection`, `surface`, `stepup` are what they stand on.
//! - `discovery` (#511) finds the desktop on the LAN when its stored
//!   address has gone stale; `background` (#516) is the OS-granted
//!   refresh window.
//!
//! # The client commands (what `src/api/remote.ts` calls)
//!
//! | command | arguments | returns |
//! |---|---|---|
//! | `pair_from_qr` | `payload: string` (the QR's text), `deviceName?: string` | the desktop's name |
//! | `unpair` | | |
//! | `connection_state` | | `{state, desktop, last_poll, protocol_version}` |
//! | `remote_call` | `command: string`, `args?: object` | the command's own result |
//! | `subscribe_events` | | |
//!
//! `remote_call` refuses commands outside the desktop's allowlist before
//! anything is sent, signs destructive ones, and while the desktop is
//! unreachable serves `get_cached` from the cached snapshot and refuses
//! write and destructive commands. `subscribe_events` starts the event
//! stream, or wakes it: the frontend calls it once on first `listen`
//! and again each time the app returns to the foreground. Every change
//! of connection state is also emitted as the `connection-state` event
//! with the same object `connection_state` returns.

pub mod background;
mod client;
mod companion;
mod connection;
pub mod discovery;
mod events;
mod keys;
/// The constants this crate shares with the desktop, asserted against the
/// desktop's own source (#854). Test-only.
#[cfg(test)]
mod mirrored;
pub mod notify;
mod pairing;
mod stepup;
mod store;
mod surface;
#[cfg(test)]
mod testing;

use companion::Companion;
use connection::EventSink;
use serde_json::value::RawValue;
use serde_json::Value;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

/// Events reach the webview through the app handle. The payload is
/// already JSON (the desktop's bytes, or a report serialised here), so
/// it goes out as a `RawValue`: Tauri serialises that verbatim, and the
/// webview receives the same string the desktop's webview did.
impl EventSink for AppHandle {
    fn emit(&self, name: &str, json: &str) {
        match RawValue::from_string(json.to_string()) {
            Ok(raw) => {
                if let Err(e) = Emitter::emit(self, name, &raw) {
                    log::warn!("companion: could not emit {name}: {e}");
                }
            }
            Err(e) => log::warn!("companion: {name} payload is not JSON: {e}"),
        }
    }
}

#[tauri::command]
async fn pair_from_qr(
    state: State<'_, Arc<Companion>>,
    payload: String,
    device_name: Option<String>,
) -> Result<String, String> {
    state.pair(&payload, device_name).await
}

#[tauri::command]
fn unpair(state: State<'_, Arc<Companion>>) -> Result<(), String> {
    state.unpair()
}

#[tauri::command]
fn connection_state(state: State<'_, Arc<Companion>>) -> connection::Report {
    state.connection_state()
}

#[tauri::command]
async fn remote_call(
    state: State<'_, Arc<Companion>>,
    command: String,
    args: Option<Value>,
) -> Result<Value, String> {
    state
        .call(
            &command,
            args.unwrap_or_else(|| Value::Object(Default::default())),
        )
        .await
}

#[tauri::command]
fn subscribe_events(state: State<'_, Arc<Companion>>) -> Result<(), String> {
    state.subscribe()
}

/// The vault key for the settings store: 32 random bytes, made once
/// and kept beside the snapshot. See `store.rs` on what protects it.
fn vault_key(dir: &std::path::Path) -> Result<Vec<u8>, String> {
    let path = dir.join(store::VAULT_KEY_FILE);
    match std::fs::read(&path) {
        Ok(key) if key.len() == store::VAULT_KEY_LEN => Ok(key),
        Ok(_) => Err(format!(
            "{} is not a {}-byte key",
            path.display(),
            store::VAULT_KEY_LEN
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = keys::random_bytes::<{ store::VAULT_KEY_LEN }>()
                .map_err(|e| e.to_string())?
                .to_vec();
            std::fs::write(&path, &key).map_err(|e| format!("{}: {e}", path.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
            Ok(key)
        }
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Open the store, pick the keys, restore a pairing, and hand the
/// companion to Tauri as managed state.
///
/// A store that cannot be opened is a startup failure, not a silent
/// fall-back to memory: an app that forgot its pairing without saying
/// so would be worse than one that says why it cannot start, and the
/// log names the file to remove.
fn setup(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&dir)?;
    let key = vault_key(&dir)?;
    let store: Arc<dyn store::Store> = Arc::new(store::StrongholdStore::open(
        &dir.join(store::SNAPSHOT_FILE),
        key,
    )?);
    // The keys plugin (#513) first; software keys where it reports the
    // hardware cannot hold any (the simulator, a desktop host), with a
    // warning in the log. See `keys::FallbackKeys`.
    let keys: Arc<dyn keys::DeviceKeys> = Arc::new(keys::FallbackKeys::new(
        keys::HardwareKeys::new(app.handle().clone()),
        keys::SoftwareKeys::new(store.clone()),
    ));
    let companion = Companion::new(
        store,
        keys,
        Arc::new(app.handle().clone()),
        Arc::new(|f| {
            tauri::async_runtime::spawn(f);
        }),
    );
    companion.load()?;
    // In an `Arc` so the background window (`background::install`) can
    // hold the same companion the commands drive.
    app.manage(Arc::new(companion));
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        // Same logging shape as the desktop: a GUI app has no stderr, and
        // "it would not pair" is uninvestigable without a file to ask for.
        // Never log a private key, a pairing token, or a repository owner
        // -- see CONTRIBUTING and check-privacy.sh.
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .build(),
        )
        // Opportunistic background refresh (#516): the OS-granted window
        // on each platform, running the refresher `background::install`
        // puts in state. On a desktop host it registers an inert
        // scheduler.
        .plugin(tauri_plugin_headstate_refresh::init())
        // Local user notifications (#789), posted from inside the window
        // the plugin above is granted -- which is the whole delivery
        // path, and the reason delivery is best-effort rather than
        // instant. On a desktop host it registers an `Unavailable`
        // bridge whose every call SAYS SO in the log, so a platform
        // with no notification API is distinguishable from one with
        // nothing to say.
        .plugin(tauri_plugin_headstate_notify::init())
        // External links, exactly as the desktop registers them:
        // the shared frontend's `ExternalLink` calls `openUrl` for
        // every PR title and every "View on GitHub", and the phone
        // had no plugin to answer it.
        .plugin(tauri_plugin_opener::init());
    // The scanner is UI the frontend drives (`scan()` from
    // `@tauri-apps/plugin-barcode-scanner`, then `pair_from_qr` with the
    // text); the crate is empty on the desktop host.
    #[cfg(mobile)]
    let builder = builder.plugin(tauri_plugin_barcode_scanner::init());
    builder
        .setup(|app| {
            setup(app)?;
            // After `setup`: the refresher is built over the managed
            // companion.
            background::install(app.handle());
            Ok(())
        })
        // Hardware-backed step-up keys and the session identity (#513).
        // On a desktop host it registers a stub whose every call is
        // `Error::Unavailable`, so this `run` compiles and tests here.
        // The client (#514) reaches it through
        // `tauri_plugin_headstate_keys::HeadstateKeysExt::headstate_keys`.
        .plugin(tauri_plugin_headstate_keys::init())
        .invoke_handler(tauri::generate_handler![
            pair_from_qr,
            unpair,
            connection_state,
            remote_call,
            subscribe_events,
            // The phone's own notification preferences (#789). NOT the
            // desktop's `get_notify_prefs`, which is `Class::Local` and
            // correctly so -- see `notify::PhoneNotifyPrefs`.
            notify::get_phone_notify_prefs,
            notify::set_phone_notify_prefs,
            notify::get_session_mutes,
            notify::set_session_muted,
            notify::poll_session_toasts,
            notify::take_notification_session,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Headstate Companion");
}

#[cfg(test)]
mod tests {
    /// The resolved dependency graph, for ALL targets: Cargo.lock lists
    /// every crate any platform would build, so this is stricter than a
    /// per-target `cargo tree`.
    const LOCK: &str = include_str!("../Cargo.lock");

    fn locked_crates() -> Vec<&'static str> {
        let names: Vec<&str> = LOCK
            .lines()
            .filter_map(|l| l.strip_prefix("name = \""))
            .map(|l| l.trim_end_matches('"'))
            .collect();
        assert!(!names.is_empty(), "Cargo.lock parsed to no packages");
        names
    }

    /// Crates the desktop needs and the phone must never link. The spec
    /// makes this the reason the companion is a separate crate at all:
    /// the phone has no GitHub token, no SQLite ledger, and nothing to
    /// clean. A path dependency on `headstate_lib`, or a copy-pasted
    /// dependency block, would drag these in without any code change
    /// noticing -- so the lockfile itself is asserted.
    #[test]
    fn no_desktop_only_crates_in_lock() {
        let names = locked_crates();
        for forbidden in [
            "octocrab",
            "rusqlite",
            "libsqlite3-sys",
            "bollard",
            "git2",
            "libgit2-sys",
            "headstate",
        ] {
            assert!(
                !names.contains(&forbidden),
                "{forbidden} is in src-mobile/Cargo.lock; the companion must not link it"
            );
        }
    }

    /// The TLS stack must sit on the aws-lc-rs provider: it is the only
    /// one with X25519MLKEM768. The other half of that rule -- no `ring`
    /// beside it -- is NOT asserted here, because the lockfile is
    /// all-targets and `ring` is resolved for platforms this crate never
    /// builds (it is absent from `cargo tree` on the host, iOS, and
    /// Android). `deny.toml` bans it on the two phone targets instead.
    #[test]
    fn tls_is_on_aws_lc_rs() {
        assert!(
            locked_crates().contains(&"aws-lc-rs"),
            "aws-lc-rs missing from the lock"
        );
    }

    /// The background refresh plugin is registered and given a
    /// refresher; without `install` a window would find nothing to run.
    #[test]
    fn the_background_refresh_is_wired() {
        let src = include_str!("lib.rs");
        assert!(src.contains(".plugin(tauri_plugin_headstate_refresh::init())"));
        assert!(src.contains("background::install(app.handle())"));
    }

    /// The five commands the spec lists, registered under those names.
    #[test]
    fn the_client_commands_are_registered() {
        let src = include_str!("lib.rs");
        let start = src.find("generate_handler![").unwrap();
        let block = &src[start..start + src[start..].find(']').unwrap()];
        for name in [
            "pair_from_qr",
            "unpair",
            "connection_state",
            "remote_call",
            "subscribe_events",
        ] {
            assert!(block.contains(name), "{name} is not registered");
        }
    }

    #[test]
    fn the_vault_key_is_made_once_and_read_back() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("headstate-companion-key");
        std::fs::create_dir_all(&dir).unwrap();
        let a = super::vault_key(&dir).unwrap();
        let b = super::vault_key(&dir).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), super::store::VAULT_KEY_LEN);
        std::fs::write(dir.join(super::store::VAULT_KEY_FILE), b"short").unwrap();
        assert!(super::vault_key(&dir).is_err());
    }
}

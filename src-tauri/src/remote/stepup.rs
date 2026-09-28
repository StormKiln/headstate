//! Step-up signatures for destructive commands: the desktop side of
//! "Step-up for destructive commands" in the design spec.
//!
//! A phone that wants to delete something proves, per request, that it
//! still holds the biometric-gated signing keys it registered at pairing.
//! [`verify`] is the whole check and is transport-agnostic: the
//! `POST /v1/call/{command}` handler in `remote/listener.rs` calls it with
//! the paired device, the parsed body, and the raw header value, BEFORE
//! `surface::dispatch`, and only when `surface::class_of` says the
//! command is destructive. Read and write commands carry no signature.
//!
//! # What is here and what is in `headstate-stepup`
//!
//! The protocol itself -- the canonical bytes both signatures cover, the
//! `X-Headstate-Signature` grammar, and the verification of both
//! signatures -- lives in the `headstate-stepup` crate, which the phone
//! (`src-mobile`) and its keys plugin depend on by the same path. Read
//! that crate's docs for the header, the signed bytes, and the order of
//! checks; there is nothing to read twice, which is the point (#695).
//!
//! What is here is what only the desktop has: the [`NonceWindow`] that
//! remembers a verified nonce for as long as it could be replayed, the
//! adapter from a [`PairedDevice`] row to the two public keys the
//! verifier wants, and the native notification a destructive command
//! posts. None of that has a byte on the wire, so none of it belongs in
//! a crate the phone links.
//!
//! # Mounting
//!
//! ```ignore
//! // In the /v1/call/{command} handler, after the client certificate has
//! // been mapped to its PairedDevice row and before dispatch:
//! if class == Class::Destructive {
//!     let header = headers.get(HEADER).and_then(|v| v.to_str().ok());
//!     stepup::verify(&device, &command, &args, header, Utc::now().timestamp(), &nonces)
//!         .map_err(|e| (e.http_status(), e.to_string()))?;
//! }
//! let out = surface::dispatch(&app, &command, args, &device.name).await?;
//! if class == Class::Destructive {
//!     stepup::notify_destructive(&app, &device.name, &command);
//! }
//! ```
//!
//! `nonces` is one [`NonceWindow`] for the whole listener, managed in
//! Tauri state or held in the router; it is `Send + Sync` and takes
//! `&self`.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::AppHandle;

use crate::store::devices::PairedDevice;

// The protocol, re-exported so the listener and the rest of `remote`
// keep spelling these `stepup::HEADER`, `stepup::StepUpError` and so on.
// One implementation, two names for it.
pub use headstate_stepup::{
    canonical_bytes, SignatureHeader, StepUpError, HEADER, MAX_SKEW_SECS, NONCE_LEN,
};
use headstate_stepup::{NonceLog, PairedKeys};

/// Nonces seen from each device, remembered for as long as their
/// timestamp could still pass the skew check.
///
/// One per listener. Keyed by device fingerprint so two phones choosing
/// the same nonce do not block each other, and pruned by time on every
/// insert, so it holds at most one window's worth of destructive calls,
/// which is a handful.
#[derive(Debug, Default)]
pub struct NonceWindow {
    /// `(device fingerprint, nonce) -> the request's timestamp`.
    seen: Mutex<HashMap<(String, [u8; NONCE_LEN]), i64>>,
}

impl NonceWindow {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a nonce for a device. `false` if this device already used
    /// it and that use is still inside the window; `true` if it is new
    /// and is now remembered.
    ///
    /// Check and insert happen under one lock, so two concurrent replays
    /// cannot both be admitted.
    fn admit(&self, device_fp: &str, nonce: [u8; NONCE_LEN], timestamp: i64, now: i64) -> bool {
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        // Anything whose timestamp can no longer pass the skew check can
        // no longer be replayed, so forget it.
        seen.retain(|_, ts| (now - *ts).abs() <= MAX_SKEW_SECS);
        use std::collections::hash_map::Entry;
        match seen.entry((device_fp.to_string(), nonce)) {
            Entry::Occupied(_) => false,
            Entry::Vacant(slot) => {
                slot.insert(timestamp);
                true
            }
        }
    }
}

/// One device's view of the window, which is what the verifier's
/// [`NonceLog`] is: it knows nothing about fingerprints or pruning, only
/// whether THIS nonce may be spent. `now` is captured here because the
/// verifier passes the request's timestamp, not the desktop's clock, and
/// pruning needs the clock.
struct DeviceNonces<'a> {
    window: &'a NonceWindow,
    device_fp: &'a str,
    now: i64,
}

impl NonceLog for DeviceNonces<'_> {
    fn admit(&self, nonce: [u8; NONCE_LEN], timestamp: i64) -> bool {
        self.window
            .admit(self.device_fp, nonce, timestamp, self.now)
    }
}

/// Verify the step-up on a destructive request.
///
/// The check itself is `headstate_stepup::verify`; this is the desktop's
/// binding of it -- the pairing row's two public keys, and the replay
/// window scoped to that device. `header` is the raw
/// `X-Headstate-Signature` value, or `None` when the request had no such
/// header. `args` is the parsed JSON body exactly as it will be handed
/// to `surface::dispatch`. `now` is the desktop's clock in Unix seconds,
/// passed in rather than read so the window is testable. On `Ok(())` the
/// nonce has been recorded in `nonces`; on any `Err` nothing was
/// recorded.
pub fn verify(
    device: &PairedDevice,
    command: &str,
    args: &Value,
    header: Option<&str>,
    now: i64,
    nonces: &NonceWindow,
) -> Result<(), StepUpError> {
    headstate_stepup::verify(
        &PairedKeys {
            ecdsa: &device.ecdsa_pubkey,
            mldsa: device.mldsa_pubkey.as_deref(),
        },
        command,
        args,
        header,
        now,
        &DeviceNonces {
            window: nonces,
            device_fp: &device.cert_fp,
            now,
        },
    )
}

/// The notification text for a destructive command run for a phone:
/// `(title, body)`. Separate from [`notify_destructive`] so the wording
/// is testable without the plugin.
///
/// The title carries both facts a glance needs, which device and which
/// command, in the command's own name so it matches the log line
/// `dispatch` wrote. The body says what to do if it was not you.
pub fn destructive_notice(device_name: &str, command: &str) -> (String, String) {
    (
        format!("{device_name} ran {command}"),
        "A destructive command from a paired phone. If this was not you, revoke the device in Settings."
            .to_string(),
    )
}

/// Post the native notification for a destructive command that was just
/// executed on behalf of `device_name`. The second, independent signal
/// the spec asks for; call it after `dispatch` returns, whether or not
/// the command itself succeeded, because the attempt is the news.
///
/// Failure is logged and swallowed, as with every other notification in
/// the app: the command has already run, and the notification is an
/// affordance.
pub fn notify_destructive(app: &AppHandle, device_name: &str, command: &str) {
    use tauri_plugin_notification::NotificationExt;

    if !crate::poll::notification_allowed(app) {
        return;
    }
    let (title, body) = destructive_notice(device_name, command);
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        log::warn!("failed to show notification: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use headstate_stepup::build_header;
    use ml_dsa::{MlDsa65, Seed};
    use p256::ecdsa::signature::Signer;
    use serde_json::json;

    const NOW: i64 = 1_788_566_400;
    const NONCE_B64: &str = "AAECAwQFBgcICQoLDA0ODw";
    const CMD: &str = "remove_worktree";

    /// A phone's step-up keys, made deterministically so no test needs
    /// an RNG.
    ///
    /// Headers are built with `headstate_stepup::build_header`, the same
    /// function the real companion calls, so nothing here re-implements
    /// the grammar the parser opposite it reads. The protocol's own
    /// tests cover the grammar and the signature checks; what is left
    /// for this suite is what only the desktop has -- the row, the
    /// window, and the notification.
    struct Phone {
        ecdsa: p256::ecdsa::SigningKey,
        mldsa: Option<ml_dsa::SigningKey<MlDsa65>>,
        fp: String,
    }

    impl Phone {
        fn new(with_mldsa: bool, seed: u8) -> Self {
            let ecdsa = p256::ecdsa::SigningKey::from_bytes(&[seed; 32].into()).unwrap();
            let mldsa = with_mldsa
                .then(|| ml_dsa::SigningKey::<MlDsa65>::from_seed(&Seed::from([seed; 32])));
            Self {
                ecdsa,
                mldsa,
                fp: format!("{seed:064x}"),
            }
        }

        fn paired(&self) -> PairedDevice {
            PairedDevice {
                id: 1,
                name: "Octocat's phone".into(),
                cert_fp: self.fp.clone(),
                cert_der: vec![0x30],
                // `to_sec1_point` is `to_encoded_point` renamed in
                // elliptic-curve 0.14; `false` is still "uncompressed",
                // so this is the same 65 bytes starting 0x04.
                ecdsa_pubkey: self
                    .ecdsa
                    .verifying_key()
                    .to_sec1_point(false)
                    .as_bytes()
                    .to_vec(),
                mldsa_pubkey: self
                    .mldsa
                    .as_ref()
                    .map(|k| k.expanded_key().verifying_key().encode().to_vec()),
                paired_at: "2026-09-05T00:00:00Z".into(),
                last_seen: None,
                transcripts_allowed: true,
                reveal_allowed: false,
            }
        }

        fn ecdsa_sig(&self, msg: &[u8]) -> Vec<u8> {
            let sig: p256::ecdsa::Signature = self.ecdsa.sign(msg);
            sig.to_bytes().to_vec()
        }

        fn mldsa_sig(&self, msg: &[u8]) -> Vec<u8> {
            self.mldsa
                .as_ref()
                .expect("phone has ML-DSA")
                .expanded_key()
                .sign_deterministic(msg, b"")
                .unwrap()
                .encode()
                .to_vec()
        }

        /// A well-formed header for `command`/`args`, with every
        /// signature this phone can produce.
        fn header(&self, command: &str, args: &Value, nonce: &str, ts: i64) -> String {
            let msg = canonical_bytes(command, args, nonce, ts);
            build_header(
                ts,
                nonce,
                &self.ecdsa_sig(&msg),
                self.mldsa.as_ref().map(|_| self.mldsa_sig(&msg)).as_deref(),
            )
        }
    }

    fn args() -> Value {
        json!({
            "worktreePath": "/home/octocat/src/hello-world/.worktrees/feature",
            "repoPath": "/home/octocat/src/hello-world",
        })
    }

    /// The pairing row's two key columns reach the verifier, and a row
    /// whose ML-DSA column is `None` is an ECDSA-only pairing. This is
    /// the adapter this module exists for; the checks it feeds are the
    /// shared crate's.
    #[test]
    fn the_pairing_rows_keys_are_what_gets_verified() {
        let phone = Phone::new(true, 1);
        let h = phone.header(CMD, &args(), NONCE_B64, NOW);
        verify(
            &phone.paired(),
            CMD,
            &args(),
            Some(&h),
            NOW,
            &NonceWindow::new(),
        )
        .unwrap();

        // The same header against a row carrying another phone's keys.
        let mut wrong = Phone::new(true, 2).paired();
        wrong.cert_fp = phone.fp.clone();
        assert_eq!(
            verify(&wrong, CMD, &args(), Some(&h), NOW, &NonceWindow::new()).unwrap_err(),
            StepUpError::BadEcdsa
        );

        // A row with no ML-DSA key refuses a header that has one.
        let mut ecdsa_only = phone.paired();
        ecdsa_only.mldsa_pubkey = None;
        assert_eq!(
            verify(
                &ecdsa_only,
                CMD,
                &args(),
                Some(&h),
                NOW,
                &NonceWindow::new()
            )
            .unwrap_err(),
            StepUpError::UnexpectedMldsa
        );
    }

    /// A damaged row is the desktop's fault, not the phone's, and the
    /// status says so. The mapping is the shared crate's; that it is
    /// reached from a `PairedDevice` is this module's.
    #[test]
    fn unreadable_stored_keys_are_the_desktops_fault() {
        let phone = Phone::new(true, 1);
        let h = phone.header(CMD, &args(), NONCE_B64, NOW);
        let mut device = phone.paired();
        device.ecdsa_pubkey = vec![0x04; 65];
        let err = verify(&device, CMD, &args(), Some(&h), NOW, &NonceWindow::new()).unwrap_err();
        assert_eq!(err, StepUpError::BadStoredKey("ECDSA P-256"));
        assert_eq!(err.http_status(), 500);

        let mut device = phone.paired();
        device.mldsa_pubkey = Some(vec![0x11; 100]);
        let err = verify(&device, CMD, &args(), Some(&h), NOW, &NonceWindow::new()).unwrap_err();
        assert_eq!(err, StepUpError::BadStoredKey("ML-DSA-65"));
    }

    #[test]
    fn reused_nonce_refused_and_a_failed_request_burns_none() {
        let phone = Phone::new(true, 1);
        let device = phone.paired();
        let nonces = NonceWindow::new();
        let h = phone.header(CMD, &args(), NONCE_B64, NOW);

        // A request that fails verification leaves the nonce unspent.
        let tampered = json!({"repoPath": "/home/octocat/src/hello-world", "worktreePath": "/"});
        assert_eq!(
            verify(&device, CMD, &tampered, Some(&h), NOW, &nonces).unwrap_err(),
            StepUpError::BadEcdsa
        );

        verify(&device, CMD, &args(), Some(&h), NOW, &nonces).unwrap();
        let err = verify(&device, CMD, &args(), Some(&h), NOW + 30, &nonces).unwrap_err();
        assert_eq!(err, StepUpError::NonceReused);
        assert_eq!(err.http_status(), 403);

        // Another device is free to use the same nonce: the window is
        // keyed by fingerprint, which is the whole reason it is here and
        // not in the shared crate.
        let other = Phone::new(true, 2);
        let h2 = other.header(CMD, &args(), NONCE_B64, NOW);
        verify(&other.paired(), CMD, &args(), Some(&h2), NOW, &nonces).unwrap();

        // Once the timestamp itself is outside the window a replay is
        // refused for staleness, which is the same refusal with a more
        // useful message, and the next admitted request prunes the entry.
        let err = verify(&device, CMD, &args(), Some(&h), NOW + 61, &nonces).unwrap_err();
        assert_eq!(err, StepUpError::StaleTimestamp { skew: -61 });
        assert_eq!(
            nonces.seen.lock().unwrap().len(),
            2,
            "pruning waits for an admit"
        );
        let later = phone.header(CMD, &args(), "AAECAwQFBgcICQoLDA0OHw", NOW + 61);
        verify(&device, CMD, &args(), Some(&later), NOW + 61, &nonces).unwrap();
        assert_eq!(
            nonces.seen.lock().unwrap().len(),
            1,
            "pruned to the live entry"
        );
    }

    /// The window prunes and admits by the DESKTOP's clock while
    /// remembering the request's own timestamp, which only shows up
    /// through the desktop's binding: the verifier hands the log a
    /// timestamp and nothing else.
    #[test]
    fn the_window_prunes_by_the_desktop_clock() {
        let phone = Phone::new(false, 1);
        let device = phone.paired();
        let nonces = NonceWindow::new();
        // Signed sixty seconds in the past: inside the window, and what
        // is remembered is `ts`, not `now`.
        let h = phone.header(CMD, &args(), NONCE_B64, NOW - 60);
        verify(&device, CMD, &args(), Some(&h), NOW, &nonces).unwrap();
        assert_eq!(
            *nonces.seen.lock().unwrap().values().next().unwrap(),
            NOW - 60
        );
    }

    #[test]
    fn nonce_window_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NonceWindow>();
    }

    /// The header constant and the canonical bytes reach the listener
    /// through this module. Cheap, and it fails if a re-export is
    /// dropped in a refactor.
    #[test]
    fn the_protocol_is_re_exported_unchanged() {
        assert_eq!(HEADER, "X-Headstate-Signature");
        assert_eq!(MAX_SKEW_SECS, 60);
        assert_eq!(NONCE_LEN, 16);
        assert_eq!(canonical_bytes(CMD, &args(), NONCE_B64, NOW).len(), 203);
        let phone = Phone::new(false, 1);
        let h = phone.header(CMD, &args(), NONCE_B64, NOW);
        assert_eq!(SignatureHeader::parse(&h).unwrap().nonce, NONCE_B64);
        assert!(URL_SAFE_NO_PAD.decode(NONCE_B64).is_ok());
    }

    #[test]
    fn destructive_notice_names_the_device_and_the_command() {
        let (title, body) = destructive_notice("Octocat's phone", "remove_worktree");
        assert_eq!(title, "Octocat's phone ran remove_worktree");
        assert_eq!(
            body,
            "A destructive command from a paired phone. If this was not you, revoke the device in Settings."
        );
    }
}

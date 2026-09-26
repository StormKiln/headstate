//! Pairing a phone with this desktop: the QR payload, the single-use
//! token, the `/v1/pair` handshake, the approve/deny decision, and
//! revocation. "Pairing" in the design spec.
//!
//! Transport-agnostic on purpose. Nothing here knows about axum or TLS:
//! [`handle_pair`] takes the decoded request plus the two certificate
//! fingerprints the listener already has, and returns an outcome the
//! listener turns into an HTTP status. That is what lets every rule in
//! the protocol be tested on an in-memory database without a socket.
//!
//! # Conventions the phone and the listener must share
//!
//! - A **fingerprint** is the SHA256 of a certificate's DER, as 64
//!   lowercase hex characters with no prefix. That is the string stored
//!   in `paired_devices.cert_fp`, the string passed to [`handle_pair`]
//!   for both peers, and the string in the `pairing-request` event. The
//!   QR payload's `fp` field carries it as `sha256:<hex>`, exactly as the
//!   spec shows it.
//! - The **token** is 32 random bytes; it travels as base64url without
//!   padding in both the QR payload and the pair request.
//! - The **proof** is `HMAC-SHA256(key = the 32 raw token bytes,
//!   message = client_fp || server_fp)` over the two hex strings as
//!   ASCII, with no separator, sent as base64url without padding.
//!   [`proof`] computes it, so the mobile crate's tests and the desktop
//!   agree by construction.
//! - The **signing keys** are standard base64 with padding: `ecdsa_p256`
//!   is the 65-byte SEC1 uncompressed point, `mldsa_65` the 1952-byte
//!   ML-DSA-65 public key.
//!
//! # What the listener (`remote/listener.rs`) needs from here
//!
//! - [`PairingState::pairing_open`]: the client-certificate verifier
//!   admits an UNPAIRED certificate only while this is true, and only
//!   for the `/v1/pair` path. Every other path refuses at the handshake.
//! - [`PairingState::subscribe_revocations`]: a broadcast of
//!   fingerprints whose rows were just deleted. The listener drops every
//!   open connection presenting that certificate.
//! - [`PairingState::is_device_paired`] and [`PairingState::paired_device`]:
//!   the verifier's per-handshake and per-request lookups, answered from
//!   an in-memory copy of `paired_devices` that [`PairingState::respond`]
//!   and [`PairingState::revoke`] refresh, so neither touches SQLite.
//!   `remote/gate.rs` implements the listener's `PairedCerts` over these
//!   and loads the copy once at startup.
//! - The desktop's own fingerprint and display name come from
//!   [`IdentityInfo`], which `remote/gate.rs` implements over the
//!   identity the listener loaded.
//!
//! `POST /v1/pair` is mounted in `remote/listener.rs`: it decodes the
//! body, builds the [`PeerCert`] from the handshake's client certificate,
//! and maps [`handle_pair`]'s result with [`PairError::http_status`].
//! `handle_pair` is `Send` and holds no lock across an await, so it runs
//! directly inside the tokio handler. It blocks for as long as the user
//! takes to answer the modal, up to [`DECISION_TIMEOUT`]; the listener
//! puts no shorter request timeout in front of it.

use crate::store::devices::{self, NewDevice, PairedDevice};
use crate::store::{open_db, StoreError};
use base64::engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD as BASE64URL};
use base64::Engine;
// `KeyInit` alongside `Mac` since hmac 0.13: `new_from_slice` moved onto
// that trait, while `update`/`verify_slice` stayed on `Mac`.
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::{broadcast, oneshot};

/// The listener's fixed port, from the dynamic range so it never collides
/// with a well-known service.
pub const PORT: u16 = 41919;

/// The QR payload's `v`: the wire protocol version, as the spec says
/// ("the same integer is the QR's `v`"). 2 since the certificates moved
/// to ML-DSA-65 (#521); a phone accepts a 2 and nothing else.
pub const QR_VERSION: u8 = 2;

/// How long a pairing token stays valid after it is issued.
pub const TOKEN_TTL: Duration = Duration::from_secs(120);

/// How long the phone waits for the user to answer the modal before the
/// desktop gives up and returns 403.
pub const DECISION_TIMEOUT: Duration = Duration::from_secs(120);

/// The Tauri event that asks the UI to show the confirmation modal.
/// Payload: [`PairingRequestEvent`].
pub const PAIRING_REQUEST_EVENT: &str = "pairing-request";

const TOKEN_LEN: usize = 32;
const ECDSA_P256_LEN: usize = 65;
const MLDSA_65_LEN: usize = 1952;

// ---------------------------------------------------------------------
// Identity seam (implemented by remote/gate.rs)
// ---------------------------------------------------------------------

/// What pairing needs to know about this desktop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// SHA256 of the desktop certificate's DER, lowercase hex, no prefix.
    pub fingerprint: String,
    /// What the phone shows for this desktop, e.g. "octocat's laptop".
    pub display_name: String,
}

/// The desktop's certificate identity. `remote/gate.rs` implements this
/// over the key pair `remote/identity.rs` owns; pairing only ever reads
/// the two strings.
pub trait IdentityInfo: Send + Sync {
    /// Fails when the desktop has no certificate yet, with a message the
    /// UI can show. Never invents a fingerprint: a QR code carrying a
    /// made-up `fp` would let a phone pin the wrong thing, and nothing
    /// downstream could tell.
    fn identity(&self) -> Result<Identity, String>;
}

/// Managed Tauri state holding whichever [`IdentityInfo`] is wired in.
pub struct DesktopIdentity(pub Arc<dyn IdentityInfo>);

// ---------------------------------------------------------------------
// Wire shapes
// ---------------------------------------------------------------------

/// What the QR code encodes. Field names are the wire format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QrPayload {
    pub v: u8,
    pub name: String,
    pub addrs: Vec<String>,
    pub port: u16,
    /// `sha256:<hex>`.
    pub fp: String,
    /// base64url, no padding.
    pub token: String,
    /// Unix seconds.
    pub exp: i64,
}

/// Body of `POST /v1/pair`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairRequest {
    pub token: String,
    pub device_name: String,
    pub signing_keys: SigningKeys,
    pub proof: String,
}

/// The phone's step-up public keys, standard base64.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SigningKeys {
    pub ecdsa_p256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mldsa_65: Option<String>,
}

/// The certificate the client presented at the TLS handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerCert {
    /// Lowercase hex SHA256 of `der`.
    pub fingerprint: String,
    pub der: Vec<u8>,
}

/// Payload of the [`PAIRING_REQUEST_EVENT`] event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairingRequestEvent {
    /// Hand this back to `respond_to_pairing`.
    pub request_id: u64,
    pub device_name: String,
    /// Lowercase hex, no prefix. The UI groups it into blocks of four
    /// for display; the phone shows the same string.
    pub fingerprint: String,
    /// Whether the phone offered a post-quantum step-up key.
    pub has_mldsa: bool,
}

/// The 200 body of `/v1/pair`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairOutcome {
    pub device_id: i64,
    pub device_name: String,
}

/// Why `/v1/pair` refused. [`PairError::http_status`] is the mapping the
/// listener uses; the message is safe to send to the phone.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PairError {
    /// The body could not be used. Does NOT consume the token, so a
    /// phone with a bug can retry against the same QR.
    #[error("bad pair request: {0}")]
    BadRequest(String),
    /// Unknown, expired, or already-used token.
    #[error("pairing token is invalid or expired")]
    InvalidToken,
    /// The token was right but the HMAC over the fingerprints was not.
    #[error("pairing proof did not verify")]
    BadProof,
    /// The user clicked Deny.
    #[error("pairing was denied")]
    Denied,
    /// The user did not answer within [`DECISION_TIMEOUT`].
    #[error("pairing was not approved in time")]
    Timeout,
}

impl PairError {
    /// 400 for a malformed body; 403 for everything else, as the spec
    /// says for deny and timeout, and for token and proof failures
    /// because telling an attacker which of the two failed helps only
    /// the attacker.
    pub fn http_status(&self) -> u16 {
        match self {
            PairError::BadRequest(_) => 400,
            _ => 403,
        }
    }
}

/// Compute the pairing proof exactly as [`handle_pair`] verifies it.
/// Exposed so the phone side and its tests share one definition.
pub fn proof(token: &[u8], client_fp: &str, server_fp: &str) -> String {
    BASE64URL.encode(proof_bytes(token, client_fp, server_fp))
}

fn proof_bytes(token: &[u8], client_fp: &str, server_fp: &str) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(token).expect("HMAC accepts any key length");
    mac.update(client_fp.as_bytes());
    mac.update(server_fp.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// The `fp` field of the QR payload from a bare hex fingerprint.
fn qr_fingerprint(hex: &str) -> String {
    format!("sha256:{hex}")
}

// ---------------------------------------------------------------------
// State
// ---------------------------------------------------------------------

/// A freshly issued token, as the QR payload carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedToken {
    /// base64url, no padding.
    pub token: String,
    /// Unix seconds.
    pub exp: i64,
}

/// What to do when a device with the same name is already paired. The
/// spec's rule: the old row is replaced only if the user confirms, and
/// otherwise both coexist -- so the desktop never picks either on its
/// own. Irrelevant when the name is free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameName {
    /// The UI has not asked the user. A same-name approve is refused
    /// with [`RespondError::NameTaken`] and stays pending, so the UI can
    /// ask and answer again with one of the other two.
    Undecided,
    /// Delete the old row(s) and close their connections, then insert.
    Replace,
    /// Insert alongside; both rows stay.
    KeepBoth,
}

/// The user's answer to the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairDecision {
    Approve { same_name: SameName },
    Deny,
}

/// Why `respond` could not record a decision.
#[derive(Debug, thiserror::Error)]
pub enum RespondError {
    /// No such pending request: it was answered, timed out, or never was.
    #[error("no pending pairing request with that id")]
    UnknownRequest,
    /// A device with this name is already paired and the decision was
    /// [`SameName::Undecided`]. The request is still pending; answer
    /// again with replace or keep-both.
    #[error("a device named {0:?} is already paired")]
    NameTaken(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Timings, overridable so a test does not wait two minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairingConfig {
    pub token_ttl: Duration,
    pub decision_timeout: Duration,
}

impl Default for PairingConfig {
    fn default() -> Self {
        Self {
            token_ttl: TOKEN_TTL,
            decision_timeout: DECISION_TIMEOUT,
        }
    }
}

struct Token {
    bytes: [u8; TOKEN_LEN],
    expires: Instant,
}

struct Pending {
    device: NewDevice,
    reply: oneshot::Sender<Result<PairOutcome, PairError>>,
}

#[derive(Default)]
struct Inner {
    /// At most one outstanding token: the UI shows one QR at a time and
    /// issuing a new one is how "Pair a phone" is re-opened, so the old
    /// one dies with the screen that showed it.
    token: Option<Token>,
    pending: HashMap<u64, Pending>,
    next_id: u64,
}

/// Everything pairing remembers between requests. One per app, managed
/// as `Arc<PairingState>` so the listener task can hold a clone.
pub struct PairingState {
    inner: Mutex<Inner>,
    notify: Box<dyn Fn(PairingRequestEvent) + Send + Sync>,
    revocations: broadcast::Sender<String>,
    config: PairingConfig,
    /// `paired_devices` by fingerprint, for the listener. The TLS
    /// verifier asks on every handshake and the path gate on every
    /// request, from tokio workers that must not wait on SQLite; this
    /// copy answers both, and every write to the table in this module
    /// refreshes it before the caller hears the outcome.
    devices: RwLock<HashMap<String, PairedDevice>>,
}

impl PairingState {
    /// `notify` is called, outside any lock, with each new request the
    /// user must decide on; in the app it emits [`PAIRING_REQUEST_EVENT`].
    pub fn new(notify: impl Fn(PairingRequestEvent) + Send + Sync + 'static) -> Self {
        Self::with_config(PairingConfig::default(), notify)
    }

    pub fn with_config(
        config: PairingConfig,
        notify: impl Fn(PairingRequestEvent) + Send + Sync + 'static,
    ) -> Self {
        // 16 is plenty: revocations are user clicks, and a lagged
        // receiver only means the listener re-checks its connection
        // list, which it can do from `paired_devices` anyway.
        let (revocations, _) = broadcast::channel(16);
        Self {
            inner: Mutex::new(Inner::default()),
            notify: Box::new(notify),
            revocations,
            config,
            devices: RwLock::new(HashMap::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A poisoned lock means a panic while holding it; the state is a
        // token and some pending replies, none of which a panic can
        // leave half-written in a way that matters.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Replace the in-memory copy of `paired_devices` with the table's
    /// current rows. Called once at startup by `remote/gate.rs`, and by
    /// [`respond`](Self::respond) and [`revoke`](Self::revoke) after
    /// every write, so the listener's answers are never staler than the
    /// last change made through this module.
    pub fn reload_devices(&self, conn: &Connection) -> Result<(), StoreError> {
        let rows = devices::list(conn)?;
        let map = rows.into_iter().map(|d| (d.cert_fp.clone(), d)).collect();
        *self.devices.write().unwrap_or_else(|e| e.into_inner()) = map;
        Ok(())
    }

    /// Reload after a write, or say so: the write itself succeeded and
    /// the caller must still hear that, but a listener that cannot see
    /// it is worth a warning.
    fn refresh_devices(&self, conn: &Connection) {
        if let Err(e) = self.reload_devices(conn) {
            log::warn!("could not refresh the paired devices the listener checks: {e}");
        }
    }

    /// Whether a device with this fingerprint is paired, from the
    /// in-memory copy. Cheap and non-blocking: the TLS handshake path.
    pub fn is_device_paired(&self, cert_fp: &str) -> bool {
        self.devices
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(cert_fp)
    }

    /// The paired device presenting this fingerprint, with its step-up
    /// keys, for `/v1/call`. `None` when not paired.
    pub fn paired_device(&self, cert_fp: &str) -> Option<PairedDevice> {
        self.devices
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(cert_fp)
            .cloned()
    }

    /// Mint a new token, replacing any outstanding one.
    pub fn issue_token(&self) -> IssuedToken {
        use rand::RngCore;
        let mut bytes = [0u8; TOKEN_LEN];
        rand::rng().fill_bytes(&mut bytes);
        let expires = Instant::now() + self.config.token_ttl;
        let exp = (SystemTime::now() + self.config.token_ttl)
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.lock().token = Some(Token { bytes, expires });
        IssuedToken {
            token: BASE64URL.encode(bytes),
            exp,
        }
    }

    /// Whether an unexpired token is outstanding -- the only time the
    /// verifier may admit an unpaired certificate.
    pub fn pairing_open(&self) -> bool {
        self.lock()
            .token
            .as_ref()
            .is_some_and(|t| Instant::now() < t.expires)
    }

    /// Fingerprints of devices that were just revoked. Subscribe before
    /// accepting connections; a message means "drop every connection
    /// presenting this certificate".
    pub fn subscribe_revocations(&self) -> broadcast::Receiver<String> {
        self.revocations.subscribe()
    }

    /// Requests waiting on the user, oldest first. For a UI that mounts
    /// after the event fired.
    pub fn pending(&self) -> Vec<PairingRequestEvent> {
        let inner = self.lock();
        let mut ids: Vec<_> = inner.pending.keys().copied().collect();
        ids.sort_unstable();
        ids.into_iter()
            .map(|id| event_for(id, &inner.pending[&id].device))
            .collect()
    }

    /// Consume the token if `presented` matches and it is unexpired.
    fn take_token(&self, presented: &[u8]) -> Result<[u8; TOKEN_LEN], PairError> {
        let mut inner = self.lock();
        let token = inner.token.as_ref().ok_or(PairError::InvalidToken)?;
        if Instant::now() >= token.expires {
            inner.token = None;
            return Err(PairError::InvalidToken);
        }
        if !bool::from(token.bytes.ct_eq(presented)) {
            return Err(PairError::InvalidToken);
        }
        // Single use: whatever happens next, this token is spent. A
        // failed proof burns it too -- the spec's order is verify then
        // invalidate, but a proof failure after a matching token means
        // the client has the token and not the key, and letting it try
        // again with the same token gains nothing.
        Ok(inner.token.take().expect("checked above").bytes)
    }

    fn register(
        &self,
        device: NewDevice,
    ) -> (u64, oneshot::Receiver<Result<PairOutcome, PairError>>) {
        let (reply, rx) = oneshot::channel();
        let event = {
            let mut inner = self.lock();
            let id = inner.next_id;
            inner.next_id += 1;
            let event = event_for(id, &device);
            inner.pending.insert(id, Pending { device, reply });
            event
        };
        let id = event.request_id;
        (self.notify)(event);
        (id, rx)
    }

    /// Record the user's answer. On approve the row is inserted here,
    /// before the phone hears 200, so the verifier sees it on the very
    /// next handshake.
    pub fn respond(
        &self,
        conn: &Connection,
        request_id: u64,
        decision: PairDecision,
    ) -> Result<(), RespondError> {
        let same_name = match decision {
            PairDecision::Deny => {
                let pending = self
                    .lock()
                    .pending
                    .remove(&request_id)
                    .ok_or(RespondError::UnknownRequest)?;
                let _ = pending.reply.send(Err(PairError::Denied));
                return Ok(());
            }
            PairDecision::Approve { same_name } => same_name,
        };

        // Check the name BEFORE taking the request out of the map, so a
        // refused approve leaves it pending for a second answer.
        let name = {
            let inner = self.lock();
            inner
                .pending
                .get(&request_id)
                .map(|p| p.device.name.clone())
                .ok_or(RespondError::UnknownRequest)?
        };
        let existing = devices::find_by_name(conn, &name)?;
        let to_replace: &[PairedDevice] = match same_name {
            _ if existing.is_empty() => &[],
            SameName::Undecided => return Err(RespondError::NameTaken(name)),
            SameName::Replace => &existing,
            SameName::KeepBoth => &[],
        };

        let pending = self
            .lock()
            .pending
            .remove(&request_id)
            .ok_or(RespondError::UnknownRequest)?;

        let result = (|| -> Result<PairOutcome, StoreError> {
            for old in to_replace {
                if devices::revoke(conn, old.id)?.is_some() {
                    let _ = self.revocations.send(old.cert_fp.clone());
                }
            }
            let device_id = devices::insert(conn, &pending.device)?;
            log::info!("paired device {device_id}");
            Ok(PairOutcome {
                device_id,
                device_name: name,
            })
        })();

        // Before the phone hears 200: its very next handshake must find
        // the row. Also after a failure, since a replace may have
        // deleted the old row before the insert failed.
        self.refresh_devices(conn);
        match result {
            Ok(outcome) => {
                let _ = pending.reply.send(Ok(outcome));
                Ok(())
            }
            Err(e) => {
                // The phone must not be told 200 for a row that does not
                // exist; it gets a refusal and can pair again.
                let _ = pending.reply.send(Err(PairError::Denied));
                Err(e.into())
            }
        }
    }

    /// Delete a device and tell the listener to close its connections.
    /// Returns the removed row, or `None` if there was none.
    pub fn revoke(&self, conn: &Connection, id: i64) -> Result<Option<PairedDevice>, StoreError> {
        let removed = devices::revoke(conn, id)?;
        if let Some(device) = &removed {
            log::info!("revoked device {id} ({})", device.name);
            // The copy first, then the broadcast: a listener reacting to
            // the broadcast by re-checking must already see it gone.
            self.refresh_devices(conn);
            let _ = self.revocations.send(device.cert_fp.clone());
        }
        Ok(removed)
    }

    /// Set what one device may read of the session transcripts (#1488),
    /// and refresh the listener's copy so the very next call is judged by
    /// the new setting. Returns whether the device still existed.
    pub fn set_transcript_access(
        &self,
        conn: &Connection,
        id: i64,
        transcripts_allowed: bool,
        reveal_allowed: bool,
    ) -> Result<bool, StoreError> {
        let changed =
            devices::set_transcript_access(conn, id, transcripts_allowed, reveal_allowed)?;
        if changed {
            log::info!(
                "device {id}: transcripts {}, reveal {}",
                if transcripts_allowed { "on" } else { "off" },
                if reveal_allowed { "on" } else { "off" }
            );
            self.refresh_devices(conn);
        }
        Ok(changed)
    }
}

fn event_for(request_id: u64, device: &NewDevice) -> PairingRequestEvent {
    PairingRequestEvent {
        request_id,
        device_name: device.name.clone(),
        fingerprint: device.cert_fp.clone(),
        has_mldsa: device.mldsa_pubkey.is_some(),
    }
}

// ---------------------------------------------------------------------
// The /v1/pair handshake
// ---------------------------------------------------------------------

/// Run the desktop side of `POST /v1/pair` for one request and wait for
/// the user's decision. See the module docs for how to mount it.
///
/// Order of checks: body shape (400, token untouched), token (403),
/// proof (403), then the token is spent, the `pairing-request` event
/// fires, and this future waits up to [`DECISION_TIMEOUT`] for
/// [`PairingState::respond`]. Approve inserts the row before this
/// returns `Ok`.
pub async fn handle_pair(
    state: &PairingState,
    req: PairRequest,
    client: &PeerCert,
    server_fp: &str,
) -> Result<PairOutcome, PairError> {
    let device = decode_request(&req, client)?;
    let presented = BASE64URL
        .decode(&req.token)
        .map_err(|_| PairError::InvalidToken)?;
    let presented_proof = BASE64URL
        .decode(&req.proof)
        .map_err(|_| PairError::BadProof)?;

    let token = state.take_token(&presented)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&token).expect("HMAC accepts any key length");
    mac.update(client.fingerprint.as_bytes());
    mac.update(server_fp.as_bytes());
    mac.verify_slice(&presented_proof)
        .map_err(|_| PairError::BadProof)?;

    let (id, mut rx) = state.register(device);
    match tokio::time::timeout(state.config.decision_timeout, &mut rx).await {
        Ok(Ok(result)) => result,
        // The sender was dropped without an answer; nothing can answer now.
        Ok(Err(_)) => Err(PairError::Timeout),
        Err(_elapsed) => {
            // Remove it so the UI cannot approve a phone that has given
            // up. If `respond` got there first the entry is gone and an
            // answer is on the channel (or the sender is being dropped);
            // honour it rather than telling the phone "timeout" about a
            // row that exists.
            let removed = state.lock().pending.remove(&id).is_some();
            if removed {
                Err(PairError::Timeout)
            } else {
                rx.await.unwrap_or(Err(PairError::Timeout))
            }
        }
    }
}

fn decode_request(req: &PairRequest, client: &PeerCert) -> Result<NewDevice, PairError> {
    let name = req.device_name.trim();
    if name.is_empty() {
        return Err(PairError::BadRequest("device_name is empty".into()));
    }
    let ecdsa = BASE64
        .decode(&req.signing_keys.ecdsa_p256)
        .map_err(|_| PairError::BadRequest("ecdsa_p256 is not base64".into()))?;
    if ecdsa.len() != ECDSA_P256_LEN || ecdsa[0] != 0x04 {
        return Err(PairError::BadRequest(
            "ecdsa_p256 must be a 65-byte SEC1 uncompressed point".into(),
        ));
    }
    let mldsa = match &req.signing_keys.mldsa_65 {
        None => None,
        Some(b64) => {
            let key = BASE64
                .decode(b64)
                .map_err(|_| PairError::BadRequest("mldsa_65 is not base64".into()))?;
            if key.len() != MLDSA_65_LEN {
                return Err(PairError::BadRequest(
                    "mldsa_65 must be a 1952-byte ML-DSA-65 public key".into(),
                ));
            }
            Some(key)
        }
    };
    Ok(NewDevice {
        name: name.to_string(),
        cert_fp: client.fingerprint.clone(),
        cert_der: client.der.clone(),
        ecdsa_pubkey: ecdsa,
        mldsa_pubkey: mldsa,
    })
}

// ---------------------------------------------------------------------
// Addresses for the QR payload
// ---------------------------------------------------------------------

/// Every address a phone could try, IPv4 before IPv6, each once.
///
/// Loopback is useless to another device. IPv6 link-local (`fe80::/10`)
/// is dropped too: it is only routable with a scope id the phone cannot
/// know, so listing it would add a guaranteed-failing attempt ahead of
/// the overlay address. Overlay addresses (Tailscale's `100.64.0.0/10`,
/// a WireGuard `10.x`) are ordinary interface addresses and come through
/// like any other.
pub fn usable_addrs(all: impl IntoIterator<Item = IpAddr>) -> Vec<String> {
    let mut v4 = Vec::new();
    let mut v6 = Vec::new();
    for ip in all {
        match ip {
            IpAddr::V4(a) if !a.is_loopback() && !a.is_unspecified() => {
                v4.push(IpAddr::V4(a));
            }
            IpAddr::V6(a)
                if !a.is_loopback() && !a.is_unspecified() && !a.is_unicast_link_local() =>
            {
                v6.push(IpAddr::V6(a));
            }
            _ => {}
        }
    }
    let mut out: Vec<String> = Vec::new();
    for ip in v4.into_iter().chain(v6) {
        let s = ip.to_string();
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

fn local_addrs() -> Result<Vec<String>, String> {
    let ifaces = if_addrs::get_if_addrs().map_err(|e| format!("could not list addresses: {e}"))?;
    Ok(usable_addrs(ifaces.into_iter().map(|i| i.ip())))
}

// ---------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------

/// A paired device as the Settings screen sees it: no key material.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairedDeviceSummary {
    pub id: i64,
    pub name: String,
    /// Lowercase hex, no prefix.
    pub cert_fp: String,
    pub has_mldsa: bool,
    pub paired_at: String,
    pub last_seen: Option<String>,
    /// "Allow this phone to read session transcripts" (#1488).
    pub transcripts_allowed: bool,
    /// "Allow this phone to reveal hidden text" (#1488).
    pub reveal_allowed: bool,
}

impl From<PairedDevice> for PairedDeviceSummary {
    fn from(d: PairedDevice) -> Self {
        Self {
            id: d.id,
            name: d.name,
            cert_fp: d.cert_fp,
            has_mldsa: d.mldsa_pubkey.is_some(),
            paired_at: d.paired_at,
            last_seen: d.last_seen,
            transcripts_allowed: d.transcripts_allowed,
            reveal_allowed: d.reveal_allowed,
        }
    }
}

/// Build the [`PairingState`] the app manages, wired to emit
/// [`PAIRING_REQUEST_EVENT`] on the given handle.
pub fn new_state(app: AppHandle) -> Arc<PairingState> {
    Arc::new(PairingState::new(move |event| {
        if let Err(e) = app.emit(PAIRING_REQUEST_EVENT, &event) {
            log::warn!("could not emit {PAIRING_REQUEST_EVENT}: {e}");
        }
    }))
}

/// Settings > Pair a phone. Mints a token and returns what the QR code
/// should encode.
///
/// Starts the listener first if it is not already up -- "the desktop
/// enables the listener if it is not already on" in the spec -- because
/// the identity whose fingerprint the QR carries is loaded by that
/// start, and a QR pointing at a port nothing answers on is worse than
/// an error. The Settings screen turns the setting on before calling
/// this, so the start here is normally a no-op; it is the guarantee,
/// not the usual path.
#[tauri::command]
pub async fn issue_pairing_token(
    app: AppHandle,
    state: State<'_, Arc<PairingState>>,
    identity: State<'_, DesktopIdentity>,
) -> Result<QrPayload, String> {
    crate::remote::gate::start(&app).await?;
    let identity = identity.0.identity()?;
    let addrs = local_addrs()?;
    if addrs.is_empty() {
        return Err("this machine has no network address a phone could reach".into());
    }
    let issued = state.issue_token();
    log::info!("pairing token issued, {} address(es) offered", addrs.len());
    Ok(QrPayload {
        v: QR_VERSION,
        name: identity.display_name,
        addrs,
        port: PORT,
        fp: qr_fingerprint(&identity.fingerprint),
        token: issued.token,
        exp: issued.exp,
    })
}

/// The user's answer to the modal.
///
/// `replace_existing` only matters on approve, and only when a device
/// with the same name is already paired: `Some(true)` replaces it,
/// `Some(false)` keeps both, and `None` (the UI has not asked) fails
/// with a message naming the device while the request stays pending for
/// a second answer. Three states rather than a bool so that "keep both"
/// is a choice the user made, never a default the UI fell into.
#[tauri::command]
pub fn respond_to_pairing(
    app: AppHandle,
    state: State<'_, Arc<PairingState>>,
    request_id: u64,
    approve: bool,
    replace_existing: Option<bool>,
) -> Result<(), String> {
    let decision = if approve {
        let same_name = match replace_existing {
            None => SameName::Undecided,
            Some(true) => SameName::Replace,
            Some(false) => SameName::KeepBoth,
        };
        PairDecision::Approve { same_name }
    } else {
        PairDecision::Deny
    };
    let conn = open_db(&crate::commands::db_path(&app)).map_err(|e| e.to_string())?;
    state
        .respond(&conn, request_id, decision)
        .map_err(|e| e.to_string())
}

/// Settings > Paired devices.
#[tauri::command]
pub fn list_paired_devices(app: AppHandle) -> Result<Vec<PairedDeviceSummary>, String> {
    let conn = open_db(&crate::commands::db_path(&app)).map_err(|e| e.to_string())?;
    let rows = devices::list(&conn).map_err(|e| e.to_string())?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Revoke: delete the row and close that certificate's connections.
/// Succeeds when the row is already gone, so a double click is harmless.
#[tauri::command]
pub fn revoke_paired_device(
    app: AppHandle,
    state: State<'_, Arc<PairingState>>,
    id: i64,
) -> Result<(), String> {
    let conn = open_db(&crate::commands::db_path(&app)).map_err(|e| e.to_string())?;
    state
        .revoke(&conn, id)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Settings > Paired devices: what one phone may read of the session
/// transcripts (#1488).
///
/// Both switches in one call, because they are one decision: reveal
/// means nothing for a phone that may not read transcripts at all, and
/// the UI disables it then. Stored as sent rather than coerced, so
/// turning transcripts back on restores the reveal choice the owner made.
///
/// Fails naming the device when it was revoked in the meantime -- a
/// setting saved on nothing must not look saved.
#[tauri::command]
pub fn set_paired_device_access(
    app: AppHandle,
    state: State<'_, Arc<PairingState>>,
    id: i64,
    transcripts_allowed: bool,
    reveal_allowed: bool,
) -> Result<(), String> {
    let conn = open_db(&crate::commands::db_path(&app)).map_err(|e| e.to_string())?;
    match state.set_transcript_access(&conn, id, transcripts_allowed, reveal_allowed) {
        Ok(true) => Ok(()),
        Ok(false) => Err("this phone is no longer paired".into()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use tokio::sync::mpsc;

    const SERVER_FP: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const CLIENT_FP: &str = "abababababababababababababababababababababababababababababababab";

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        conn
    }

    fn client() -> PeerCert {
        PeerCert {
            fingerprint: CLIENT_FP.into(),
            der: vec![0x30, 0x82, 0x02, 0x01],
        }
    }

    fn keys(with_mldsa: bool) -> SigningKeys {
        SigningKeys {
            ecdsa_p256: BASE64.encode([0x04; ECDSA_P256_LEN]),
            mldsa_65: with_mldsa.then(|| BASE64.encode([0x11; MLDSA_65_LEN])),
        }
    }

    fn request(issued: &IssuedToken, name: &str, with_mldsa: bool) -> PairRequest {
        let token = BASE64URL.decode(&issued.token).unwrap();
        PairRequest {
            token: issued.token.clone(),
            device_name: name.into(),
            signing_keys: keys(with_mldsa),
            proof: proof(&token, CLIENT_FP, SERVER_FP),
        }
    }

    /// A state whose events land on a channel the test can wait on.
    fn state(
        config: PairingConfig,
    ) -> (
        Arc<PairingState>,
        mpsc::UnboundedReceiver<PairingRequestEvent>,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        let state = PairingState::with_config(config, move |e| {
            let _ = tx.send(e);
        });
        (Arc::new(state), rx)
    }

    fn quick() -> PairingConfig {
        PairingConfig {
            token_ttl: Duration::from_secs(60),
            decision_timeout: Duration::from_secs(5),
        }
    }

    #[test]
    fn a_token_is_32_random_bytes_and_replaces_the_previous_one() {
        let (state, _) = state(quick());
        let a = state.issue_token();
        let b = state.issue_token();
        assert_ne!(a.token, b.token);
        assert_eq!(BASE64URL.decode(&a.token).unwrap().len(), TOKEN_LEN);
        assert!(state.pairing_open());
        // Only the newest token is live.
        let stale = BASE64URL.decode(&a.token).unwrap();
        assert_eq!(state.take_token(&stale), Err(PairError::InvalidToken));
        let live = BASE64URL.decode(&b.token).unwrap();
        assert!(state.take_token(&live).is_ok());
        assert!(!state.pairing_open(), "spent tokens close the window");
    }

    #[test]
    fn the_exp_field_is_about_ttl_from_now() {
        let (state, _) = state(quick());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let issued = state.issue_token();
        assert!((issued.exp - now - 60).abs() <= 1);
    }

    #[tokio::test]
    async fn an_expired_token_is_refused() {
        let (state, _) = state(PairingConfig {
            token_ttl: Duration::ZERO,
            ..quick()
        });
        let issued = state.issue_token();
        assert!(!state.pairing_open());
        let err = handle_pair(
            &state,
            request(&issued, "Octocat's phone", false),
            &client(),
            SERVER_FP,
        )
        .await
        .unwrap_err();
        assert_eq!(err, PairError::InvalidToken);
        assert_eq!(err.http_status(), 403);
    }

    #[tokio::test]
    async fn a_token_nobody_issued_is_refused() {
        let (state, _) = state(quick());
        let fake = IssuedToken {
            token: BASE64URL.encode([7u8; TOKEN_LEN]),
            exp: 0,
        };
        let err = handle_pair(
            &state,
            request(&fake, "Octocat's phone", false),
            &client(),
            SERVER_FP,
        )
        .await
        .unwrap_err();
        assert_eq!(err, PairError::InvalidToken);
    }

    #[tokio::test]
    async fn a_replayed_token_is_refused() {
        let (state, mut events) = state(quick());
        let issued = state.issue_token();
        let req = request(&issued, "Octocat's phone", false);

        let first = {
            let state = state.clone();
            let req = req.clone();
            tokio::spawn(async move { handle_pair(&state, req, &client(), SERVER_FP).await })
        };
        let event = events
            .recv()
            .await
            .expect("the first request reaches the user");

        // Same token, second time: spent.
        let err = handle_pair(&state, req, &client(), SERVER_FP)
            .await
            .unwrap_err();
        assert_eq!(err, PairError::InvalidToken);
        assert_eq!(
            state.pending().len(),
            1,
            "the replay must not reach the user"
        );

        state
            .respond(&db(), event.request_id, PairDecision::Deny)
            .unwrap();
        assert_eq!(first.await.unwrap(), Err(PairError::Denied));
    }

    #[tokio::test]
    async fn a_wrong_proof_is_refused_and_spends_the_token() {
        let (state, mut events) = state(quick());
        let issued = state.issue_token();
        let mut req = request(&issued, "Octocat's phone", false);
        // Right token, right fingerprints, wrong key.
        req.proof = proof(&[9u8; TOKEN_LEN], CLIENT_FP, SERVER_FP);

        let err = handle_pair(&state, req.clone(), &client(), SERVER_FP)
            .await
            .unwrap_err();
        assert_eq!(err, PairError::BadProof);
        assert_eq!(err.http_status(), 403);
        assert!(events.try_recv().is_err(), "no modal for a bad proof");
        assert!(!state.pairing_open(), "a bad proof spends the token");

        // A proof over a different server fingerprint (a captured token
        // replayed by something that is not the desktop the phone
        // scanned) also fails.
        let issued = state.issue_token();
        let token = BASE64URL.decode(&issued.token).unwrap();
        req.token = issued.token;
        req.proof = proof(&token, CLIENT_FP, "2222");
        assert_eq!(
            handle_pair(&state, req.clone(), &client(), SERVER_FP)
                .await
                .unwrap_err(),
            PairError::BadProof
        );
        assert!(!state.pairing_open(), "the token is gone either way");
    }

    #[tokio::test]
    async fn a_malformed_body_is_400_and_keeps_the_token() {
        let (state, _) = state(quick());
        let issued = state.issue_token();

        let mut req = request(&issued, "Octocat's phone", false);
        req.signing_keys.ecdsa_p256 = BASE64.encode([0x04; 33]);
        let err = handle_pair(&state, req, &client(), SERVER_FP)
            .await
            .unwrap_err();
        assert!(matches!(err, PairError::BadRequest(_)), "{err:?}");
        assert_eq!(err.http_status(), 400);

        let mut req = request(&issued, "Octocat's phone", true);
        req.signing_keys.mldsa_65 = Some(BASE64.encode([0u8; 3309]));
        assert!(matches!(
            handle_pair(&state, req, &client(), SERVER_FP).await,
            Err(PairError::BadRequest(_))
        ));

        let req = request(&issued, "   ", false);
        assert!(matches!(
            handle_pair(&state, req, &client(), SERVER_FP).await,
            Err(PairError::BadRequest(_))
        ));

        assert!(state.pairing_open(), "a 400 does not spend the token");
    }

    #[tokio::test]
    async fn approve_inserts_the_row_and_returns_200() {
        let (state, mut events) = state(quick());
        let conn = db();
        let issued = state.issue_token();
        let req = request(&issued, "Octocat's phone", true);

        let handshake = {
            let state = state.clone();
            tokio::spawn(async move { handle_pair(&state, req, &client(), SERVER_FP).await })
        };
        let event = events.recv().await.unwrap();
        assert_eq!(
            event,
            PairingRequestEvent {
                request_id: event.request_id,
                device_name: "Octocat's phone".into(),
                fingerprint: CLIENT_FP.into(),
                has_mldsa: true,
            }
        );
        assert_eq!(state.pending(), vec![event.clone()]);

        // The name is free, so an undecided approve is a plain approve.
        state
            .respond(
                &conn,
                event.request_id,
                PairDecision::Approve {
                    same_name: SameName::Undecided,
                },
            )
            .unwrap();

        let outcome = handshake.await.unwrap().expect("approved");
        assert_eq!(outcome.device_name, "Octocat's phone");
        let row = devices::find_by_fingerprint(&conn, CLIENT_FP)
            .unwrap()
            .expect("approve inserts the row");
        assert_eq!(row.id, outcome.device_id);
        assert_eq!(row.cert_der, client().der);
        assert_eq!(row.ecdsa_pubkey, vec![0x04; ECDSA_P256_LEN]);
        assert_eq!(row.mldsa_pubkey, Some(vec![0x11; MLDSA_65_LEN]));
        assert!(state.pending().is_empty());
        assert!(!state.pairing_open());
    }

    #[tokio::test]
    async fn deny_returns_403_and_inserts_nothing() {
        let (state, mut events) = state(quick());
        let conn = db();
        let issued = state.issue_token();
        let req = request(&issued, "Octocat's phone", false);

        let handshake = {
            let state = state.clone();
            tokio::spawn(async move { handle_pair(&state, req, &client(), SERVER_FP).await })
        };
        let event = events.recv().await.unwrap();
        state
            .respond(&conn, event.request_id, PairDecision::Deny)
            .unwrap();

        let err = handshake.await.unwrap().unwrap_err();
        assert_eq!(err, PairError::Denied);
        assert_eq!(err.http_status(), 403);
        assert!(devices::list(&conn).unwrap().is_empty());
        // Answered once; a second answer has nothing to act on.
        assert!(matches!(
            state.respond(&conn, event.request_id, PairDecision::Deny),
            Err(RespondError::UnknownRequest)
        ));
    }

    #[tokio::test]
    async fn no_answer_in_time_returns_403_and_forgets_the_request() {
        let (state, mut events) = state(PairingConfig {
            decision_timeout: Duration::from_millis(20),
            ..quick()
        });
        let conn = db();
        let issued = state.issue_token();
        let req = request(&issued, "Octocat's phone", false);

        let err = handle_pair(&state, req, &client(), SERVER_FP)
            .await
            .unwrap_err();
        assert_eq!(err, PairError::Timeout);
        assert_eq!(err.http_status(), 403);

        let event = events.recv().await.unwrap();
        assert!(state.pending().is_empty());
        // The phone has given up; approving now must not create a row.
        assert!(matches!(
            state.respond(
                &conn,
                event.request_id,
                PairDecision::Approve {
                    same_name: SameName::Undecided
                }
            ),
            Err(RespondError::UnknownRequest)
        ));
        assert!(devices::list(&conn).unwrap().is_empty());
    }

    /// Pairs a second phone called "Octocat's phone" while one is
    /// already on file, and returns the event to answer.
    async fn same_name_handshake(
        state: &Arc<PairingState>,
        events: &mut mpsc::UnboundedReceiver<PairingRequestEvent>,
        conn: &Connection,
    ) -> (
        tokio::task::JoinHandle<Result<PairOutcome, PairError>>,
        PairingRequestEvent,
    ) {
        devices::insert(
            conn,
            &NewDevice {
                name: "Octocat's phone".into(),
                cert_fp: "0ld".into(),
                cert_der: vec![1],
                ecdsa_pubkey: vec![0x04; ECDSA_P256_LEN],
                mldsa_pubkey: None,
            },
        )
        .unwrap();

        let issued = state.issue_token();
        let req = request(&issued, "Octocat's phone", false);
        let handshake = {
            let state = state.clone();
            tokio::spawn(async move { handle_pair(&state, req, &client(), SERVER_FP).await })
        };
        let event = events.recv().await.unwrap();
        (handshake, event)
    }

    #[tokio::test]
    async fn re_pairing_the_same_name_is_refused_until_the_user_chooses() {
        let (state, mut events) = state(quick());
        let conn = db();
        let mut revoked = state.subscribe_revocations();
        let (handshake, event) = same_name_handshake(&state, &mut events, &conn).await;

        // Undecided: refused, nothing changed, still pending.
        let err = state
            .respond(
                &conn,
                event.request_id,
                PairDecision::Approve {
                    same_name: SameName::Undecided,
                },
            )
            .unwrap_err();
        assert!(
            matches!(err, RespondError::NameTaken(ref n) if n == "Octocat's phone"),
            "{err}"
        );
        assert_eq!(state.pending().len(), 1);
        assert_eq!(devices::list(&conn).unwrap().len(), 1);
        assert!(revoked.try_recv().is_err());

        // Replace: the old row goes, its connections are told to close,
        // and the new row is the only one.
        state
            .respond(
                &conn,
                event.request_id,
                PairDecision::Approve {
                    same_name: SameName::Replace,
                },
            )
            .unwrap();
        assert!(handshake.await.unwrap().is_ok());
        let all = devices::list(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].cert_fp, CLIENT_FP);
        assert_eq!(revoked.try_recv().unwrap(), "0ld");
    }

    #[tokio::test]
    async fn keeping_both_leaves_the_old_row_and_its_connections_alone() {
        let (state, mut events) = state(quick());
        let conn = db();
        let mut revoked = state.subscribe_revocations();
        let (handshake, event) = same_name_handshake(&state, &mut events, &conn).await;

        state
            .respond(
                &conn,
                event.request_id,
                PairDecision::Approve {
                    same_name: SameName::KeepBoth,
                },
            )
            .unwrap();
        assert!(handshake.await.unwrap().is_ok());

        let fps: Vec<_> = devices::list(&conn)
            .unwrap()
            .into_iter()
            .map(|d| d.cert_fp)
            .collect();
        assert_eq!(fps, vec!["0ld".to_string(), CLIENT_FP.to_string()]);
        assert!(revoked.try_recv().is_err(), "nothing was revoked");
    }

    #[test]
    fn revoke_deletes_the_row_and_broadcasts_the_fingerprint() {
        let (state, _) = state(quick());
        let conn = db();
        let mut revoked = state.subscribe_revocations();
        let id = devices::insert(
            &conn,
            &NewDevice {
                name: "Octocat's phone".into(),
                cert_fp: CLIENT_FP.into(),
                cert_der: vec![1],
                ecdsa_pubkey: vec![0x04; ECDSA_P256_LEN],
                mldsa_pubkey: None,
            },
        )
        .unwrap();

        let removed = state.revoke(&conn, id).unwrap().unwrap();
        assert_eq!(removed.cert_fp, CLIENT_FP);
        assert_eq!(revoked.try_recv().unwrap(), CLIENT_FP);
        assert!(devices::list(&conn).unwrap().is_empty());

        // Already gone: no error, no second broadcast.
        assert!(state.revoke(&conn, id).unwrap().is_none());
        assert!(revoked.try_recv().is_err());
    }

    /// RFC 4231 test case 2, so the HMAC is the standard one and not a
    /// homegrown variant the phone would have to reproduce.
    #[test]
    fn the_proof_is_standard_hmac_sha256() {
        let mac = proof_bytes(b"Jefe", "what do ya want ", "for nothing?");
        assert_eq!(
            mac,
            [
                0x5b, 0xdc, 0xc1, 0x46, 0xbf, 0x60, 0x75, 0x4e, 0x6a, 0x04, 0x24, 0x26, 0x08, 0x95,
                0x75, 0xc7, 0x5a, 0x00, 0x3f, 0x08, 0x9d, 0x27, 0x39, 0x83, 0x9d, 0xec, 0x58, 0xb9,
                0x64, 0xec, 0x38, 0x43
            ]
        );
    }

    /// One integer on the wire, two constants in the code: the QR's `v`
    /// and `/v1/hello`'s `protocol_version` must never drift apart.
    #[test]
    fn the_qr_version_is_the_protocol_version() {
        assert_eq!(
            u32::from(QR_VERSION),
            crate::remote::listener::PROTOCOL_VERSION
        );
    }

    #[test]
    fn the_qr_payload_matches_the_spec_shape() {
        let payload = QrPayload {
            v: QR_VERSION,
            name: "octocat's laptop".into(),
            addrs: vec!["192.0.2.10".into(), "100.64.0.7".into()],
            port: PORT,
            fp: qr_fingerprint(SERVER_FP),
            token: "abc".into(),
            exp: 1757068800,
        };
        let json: serde_json::Value = serde_json::to_value(&payload).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "v": 2,
                "name": "octocat's laptop",
                "addrs": ["192.0.2.10", "100.64.0.7"],
                "port": 41919,
                "fp": format!("sha256:{SERVER_FP}"),
                "token": "abc",
                "exp": 1757068800
            })
        );
    }

    #[test]
    fn the_pair_request_accepts_a_missing_mldsa_key() {
        let json = r#"{"token":"t","device_name":"Octocat's phone",
            "signing_keys":{"ecdsa_p256":"BA=="},"proof":"p"}"#;
        let req: PairRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.signing_keys.mldsa_65, None);
    }

    #[test]
    fn addrs_drop_loopback_link_local_and_duplicates_and_keep_overlays() {
        let all = [
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            IpAddr::V6("fe80::1".parse().unwrap()),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            IpAddr::V6("2001:db8::7".parse().unwrap()),
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)),
            IpAddr::V4(Ipv4Addr::new(100, 64, 0, 7)),
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)),
        ];
        assert_eq!(
            usable_addrs(all),
            vec!["192.0.2.10", "100.64.0.7", "2001:db8::7"]
        );
    }

    /// The listener's copy follows every write made through this module
    /// and a reload from the table, and nothing else.
    #[tokio::test]
    async fn the_device_copy_follows_approve_revoke_and_reload() {
        let (state, mut events) = state(quick());
        let conn = db();
        assert!(!state.is_device_paired(CLIENT_FP));
        assert_eq!(state.paired_device(CLIENT_FP), None);

        // A row written behind the module's back is invisible until a
        // reload, which is why gate.rs reloads once at startup.
        devices::insert(
            &conn,
            &NewDevice {
                name: "Tablet".into(),
                cert_fp: "0ld".into(),
                cert_der: vec![1],
                ecdsa_pubkey: vec![0x04; ECDSA_P256_LEN],
                mldsa_pubkey: None,
            },
        )
        .unwrap();
        assert!(!state.is_device_paired("0ld"));
        state.reload_devices(&conn).unwrap();
        assert!(state.is_device_paired("0ld"));

        // Approve: visible before the phone hears 200, with its keys.
        let issued = state.issue_token();
        let req = request(&issued, "Octocat's phone", true);
        let handshake = {
            let state = state.clone();
            tokio::spawn(async move { handle_pair(&state, req, &client(), SERVER_FP).await })
        };
        let event = events.recv().await.unwrap();
        state
            .respond(
                &conn,
                event.request_id,
                PairDecision::Approve {
                    same_name: SameName::Undecided,
                },
            )
            .unwrap();
        let outcome = handshake.await.unwrap().unwrap();
        let device = state
            .paired_device(CLIENT_FP)
            .expect("paired after approve");
        assert_eq!(device.id, outcome.device_id);
        assert_eq!(device.name, "Octocat's phone");
        assert_eq!(device.ecdsa_pubkey, vec![0x04; ECDSA_P256_LEN]);
        assert_eq!(device.mldsa_pubkey, Some(vec![0x11; MLDSA_65_LEN]));

        // Revoke: gone before the broadcast is sent.
        let mut revoked = state.subscribe_revocations();
        state.revoke(&conn, outcome.device_id).unwrap();
        assert!(!state.is_device_paired(CLIENT_FP));
        assert!(state.is_device_paired("0ld"), "only the revoked one goes");
        assert_eq!(revoked.try_recv().unwrap(), CLIENT_FP);
    }
}

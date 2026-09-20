//! The mTLS listener a paired phone talks to.
//!
//! axum on a tokio task, port [`PORT`] on every interface (IPv4 and
//! IPv6 on one dual-stack socket), TLS 1.3 only, rustls on the
//! aws-lc-rs provider (which carries ML-DSA as of rustls 0.23.44; see
//! [`provider`]). Client certificates are required at the handshake,
//! must be ML-DSA-65, and are checked by fingerprint against the paired
//! devices -- no CA, no chain -- so an unpaired client never reaches
//! HTTP.
//!
//! # The seams
//!
//! The listener does not know how devices are stored or how commands
//! run. It asks a [`PairedCerts`] -- "is this fingerprint paired, and
//! what are its keys?" and "is a pairing window open?" -- which
//! `gate.rs` implements over the pairing state's in-memory copy of
//! `paired_devices`; and it hands each `/v1/call` to a [`CommandHost`],
//! which `gate.rs` implements over `surface::dispatch`. Both are traits
//! so the whole surface can be driven end to end on loopback with no
//! database file and no Tauri app (`remote/loopback_tests.rs`).
//!
//! # What the handshake admits, and what HTTP then allows
//!
//! At the TLS layer a certificate is admitted if it is paired, OR if a
//! pairing window is open -- the phone about to pair has no row yet and
//! must get as far as `POST /v1/pair`. Above TLS, [`require_paired`]
//! lets an unpaired peer reach [`PAIR_PATH`] and nothing else, and
//! re-asks `is_paired` on EVERY request rather than trusting the
//! handshake, so a device revoked mid-connection is refused on its next
//! request, not only its next handshake. Handlers find who is calling
//! in the [`Peer`] request extension.
//!
//! Revocation also closes the connections a revoked certificate already
//! has open, as the spec says: every connection task watches the
//! pairing task's revocation broadcast and, on its own fingerprint,
//! shuts the HTTP connection down gracefully -- an in-flight response
//! finishes (the `/v1/events` stream ends itself on the same broadcast)
//! and nothing further is served -- with a short deadline after which
//! the connection is dropped regardless.
//!
//! # Routes and statuses
//!
//! - `GET /v1/hello`, `GET /v1/events`: see [`Hello`] and `remote/events.rs`.
//! - `POST /v1/pair`: `remote/pairing.rs`. 200 with a `PairOutcome`;
//!   400 for a body that does not decode; otherwise `PairError::http_status`.
//! - `POST /v1/call/{command}`: `remote/surface.rs` and `remote/stepup.rs`.
//!   200 with the command's JSON result; 404 unknown command, 403
//!   desktop-only command or not paired, 400 for a body that is not JSON
//!   or does not decode as the command's arguments, 500 when the command
//!   itself failed (the body is its message); a destructive command's
//!   signature is checked first and refused with `StepUpError::http_status`.
//!
//! Error bodies are plain text: the message the matching error type
//! prints, which is written to be safe to send.

use crate::remote::events::{self, Hub};
use crate::remote::identity::{fingerprint_of, Identity};
use crate::remote::pairing::{self, PairRequest, PairingState, PeerCert};
use crate::remote::stepup::{self, NonceWindow};
use crate::remote::surface::{self, Class, RemoteError};
use crate::store::devices::PairedDevice;
use axum::body::Bytes;
use axum::extract::{Extension, Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use hyper_util::rt::TokioIo;
use rustls::crypto::{aws_lc_rs, CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    CertificateError, DigitallySignedStruct, DistinguishedName, Error as TlsError, PeerMisbehaved,
    ServerConfig, SignatureScheme,
};
use serde::Serialize;
use serde_json::Value;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tokio_rustls::TlsAcceptor;

/// Fixed, from the dynamic range so it never collides with a well-known
/// service. The phone learns it from the pairing QR.
pub const PORT: u16 = 41919;

/// Bumped deliberately, in the spec, when the surface or the pairing
/// payload changes shape. Returned by `/v1/hello` and embedded in the QR.
///
/// 2 (#521): both TLS certificates are ML-DSA-65. The desktop's own
/// fingerprint changed with its certificate and the verifier admits no
/// P-256 client certificate, so every phone from protocol 1 re-pairs.
pub const PROTOCOL_VERSION: u32 = 2;

/// The one signature scheme a phone may sign the handshake with. Named
/// here, once, because the verifier both advertises it and enforces it.
const CLIENT_SIGNATURE_SCHEME: SignatureScheme = SignatureScheme::ML_DSA_65;

/// The one path an unpaired peer may reach.
pub const PAIR_PATH: &str = "/v1/pair";

/// The prefix of the command route; the command name is the rest.
pub const CALL_PREFIX: &str = "/v1/call/";

/// How long a revoked device's connection gets to finish what it was
/// sending before it is dropped. Long enough for an event stream to
/// write its final chunk; far shorter than any phone's retry.
const REVOKE_GRACE: Duration = Duration::from_secs(5);

/// What the verifier needs to know about paired devices.
///
/// Implemented over the pairing state's copy of `paired_devices` in
/// `gate.rs`; over a `HashMap` in the tests below. Every method is
/// called on the TLS handshake path or on a request, so they must be
/// cheap and must not block: read memory, not the database.
pub trait PairedCerts: Send + Sync {
    /// Whether a device with this certificate fingerprint (lowercase hex
    /// SHA256 of its DER) is currently paired. Revocation is this
    /// returning `false` where it returned `true` before.
    fn is_paired(&self, sha256_fp_hex: &str) -> bool;
    /// Whether a pairing token is live, so an unpaired certificate may
    /// be admitted to reach `/v1/pair`.
    fn pairing_window_open(&self) -> bool;
    /// The paired device's row -- its name for the log and its step-up
    /// keys for `/v1/call` -- or `None` when not paired.
    fn device(&self, sha256_fp_hex: &str) -> Option<PairedDevice>;
}

/// Where `/v1/call` sends a command once the listener has admitted it.
///
/// A trait rather than a direct call to `surface::dispatch` because
/// that needs the live `AppHandle`, which no test can construct; the
/// loopback test supplies a host that records what it was asked. The
/// listener has already checked that the command exists, is not
/// desktop-only, and (when destructive) carried a valid signature.
pub trait CommandHost: Send + Sync {
    /// Run `command` with `args` on behalf of the named device;
    /// `surface::dispatch` in production.
    fn dispatch<'a>(
        &'a self,
        command: &'a str,
        args: Value,
        device_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RemoteError>> + Send + 'a>>;
    /// A destructive command just ran for the named device;
    /// `stepup::notify_destructive` in production.
    fn notify_destructive(&self, device_name: &str, command: &str);
}

/// Who is on the other end of the connection. Inserted into every
/// request's extensions by the connection task, from the certificate
/// the handshake verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    /// Lowercase hex SHA256 of the peer's certificate DER.
    pub fingerprint: String,
    /// The certificate itself, which `/v1/pair` stores.
    pub der: Vec<u8>,
}

/// How `/v1/hello` learns the signed-in GitHub login. A closure rather
/// than a client handle so the listener neither depends on octocrab nor
/// needs a network in tests. `None` when not signed in or unreachable;
/// the first `Some` is cached for the listener's lifetime, as the
/// frontend already caches the same answer.
pub type ViewerLookup =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Option<String>> + Send>> + Send + Sync>;

/// Everything `start` needs.
pub struct ListenerConfig {
    /// `[::]:41919` in production, which binds IPv6 AND IPv4 on one
    /// socket (falling back to `0.0.0.0` where IPv6 is disabled), so
    /// every address the QR lists is one the listener answers on;
    /// `127.0.0.1:0` in tests.
    pub bind: SocketAddr,
    pub identity: Identity,
    pub paired: Arc<dyn PairedCerts>,
    /// The pairing protocol behind `POST /v1/pair`.
    pub pairing: Arc<PairingState>,
    /// Where `POST /v1/call/{command}` runs commands.
    pub host: Arc<dyn CommandHost>,
    /// The RUNTIME version from `package_info()`, not `CARGO_PKG_VERSION`,
    /// for the reason `latest_release` gives.
    pub desktop_version: String,
    pub viewer_login: ViewerLookup,
    /// The event fan-out `/v1/events` subscribers hang off. Lives in
    /// `gate::Remote` across listener restarts.
    pub events: Arc<Hub>,
    /// `PairingState::subscribe_revocations()`. Each event stream
    /// `resubscribe`s so a revocation reaches every open stream.
    pub revocations: broadcast::Receiver<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ListenerError {
    #[error("could not listen on {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        source: std::io::Error,
    },
    #[error("could not build the TLS configuration: {0}")]
    Tls(#[from] TlsError),
}

/// The body of `GET /v1/hello`.
#[derive(Serialize, Debug)]
pub struct Hello {
    pub desktop_version: String,
    pub protocol_version: u32,
    pub viewer_login: Option<String>,
}

struct AppState {
    paired: Arc<dyn PairedCerts>,
    pairing: Arc<PairingState>,
    host: Arc<dyn CommandHost>,
    /// The desktop certificate's fingerprint, the second half of the
    /// pairing proof.
    server_fp: String,
    /// One replay window for the whole listener, as `stepup` asks.
    nonces: NonceWindow,
    desktop_version: String,
    viewer_login: ViewerLookup,
    viewer_cache: tokio::sync::OnceCell<String>,
    events: Arc<Hub>,
    revocations: broadcast::Receiver<String>,
}

/// A running listener. Dropping it stops accepting; [`Handle::stop`]
/// additionally waits for the accept loop to finish and every open
/// connection to be torn down, which is what "Allow phone connections:
/// off" should mean.
///
/// What `stop` cannot promise is that the kernel stops completing TCP
/// handshakes on the port the instant it returns. macOS has no
/// `SOCK_CLOEXEC`, so there is a window between `socket(2)` and the
/// `fcntl(2)` that marks the fd close-on-exec in which a child spawned
/// from another thread (this app shells out to git and gh constantly)
/// inherits the listening socket and keeps it open until it exits;
/// `shutdown(2)` on a listening socket is `ENOTCONN` there, so nothing
/// on this side can take it back. Such a connection is never served:
/// the accept loop is gone, so no handshake completes and the phone
/// sees a dead port rather than a desktop. The test
/// `stop_closes_the_port` asserts exactly that boundary.
pub struct Handle {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl Handle {
    /// Where the listener actually bound -- matters when `bind` asked
    /// for port 0.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stop accepting, drop every open connection, and wait for both.
    pub async fn stop(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        let _ = (&mut self.task).await;
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

/// The provider the listener uses, regardless of the process default.
///
/// aws-lc-rs, the only rustls provider with a post-quantum key
/// exchange, and since 0.23.44 the one that also loads ML-DSA keys and
/// verifies ML-DSA signatures. That is what signs the handshake with the
/// desktop's ML-DSA-65 key and verifies the phone's, so every TLS config
/// in this module is built on it.
///
/// This used to be `rustls-post-quantum::provider()`, back when rustls
/// only NAMED the ML-DSA schemes. Both halves moved into the plain
/// provider and the crate went away in #588.
///
/// X25519MLKEM768 placed FIRST explicitly, and that mattered more than
/// it looked. rustls orders it first only under its
/// `prefer-post-quantum` cargo feature, which the post-quantum crate
/// happened to turn on -- so removing that crate turned it off, and
/// this line is why nothing changed. Spelling the order out means the
/// preference does not depend on a feature flag anyone can drop by
/// accident; `key_exchange_is_hybrid_post_quantum` holds it.
fn provider() -> CryptoProvider {
    CryptoProvider {
        kx_groups: vec![
            aws_lc_rs::kx_group::X25519MLKEM768,
            aws_lc_rs::kx_group::X25519,
            aws_lc_rs::kx_group::SECP256R1,
        ],
        ..aws_lc_rs::default_provider()
    }
}

/// Accepts a client certificate by fingerprint alone, and only an
/// ML-DSA-65 one.
///
/// No chain building: the certificate is self-signed by the phone and
/// the desktop learned its fingerprint at pairing. The one thing this
/// still verifies cryptographically is the handshake signature -- that
/// the peer HOLDS the private key for the certificate it presented --
/// because without that a fingerprint is a public value anyone could
/// replay.
///
/// ML-DSA-65 and nothing else, in both directions: the scheme list
/// goes out in CertificateRequest, so a phone with a pre-protocol-2
/// P-256 session certificate finds nothing it can sign with and the
/// handshake ends there; and `verify_tls13_signature` refuses any other
/// scheme should a peer sign with one it was not offered. Refusing
/// P-256 outright rather than accepting both through a transition is
/// deliberate: every phone re-pairs for protocol 2 anyway, because the
/// desktop's own fingerprint changed with its certificate, so there is
/// no pairing a P-256 certificate could still belong to.
struct PairedVerifier {
    paired: Arc<dyn PairedCerts>,
    algs: WebPkiSupportedAlgorithms,
}

impl std::fmt::Debug for PairedVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairedVerifier").finish_non_exhaustive()
    }
}

impl ClientCertVerifier for PairedVerifier {
    /// Empty: sending no CA hints tells the phone "any certificate",
    /// which is right, since it has a self-signed one.
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, TlsError> {
        let fp = fingerprint_of(end_entity.as_ref());
        if self.paired.is_paired(&fp) || self.paired.pairing_window_open() {
            Ok(ClientCertVerified::assertion())
        } else {
            Err(TlsError::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    /// Never reached: the config offers TLS 1.3 only. Refuse rather than
    /// accept, so a future config change cannot quietly widen this.
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, TlsError> {
        Err(TlsError::General("TLS 1.2 is not offered".into()))
    }

    /// ML-DSA-65 only. The provider's algorithm table maps that scheme
    /// to webpki's ML-DSA-65 verifier, which also checks that the
    /// certificate's public key is an ML-DSA-65 key -- so the scheme,
    /// the key and the signature all have to agree.
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, TlsError> {
        if dss.scheme != CLIENT_SIGNATURE_SCHEME {
            return Err(PeerMisbehaved::SignedHandshakeWithUnadvertisedSigScheme.into());
        }
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algs)
    }

    /// What CertificateRequest offers the phone.
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![CLIENT_SIGNATURE_SCHEME]
    }
}

/// TLS 1.3 only, the desktop's identity, client certificates required
/// and checked by [`PairedVerifier`], and no session resumption.
fn server_config(
    identity: &Identity,
    paired: Arc<dyn PairedCerts>,
) -> Result<ServerConfig, TlsError> {
    let provider = Arc::new(provider());
    let verifier = PairedVerifier {
        paired,
        algs: provider.signature_verification_algorithms,
    };
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(verifier))
        .with_single_cert(vec![identity.cert()], identity.key())?;
    // No resumption, ever. A resumed TLS 1.3 handshake restores the
    // client certificate from the ticket instead of asking for it, so
    // `PairedVerifier` never runs -- and a phone revoked after its
    // first connection would walk back in on the ticket it kept. The
    // spec's guarantee is "revoked means refused on the next
    // handshake", which holds only if every handshake is a full one.
    // Tickets are what a client offers; the session store is where
    // rustls keeps what a ticket points at. Zeroing the one and
    // emptying the other closes both halves, so a future change to
    // either cannot quietly reopen it.
    config.send_tls13_tickets = 0;
    config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
    Ok(config)
}

/// The path-level gate above TLS: an unpaired peer reaches `/v1/pair`
/// and nothing else. Re-checked per request so revocation takes effect
/// on the next request of an already-open connection.
async fn require_paired(State(st): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let Some(peer) = req.extensions().get::<Peer>() else {
        // The connection task always inserts one; its absence is a bug
        // in this file, not a client condition, and must not pass.
        log::error!("remote: a request arrived without a peer fingerprint");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if req.uri().path() == PAIR_PATH || st.paired.is_paired(&peer.fingerprint) {
        next.run(req).await
    } else {
        (StatusCode::FORBIDDEN, "not paired").into_response()
    }
}

async fn hello(State(st): State<Arc<AppState>>) -> Json<Hello> {
    let lookup = st.viewer_login.clone();
    let viewer_login = st
        .viewer_cache
        .get_or_try_init(|| async move { lookup().await.ok_or(()) })
        .await
        .ok()
        .cloned();
    Json(Hello {
        desktop_version: st.desktop_version.clone(),
        protocol_version: PROTOCOL_VERSION,
        viewer_login,
    })
}

/// `GET /v1/events`: the stream described in `remote/events.rs`.
async fn events(State(st): State<Arc<AppState>>, Extension(peer): Extension<Peer>) -> Response {
    let sub = events::Subscriber {
        fingerprint: peer.fingerprint,
        revocations: st.revocations.resubscribe(),
        paired: st.paired.clone(),
    };
    match events::subscribe(&st.events, sub).await {
        Some(sse) => sse.into_response(),
        None => (StatusCode::FORBIDDEN, "not paired").into_response(),
    }
}

/// A plain-text error response with the status an error type chose.
fn refusal(status: u16, message: String) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, message).into_response()
}

/// The request body as the JSON the command expects, or the 400 message
/// to send. Empty means "no arguments", which `surface::dispatch` reads
/// as `{}`.
fn parse_body(body: &Bytes) -> Result<Value, String> {
    if body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(body).map_err(|e| format!("request body is not JSON: {e}"))
}

/// `POST /v1/pair`: the handshake in `remote/pairing.rs`, for the
/// certificate this connection presented.
async fn pair(
    State(st): State<Arc<AppState>>,
    Extension(peer): Extension<Peer>,
    body: Bytes,
) -> Response {
    let req: PairRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => return refusal(400, format!("bad pair request: {e}")),
    };
    let client = PeerCert {
        fingerprint: peer.fingerprint,
        der: peer.der,
    };
    match pairing::handle_pair(&st.pairing, req, &client, &st.server_fp).await {
        Ok(outcome) => Json(outcome).into_response(),
        Err(e) => refusal(e.http_status(), e.to_string()),
    }
}

/// `POST /v1/call/{command}`: the contract in `remote/surface.rs`, with
/// the step-up check from `remote/stepup.rs` in front of a destructive
/// command.
async fn call(
    State(st): State<Arc<AppState>>,
    Extension(peer): Extension<Peer>,
    Path(command): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // The gate admitted the fingerprint; the row is what the signature
    // check and the log line need. Gone between the two means revoked.
    let Some(device) = st.paired.device(&peer.fingerprint) else {
        return refusal(403, "not paired".into());
    };
    let class = match surface::class_of(&command) {
        None => return refusal_for(RemoteError::Unknown(command)),
        Some(Class::Local) => return refusal_for(RemoteError::Local(command)),
        Some(class) => class,
    };
    let args = match parse_body(&body) {
        Ok(args) => args,
        Err(message) => return refusal(400, message),
    };
    if class == Class::Destructive {
        // A header that is not even ASCII is a header the phone built
        // wrong, not a missing one; `""` parses as malformed.
        let header = headers
            .get(stepup::HEADER)
            .map(|v| v.to_str().unwrap_or(""));
        let now = chrono::Utc::now().timestamp();
        if let Err(e) = stepup::verify(&device, &command, &args, header, now, &st.nonces) {
            log::warn!(
                "remote: refused a destructive {command} from {}: {e}",
                device.name
            );
            return refusal(e.http_status(), e.to_string());
        }
    }
    let result = st.host.dispatch(&command, args, &device.name).await;
    if class == Class::Destructive {
        // The attempt is the news, whether or not it succeeded.
        st.host.notify_destructive(&device.name, &command);
    }
    match result {
        Ok(value) => Json(value).into_response(),
        Err(e) => refusal_for(e),
    }
}

/// The wire form of a refusal.
///
/// A `Command` rejection travels as JSON carrying its classified kind
/// beside the message (#1202); every other variant stays plain text.
///
/// # Why this does not break an already-paired phone
///
/// `client.rs`'s `status_message` reads a JSON body's `error` or
/// `message` key BEFORE falling back to the raw body, and has done
/// since before this change -- see its test at `client.rs:954`. So a
/// companion built against the old plain-text format extracts
/// `message`, ignores `kind` it does not know, and shows exactly the
/// string it showed before. The forward compatibility was already
/// there; this relies on it rather than adding a version gate.
///
/// The other three variants are the listener's own refusals, not a
/// command's, and they have consumers that expect text. They are left
/// alone: this increment changes one path, so there is no half-migrated
/// state to reason about.
fn refusal_for(e: RemoteError) -> Response {
    let status = e.http_status();
    match e {
        RemoteError::Command(err) => {
            let code = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            (code, Json(err)).into_response()
        }
        other => refusal(status, other.to_string()),
    }
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/hello", get(hello))
        .route(events::PATH, get(events))
        .route(PAIR_PATH, post(pair))
        .route(&format!("{CALL_PREFIX}{{command}}"), post(call))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_paired,
        ))
        .with_state(state)
}

/// A bound, listening socket at `addr`.
///
/// Not `TcpListener::bind`, because an IPv6 wildcard bound that way is
/// IPv6-only on Windows (`IPV6_V6ONLY` defaults on there, off on Linux
/// and macOS), and the QR lists the machine's IPv4 addresses too. The
/// option is cleared explicitly so one socket serves both families on
/// all three platforms. `SO_REUSEADDR` matches what tokio sets on unix,
/// so toggling the feature off and on does not wait out `TIME_WAIT`;
/// it is not set on Windows, where it means something else.
fn bind_socket(addr: SocketAddr) -> std::io::Result<std::net::TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let domain = if addr.is_ipv6() {
        Domain::IPV6
    } else {
        Domain::IPV4
    };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    if addr.is_ipv6() {
        socket.set_only_v6(false)?;
    }
    #[cfg(not(windows))]
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(128)?;
    Ok(socket.into())
}

/// `bind_socket`, with the IPv4 wildcard as the fallback when the IPv6
/// wildcard cannot be bound at all -- a kernel booted with IPv6 off.
/// A port in use fails the same way on both and is reported as such.
fn bind(addr: SocketAddr) -> std::io::Result<std::net::TcpListener> {
    match bind_socket(addr) {
        Ok(listener) => Ok(listener),
        Err(e) if addr.is_ipv6() && addr.ip().is_unspecified() => {
            let v4 = SocketAddr::from((Ipv4Addr::UNSPECIFIED, addr.port()));
            log::warn!("remote: could not listen on {addr} ({e}); trying {v4}");
            bind_socket(v4)
        }
        Err(e) => Err(e),
    }
}

/// Bind, then serve on a spawned task until the handle is stopped.
///
/// Binding happens here rather than on the task so a port already in
/// use is reported to the caller -- and to the Settings toggle -- as an
/// error, not logged from a task nobody is watching.
pub async fn start(cfg: ListenerConfig) -> Result<Handle, ListenerError> {
    let tls = server_config(&cfg.identity, cfg.paired.clone())?;
    let bind_err = |source| ListenerError::Bind {
        addr: cfg.bind,
        source,
    };
    let listener = bind(cfg.bind)
        .and_then(TcpListener::from_std)
        .map_err(bind_err)?;
    let addr = listener.local_addr().map_err(bind_err)?;

    let revocations = cfg.revocations.resubscribe();
    let state = Arc::new(AppState {
        paired: cfg.paired,
        pairing: cfg.pairing,
        host: cfg.host,
        server_fp: cfg.identity.fingerprint(),
        nonces: NonceWindow::new(),
        desktop_version: cfg.desktop_version,
        viewer_login: cfg.viewer_login,
        viewer_cache: tokio::sync::OnceCell::new(),
        events: cfg.events,
        revocations: cfg.revocations,
    });
    let paired = state.paired.clone();
    let app = router(state);
    let acceptor = TlsAcceptor::from(Arc::new(tls));
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let task = tokio::spawn(accept_loop(
        listener,
        acceptor,
        app,
        paired,
        revocations,
        shutdown_rx,
    ));

    log::info!("remote: listening on {addr}");
    Ok(Handle {
        addr,
        shutdown: Some(shutdown_tx),
        task,
    })
}

async fn accept_loop(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    app: Router,
    paired: Arc<dyn PairedCerts>,
    revocations: broadcast::Receiver<String>,
    mut shutdown: oneshot::Receiver<()>,
) {
    let mut conns: JoinSet<()> = JoinSet::new();
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => match accepted {
                Ok((tcp, from)) => {
                    conns.spawn(serve_connection(
                        tcp,
                        from,
                        acceptor.clone(),
                        app.clone(),
                        paired.clone(),
                        revocations.resubscribe(),
                    ));
                }
                Err(e) => {
                    // Out of file descriptors, most likely. Do not spin.
                    log::warn!("remote: accept failed: {e}");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            },
        }
        // Reap finished connections so the set does not grow without
        // bound over a long session.
        while conns.try_join_next().is_some() {}
    }
    // Abort every open connection: "off" means off, not "no new ones".
    conns.shutdown().await;
    log::info!("remote: listener stopped");
}

/// One connection: handshake, then HTTP/1.1 until it closes -- or until
/// the certificate it presented is revoked.
async fn serve_connection(
    tcp: TcpStream,
    from: SocketAddr,
    acceptor: TlsAcceptor,
    app: Router,
    paired: Arc<dyn PairedCerts>,
    mut revocations: broadcast::Receiver<String>,
) {
    let tls = match acceptor.accept(tcp).await {
        Ok(tls) => tls,
        Err(e) => {
            // The expected outcome for an unpaired device, and the
            // signal a user wants to see when "something keeps trying".
            log::info!("remote: refused a connection from {from}: {e}");
            return;
        }
    };
    // The verifier has already approved this certificate; here it is
    // only named. Client auth is mandatory, so a handshake that
    // completed presented one.
    let Some(der) = tls
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|chain| chain.first())
        .map(|cert| cert.as_ref().to_vec())
    else {
        log::error!("remote: a handshake completed without a client certificate");
        return;
    };
    let fp = fingerprint_of(&der);
    let peer = Peer {
        fingerprint: fp.clone(),
        der,
    };

    let service = hyper::service::service_fn(move |mut req: Request<hyper::body::Incoming>| {
        req.extensions_mut().insert(peer.clone());
        let mut app = app.clone();
        async move { tower_service::Service::call(&mut app, req).await }
    });
    let conn =
        hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(tls), service);
    tokio::pin!(conn);
    let result = tokio::select! {
        result = &mut conn => result,
        _ = revoked(&mut revocations, &fp, paired.as_ref()) => {
            log::info!("remote: closing the connection from {from}: its device was revoked");
            // Graceful: an in-flight response completes and no further
            // request is served. The event stream ends itself on the
            // same broadcast, so "in flight" is bounded -- and the
            // deadline is for anything that is not.
            conn.as_mut().graceful_shutdown();
            match tokio::time::timeout(REVOKE_GRACE, &mut conn).await {
                Ok(result) => result,
                Err(_) => {
                    log::info!("remote: dropped the connection from {from} after the grace period");
                    return;
                }
            }
        }
    };
    if let Err(e) = result {
        // A phone going out of range closes mid-request; that is
        // ordinary and not worth a warning.
        log::debug!("remote: connection from {from} ended: {e}");
    }
}

/// Resolves when `fp` is revoked. A lagged broadcast may have carried
/// this device's revocation, so it falls back to asking; a closed one
/// means the pairing state is gone and nothing can revoke anymore, and
/// the connection is left to the per-request gate.
async fn revoked(rx: &mut broadcast::Receiver<String>, fp: &str, paired: &dyn PairedCerts) {
    loop {
        match rx.recv().await {
            Ok(revoked) if revoked == fp => return,
            Ok(_) => {}
            Err(RecvError::Lagged(_)) => {
                if !paired.is_paired(fp) {
                    return;
                }
            }
            Err(RecvError::Closed) => std::future::pending::<()>().await,
        }
    }
}

/// `pub(crate)`: `remote/events.rs` drives its stream tests through the
/// same in-memory pairing store and pinned client as the tests here.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::remote::events::SnapshotSource;
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::ServerName;
    use rustls::{ClientConfig, NamedGroup};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::client::TlsStream;

    /// The in-memory `PairedCerts` the spec's verifier tests run against.
    #[derive(Default)]
    pub(crate) struct MemoryCerts {
        paired: Mutex<HashMap<String, PairedDevice>>,
        window: AtomicBool,
    }

    /// A row for a device the tests only ever identify by fingerprint.
    /// The keys are placeholders; a test that verifies signatures pairs
    /// a real row with `pair_device`.
    pub(crate) fn placeholder_device(fp: &str) -> PairedDevice {
        PairedDevice {
            id: 1,
            name: "Octocat's phone".into(),
            cert_fp: fp.to_string(),
            cert_der: vec![0x30],
            ecdsa_pubkey: vec![0x04; 65],
            mldsa_pubkey: None,
            paired_at: "2026-09-05T00:00:00Z".into(),
            last_seen: None,
        }
    }

    impl MemoryCerts {
        pub(crate) fn pair(&self, fp: &str) {
            self.pair_device(placeholder_device(fp));
        }
        pub(crate) fn pair_device(&self, device: PairedDevice) {
            self.paired
                .lock()
                .unwrap()
                .insert(device.cert_fp.clone(), device);
        }
        pub(crate) fn revoke(&self, fp: &str) {
            self.paired.lock().unwrap().remove(fp);
        }
        fn open_window(&self, open: bool) {
            self.window.store(open, Ordering::SeqCst);
        }
    }

    impl PairedCerts for MemoryCerts {
        fn is_paired(&self, fp: &str) -> bool {
            self.paired.lock().unwrap().contains_key(fp)
        }
        fn pairing_window_open(&self) -> bool {
            self.window.load(Ordering::SeqCst)
        }
        fn device(&self, fp: &str) -> Option<PairedDevice> {
            self.paired.lock().unwrap().get(fp).cloned()
        }
    }

    /// A `CommandHost` that runs nothing and remembers everything: what
    /// it was asked to dispatch, and which destructive calls it was told
    /// to announce. Answers `{"ran": <command>}` unless told to fail.
    #[derive(Default)]
    pub(crate) struct RecordingHost {
        pub(crate) calls: Mutex<Vec<(String, Value, String)>>,
        pub(crate) notices: Mutex<Vec<(String, String)>>,
        pub(crate) fail_with: Mutex<Option<RemoteError>>,
    }

    impl CommandHost for RecordingHost {
        fn dispatch<'a>(
            &'a self,
            command: &'a str,
            args: Value,
            device_name: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Value, RemoteError>> + Send + 'a>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push((
                    command.to_string(),
                    args,
                    device_name.to_string(),
                ));
                match self.fail_with.lock().unwrap().clone() {
                    Some(e) => Err(e),
                    None => Ok(serde_json::json!({ "ran": command })),
                }
            })
        }
        fn notify_destructive(&self, device_name: &str, command: &str) {
            self.notices
                .lock()
                .unwrap()
                .push((device_name.to_string(), command.to_string()));
        }
    }

    /// The signature scheme the phone-side verifier saw the desktop
    /// sign CertificateVerify with, recorded so a test can assert the
    /// handshake used ML-DSA-65 rather than infer it.
    pub(crate) type SchemeSeen = Arc<Mutex<Option<SignatureScheme>>>;

    /// The phone's side of the pin: accept the server certificate whose
    /// fingerprint matches, verify it holds the key -- with ML-DSA-65
    /// and nothing else -- refuse anything else. Mirrors what
    /// `src-mobile/src/client.rs` does.
    #[derive(Debug)]
    struct PinnedServer {
        fp: String,
        algs: WebPkiSupportedAlgorithms,
        seen: SchemeSeen,
    }

    impl ServerCertVerifier for PinnedServer {
        fn verify_server_cert(
            &self,
            end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, TlsError> {
            if fingerprint_of(end_entity.as_ref()) == self.fp {
                Ok(ServerCertVerified::assertion())
            } else {
                Err(TlsError::InvalidCertificate(
                    CertificateError::ApplicationVerificationFailure,
                ))
            }
        }
        fn verify_tls12_signature(
            &self,
            _m: &[u8],
            _c: &CertificateDer<'_>,
            _d: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, TlsError> {
            Err(TlsError::General("TLS 1.2 is not offered".into()))
        }
        fn verify_tls13_signature(
            &self,
            m: &[u8],
            c: &CertificateDer<'_>,
            d: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, TlsError> {
            if d.scheme != SignatureScheme::ML_DSA_65 {
                return Err(PeerMisbehaved::SignedHandshakeWithUnadvertisedSigScheme.into());
            }
            *self.seen.lock().unwrap() = Some(d.scheme);
            rustls::crypto::verify_tls13_signature(m, c, d, &self.algs)
        }
        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            vec![SignatureScheme::ML_DSA_65]
        }
    }

    pub(crate) struct Server {
        pub(crate) handle: Handle,
        pub(crate) fp: String,
        pub(crate) certs: Arc<MemoryCerts>,
        /// The pairing task's side of the revocation broadcast.
        pub(crate) revocations: broadcast::Sender<String>,
        pub(crate) host: Arc<RecordingHost>,
    }

    /// A hub with nothing to snapshot, for tests that never open the
    /// event stream.
    pub(crate) fn no_snapshot() -> SnapshotSource {
        Arc::new(|| Box::pin(async { None }))
    }

    /// A pairing state whose modal nobody answers, for tests that never
    /// post to `/v1/pair`.
    pub(crate) fn idle_pairing() -> Arc<PairingState> {
        Arc::new(PairingState::new(|_| {}))
    }

    async fn serve(certs: Arc<MemoryCerts>) -> Server {
        serve_with(certs, Arc::new(Hub::new(no_snapshot()))).await
    }

    pub(crate) async fn serve_with(certs: Arc<MemoryCerts>, events: Arc<Hub>) -> Server {
        serve_at("127.0.0.1:0".parse().unwrap(), certs, events).await
    }

    pub(crate) async fn serve_at(
        bind: SocketAddr,
        certs: Arc<MemoryCerts>,
        events: Arc<Hub>,
    ) -> Server {
        let identity = Identity::generate().unwrap();
        let fp = identity.fingerprint();
        let (revocations, rx) = broadcast::channel(16);
        let host = Arc::new(RecordingHost::default());
        let handle = start(ListenerConfig {
            bind,
            identity,
            paired: certs.clone(),
            pairing: idle_pairing(),
            host: host.clone(),
            desktop_version: "9.9.9".into(),
            viewer_login: Arc::new(|| Box::pin(async { Some("octocat".to_string()) })),
            events,
            revocations: rx,
        })
        .await
        .unwrap();
        Server {
            handle,
            fp,
            certs,
            revocations,
            host,
        }
    }

    pub(crate) fn client_config(phone: Option<&Identity>, server_fp: &str) -> Arc<ClientConfig> {
        pinned_client(phone, server_fp).0
    }

    /// `client_config`, plus the handle its verifier records the
    /// desktop's signature scheme into.
    pub(crate) fn pinned_client(
        phone: Option<&Identity>,
        server_fp: &str,
    ) -> (Arc<ClientConfig>, SchemeSeen) {
        let provider = Arc::new(provider());
        let seen: SchemeSeen = Arc::default();
        let verifier = PinnedServer {
            fp: server_fp.to_string(),
            algs: provider.signature_verification_algorithms,
            seen: seen.clone(),
        };
        let builder = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier));
        let cfg = match phone {
            Some(id) => builder
                .with_client_auth_cert(vec![id.cert()], id.key())
                .unwrap(),
            None => builder.with_no_client_auth(),
        };
        (Arc::new(cfg), seen)
    }

    pub(crate) struct Reply {
        pub(crate) status: u16,
        pub(crate) body: String,
        pub(crate) kx: Option<NamedGroup>,
    }

    /// A fresh mTLS connection to the listener, handshake complete.
    pub(crate) async fn connect(
        addr: SocketAddr,
        phone: Option<&Identity>,
        server_fp: &str,
    ) -> Result<TlsStream<TcpStream>, String> {
        let tcp = TcpStream::connect(addr).await.map_err(|e| e.to_string())?;
        handshake(tcp, phone, server_fp).await
    }

    /// The mTLS handshake over a TCP connection already made, on a
    /// fresh client config -- so it never carries a ticket from an
    /// earlier connection.
    async fn handshake(
        tcp: TcpStream,
        phone: Option<&Identity>,
        server_fp: &str,
    ) -> Result<TlsStream<TcpStream>, String> {
        handshake_with(tcp, client_config(phone, server_fp)).await
    }

    /// The handshake on a config the caller keeps: rustls's client
    /// resumption is on by default, so a config reused across
    /// connections offers whatever ticket the previous one earned.
    async fn handshake_with(
        tcp: TcpStream,
        cfg: Arc<ClientConfig>,
    ) -> Result<TlsStream<TcpStream>, String> {
        let connector = tokio_rustls::TlsConnector::from(cfg);
        let name = ServerName::try_from("localhost").unwrap();
        connector
            .connect(name, tcp)
            .await
            .map_err(|e| format!("handshake: {e}"))
    }

    /// One HTTP/1.1 GET over a fresh mTLS connection; see `request`.
    async fn get(
        addr: SocketAddr,
        path: &str,
        phone: Option<&Identity>,
        server_fp: &str,
    ) -> Result<Reply, String> {
        request(addr, phone, server_fp, "GET", path, &[], None).await
    }

    /// One HTTP/1.1 request over a fresh mTLS connection. `Err` for any
    /// failure at any layer -- a refused handshake shows up as the
    /// server's alert on the first read, after the client believed its
    /// half of the TLS 1.3 handshake was complete.
    pub(crate) async fn request(
        addr: SocketAddr,
        phone: Option<&Identity>,
        server_fp: &str,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> Result<Reply, String> {
        let mut tls = connect(addr, phone, server_fp).await?;
        let mut head =
            format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        let body = body.unwrap_or("");
        head.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        tls.write_all(head.as_bytes())
            .await
            .map_err(|e| format!("write: {e}"))?;
        let mut raw = Vec::new();
        // The server closes after the response (Connection: close); a
        // refusal closes without one.
        let _ = tls.read_to_end(&mut raw).await;
        let text = String::from_utf8_lossy(&raw).to_string();
        let (head, body) = text
            .split_once("\r\n\r\n")
            .ok_or_else(|| format!("no response (got {} bytes)", raw.len()))?;
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| format!("bad status line: {head}"))?;
        let kx = tls
            .get_ref()
            .1
            .negotiated_key_exchange_group()
            .map(|g| g.name());
        Ok(Reply {
            status,
            body: body.to_string(),
            kx,
        })
    }

    #[tokio::test]
    async fn a_paired_phone_gets_hello() {
        let certs = Arc::new(MemoryCerts::default());
        let server = serve(certs).await;
        let phone = Identity::generate().unwrap();
        server.certs.pair(&phone.fingerprint());

        let reply = get(
            server.handle.local_addr(),
            "/v1/hello",
            Some(&phone),
            &server.fp,
        )
        .await
        .unwrap();
        assert_eq!(reply.status, 200);
        let v: serde_json::Value = serde_json::from_str(&reply.body).unwrap();
        assert_eq!(v["desktop_version"], "9.9.9");
        assert_eq!(v["protocol_version"], 2);
        assert_eq!(v["viewer_login"], "octocat");
        server.handle.stop().await;
    }

    #[tokio::test]
    async fn an_unpaired_phone_fails_the_handshake() {
        let certs = Arc::new(MemoryCerts::default());
        let server = serve(certs).await;
        let phone = Identity::generate().unwrap();

        let reply = get(
            server.handle.local_addr(),
            "/v1/hello",
            Some(&phone),
            &server.fp,
        )
        .await;
        assert!(
            reply.is_err(),
            "an unpaired certificate must not reach HTTP"
        );
        server.handle.stop().await;
    }

    #[tokio::test]
    async fn a_phone_without_a_certificate_is_refused() {
        let certs = Arc::new(MemoryCerts::default());
        let server = serve(certs).await;
        server.certs.open_window(true);

        let reply = get(server.handle.local_addr(), PAIR_PATH, None, &server.fp).await;
        assert!(
            reply.is_err(),
            "client auth is mandatory even while pairing"
        );
        server.handle.stop().await;
    }

    #[tokio::test]
    async fn a_revoked_phone_is_refused_on_its_next_handshake() {
        let certs = Arc::new(MemoryCerts::default());
        let server = serve(certs).await;
        let phone = Identity::generate().unwrap();
        server.certs.pair(&phone.fingerprint());
        let addr = server.handle.local_addr();

        assert_eq!(
            get(addr, "/v1/hello", Some(&phone), &server.fp)
                .await
                .unwrap()
                .status,
            200
        );
        server.certs.revoke(&phone.fingerprint());
        assert!(get(addr, "/v1/hello", Some(&phone), &server.fp)
            .await
            .is_err());
        // And a different, still-paired phone is unaffected.
        let other = Identity::generate().unwrap();
        server.certs.pair(&other.fingerprint());
        assert_eq!(
            get(addr, "/v1/hello", Some(&other), &server.fp)
                .await
                .unwrap()
                .status,
            200
        );
        server.handle.stop().await;
    }

    /// One HTTP/1.1 GET of `/v1/hello` on a connection the caller made,
    /// reading to the close -- which also drains the session tickets a
    /// TLS 1.3 server sends right after the handshake, so the client's
    /// store holds them for its next connection.
    async fn hello_on(tls: &mut TlsStream<TcpStream>) -> Result<u16, String> {
        tls.write_all(b"GET /v1/hello HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .map_err(|e| format!("write: {e}"))?;
        let mut raw = Vec::new();
        let _ = tls.read_to_end(&mut raw).await;
        let text = String::from_utf8_lossy(&raw);
        text.split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| format!("no response (got {} bytes)", raw.len()))
    }

    /// The desktop never resumes a session: a client that reuses its
    /// config (and so its ticket store) still gets a full handshake,
    /// which is the only kind that runs the certificate verifier.
    #[tokio::test]
    async fn the_listener_never_resumes_a_session() {
        let certs = Arc::new(MemoryCerts::default());
        let server = serve(certs).await;
        let phone = Identity::generate().unwrap();
        server.certs.pair(&phone.fingerprint());
        let addr = server.handle.local_addr();
        let cfg = client_config(Some(&phone), &server.fp);

        for attempt in 1..=2 {
            let tcp = TcpStream::connect(addr).await.unwrap();
            let mut tls = handshake_with(tcp, cfg.clone()).await.unwrap();
            assert_eq!(hello_on(&mut tls).await.unwrap(), 200);
            assert_eq!(
                tls.get_ref().1.handshake_kind(),
                Some(rustls::HandshakeKind::Full),
                "connection {attempt} must be a full handshake"
            );
        }
        server.handle.stop().await;
    }

    /// The attack: a phone that was paired keeps the TLS 1.3 ticket its
    /// first connection earned; after revocation it offers that ticket,
    /// and a server that accepted it would skip the certificate
    /// exchange -- and `PairedVerifier` -- entirely. Revocation must
    /// bite on the next handshake whether or not a ticket is offered.
    #[tokio::test]
    async fn a_revoked_phone_cannot_resume_its_earlier_session() {
        let certs = Arc::new(MemoryCerts::default());
        let server = serve(certs).await;
        let phone = Identity::generate().unwrap();
        server.certs.pair(&phone.fingerprint());
        let addr = server.handle.local_addr();
        // One config for both connections: its session store keeps the
        // ticket from the first, and the second offers it.
        let cfg = client_config(Some(&phone), &server.fp);

        let tcp = TcpStream::connect(addr).await.unwrap();
        let mut tls = handshake_with(tcp, cfg.clone()).await.unwrap();
        assert_eq!(hello_on(&mut tls).await.unwrap(), 200);

        server.certs.revoke(&phone.fingerprint());

        let tcp = TcpStream::connect(addr).await.unwrap();
        let reply = match handshake_with(tcp, cfg).await {
            // A refusal surfaces on the first read: the client believes
            // its half of the TLS 1.3 handshake is complete.
            Ok(mut tls) => hello_on(&mut tls).await,
            Err(e) => Err(e),
        };
        assert!(
            reply.is_err(),
            "a revoked certificate must not reach HTTP by resuming: got {reply:?}"
        );
        server.handle.stop().await;
    }

    /// While a pairing window is open the handshake admits an unpaired
    /// certificate -- and the path gate then allows it `/v1/pair` only.
    /// 405 rather than 403 on a GET of the pair path proves it got past
    /// the gate and reached the (POST-only) handler.
    #[tokio::test]
    async fn an_open_pairing_window_admits_an_unpaired_phone_to_pair_and_nothing_else() {
        let certs = Arc::new(MemoryCerts::default());
        let server = serve(certs).await;
        let phone = Identity::generate().unwrap();
        server.certs.open_window(true);
        let addr = server.handle.local_addr();

        let hello = get(addr, "/v1/hello", Some(&phone), &server.fp)
            .await
            .unwrap();
        assert_eq!(hello.status, 403);
        let call = request(
            addr,
            Some(&phone),
            &server.fp,
            "POST",
            "/v1/call/get_cached",
            &[],
            None,
        )
        .await
        .unwrap();
        assert_eq!(call.status, 403);
        assert!(server.host.calls.lock().unwrap().is_empty());

        let pair = get(addr, PAIR_PATH, Some(&phone), &server.fp)
            .await
            .unwrap();
        assert_eq!(pair.status, 405);

        // Window closed again: back to failing the handshake.
        server.certs.open_window(false);
        assert!(get(addr, "/v1/hello", Some(&phone), &server.fp)
            .await
            .is_err());
        server.handle.stop().await;
    }

    /// The spec's reason for aws-lc-rs: PR titles recorded off the wire
    /// today must stay private, so the key exchange is hybrid
    /// post-quantum. Held here explicitly rather than via a rustls
    /// feature flag; see `provider()`.
    #[tokio::test]
    async fn key_exchange_is_hybrid_post_quantum() {
        let certs = Arc::new(MemoryCerts::default());
        let server = serve(certs).await;
        let phone = Identity::generate().unwrap();
        server.certs.pair(&phone.fingerprint());

        let reply = get(
            server.handle.local_addr(),
            "/v1/hello",
            Some(&phone),
            &server.fp,
        )
        .await
        .unwrap();
        assert_eq!(reply.kx, Some(NamedGroup::X25519MLKEM768));
        server.handle.stop().await;
    }

    /// ML-DSA-65 on both sides, observed rather than assumed. The
    /// phone-side verifier records the scheme the desktop signed
    /// CertificateVerify with and the leaf it signed under; the desktop's
    /// verifier admits no scheme but ML-DSA-65, so a completed handshake
    /// is itself the proof that the phone's CertificateVerify was
    /// ML-DSA-65 under its ML-DSA-65 certificate.
    #[tokio::test]
    async fn the_handshake_signs_with_ml_dsa_65_on_both_sides() {
        use crate::remote::identity::testing::is_ml_dsa_65_certificate;
        let certs = Arc::new(MemoryCerts::default());
        let server = serve(certs).await;
        let phone = Identity::generate().unwrap();
        server.certs.pair(&phone.fingerprint());

        let (cfg, seen) = pinned_client(Some(&phone), &server.fp);
        let tcp = TcpStream::connect(server.handle.local_addr())
            .await
            .unwrap();
        let mut tls = handshake_with(tcp, cfg).await.unwrap();
        assert_eq!(hello_on(&mut tls).await.unwrap(), 200);

        assert_eq!(*seen.lock().unwrap(), Some(SignatureScheme::ML_DSA_65));
        let leaf = tls.get_ref().1.peer_certificates().unwrap()[0].clone();
        assert_eq!(fingerprint_of(leaf.as_ref()), server.fp);
        assert!(is_ml_dsa_65_certificate(leaf.as_ref()));
        assert!(is_ml_dsa_65_certificate(phone.cert().as_ref()));
        server.handle.stop().await;
    }

    /// A phone from before protocol 2 presents an ECDSA P-256 session
    /// certificate. Even paired by fingerprint, it never reaches HTTP:
    /// the desktop offers no scheme it can sign with. The listener is
    /// not broken for anyone else while it tries.
    #[tokio::test]
    async fn a_p256_client_certificate_is_refused_even_when_paired() {
        let certs = Arc::new(MemoryCerts::default());
        let server = serve(certs).await;
        let addr = server.handle.local_addr();
        let old_phone = Identity::p256_for_tests();
        server.certs.pair(&old_phone.fingerprint());
        server.certs.open_window(true);

        assert!(
            get(addr, "/v1/hello", Some(&old_phone), &server.fp)
                .await
                .is_err(),
            "a P-256 client certificate must fail the handshake"
        );
        assert!(
            get(addr, PAIR_PATH, Some(&old_phone), &server.fp)
                .await
                .is_err(),
            "not even to pair: the window does not widen the scheme"
        );

        let phone = Identity::generate().unwrap();
        server.certs.pair(&phone.fingerprint());
        assert_eq!(
            get(addr, "/v1/hello", Some(&phone), &server.fp)
                .await
                .unwrap()
                .status,
            200
        );
        server.handle.stop().await;
    }

    /// What CertificateRequest advertises, and what the verifier will
    /// check: one scheme.
    #[test]
    fn the_verifier_offers_ml_dsa_65_and_nothing_else() {
        let provider = provider();
        let verifier = PairedVerifier {
            paired: Arc::new(MemoryCerts::default()),
            algs: provider.signature_verification_algorithms,
        };
        assert_eq!(
            verifier.supported_verify_schemes(),
            vec![SignatureScheme::ML_DSA_65]
        );
        // And the provider it delegates to can verify that scheme --
        // the plain aws-lc-rs one cannot, which is the whole reason for
        // the post-quantum provider.
        assert!(provider
            .signature_verification_algorithms
            .supported_schemes()
            .contains(&SignatureScheme::ML_DSA_65));
        // NOT asserted: that plain aws-lc-rs cannot verify ML-DSA-65.
        // It could not when this was written, which is why the
        // post-quantum provider is here at all; as of rustls 0.23.44 it
        // can. Pinning a third party's LACK of a feature makes its
        // arrival a red build, which is what happened. What matters is
        // the line above -- the provider this listener uses does verify
        // the scheme -- and that is still asserted. Whether the
        // post-quantum crate is still needed is #588.
    }

    /// The pin on the phone's side is real: a server presenting some
    /// other certificate is refused by the test client. Without this the
    /// tests above could pass against any server at all.
    #[tokio::test]
    async fn the_client_refuses_a_server_whose_fingerprint_does_not_match() {
        let certs = Arc::new(MemoryCerts::default());
        let server = serve(certs).await;
        let phone = Identity::generate().unwrap();
        server.certs.pair(&phone.fingerprint());
        let wrong_fp = Identity::generate().unwrap().fingerprint();

        let reply = get(
            server.handle.local_addr(),
            "/v1/hello",
            Some(&phone),
            &wrong_fp,
        )
        .await;
        assert!(reply.is_err());
        server.handle.stop().await;
    }

    #[tokio::test]
    async fn hello_reports_no_login_when_not_signed_in() {
        let certs = Arc::new(MemoryCerts::default());
        let identity = Identity::generate().unwrap();
        let fp = identity.fingerprint();
        let handle = start(ListenerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            identity,
            paired: certs.clone(),
            pairing: idle_pairing(),
            host: Arc::new(RecordingHost::default()),
            desktop_version: "9.9.9".into(),
            viewer_login: Arc::new(|| Box::pin(async { None })),
            events: Arc::new(Hub::new(no_snapshot())),
            revocations: broadcast::channel(1).1,
        })
        .await
        .unwrap();
        let phone = Identity::generate().unwrap();
        certs.pair(&phone.fingerprint());

        let reply = get(handle.local_addr(), "/v1/hello", Some(&phone), &fp)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&reply.body).unwrap();
        assert!(v["viewer_login"].is_null());
        handle.stop().await;
    }

    /// Off means off: once `stop()` has returned, nothing answers on
    /// the port.
    ///
    /// A refused TCP connect is the usual outcome and the strong form
    /// of the check. It is not the only acceptable one. macOS has no
    /// `SOCK_CLOEXEC`, so between `socket(2)` and the `fcntl(2)` that
    /// sets `FD_CLOEXEC` a child spawned by another test thread (the
    /// packages and worktrees tests shell out) inherits the listening
    /// socket and the kernel keeps completing handshakes on it until
    /// that child exits -- `shutdown(2)` on a listening socket is
    /// `ENOTCONN` there, so `stop()` cannot take it back. Measured with
    /// four threads spawning `sleep 0.01` beside 3000 stop-then-connect
    /// rounds: 1, 5 and 0 leaked ports per run, and 0 of 3000 twice
    /// without the spawning. CI runs the suite with `--test-threads=8`
    /// and saw exactly this once in three runs (#537). What `stop()`
    /// does guarantee is that the accept loop is gone and this process
    /// holds no reference, and that is what the second half asserts: an
    /// orphaned socket has nobody to answer a ClientHello, whereas a
    /// listener `stop()` failed to shut down answers it in milliseconds.
    #[tokio::test]
    async fn stop_closes_the_port() {
        let certs = Arc::new(MemoryCerts::default());
        let phone = Identity::generate().unwrap();
        certs.pair(&phone.fingerprint());
        let server = serve(certs).await;
        let addr = server.handle.local_addr();
        connect(addr, Some(&phone), &server.fp)
            .await
            .expect("reachable before stop");

        server.handle.stop().await;
        let Ok(tcp) = TcpStream::connect(addr).await else {
            return; // closed: the port refuses outright
        };
        // The kernel completed the handshake, so something still holds
        // the socket. Only an inherited copy is allowed to: that one
        // never talks TLS back.
        let served = tokio::time::timeout(
            Duration::from_secs(2),
            handshake(tcp, Some(&phone), &server.fp),
        )
        .await;
        assert!(
            !matches!(served, Ok(Ok(_))),
            "the listener must be gone once the toggle is off, but it completed a handshake"
        );
    }

    #[tokio::test]
    async fn a_port_in_use_is_reported_not_logged() {
        let certs = Arc::new(MemoryCerts::default());
        let server = serve(certs.clone()).await;
        let err = start(ListenerConfig {
            bind: server.handle.local_addr(),
            identity: Identity::generate().unwrap(),
            paired: certs,
            pairing: idle_pairing(),
            host: Arc::new(RecordingHost::default()),
            desktop_version: "9.9.9".into(),
            viewer_login: Arc::new(|| Box::pin(async { None })),
            events: Arc::new(Hub::new(no_snapshot())),
            revocations: broadcast::channel(1).1,
        })
        .await
        .err()
        .expect("binding a taken port must fail");
        assert!(matches!(err, ListenerError::Bind { .. }));
        assert!(err.to_string().contains("could not listen on"));
        server.handle.stop().await;
    }

    /// Production binds the IPv6 wildcard, and the QR lists IPv4
    /// addresses: a phone on IPv4 must reach it. On a runner with IPv6
    /// off the fallback to `0.0.0.0` makes the same test pass, which is
    /// the property wanted either way.
    ///
    /// The port is retried when it turns out to be shared. With
    /// `SO_REUSEADDR` set, a BSD kernel hands a wildcard `bind(0)` any
    /// port no other *wildcard* socket holds, so `[::]:0` can land on a
    /// port some other process is listening on at `127.0.0.1` -- and
    /// that process, being the more specific match, gets the IPv4
    /// connect. It answers the ClientHello with whatever it speaks,
    /// which the pinned handshake reports as a corrupt message (seen
    /// once in 30 local runs of the suite, this desktop having seven
    /// such listeners). A refused connect is not retried: that is the
    /// bug this test exists to catch.
    ///
    /// # Gated (#853)
    ///
    /// The retry above is a mitigation, not a fix, and the doc already
    /// records the failure rate that proves it: one in 30 local runs on a
    /// desktop with seven loopback listeners. Five retries lower the odds,
    /// they do not bound them -- all five can lose on a host that has many
    /// -- and CI runs eight test threads each binding ports, so the suite
    /// is itself a source of the contention. #853 calls this the strongest
    /// candidate after #834 for that reason.
    ///
    /// Gated rather than fixed because the collision is not in this
    /// code's gift to avoid. It is a kernel behaviour: with
    /// `SO_REUSEADDR`, a wildcard `bind(0)` may be handed a port that a
    /// more-specific `127.0.0.1` socket already holds, and which process
    /// wins the IPv4 connect is then decided by the host's listener set.
    /// No assertion this test could make about its own sockets changes
    /// that, and an in-test retry is already present and already
    /// insufficient -- adding more would be the #811 mistake (a retry
    /// that hides a real breakage is worse than the flake).
    ///
    /// The property is still worth checking on demand: the IPv4
    /// reachability of the IPv6 wildcard is what lets a phone reach the
    /// desktop from the QR, and `loopback_tests.rs` exercises the
    /// pairing path over loopback on every run, so the handshake itself
    /// is not left uncovered.
    ///
    /// Run with: `HEADSTATE_LIVE_MEASUREMENT=1 cargo test -- --ignored`
    #[tokio::test]
    #[ignore]
    async fn the_ipv6_wildcard_answers_on_ipv4_too() {
        if std::env::var("HEADSTATE_LIVE_MEASUREMENT").is_err() {
            println!("set HEADSTATE_LIVE_MEASUREMENT=1 to run this host-dependent port test");
            return;
        }
        let phone = Identity::generate().unwrap();
        let mut last = String::new();
        for _ in 0..5 {
            let certs = Arc::new(MemoryCerts::default());
            certs.pair(&phone.fingerprint());
            let server = serve_at(
                "[::]:0".parse().unwrap(),
                certs,
                Arc::new(Hub::new(no_snapshot())),
            )
            .await;
            let port = server.handle.local_addr().port();
            let v4 = SocketAddr::from(([127, 0, 0, 1], port));
            let reply = get(v4, "/v1/hello", Some(&phone), &server.fp).await;
            server.handle.stop().await;
            match reply {
                Ok(reply) => {
                    assert_eq!(reply.status, 200);
                    return;
                }
                Err(e) if e.starts_with("handshake:") => last = e,
                Err(e) => panic!("reachable over IPv4 loopback: {e}"),
            }
        }
        panic!("five wildcard ports in a row answered as somebody else: {last}");
    }

    /// Revocation closes what the device already has open, not only
    /// what it opens next: an idle keep-alive connection is shut by the
    /// desktop when the broadcast names its certificate.
    #[tokio::test]
    async fn revoking_a_device_closes_its_idle_connection() {
        let certs = Arc::new(MemoryCerts::default());
        let server = serve(certs).await;
        let phone = Identity::generate().unwrap();
        server.certs.pair(&phone.fingerprint());
        let other = Identity::generate().unwrap();
        server.certs.pair(&other.fingerprint());
        let addr = server.handle.local_addr();

        let open = |id: &Identity| {
            let fp = server.fp.clone();
            let id = id.clone();
            async move {
                let mut tls = connect(addr, Some(&id), &fp).await.unwrap();
                tls.write_all(b"GET /v1/hello HTTP/1.1\r\nHost: localhost\r\n\r\n")
                    .await
                    .unwrap();
                let mut buf = vec![0u8; 4096];
                let n = tls.read(&mut buf).await.unwrap();
                assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
                tls
            }
        };
        let mut revoked = open(&phone).await;
        let mut kept = open(&other).await;

        server.certs.revoke(&phone.fingerprint());
        server.revocations.send(phone.fingerprint()).unwrap();

        let mut buf = [0u8; 16];
        let closed = tokio::time::timeout(Duration::from_secs(5), revoked.read(&mut buf)).await;
        assert!(
            matches!(closed, Ok(Ok(0)) | Ok(Err(_))),
            "the revoked device's connection must be closed: {closed:?}"
        );
        // The other device's connection still serves.
        kept.write_all(b"GET /v1/hello HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let n = kept.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
        server.handle.stop().await;
    }
}

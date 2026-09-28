//! A stand-in desktop for the loopback tests: TLS 1.3 on the same
//! provider as the real listener, a client-certificate verifier that
//! admits paired fingerprints (or anything while the pairing window is
//! open), the desktop's path gate above it, and just enough HTTP/1.1 to
//! answer `reqwest` -- including a chunked `text/event-stream` body.
//!
//! Not the desktop's listener. The spec's loopback test against the
//! real listener is #517's; this exercises the phone's side of every
//! contract on its own.

use rustls::pki_types::{CertificateDer, PrivateKeyDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::WebPkiClientVerifier;
use rustls::{
    CertificateError, DigitallySignedStruct, DistinguishedName, Error as TlsError, NamedGroup,
    PeerMisbehaved, ServerConfig, SignatureScheme,
};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio_rustls::TlsAcceptor;

use crate::client::provider;
use crate::keys::fingerprint_of;

/// One request the server saw, after the path gate.
#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub peer_fp: String,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// What to answer a path with.
#[derive(Debug, Clone)]
pub(crate) enum Reply {
    Body {
        status: u16,
        content_type: &'static str,
        body: String,
    },
    /// An event stream: the frames, then the connection is held open
    /// until [`TestServer::end_streams`] (or, with `hold` false, closed
    /// cleanly right after the frames).
    Sse {
        frames: Vec<(String, String)>,
        hold: bool,
    },
    /// Read the request and never answer it: a desktop that is up, has
    /// accepted the connection, and is still working (#1466).
    Stall,
    /// A JSON body, gzipped when the request's `Accept-Encoding` names
    /// gzip and sent plain otherwise. That is what the real listener does
    /// on `/v1/call/*` (#1478).
    GzipJson { status: u16, body: String },
}

impl Reply {
    pub fn json(status: u16, body: Value) -> Self {
        Reply::Body {
            status,
            content_type: "application/json",
            body: body.to_string(),
        }
    }
    pub fn text(status: u16, body: &str) -> Self {
        Reply::Body {
            status,
            content_type: "text/plain; charset=utf-8",
            body: body.to_string(),
        }
    }
    pub fn sse(frames: &[(&str, &str)], hold: bool) -> Self {
        Reply::Sse {
            frames: frames
                .iter()
                .map(|(n, d)| (n.to_string(), d.to_string()))
                .collect(),
            hold,
        }
    }
}

#[derive(Default)]
struct Shared {
    paired: Mutex<HashSet<String>>,
    window: AtomicBool,
    replies: Mutex<HashMap<String, Reply>>,
    requests: Mutex<Vec<Request>>,
    last_kx: Mutex<Option<NamedGroup>>,
    /// The scheme the last client CertificateVerify was checked with.
    last_sig: Mutex<Option<SignatureScheme>>,
    end_streams: Notify,
    /// Set by [`TestServer::go_away`]: every new connection is closed
    /// before the handshake, while the port stays bound.
    gone: AtomicBool,
}

/// The desktop's client-cert rule: paired, or the pairing window is
/// open. The handshake signature is verified so the fingerprint alone
/// admits nobody -- and, as on the desktop, with ML-DSA-65 only.
struct Verifier {
    shared: Arc<Shared>,
    algs: rustls::crypto::WebPkiSupportedAlgorithms,
}

impl std::fmt::Debug for Verifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Verifier").finish_non_exhaustive()
    }
}

impl ClientCertVerifier for Verifier {
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
        if self.shared.paired.lock().unwrap().contains(&fp)
            || self.shared.window.load(Ordering::SeqCst)
        {
            Ok(ClientCertVerified::assertion())
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
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, TlsError> {
        Err(TlsError::General("TLS 1.2 is not offered".into()))
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, TlsError> {
        if d.scheme != SignatureScheme::ML_DSA_65 {
            return Err(PeerMisbehaved::SignedHandshakeWithUnadvertisedSigScheme.into());
        }
        *self.shared.last_sig.lock().unwrap() = Some(d.scheme);
        rustls::crypto::verify_tls13_signature(m, c, d, &self.algs)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ML_DSA_65]
    }
}

pub(crate) struct TestServer {
    /// The server certificate's fingerprint, lowercase hex: what a QR
    /// would carry.
    pub fp: String,
    cert_der: Vec<u8>,
    port: u16,
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for TestServer {
    /// Closes the listening socket and ends every held stream, so a
    /// dropped server is gone from the phone's point of view: the
    /// stream ends and the next connect is refused.
    fn drop(&mut self) {
        self.task.abort();
        self.shared.end_streams.notify_waiters();
    }
}

/// A self-signed desktop certificate of the given algorithm: ML-DSA-65
/// is what a protocol-2 desktop presents, P-256 what a 5.0 one does.
fn server_identity(
    alg: &'static rcgen::SignatureAlgorithm,
) -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
    let key = rcgen::KeyPair::generate_for(alg).unwrap();
    let mut params = rcgen::CertificateParams::default();
    let mut dn = rcgen::DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, "Headstate desktop");
    params.distinguished_name = dn;
    let cert = params.self_signed(&key).unwrap();
    (
        cert.der().clone(),
        PrivateKeyDer::Pkcs8(key.serialize_der().into()),
    )
}

impl TestServer {
    pub async fn start() -> Self {
        Self::start_with(&rcgen::PKCS_ML_DSA_65).await
    }

    /// A desktop from before protocol 2, for proving the client refuses
    /// one.
    pub async fn start_with_p256_certificate() -> Self {
        Self::start_with(&rcgen::PKCS_ECDSA_P256_SHA256).await
    }

    async fn start_with(alg: &'static rcgen::SignatureAlgorithm) -> Self {
        let (cert, key) = server_identity(alg);
        let cert_der = cert.as_ref().to_vec();
        let fp = fingerprint_of(cert.as_ref());
        let shared = Arc::new(Shared::default());
        let provider = Arc::new(provider());
        let verifier = Verifier {
            shared: shared.clone(),
            algs: provider.signature_verification_algorithms,
        };
        // `WebPkiClientVerifier` is named only so the import proves the
        // custom verifier is the one in use; rustls's default would
        // want a CA.
        let _ = std::any::type_name::<WebPkiClientVerifier>();
        let config = ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_client_cert_verifier(Arc::new(verifier))
            .with_single_cert(vec![cert], key)
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(accept_loop(listener, acceptor, shared.clone()));
        Self {
            fp,
            cert_der,
            port,
            shared,
            task,
        }
    }

    pub fn addr(&self) -> String {
        "127.0.0.1".into()
    }
    pub fn port(&self) -> u16 {
        self.port
    }
    /// Whether the certificate this server presents names id-ml-dsa-65
    /// in all three places a self-signed certificate does.
    pub fn cert_is_ml_dsa_65(&self) -> bool {
        let oid = [
            0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x03, 0x12,
        ];
        self.cert_der
            .windows(oid.len())
            .filter(|w| *w == oid)
            .count()
            == 3
    }
    pub fn last_sig(&self) -> Option<SignatureScheme> {
        *self.shared.last_sig.lock().unwrap()
    }
    pub fn pair(&self, fp: &str) {
        self.shared.paired.lock().unwrap().insert(fp.to_string());
    }
    pub fn revoke(&self, fp: &str) {
        self.shared.paired.lock().unwrap().remove(fp);
    }
    pub fn open_window(&self, open: bool) {
        self.shared.window.store(open, Ordering::SeqCst);
    }
    pub fn reply(&self, path: &str, reply: Reply) {
        self.shared
            .replies
            .lock()
            .unwrap()
            .insert(path.to_string(), reply);
    }
    pub fn requests(&self) -> Vec<Request> {
        self.shared.requests.lock().unwrap().clone()
    }
    pub fn last_kx(&self) -> Option<NamedGroup> {
        *self.shared.last_kx.lock().unwrap()
    }
    /// End every held-open event stream, the way the desktop ends one
    /// on revocation, lag, or stop.
    pub fn end_streams(&self) {
        self.shared.end_streams.notify_waiters();
    }
    /// The desktop goes away, as far as the phone can tell: held streams
    /// end and every new connection is closed before the TLS handshake,
    /// so a connect fails the way it does against a dead desktop.
    ///
    /// Use this instead of `drop(server)` when a test needs the desktop
    /// GONE. Dropping frees the port, and under a parallel test run
    /// another test's server can bind it -- the phone then reaches a
    /// stranger with a different certificate and reports a fingerprint
    /// mismatch instead of "unreachable". That burned a release commit
    /// (`a_window_while_the_desktop_is_away_touches_nothing`, CI, the
    /// merge of #1470). Keeping the listener bound makes the port
    /// impossible to reuse for as long as the server lives.
    pub fn go_away(&self) {
        self.shared.gone.store(true, Ordering::SeqCst);
        self.shared.end_streams.notify_waiters();
    }
    /// The QR a desktop would show for this server.
    pub fn qr(&self, token_b64url: &str, exp: i64) -> String {
        json!({
            "v": 2,
            "name": "octocat's laptop",
            "addrs": [self.addr()],
            "port": self.port,
            "fp": format!("sha256:{}", self.fp),
            "token": token_b64url,
            "exp": exp,
        })
        .to_string()
    }
}

async fn accept_loop(listener: TcpListener, acceptor: TlsAcceptor, shared: Arc<Shared>) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        if shared.gone.load(Ordering::SeqCst) {
            drop(tcp);
            continue;
        }
        let acceptor = acceptor.clone();
        let shared = shared.clone();
        tokio::spawn(async move {
            let Ok(mut tls) = acceptor.accept(tcp).await else {
                return;
            };
            let (peer_fp, kx) = {
                let conn = tls.get_ref().1;
                let fp = conn
                    .peer_certificates()
                    .and_then(|c| c.first())
                    .map(|c| fingerprint_of(c.as_ref()))
                    .unwrap_or_default();
                (fp, conn.negotiated_key_exchange_group().map(|g| g.name()))
            };
            *shared.last_kx.lock().unwrap() = kx;
            let Some(req) = read_request(&mut tls, peer_fp).await else {
                return;
            };
            let paired = shared.paired.lock().unwrap().contains(&req.peer_fp);
            if !paired && req.path != "/v1/pair" {
                let _ = write_body(&mut tls, 403, "text/plain; charset=utf-8", "not paired").await;
                return;
            }
            let reply = shared
                .replies
                .lock()
                .unwrap()
                .get(&req.path)
                .cloned()
                .unwrap_or_else(|| default_reply(&req));
            let accepts_gzip = req
                .header("accept-encoding")
                .is_some_and(|v| v.split(',').any(|c| c.trim() == "gzip"));
            shared.requests.lock().unwrap().push(req);
            match reply {
                Reply::Body {
                    status,
                    content_type,
                    body,
                } => {
                    let _ = write_body(&mut tls, status, content_type, &body).await;
                }
                Reply::Sse { frames, hold } => {
                    let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                                cache-control: no-cache\r\ntransfer-encoding: chunked\r\n\r\n";
                    if tls.write_all(head.as_bytes()).await.is_err() {
                        return;
                    }
                    for (name, data) in frames {
                        let frame = format!("event: {name}\ndata: {data}\n\n");
                        if write_chunk(&mut tls, frame.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                    if hold {
                        shared.end_streams.notified().await;
                    }
                    let _ = tls.write_all(b"0\r\n\r\n").await;
                    let _ = tls.shutdown().await;
                }
                Reply::GzipJson { status, body } => {
                    let _ = write_gzip_json(&mut tls, status, &body, accepts_gzip).await;
                }
                Reply::Stall => {
                    // Held until the client gives up and closes its end,
                    // which is what a real slow desktop sees.
                    let mut sink = [0u8; 64];
                    while matches!(tls.read(&mut sink).await, Ok(n) if n > 0) {}
                }
            }
        });
    }
}

fn default_reply(req: &Request) -> Reply {
    match req.path.as_str() {
        "/v1/hello" => Reply::json(
            200,
            json!({"desktop_version": "9.9.9", "protocol_version": 2, "viewer_login": "octocat"}),
        ),
        "/v1/pair" => {
            let body: Value = serde_json::from_str(&req.body).unwrap_or(Value::Null);
            Reply::json(
                200,
                json!({"device_id": 1, "device_name": body["device_name"].as_str().unwrap_or("")}),
            )
        }
        _ => Reply::text(404, "no such route"),
    }
}

async fn read_request<S: AsyncReadExt + Unpin>(s: &mut S, peer_fp: String) -> Option<Request> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        let n = s.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let mut request_line = lines.next()?.split_whitespace();
    let method = request_line.next()?.to_string();
    let path = request_line.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = s.read(&mut tmp).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    Some(Request {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).to_string(),
        peer_fp,
    })
}

async fn write_body<S: AsyncWriteExt + Unpin>(
    s: &mut S,
    status: u16,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\n\
         content-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).await?;
    s.write_all(body.as_bytes()).await?;
    s.shutdown().await
}

/// A JSON body, gzipped only if the client asked for it.
async fn write_gzip_json<S: AsyncWriteExt + Unpin>(
    s: &mut S,
    status: u16,
    body: &str,
    accepts_gzip: bool,
) -> std::io::Result<()> {
    if !accepts_gzip {
        return write_body(s, status, "application/json", body).await;
    }
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut enc, body.as_bytes())?;
    let gz = enc.finish()?;
    let head = format!(
        "HTTP/1.1 {status} OK\r\ncontent-type: application/json\r\n\
         content-encoding: gzip\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        gz.len()
    );
    s.write_all(head.as_bytes()).await?;
    s.write_all(&gz).await?;
    s.shutdown().await
}

async fn write_chunk<S: AsyncWriteExt + Unpin>(s: &mut S, data: &[u8]) -> std::io::Result<()> {
    s.write_all(format!("{:x}\r\n", data.len()).as_bytes())
        .await?;
    s.write_all(data).await?;
    s.write_all(b"\r\n").await?;
    s.flush().await
}

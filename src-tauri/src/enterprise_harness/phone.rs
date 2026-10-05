//! Synthetic phone transport; compiled only into the opt-in enterprise driver.
use crate::remote::identity::{fingerprint_of, Identity};
fn provider() -> rustls::crypto::CryptoProvider {
    rustls::crypto::CryptoProvider {
        kx_groups: vec![
            rustls::crypto::aws_lc_rs::kx_group::X25519MLKEM768,
            rustls::crypto::aws_lc_rs::kx_group::X25519,
            rustls::crypto::aws_lc_rs::kx_group::SECP256R1,
        ],
        ..rustls::crypto::aws_lc_rs::default_provider()
    }
}
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, Error as TlsError, NamedGroup,
    PeerMisbehaved, SignatureScheme,
};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::client::TlsStream;
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
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
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

pub(crate) struct SseClient {
    tls: TlsStream<TcpStream>,
    raw: Vec<u8>,
    pub(crate) status: u16,
    pub(crate) content_type: String,
    decoded: Vec<u8>,
    done: bool,
}

impl SseClient {
    pub(crate) async fn connect(addr: SocketAddr, phone: &Identity, server_fp: &str) -> Self {
        let connector = tokio_rustls::TlsConnector::from(client_config(Some(phone), server_fp));
        let tcp = TcpStream::connect(addr).await.unwrap();
        let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        let mut tls = connector.connect(name, tcp).await.unwrap();
        tls.write_all(
            b"GET /v1/events HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\r\n",
        )
        .await
        .unwrap();

        let mut raw = Vec::new();
        let head_end = loop {
            if let Some(i) = find(&raw, b"\r\n\r\n") {
                break i;
            }
            let mut buf = [0u8; 4096];
            let n = read_with_timeout(&mut tls, &mut buf).await;
            assert!(n > 0, "connection closed before the response head");
            raw.extend_from_slice(&buf[..n]);
        };
        let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
        let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        let header = |name: &str| {
            head.lines()
                .find_map(|l| {
                    l.split_once(':')
                        .filter(|(k, _)| k.eq_ignore_ascii_case(name))
                })
                .map(|(_, v)| v.trim().to_string())
                .unwrap_or_default()
        };
        assert_eq!(
            header("transfer-encoding"),
            "chunked",
            "the test decodes hyper's chunked framing; the head was:\n{head}"
        );
        Self {
            tls,
            raw: raw[head_end + 4..].to_vec(),
            status,
            content_type: header("content-type"),
            decoded: Vec::new(),
            done: false,
        }
    }

    /// Decode and discard consumed chunks and frames; memory is bounded even in a long soak.
    pub(crate) async fn next_frame(&mut self) -> Option<(String, String)> {
        loop {
            let (body, done, consumed) = dechunk(&self.raw);
            self.raw.drain(..consumed);
            self.decoded.extend(body);
            self.done |= done;
            assert!(
                self.raw.len() + self.decoded.len() < 8 * 1024 * 1024,
                "synthetic SSE buffer overflow"
            );
            while let Some(end) = find(&self.decoded, b"\n\n") {
                let frame: Vec<_> = self.decoded.drain(..end + 2).collect();
                if let Some(frame) = parse_frames(&frame).into_iter().next() {
                    return Some(frame);
                }
            }
            if self.done {
                return None;
            }
            let mut buf = [0u8; 4096];
            let n = read_with_timeout(&mut self.tls, &mut buf).await;
            if n == 0 {
                return None;
            }
            self.raw.extend_from_slice(&buf[..n]);
        }
    }
}

async fn read_with_timeout(tls: &mut TlsStream<TcpStream>, buf: &mut [u8]) -> usize {
    match tokio::time::timeout(Duration::from_secs(45), tls.read(buf)).await {
        Ok(Ok(n)) => n,
        // The server's close_notify or a reset after it dropped the
        // connection both read as "no more bytes".
        Ok(Err(_)) => 0,
        Err(_) => panic!("no bytes from the event stream within 45s"),
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Decode the complete chunks of an HTTP/1.1 chunked body; the
/// second value is whether the terminating zero-length chunk arrived.
fn dechunk(mut b: &[u8]) -> (Vec<u8>, bool, usize) {
    let original = b.len();
    let mut out = Vec::new();
    loop {
        let Some(nl) = find(b, b"\r\n") else {
            return (out, false, original - b.len());
        };
        let size = usize::from_str_radix(std::str::from_utf8(&b[..nl]).unwrap().trim(), 16)
            .expect("a chunk-size line");
        if size == 0 {
            return (out, true, original - b.len() + nl + 2);
        }
        let start = nl + 2;
        let end = start + size;
        if b.len() < end + 2 {
            return (out, false, original - b.len());
        }
        out.extend_from_slice(&b[start..end]);
        b = &b[end + 2..];
    }
}

/// Complete frames only: `event:`/`data:` pairs separated by a
/// blank line. Comment lines (keep-alives) are skipped.
fn parse_frames(body: &[u8]) -> Vec<(String, String)> {
    let text = std::str::from_utf8(body).unwrap();
    let Some((complete, _partial)) = text.rsplit_once("\n\n") else {
        return Vec::new();
    };
    complete
        .split("\n\n")
        .filter_map(|frame| {
            let mut name = None;
            let mut data = Vec::new();
            for line in frame.lines() {
                if let Some(v) = line.strip_prefix("event: ") {
                    name = Some(v.to_string());
                } else if let Some(v) = line.strip_prefix("data: ") {
                    data.push(v);
                }
            }
            name.map(|n| (n, data.join("\n")))
        })
        .collect()
}

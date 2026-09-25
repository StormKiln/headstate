//! Pairing, from the phone's side: read the QR, make keys, connect to
//! the desktop it names, prove the token, and remember the desktop.
//!
//! The wire is the desktop's `remote/pairing.rs`:
//!
//! - QR payload `{v, name, addrs, port, fp, token, exp}`; `v` is 2 (the
//!   wire protocol version; a 1, from a 5.0 desktop, is refused); `fp`
//!   is `sha256:<64 lowercase hex>`; `token` is 32 bytes base64url
//!   unpadded; `exp` is Unix seconds.
//! - `POST /v1/pair` body `{token, device_name, signing_keys:
//!   {ecdsa_p256, mldsa_65?}, proof}`; the keys standard base64 with
//!   padding; `proof` = HMAC-SHA256(key = the 32 raw token bytes,
//!   message = client_fp || server_fp as ASCII hex, no separator),
//!   base64url unpadded.
//! - 200 with `{device_id, device_name}` on approve; 403 on deny,
//!   timeout, bad token or bad proof; 400 for a malformed body.
//!
//! The server fingerprint is checked by the TLS handshake itself: the
//! client is built with the QR's `fp` pinned, so a desktop presenting
//! any other certificate never gets as far as HTTP. On every failure
//! after the keys were made they are destroyed again; the spec says the
//! phone discards its keys on 403, and a phone that failed for any other
//! reason should not keep a half-made identity either.

use base64::engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD as BASE64URL};
use base64::Engine;
// `KeyInit` alongside `Mac` since hmac 0.13: `new_from_slice` moved onto
// that trait, while `update`/`finalize` stayed on `Mac`.
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::sync::Arc;

use crate::client::{Client, ClientError, Hello};
use crate::keys::{DeviceKeys, KeyError};
use crate::store::{get_json, put_json, Store, StoreError};

/// Store key for the pairing records.
pub const DESKTOPS_KEY: &str = "desktops";

/// `pub(crate)` so `mirrored.rs` can assert it against the desktop's
/// copy (#854); nothing outside this crate reads it.
pub(crate) const TOKEN_LEN: usize = 32;
/// The one `v` this build pairs with: the desktop's `PROTOCOL_VERSION`,
/// as the spec says ("the same integer is the QR's `v`"). Exactly, not
/// at-least: a QR from a newer desktop describes a pairing this phone
/// does not know how to complete.
pub(crate) const QR_VERSION: u8 = 2;

/// One paired desktop, as persisted. Newest first in the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Desktop {
    pub name: String,
    pub addrs: Vec<String>,
    pub port: u16,
    /// The desktop certificate's SHA256, lowercase hex, no prefix.
    pub fp: String,
    /// ISO 8601.
    pub paired_at: String,
    /// The last `/v1/hello` this desktop answered, if any.
    #[serde(default)]
    pub hello: Option<Hello>,
}

/// The `desktops` record.
#[derive(Serialize, Deserialize)]
struct Desktops {
    v: u32,
    list: Vec<Desktop>,
}

const RECORD_VERSION: u32 = 1;

/// What the QR encodes. Field names are the wire format.
#[derive(Debug, Deserialize)]
struct QrPayload {
    v: u8,
    name: String,
    addrs: Vec<String>,
    port: u16,
    fp: String,
    token: String,
    exp: i64,
}

/// A validated QR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub name: String,
    pub addrs: Vec<String>,
    pub port: u16,
    /// Lowercase hex, prefix stripped.
    pub fp: String,
    /// The 32 raw token bytes.
    pub token: Vec<u8>,
    pub exp: i64,
}

/// Body of `POST /v1/pair`.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairRequest {
    pub token: String,
    pub device_name: String,
    pub signing_keys: SigningKeys,
    pub proof: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SigningKeys {
    pub ecdsa_p256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mldsa_65: Option<String>,
}

/// The 200 body of `/v1/pair`.
#[derive(Debug, Deserialize)]
pub struct PairOutcome {
    #[allow(dead_code)]
    pub device_id: i64,
    pub device_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PairingError {
    #[error("not a Headstate pairing code: {0}")]
    BadQr(String),
    #[error("this pairing code has expired; show a new one on the desktop")]
    Expired,
    #[error("the desktop presented a certificate that does not match the pairing code")]
    FingerprintMismatch,
    /// TLS refused, but NOT because this phone rejected the desktop's
    /// certificate. Most often the reverse: the desktop refusing this
    /// phone's, which is what a session identity that is not ML-DSA-65
    /// looks like from here. Carries what TLS said, because guessing
    /// between the two and getting it wrong accuses the user's desktop
    /// of presenting a bad certificate (#640).
    #[error("the secure connection to the desktop failed: {0}")]
    Handshake(String),
    /// 403: denied, timed out, or the token was already used.
    #[error("the desktop refused the pairing: {0}")]
    Denied(String),
    #[error("the desktop rejected the pairing (HTTP {status}): {message}")]
    Rejected { status: u16, message: String },
    #[error("could not reach the desktop: {0}")]
    Unreachable(String),
    /// Reached, and did not answer in time -- not the same as the above,
    /// and not reported as it (#1466).
    #[error("the desktop took too long to answer: {0}")]
    TimedOut(String),
    #[error("{0}")]
    Protocol(String),
    #[error(transparent)]
    Keys(#[from] KeyError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Validate the QR payload. `now` is Unix seconds by the phone's clock.
pub fn parse_qr(payload: &str, now: i64) -> Result<Parsed, PairingError> {
    let bad = |m: String| PairingError::BadQr(m);
    let qr: QrPayload = serde_json::from_str(payload.trim()).map_err(|e| bad(e.to_string()))?;
    if qr.v != QR_VERSION {
        return Err(bad(format!("version {} is not supported", qr.v)));
    }
    if qr.name.trim().is_empty() {
        return Err(bad("desktop name is empty".into()));
    }
    if qr.addrs.is_empty() || qr.addrs.iter().any(|a| a.trim().is_empty()) {
        return Err(bad("no addresses".into()));
    }
    if qr.port == 0 {
        return Err(bad("port is 0".into()));
    }
    let fp = qr
        .fp
        .strip_prefix("sha256:")
        .ok_or_else(|| bad("fingerprint is not sha256:<hex>".into()))?;
    if fp.len() != 64 || !fp.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(bad("fingerprint is not 64 lowercase hex characters".into()));
    }
    let token = BASE64URL
        .decode(&qr.token)
        .map_err(|_| bad("token is not base64url".into()))?;
    if token.len() != TOKEN_LEN {
        return Err(bad(format!(
            "token is {} bytes, not {TOKEN_LEN}",
            token.len()
        )));
    }
    if qr.exp <= now {
        return Err(PairingError::Expired);
    }
    Ok(Parsed {
        name: qr.name,
        addrs: qr.addrs,
        port: qr.port,
        fp: fp.to_string(),
        token,
        exp: qr.exp,
    })
}

/// The pairing proof, exactly as the desktop's `pairing::proof`.
pub fn proof(token: &[u8], client_fp: &str, server_fp: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(token).expect("HMAC accepts any key length");
    mac.update(client_fp.as_bytes());
    mac.update(server_fp.as_bytes());
    BASE64URL.encode(mac.finalize().into_bytes())
}

/// What this phone calls itself in the pair request when the frontend
/// gives no name. The keys plugin (#513) has access to the platform's
/// device name; until it supplies one this is the platform.
pub fn default_device_name() -> &'static str {
    if cfg!(target_os = "ios") {
        "iPhone"
    } else if cfg!(target_os = "android") {
        "Android phone"
    } else {
        "Headstate Companion"
    }
}

/// Run the whole flow. On success the desktop is recorded (replacing an
/// earlier record with the same fingerprint) and the client that just
/// paired is returned, ready for `/v1/hello` and the event stream.
pub async fn pair(
    store: &dyn Store,
    keys: &dyn DeviceKeys,
    payload: &str,
    device_name: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(Desktop, Arc<Client>), PairingError> {
    let qr = parse_qr(payload, now.timestamp())?;
    let public = keys.generate()?;
    let outcome = async {
        let identity = keys.session_identity()?;
        let client = Client::new(&identity, &qr.fp, qr.addrs.clone(), qr.port)
            .map_err(|e| PairingError::Protocol(e.to_string()))?;
        let request = PairRequest {
            token: BASE64URL.encode(&qr.token),
            device_name: device_name.to_string(),
            signing_keys: SigningKeys {
                ecdsa_p256: BASE64.encode(&public.ecdsa_p256),
                mldsa_65: public.mldsa_65.as_ref().map(|k| BASE64.encode(k)),
            },
            proof: proof(&qr.token, &identity.fingerprint(), &qr.fp),
        };
        let outcome: PairOutcome = client.pair(&request).await.map_err(|e| match e {
            // Only OUR verifier rejecting the desktop's certificate is a
            // fingerprint mismatch. Every other TLS failure keeps its own
            // message: the handshake is mutual, so it is at least as
            // likely to be the desktop refusing THIS phone, which says
            // nothing about the desktop's identity (#640).
            ClientError::FingerprintMismatch => PairingError::FingerprintMismatch,
            ClientError::Handshake(m) => PairingError::Handshake(m),
            ClientError::Unreachable(m) => PairingError::Unreachable(m),
            ClientError::TimedOut(m) => PairingError::TimedOut(m),
            ClientError::Status {
                status: 403,
                message,
            } => PairingError::Denied(message),
            ClientError::Status { status, message } => PairingError::Rejected { status, message },
            ClientError::Protocol(m) => PairingError::Protocol(m),
        })?;
        log::info!(
            "companion: paired with {} as {}",
            qr.name,
            outcome.device_name
        );
        Ok::<_, PairingError>(Arc::new(client))
    }
    .await;
    let client = match outcome {
        Ok(client) => client,
        Err(e) => {
            if let Err(d) = keys.destroy() {
                log::warn!("companion: could not discard keys after a failed pairing: {d}");
            }
            return Err(e);
        }
    };
    let desktop = Desktop {
        name: qr.name,
        addrs: qr.addrs,
        port: qr.port,
        fp: qr.fp,
        paired_at: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        hello: None,
    };
    let mut list = load_desktops(store)?;
    list.retain(|d| d.fp != desktop.fp);
    list.insert(0, desktop.clone());
    save_desktops(store, &list)?;
    Ok((desktop, client))
}

pub fn load_desktops(store: &dyn Store) -> Result<Vec<Desktop>, StoreError> {
    Ok(get_json::<Desktops>(store, DESKTOPS_KEY)?
        .map(|d| d.list)
        .unwrap_or_default())
}

pub fn save_desktops(store: &dyn Store, list: &[Desktop]) -> Result<(), StoreError> {
    put_json(
        store,
        DESKTOPS_KEY,
        &Desktops {
            v: RECORD_VERSION,
            list: list.to_vec(),
        },
    )
}

pub fn forget_all(store: &dyn Store) -> Result<(), StoreError> {
    store.remove(DESKTOPS_KEY)
}

/// Remember what the desktop with fingerprint `fp` said in `/v1/hello`.
pub fn record_hello(store: &dyn Store, fp: &str, hello: &Hello) -> Result<(), StoreError> {
    let mut list = load_desktops(store)?;
    let mut changed = false;
    for d in list.iter_mut().filter(|d| d.fp == fp) {
        if d.hello.as_ref() != Some(hello) {
            d.hello = Some(hello.clone());
            changed = true;
        }
    }
    if changed {
        save_desktops(store, &list)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::SoftwareKeys;
    use crate::store::MemoryStore;
    use crate::testing::{Reply, TestServer};
    use serde_json::json;

    const SERVER_FP: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const NOW: i64 = 1757068000;

    fn token_b64() -> String {
        BASE64URL.encode([7u8; TOKEN_LEN])
    }

    fn qr(overrides: Value) -> String {
        let mut v = json!({
            "v": 2,
            "name": "octocat's laptop",
            "addrs": ["192.0.2.10", "100.64.0.7"],
            "port": 41919,
            "fp": format!("sha256:{SERVER_FP}"),
            "token": token_b64(),
            "exp": NOW + 120,
        });
        if let (Value::Object(base), Value::Object(over)) = (&mut v, overrides) {
            for (k, val) in over {
                base.insert(k, val);
            }
        }
        v.to_string()
    }

    use serde_json::Value;

    /// One integer on the wire, two constants in the code.
    #[test]
    fn the_qr_version_is_the_protocol_version() {
        assert_eq!(u32::from(QR_VERSION), crate::client::PROTOCOL_VERSION);
    }

    #[test]
    fn the_spec_qr_parses() {
        let p = parse_qr(&qr(json!({})), NOW).unwrap();
        assert_eq!(
            p,
            Parsed {
                name: "octocat's laptop".into(),
                addrs: vec!["192.0.2.10".into(), "100.64.0.7".into()],
                port: 41919,
                fp: SERVER_FP.into(),
                token: vec![7u8; TOKEN_LEN],
                exp: NOW + 120,
            }
        );
    }

    #[test]
    fn every_deviation_from_the_qr_shape_is_refused() {
        let bad = |over: Value| match parse_qr(&qr(over.clone()), NOW) {
            Err(PairingError::BadQr(m)) => m,
            other => panic!("expected BadQr for {over}, got {other:?}"),
        };
        // A 5.0 desktop's QR (P-256 certificates, protocol 1) and a
        // future one alike: only this build's version pairs.
        assert!(bad(json!({"v": 1})).contains("version 1"));
        assert!(bad(json!({"v": 3})).contains("version 3"));
        assert!(bad(json!({"name": "  "})).contains("name"));
        assert!(bad(json!({"addrs": []})).contains("addresses"));
        assert!(bad(json!({"port": 0})).contains("port"));
        assert!(bad(json!({"fp": SERVER_FP})).contains("sha256:"));
        assert!(bad(json!({"fp": format!("sha256:{}", "AB".repeat(32))})).contains("hex"));
        assert!(bad(json!({"fp": "sha256:abc"})).contains("64"));
        assert!(bad(json!({"token": "not base64url!"})).contains("base64url"));
        assert!(bad(json!({"token": BASE64URL.encode([1u8; 31])})).contains("31 bytes"));
        assert!(matches!(parse_qr("{}", NOW), Err(PairingError::BadQr(_))));
        assert!(matches!(
            parse_qr("hello", NOW),
            Err(PairingError::BadQr(_))
        ));
    }

    #[test]
    fn an_expired_code_is_expired_not_malformed() {
        assert_eq!(
            parse_qr(&qr(json!({"exp": NOW})), NOW),
            Err(PairingError::Expired)
        );
        assert!(parse_qr(&qr(json!({"exp": NOW + 1})), NOW).is_ok());
    }

    /// RFC 4231 test case 2, the desktop's own vector for the HMAC.
    #[test]
    fn the_proof_is_standard_hmac_sha256_base64url() {
        let p = proof(b"Jefe", "what do ya want ", "for nothing?");
        let expected = BASE64URL.encode([
            0x5b, 0xdc, 0xc1, 0x46, 0xbf, 0x60, 0x75, 0x4e, 0x6a, 0x04, 0x24, 0x26, 0x08, 0x95,
            0x75, 0xc7, 0x5a, 0x00, 0x3f, 0x08, 0x9d, 0x27, 0x39, 0x83, 0x9d, 0xec, 0x58, 0xb9,
            0x64, 0xec, 0x38, 0x43,
        ]);
        assert_eq!(p, expected);
        assert!(!p.contains('='));
    }

    #[test]
    fn the_pair_request_serialises_to_the_wire_shape() {
        let req = PairRequest {
            token: "t".into(),
            device_name: "Octocat's phone".into(),
            signing_keys: SigningKeys {
                ecdsa_p256: "BA==".into(),
                mldsa_65: None,
            },
            proof: "p".into(),
        };
        assert_eq!(
            serde_json::to_value(&req).unwrap(),
            json!({"token": "t", "device_name": "Octocat's phone",
                   "signing_keys": {"ecdsa_p256": "BA=="}, "proof": "p"})
        );
    }

    fn fixtures() -> (Arc<MemoryStore>, SoftwareKeys) {
        let store = Arc::new(MemoryStore::default());
        (store.clone(), SoftwareKeys::new(store))
    }

    #[tokio::test]
    async fn a_full_pairing_records_the_desktop_and_sends_the_right_proof_and_keys() {
        let (store, keys) = fixtures();
        let server = TestServer::start().await;
        server.open_window(true);
        let now = chrono::Utc::now();
        let (desktop, _client) = pair(
            store.as_ref(),
            &keys,
            &server.qr(&token_b64(), now.timestamp() + 120),
            "Octocat's phone",
            now,
        )
        .await
        .unwrap();
        assert_eq!(desktop.name, "octocat's laptop");
        assert_eq!(desktop.fp, server.fp);
        assert_eq!(desktop.port, server.port());
        assert_eq!(
            load_desktops(store.as_ref()).unwrap(),
            vec![desktop.clone()]
        );

        let req = server.requests().pop().unwrap();
        assert_eq!(
            (req.method.as_str(), req.path.as_str()),
            ("POST", "/v1/pair")
        );
        let body: PairRequest = serde_json::from_str(&req.body).unwrap();
        assert_eq!(body.device_name, "Octocat's phone");
        assert_eq!(body.token, token_b64());
        let public = keys.public_keys().unwrap();
        assert_eq!(
            BASE64.decode(&body.signing_keys.ecdsa_p256).unwrap(),
            public.ecdsa_p256
        );
        assert_eq!(
            BASE64.decode(body.signing_keys.mldsa_65.unwrap()).unwrap(),
            public.mldsa_65.unwrap()
        );
        // The desktop recomputes the proof from the cert it saw at the
        // handshake and its own fingerprint.
        assert_eq!(req.peer_fp, keys.session_identity().unwrap().fingerprint());
        assert_eq!(
            body.proof,
            proof(&[7u8; TOKEN_LEN], &req.peer_fp, &server.fp)
        );
    }

    #[tokio::test]
    async fn a_denied_pairing_discards_the_keys() {
        let (store, keys) = fixtures();
        let server = TestServer::start().await;
        server.open_window(true);
        server.reply("/v1/pair", Reply::text(403, "pairing was denied"));
        let now = chrono::Utc::now();
        let err = pair(
            store.as_ref(),
            &keys,
            &server.qr(&token_b64(), now.timestamp() + 120),
            "Octocat's phone",
            now,
        )
        .await
        .unwrap_err();
        assert_eq!(err, PairingError::Denied("pairing was denied".into()));
        assert_eq!(keys.public_keys().unwrap_err(), KeyError::NoKeys);
        assert!(load_desktops(store.as_ref()).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_desktop_with_the_wrong_certificate_never_gets_the_token() {
        let (store, keys) = fixtures();
        let server = TestServer::start().await;
        server.open_window(true);
        let now = chrono::Utc::now();
        let mut qr: Value =
            serde_json::from_str(&server.qr(&token_b64(), now.timestamp() + 120)).unwrap();
        qr["fp"] = json!(format!("sha256:{}", "00".repeat(32)));
        let err = pair(store.as_ref(), &keys, &qr.to_string(), "phone", now)
            .await
            .unwrap_err();
        assert_eq!(err, PairingError::FingerprintMismatch);
        assert!(server.requests().is_empty(), "no request reached HTTP");
        assert_eq!(keys.public_keys().unwrap_err(), KeyError::NoKeys);
    }

    #[tokio::test]
    async fn re_pairing_the_same_desktop_replaces_its_record_and_keeps_others() {
        let (store, keys) = fixtures();
        let other = Desktop {
            name: "other".into(),
            addrs: vec!["100.64.0.9".into()],
            port: 41919,
            fp: "ab".repeat(32),
            paired_at: "2026-01-01T00:00:00Z".into(),
            hello: None,
        };
        save_desktops(store.as_ref(), std::slice::from_ref(&other)).unwrap();
        let server = TestServer::start().await;
        server.open_window(true);
        let now = chrono::Utc::now();
        for _ in 0..2 {
            pair(
                store.as_ref(),
                &keys,
                &server.qr(&token_b64(), now.timestamp() + 120),
                "phone",
                now,
            )
            .await
            .unwrap();
        }
        let list = load_desktops(store.as_ref()).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].fp, server.fp, "newest first");
        assert_eq!(list[1], other);

        let hello = Hello {
            desktop_version: "9.9.9".into(),
            protocol_version: 2,
            viewer_login: Some("octocat".into()),
        };
        record_hello(store.as_ref(), &server.fp, &hello).unwrap();
        let list = load_desktops(store.as_ref()).unwrap();
        assert_eq!(list[0].hello, Some(hello));
        assert_eq!(list[1].hello, None);
        forget_all(store.as_ref()).unwrap();
        assert!(load_desktops(store.as_ref()).unwrap().is_empty());
    }
}

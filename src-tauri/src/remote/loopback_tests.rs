//! The spec's loopback integration test: the listener in-process on a
//! loopback port, driven by a synthetic phone through every route, on
//! an in-memory SQLite store, with no Tauri app and no network.
//!
//! What stands in for the app is exactly the two seams `gate.rs` fills
//! in production: the pairing state is the real one (`PairingState` is
//! the listener's `PairedCerts`, as in `gate.rs`), and the command
//! host is a recorder, because `surface::dispatch` needs the live
//! `AppHandle` no test can construct. So this proves the transport,
//! the gate, pairing, the allowlist, the step-up check, the status
//! mapping, the event stream, and revocation -- everything up to the
//! `commands::*` call, which `surface.rs` covers on the source.

use crate::remote::events::tests::SseClient;
use crate::remote::events::Hub;
use crate::remote::identity::testing::is_ml_dsa_65_certificate;
use crate::remote::identity::Identity;
use crate::remote::listener::tests::{connect, request, RecordingHost, Reply};
use crate::remote::listener::{self, CommandHost, ListenerConfig, PairedCerts};
use crate::remote::pairing::{
    self, PairDecision, PairOutcome, PairRequest, PairingConfig, PairingRequestEvent, PairingState,
    SameName, SigningKeys,
};
use crate::remote::stepup;
use crate::remote::surface::RemoteError;
use crate::store::devices;
use base64::engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD as BASE64URL};
use base64::Engine;
use ml_dsa::{MlDsa65, Seed};
use p256::ecdsa::signature::Signer;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

/// The phone: a self-signed ML-DSA-65 session certificate (what `rcgen`
/// gives the mobile crate too), a P-256 step-up key, and an ML-DSA-65
/// one.
struct Phone {
    cert: Identity,
    ecdsa: p256::ecdsa::SigningKey,
    mldsa: ml_dsa::SigningKey<MlDsa65>,
}

impl Phone {
    fn new() -> Self {
        Self {
            cert: Identity::generate().unwrap(),
            ecdsa: p256::ecdsa::SigningKey::from_bytes(&[7u8; 32].into()).unwrap(),
            mldsa: ml_dsa::SigningKey::<MlDsa65>::from_seed(&Seed::from([7u8; 32])),
        }
    }

    fn fingerprint(&self) -> String {
        self.cert.fingerprint()
    }

    /// The `signing_keys` object of the pair request.
    fn signing_keys(&self) -> SigningKeys {
        SigningKeys {
            // `to_sec1_point` since elliptic-curve 0.14; `false` still
            // means uncompressed, so the encoding is unchanged.
            ecdsa_p256: BASE64.encode(self.ecdsa.verifying_key().to_sec1_point(false).as_bytes()),
            mldsa_65: Some(BASE64.encode(self.mldsa.expanded_key().verifying_key().encode())),
        }
    }

    /// A hybrid `X-Headstate-Signature` for one call, exactly as the
    /// mobile crate will build it.
    fn signature(&self, command: &str, args: &Value, nonce: &[u8; 16], ts: i64) -> String {
        let nonce = BASE64URL.encode(nonce);
        let msg = stepup::canonical_bytes(command, args, &nonce, ts);
        let ecdsa: p256::ecdsa::Signature = self.ecdsa.sign(&msg);
        let mldsa = self
            .mldsa
            .expanded_key()
            .sign_deterministic(&msg, b"")
            .unwrap();
        format!(
            "v1;ts={ts};nonce={nonce};ecdsa={};mldsa={}",
            BASE64URL.encode(ecdsa.to_bytes()),
            BASE64URL.encode(mldsa.encode())
        )
    }
}

/// The desktop: the real pairing state over an in-memory store, the
/// real listener, a recording command host.
struct Desktop<H: CommandHost = RecordingHost> {
    addr: SocketAddr,
    fp: String,
    conn: Connection,
    pairing: Arc<PairingState>,
    /// What the `pairing-request` Tauri event would carry.
    requests: mpsc::UnboundedReceiver<PairingRequestEvent>,
    hub: Arc<Hub>,
    host: Arc<H>,
    handle: listener::Handle,
}

async fn desktop() -> Desktop {
    let hub = Arc::new(Hub::new(Arc::new(|| {
        Box::pin(async { Some("[]".to_string()) })
    })));
    desktop_on(Arc::new(RecordingHost::default()), hub).await
}

/// The same desktop, running commands on `host` and fanning events out
/// through `hub` -- which a host that emits needs to hold too.
async fn desktop_on<H: CommandHost + 'static>(host: Arc<H>, hub: Arc<Hub>) -> Desktop<H> {
    let conn = Connection::open_in_memory().unwrap();
    crate::store::migrate(&conn).unwrap();
    let (tx, requests) = mpsc::unbounded_channel();
    let pairing = Arc::new(PairingState::with_config(
        PairingConfig {
            token_ttl: Duration::from_secs(60),
            decision_timeout: Duration::from_secs(10),
        },
        move |event| {
            let _ = tx.send(event);
        },
    ));
    let identity = Identity::generate().unwrap();
    let fp = identity.fingerprint();
    let handle = listener::start(ListenerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        identity,
        paired: pairing.clone(),
        revocations: pairing.subscribe_revocations(),
        pairing: pairing.clone(),
        host: host.clone(),
        desktop_version: "9.9.9".into(),
        viewer_login: Arc::new(|| Box::pin(async { Some("octocat".to_string()) })),
        events: hub.clone(),
    })
    .await
    .unwrap();
    Desktop {
        addr: handle.local_addr(),
        fp,
        conn,
        pairing,
        requests,
        hub,
        host,
        handle,
    }
}

impl<H: CommandHost> Desktop<H> {
    /// `GET /v1/hello`; `Err` when the handshake itself is refused.
    async fn hello(&self, phone: &Phone) -> Result<Reply, String> {
        request(
            self.addr,
            Some(&phone.cert),
            &self.fp,
            "GET",
            "/v1/hello",
            &[],
            None,
        )
        .await
    }

    async fn post(
        &self,
        phone: &Phone,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> Result<Reply, String> {
        request(
            self.addr,
            Some(&phone.cert),
            &self.fp,
            "POST",
            path,
            headers,
            body,
        )
        .await
    }

    async fn call(
        &self,
        phone: &Phone,
        command: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> Reply {
        self.post(phone, &format!("/v1/call/{command}"), headers, body)
            .await
            .unwrap()
    }

    /// Step 1 of the spec's flow: Settings > Pair a phone.
    fn issue_qr(&self) -> pairing::IssuedToken {
        self.pairing.issue_token()
    }

    /// Steps 2-4: the phone posts `/v1/pair`, the desktop asks the
    /// user, the user approves. Returns what the phone received.
    async fn pair(&mut self, phone: &Phone, device_name: &str) -> PairOutcome {
        let issued = self.issue_qr();
        let token = BASE64URL.decode(&issued.token).unwrap();
        let req = PairRequest {
            token: issued.token,
            device_name: device_name.into(),
            signing_keys: phone.signing_keys(),
            proof: pairing::proof(&token, &phone.fingerprint(), &self.fp),
        };
        let body = serde_json::to_string(&req).unwrap();
        let reply = {
            let addr = self.addr;
            let fp = self.fp.clone();
            let cert = phone.cert.clone();
            tokio::spawn(async move {
                request(addr, Some(&cert), &fp, "POST", "/v1/pair", &[], Some(&body)).await
            })
        };

        // The modal: the event names the device and its fingerprint,
        // and says it offered a post-quantum key.
        let event = tokio::time::timeout(Duration::from_secs(5), self.requests.recv())
            .await
            .expect("the pairing-request event fires")
            .unwrap();
        assert_eq!(event.device_name, device_name);
        assert_eq!(event.fingerprint, phone.fingerprint());
        assert!(event.has_mldsa);
        self.pairing
            .respond(
                &self.conn,
                event.request_id,
                PairDecision::Approve {
                    same_name: SameName::Undecided,
                },
            )
            .unwrap();

        let reply = reply.await.unwrap().expect("the pair request completes");
        assert_eq!(reply.status, 200, "{}", reply.body);
        serde_json::from_str(&reply.body).unwrap()
    }
}

/// The spec's checklist, in order, on one desktop and one phone: start,
/// pair, one command per class (a destructive one with a hybrid
/// signature, and without), an event, revoke, refused.
#[tokio::test]
async fn a_phone_pairs_calls_listens_and_is_revoked() {
    let mut desktop = desktop().await;
    let phone = Phone::new();

    // Before pairing, with no window open: the handshake fails.
    assert!(desktop.hello(&phone).await.is_err());

    // Pair.
    let outcome = desktop.pair(&phone, "Octocat's phone").await;
    assert_eq!(outcome.device_name, "Octocat's phone");
    let row = devices::find_by_fingerprint(&desktop.conn, &phone.fingerprint())
        .unwrap()
        .expect("approve inserted the row");
    assert_eq!(row.id, outcome.device_id);
    assert_eq!(row.cert_der, phone.cert.cert().as_ref());
    // What the desktop stored is the certificate the handshake verified,
    // and it is an ML-DSA-65 one -- the only kind the verifier admits.
    assert!(is_ml_dsa_65_certificate(&row.cert_der));
    assert!(desktop.pairing.is_paired(&phone.fingerprint()));
    assert!(!desktop.pairing.pairing_window_open(), "the token is spent");

    // Paired: ordinary mTLS from here on.
    let hello = desktop.hello(&phone).await.unwrap();
    assert_eq!(hello.status, 200);
    let v: Value = serde_json::from_str(&hello.body).unwrap();
    assert_eq!(v["viewer_login"], "octocat");
    assert_eq!(v["protocol_version"], listener::PROTOCOL_VERSION);

    // Read.
    let reply = desktop.call(&phone, "get_cached", &[], None).await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body, r#"{"ran":"get_cached"}"#);

    // Write, with arguments as the webview would send them.
    let reply = desktop
        .call(&phone, "set_poll_interval", &[], Some(r#"{"secs": 120}"#))
        .await;
    assert_eq!(reply.status, 200);

    // Destructive without a signature: refused, never dispatched.
    let args = json!({
        "repoPath": "/home/octocat/src/hello-world",
        "worktreePath": "/home/octocat/src/hello-world/.worktrees/feature",
    });
    let body = serde_json::to_string(&args).unwrap();
    let reply = desktop
        .call(&phone, "remove_worktree", &[], Some(&body))
        .await;
    assert_eq!(reply.status, 403);
    assert_eq!(reply.body, stepup::StepUpError::Missing.to_string());

    // Destructive with a valid hybrid signature: dispatched and announced.
    let now = chrono::Utc::now().timestamp();
    let sig = phone.signature("remove_worktree", &args, &[1u8; 16], now);
    let reply = desktop
        .call(
            &phone,
            "remove_worktree",
            &[(stepup::HEADER, &sig)],
            Some(&body),
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.body);

    // The same signature again: the nonce is spent.
    let reply = desktop
        .call(
            &phone,
            "remove_worktree",
            &[(stepup::HEADER, &sig)],
            Some(&body),
        )
        .await;
    assert_eq!(reply.status, 403);
    assert_eq!(reply.body, stepup::StepUpError::NonceReused.to_string());

    // What reached the host, in order, with the device's name and the
    // parsed arguments; and exactly one destructive notice.
    let calls = desktop.host.calls.lock().unwrap().clone();
    assert_eq!(
        calls,
        vec![
            (
                "get_cached".to_string(),
                Value::Null,
                "Octocat's phone".to_string()
            ),
            (
                "set_poll_interval".to_string(),
                json!({"secs": 120}),
                "Octocat's phone".to_string()
            ),
            (
                "remove_worktree".to_string(),
                args.clone(),
                "Octocat's phone".to_string()
            ),
        ]
    );
    assert_eq!(
        *desktop.host.notices.lock().unwrap(),
        vec![("Octocat's phone".to_string(), "remove_worktree".to_string())]
    );

    // An event: the snapshot on connect, then what the desktop emits.
    let mut stream = SseClient::connect(desktop.addr, &phone.cert, &desktop.fp).await;
    assert_eq!(stream.status, 200);
    assert_eq!(
        stream.next_frame().await,
        Some(("prs-updated".into(), "[]".into()))
    );
    desktop.hub.publish("poll-state", "\"fetching\"".into());
    assert_eq!(
        stream.next_frame().await,
        Some(("poll-state".into(), "\"fetching\"".into()))
    );

    // An idle keep-alive connection, to prove revocation closes it.
    let mut idle = connect(desktop.addr, Some(&phone.cert), &desktop.fp)
        .await
        .unwrap();
    idle.write_all(b"GET /v1/hello HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut buf = vec![0u8; 4096];
    let n = idle.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));

    // Revoke, from Settings.
    desktop
        .pairing
        .revoke(&desktop.conn, outcome.device_id)
        .unwrap();
    assert!(devices::list(&desktop.conn).unwrap().is_empty());

    // The open stream ends, cleanly.
    assert_eq!(stream.next_frame().await, None);
    assert!(stream.body().1, "the stream's body is finished, not cut");
    // The idle connection is closed by the desktop.
    let closed = tokio::time::timeout(Duration::from_secs(5), idle.read(&mut buf)).await;
    assert!(
        matches!(closed, Ok(Ok(0)) | Ok(Err(_))),
        "the revoked device's idle connection must be closed: {closed:?}"
    );
    // The next handshake is refused.
    assert!(desktop.hello(&phone).await.is_err());

    desktop.handle.stop().await;
}

/// The refusals `/v1/call` owes the phone, each with the status the
/// module docs promise, and none of them reaching the host.
#[tokio::test]
async fn call_refuses_what_the_allowlist_and_the_body_rule_out() {
    let mut desktop = desktop().await;
    let phone = Phone::new();
    desktop.pair(&phone, "Octocat's phone").await;

    let unknown = desktop.call(&phone, "drop_database", &[], None).await;
    assert_eq!(unknown.status, 404);
    assert_eq!(
        unknown.body,
        RemoteError::Unknown("drop_database".into()).to_string()
    );

    let local = desktop.call(&phone, "reveal_log", &[], None).await;
    assert_eq!(local.status, 403);
    assert_eq!(
        local.body,
        RemoteError::Local("reveal_log".into()).to_string()
    );

    let revoke = desktop
        .call(&phone, "revoke_paired_device", &[], None)
        .await;
    assert_eq!(revoke.status, 403, "a phone cannot revoke a phone");

    let not_json = desktop
        .call(&phone, "get_history", &[], Some("days=14"))
        .await;
    assert_eq!(not_json.status, 400);
    assert!(not_json.body.starts_with("request body is not JSON"));

    let malformed_sig = desktop
        .call(
            &phone,
            "remove_worktree",
            &[(stepup::HEADER, "v2;nope")],
            Some("{}"),
        )
        .await;
    assert_eq!(malformed_sig.status, 400);

    assert!(desktop.host.calls.lock().unwrap().is_empty());

    // What the host itself reports comes back as its status, and as JSON
    // carrying the classified kind beside the verbatim message (#1202).
    *desktop.host.fail_with.lock().unwrap() = Some(RemoteError::Command(
        crate::remote::error_kind::CommandError::classify("boom"),
    ));
    let failed = desktop.call(&phone, "get_cached", &[], None).await;
    assert_eq!(failed.status, 500);
    let body: serde_json::Value = serde_json::from_str(&failed.body)
        .expect("a command rejection travels as JSON, not plain text");
    assert_eq!(body["message"], "boom");
    assert_eq!(body["kind"], "other");

    // A declined request is distinguishable WITHOUT reading the prose --
    // which is the whole point of the change.
    *desktop.host.fail_with.lock().unwrap() = Some(RemoteError::Command(
        crate::remote::error_kind::CommandError::classify(crate::commands::AUTH_ERR),
    ));
    let declined = desktop.call(&phone, "get_cached", &[], None).await;
    let declined: serde_json::Value = serde_json::from_str(&declined.body).unwrap();
    assert_eq!(declined["kind"], "not-asked");
    assert_eq!(declined["message"], crate::commands::AUTH_ERR);
    *desktop.host.fail_with.lock().unwrap() = Some(RemoteError::BadArgs {
        command: "get_history".into(),
        message: "missing required argument `days`".into(),
    });
    let bad_args = desktop.call(&phone, "get_history", &[], None).await;
    assert_eq!(bad_args.status, 400);

    desktop.handle.stop().await;
}

/// Pairing's refusals over the wire: a body that does not decode is
/// 400 and keeps the token; a denied request is 403 and pairs nothing.
#[tokio::test]
async fn pair_refuses_a_bad_body_and_a_denied_request() {
    let mut desktop = desktop().await;
    let phone = Phone::new();

    desktop.issue_qr();
    let reply = desktop
        .post(&phone, "/v1/pair", &[], Some("not json"))
        .await
        .unwrap();
    assert_eq!(reply.status, 400);
    assert!(reply.body.starts_with("bad pair request"));
    assert!(desktop.pairing.pairing_window_open());

    let issued = desktop.issue_qr();
    let token = BASE64URL.decode(&issued.token).unwrap();
    let req = PairRequest {
        token: issued.token,
        device_name: "Octocat's phone".into(),
        signing_keys: phone.signing_keys(),
        proof: pairing::proof(&token, &phone.fingerprint(), &desktop.fp),
    };
    let body = serde_json::to_string(&req).unwrap();
    let reply = {
        let addr = desktop.addr;
        let fp = desktop.fp.clone();
        let cert = phone.cert.clone();
        tokio::spawn(async move {
            request(addr, Some(&cert), &fp, "POST", "/v1/pair", &[], Some(&body)).await
        })
    };
    let event = desktop.requests.recv().await.unwrap();
    desktop
        .pairing
        .respond(&desktop.conn, event.request_id, PairDecision::Deny)
        .unwrap();
    let reply = reply.await.unwrap().unwrap();
    assert_eq!(reply.status, 403);
    assert_eq!(reply.body, pairing::PairError::Denied.to_string());
    assert!(devices::list(&desktop.conn).unwrap().is_empty());
    assert!(!desktop.pairing.is_paired(&phone.fingerprint()));

    desktop.handle.stop().await;
}

/// A stand-in for `size_worktrees` (#1459): takes a permit from its own
/// pool, as `commands::size_worktrees` takes `scan_permit()`, publishes
/// one `worktree-size` frame per worktree on the hub, holds the permit
/// for `hold`, and replies with every pair -- the command's two outputs,
/// the stream and the settled result.
///
/// The frames go out in one synchronous burst, with no `.await` between
/// them. On the single-threaded test runtime that means no subscriber
/// can drain between two sends, which is what makes "more frames than
/// the hub buffers" a deterministic condition rather than a race.
struct SlowSizer {
    hub: Arc<Hub>,
    permits: Arc<tokio::sync::Semaphore>,
    worktrees: usize,
    hold: Duration,
    started: AtomicUsize,
    finished: AtomicUsize,
    abandoned: AtomicUsize,
}

impl SlowSizer {
    fn new(hub: Arc<Hub>, worktrees: usize, hold: Duration) -> Self {
        Self {
            hub,
            permits: Arc::new(tokio::sync::Semaphore::new(1)),
            worktrees,
            hold,
            started: AtomicUsize::new(0),
            finished: AtomicUsize::new(0),
            abandoned: AtomicUsize::new(0),
        }
    }
}

/// Counts a dispatch future dropped before it finished: the listener
/// cancelling a call whose phone went away.
struct Abandoned<'a> {
    count: &'a AtomicUsize,
    done: bool,
}

impl Drop for Abandoned<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl CommandHost for SlowSizer {
    fn dispatch<'a>(
        &'a self,
        _command: &'a str,
        _args: Value,
        _device_name: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, RemoteError>> + Send + 'a>>
    {
        Box::pin(async move {
            let mut guard = Abandoned {
                count: &self.abandoned,
                done: false,
            };
            let _permit = self.permits.acquire().await.expect("the pool is open");
            self.started.fetch_add(1, Ordering::SeqCst);
            let mut pairs = Vec::new();
            for i in 0..self.worktrees {
                let pair = json!([format!("/src/repo/.worktrees/wt-{i}"), 1000 + i]);
                self.hub.publish("worktree-size", pair.to_string());
                pairs.push(pair);
            }
            tokio::time::sleep(self.hold).await;
            self.finished.fetch_add(1, Ordering::SeqCst);
            guard.done = true;
            Ok(Value::Array(pairs))
        })
    }
    fn notify_destructive(&self, _: &str, _: &str) {}
}

/// A paired phone against a desktop whose command host is a
/// `SlowSizer` sharing the listener's hub.
async fn sizing_desktop(worktrees: usize, hold: Duration) -> (Desktop<SlowSizer>, Phone) {
    let hub = Arc::new(Hub::new(Arc::new(|| {
        Box::pin(async { Some("[]".to_string()) })
    })));
    let host = Arc::new(SlowSizer::new(hub.clone(), worktrees, hold));
    let mut desktop = desktop_on(host, hub).await;
    let phone = Phone::new();
    desktop.pair(&phone, "Test phone").await;
    (desktop, phone)
}

/// #1459, candidate 2: a burst of `worktree-size` frames larger than the
/// hub buffers is LOST, the stream is cut, and nothing replays it -- but
/// the command's reply still carries every size.
///
/// The first half is why a phone cannot rely on the stream: the
/// subscriber that fell behind loses the whole backlog (MEASURED while
/// investigating: a 300-frame burst delivered none of them), and the
/// reconnect replays only the PR snapshot. The second half is what the
/// phone recovers from instead, and what `src/api/hooks.ts` reads: the
/// settled result is authoritative, so a row fills when the call
/// answers whether or not a single frame arrived.
#[tokio::test]
async fn a_size_burst_the_stream_drops_still_arrives_in_the_reply() {
    let burst = crate::remote::events::CAPACITY + 44;
    let (desktop, phone) = sizing_desktop(burst, Duration::ZERO).await;
    let mut stream = SseClient::connect(desktop.addr, &phone.cert, &desktop.fp).await;
    assert_eq!(
        stream.next_frame().await.map(|f| f.0),
        Some("prs-updated".into())
    );

    let reply = desktop
        .call(
            &phone,
            "size_worktrees",
            &[],
            Some(r#"{"repoPath":"/src/repo"}"#),
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    let pairs: Vec<Value> = serde_json::from_str(&reply.body).unwrap();
    assert_eq!(pairs.len(), burst, "the reply carries every size");

    let mut delivered = 0;
    while let Some((name, _)) = stream.next_frame().await {
        assert_eq!(name, "worktree-size");
        delivered += 1;
    }
    assert!(
        delivered < burst,
        "the stream carried all {burst} frames; the hub's buffer no longer drops a burst \
         this size, and this test no longer describes it"
    );

    desktop.handle.stop().await;
}

/// #1459, candidate 1: a call the phone gives up on releases its permit
/// on the desktop, so the next call is not queued behind a walk no one
/// is waiting for.
///
/// The phone's `CALL_TIMEOUT` closes the connection. The listener drops
/// the dispatch future with it, and the permit the future holds goes
/// with it -- which rules out the amplifier where every timed-out phone
/// call kept a scan permit and pushed the NEXT call past its own
/// deadline too.
///
/// QUALIFICATION, for whoever reads this next: in production the walk
/// itself runs on `spawn_blocking`, which a dropped future cannot stop.
/// The permit is released; the disk work is not, and finishes on its
/// own. That is a cost in contention, not a queue that never drains.
#[tokio::test]
async fn a_call_the_phone_abandons_releases_its_permit() {
    let (desktop, phone) = sizing_desktop(1, Duration::from_secs(30)).await;

    let mut tls = connect(desktop.addr, Some(&phone.cert), &desktop.fp)
        .await
        .unwrap();
    tls.write_all(
        b"POST /v1/call/size_worktrees HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n{}",
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while desktop.host.started.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the call reached the host");
    assert_eq!(desktop.host.permits.available_permits(), 0);

    // The phone's deadline passes: it closes the connection.
    let _ = tls.shutdown().await;
    drop(tls);

    tokio::time::timeout(Duration::from_secs(5), async {
        while desktop.host.permits.available_permits() == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the abandoned call's permit was never released");
    assert_eq!(desktop.host.abandoned.load(Ordering::SeqCst), 1);
    assert_eq!(desktop.host.finished.load(Ordering::SeqCst), 0);

    // And the next call starts at once rather than after the 30s hold.
    let mut next = connect(desktop.addr, Some(&phone.cert), &desktop.fp)
        .await
        .unwrap();
    next.write_all(
        b"POST /v1/call/size_worktrees HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n{}",
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while desktop.host.started.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the next call waited on a permit no one was using");
    drop(next);

    desktop.handle.stop().await;
}

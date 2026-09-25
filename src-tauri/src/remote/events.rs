//! Fan-out of the desktop's Tauri events to paired phones over
//! server-sent events: `GET /v1/events`.
//!
//! # How events get here
//!
//! The poll loop and a few commands `app.emit(...)` the nine events in
//! [`EVENT_NAMES`], which the frontend's hooks listen for. The hub taps
//! them with `listen_any` on the `AppHandle`, one listener per name,
//! rather than by wrapping each emit site in a helper that also pushes
//! here. Two reasons.
//!
//! A `listen_any` listener sees EVERY emit of that name, from any file,
//! including sites added after this module was written; a helper only
//! catches the sites someone remembered to convert, and a missed one is
//! silent -- the desktop updates and the phone does not.
//!
//! And what the listener receives is the very string Tauri hands the
//! webview: `emit` serialises the payload once with
//! `serde_json::to_string` and delivers that string to Rust and
//! JavaScript listeners alike (`tauri::event::EmitArgs::new`), so the
//! SSE `data:` field is the webview's JSON byte for byte, with no
//! second serialisation that could drift from it. The webview's own
//! payloads are untouched: the hub reads them and never re-emits.
//!
//! Tauri runs Rust listeners on the emitting thread, under its listener
//! lock, so the callback does one thing: push onto a
//! `tokio::sync::broadcast` channel, which never blocks and never
//! re-enters Tauri.
//!
//! # The stream a phone sees
//!
//! Each frame is
//!
//! ```text
//! event: <tauri event name>\n
//! data: <the JSON the webview received>\n
//! \n
//! ```
//!
//! The first frame is always a `prs-updated` carrying the cached
//! snapshot -- the same list `get_cached` returns -- so a phone renders
//! immediately after (re)connecting instead of waiting up to a poll
//! interval. A bare comment line `:` goes out after every [`KEEP_ALIVE`]
//! of silence so NAT tables stay warm and the phone can tell idle from
//! dead.
//!
//! The stream ends, and the phone's client reconnects, when:
//!
//! - the device's certificate is revoked. The pairing task broadcasts
//!   the fingerprint; the per-request gate in `listener.rs` cannot help
//!   here because a stream is one long request, so this watch is the
//!   only exit;
//! - the subscriber fell more than the channel's capacity behind. The
//!   dropped frames may have included a `prs-updated`, and a reconnect
//!   replays the snapshot, which is the cheapest way to be correct again;
//! - the listener stops.

use crate::remote::listener::PairedCerts;
use axum::response::sse::{self, KeepAlive, Sse};
use futures_util::stream::{self, Stream, StreamExt};
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Listener};
use tokio::sync::broadcast::{self, error::RecvError};

/// The route this module serves.
pub const PATH: &str = "/v1/events";

/// The Tauri events a phone receives, under these exact names. The
/// spec's list; the frontend's hooks in `src/api/hooks.ts` listen for
/// the same names on both builds.
///
/// COMPLETENESS against the frontend is asserted by
/// `src/lib/mirroredConstants.test.ts`, which reads this list via Vite's
/// `?raw` and requires a `POLL_EVENTS` row in `src/api/transport.test.ts`
/// for every name here, and no row for a name absent here. Nothing tied
/// the two together before #850, and `POLL_EVENTS` had in fact lost
/// `"update-run-progress"`.
///
/// The check is worth its weight because the failure it catches is
/// silent and desktop-invisible: a hook that imports Tauri's `listen`
/// directly instead of going through the transport seam works perfectly
/// on the desktop and never fires on the phone, so the page simply never
/// fills in, with no error anywhere.
pub const EVENT_NAMES: &[&str] = &[
    "prs-updated",
    "poll-state",
    "poll-error",
    "prs-truncated",
    "prs-incomplete",
    "store-error",
    "worktree-removal-progress",
    "reviewing-short",
    "update-run-progress",
    "update-run-done",
    // Widening this list widens a security boundary -- see the module
    // docs on why only these names are re-emitted. Added deliberately
    // (#657) so a phone sees branch rows as they classify rather than
    // a blank page for ten seconds; it carries branch names and
    // verdicts the phone is about to be shown anyway, and no paths.
    "branch-scan-progress",
    // Also deliberate (#724): a bulk deletion ran for ten minutes with
    // nothing on screen. Counts only -- no branch names and no paths,
    // the `worktree-removal-progress` rule.
    "branch-delete-progress",
    // The first entry to break the "counts only" rule above, and the
    // break is deliberate (#754): this one carries a worktree PATH.
    //
    // What makes it acceptable is that `size_worktrees` -- already on
    // the command allowlist -- RETURNS these same `(path, bytes)` pairs
    // to the same client over the same transport. The event is that
    // data arriving per worktree instead of in one batch at the end, so
    // it opens nothing the command has not already opened.
    //
    // It is needed because the walk has no useful upper bound: MEASURED,
    // 21.40s for a single 200 GB checkout, and the cost tracks bytes
    // rather than worktree count. Batching at the end is what left a
    // 100-worktree page showing skeletons for ten minutes.
    "worktree-size",
    // The thirteenth, and it carries the richest payload on this list: a
    // whole `Worktree`, so a path, a branch name, and a safety verdict
    // (#830).
    //
    // Weighed on the `worktree-size` test rather than waved through, and
    // it passes the same way: `classify_worktrees` -- already on the
    // command allowlist as a Read -- RETURNS a `Vec<Worktree>` of these
    // very values to this phone over this transport. The event is that
    // same data arriving per worktree instead of in one batch at the
    // end, so it opens nothing the command has not already opened, and
    // withholding it would not hide a branch name, only delay it.
    //
    // It has to be here because classification has no useful upper bound
    // either, and for a reason a per-call timeout cannot catch:
    // `content_landed` spends up to four git calls PER CHANGED FILE, so
    // one branch can cost an unbounded number of individually-bounded
    // calls. `CLASSIFY_TIMEOUT` is the ceiling; this event is how a row
    // hears the verdict before the slowest branch in the repository is
    // done. A phone feels that hardest: it cannot leave a window open,
    // so what it can show is what arrives while it is in the foreground.
    "worktree-safety",
    // The PR Stats backfill's coverage (#1093). Counts and one scope key,
    // which is the `org:X` or `owner/repo` string the phone just ASKED
    // about -- no repository list, no logins, no titles, no paths.
    //
    // Weighed on the same test as the entries above and it passes more
    // easily than most: `stats_board` is already an allowlisted Read that
    // returns a whole leaderboard for this scope to this phone over this
    // transport, so a day count about that same scope opens nothing the
    // command has not.
    //
    // It has to be here because the work is unbounded in TIME by design.
    // A backfill deliberately spends minutes to hours walking a horizon a
    // point at a time, and a phone feels that hardest -- it cannot leave a
    // window open, so what it can show is what arrives while it is in the
    // foreground. Without this the phone shows a caveat that never moves,
    // which is indistinguishable from one that is broken.
    "stats-backfill-progress",
];

/// The event name the opening snapshot frame is sent under, so the
/// phone handles it exactly as it handles a poll result.
pub const SNAPSHOT_EVENT: &str = "prs-updated";

/// Silence before a keep-alive comment is written.
pub const KEEP_ALIVE: Duration = Duration::from_secs(15);

/// Frames buffered per subscriber before it is considered lost. The
/// burstiest producer is `worktree-removal-progress`, one frame per
/// worktree removed; a phone keeps up with that unless its socket has
/// stopped draining, and then ending the stream is the right answer.
pub(crate) const CAPACITY: usize = 256;

/// One event as the webview received it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Emitted {
    /// The Tauri event name; becomes the SSE `event:` field.
    pub name: String,
    /// The serialised payload, as `serde_json::to_string` produced it;
    /// becomes the SSE `data:` field verbatim.
    pub json: String,
}

/// Produces the opening snapshot: the serialised `Vec<PullRequest>`
/// that `get_cached` returns. A closure, like the listener's
/// `ViewerLookup`, so this module needs neither the database nor a
/// Tauri app to be tested. `None` means no snapshot frame is sent (the
/// database could not be read); the phone falls back to `get_cached`
/// over `/v1/call` as it would anyway.
pub type SnapshotSource =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Option<String>> + Send>> + Send + Sync>;

/// The broadcast every subscriber hangs off. One per process, held in
/// `gate::Remote`, attached to the app once at startup; the listener
/// clones the `Arc` each time it starts.
pub struct Hub {
    tx: broadcast::Sender<Emitted>,
    snapshot: SnapshotSource,
}

impl Hub {
    pub fn new(snapshot: SnapshotSource) -> Self {
        let (tx, _) = broadcast::channel(CAPACITY);
        Self { tx, snapshot }
    }

    /// Tap every emit of every name in [`EVENT_NAMES`]. Call once; the
    /// listeners live as long as the app and are never removed.
    pub fn attach(&self, app: &AppHandle) {
        for name in EVENT_NAMES {
            let tx = self.tx.clone();
            app.listen_any(*name, move |event| {
                // No receivers is the usual state (no phone connected)
                // and not an error.
                let _ = tx.send(Emitted {
                    name: (*name).to_string(),
                    json: event.payload().to_string(),
                });
            });
        }
    }

    /// Push an event by hand. `json` must already be serialised, as
    /// `serde_json::to_string` would produce it, because it is written
    /// to the wire verbatim. Production goes through [`Hub::attach`];
    /// this is for tests and for code with no `AppHandle` in reach.
    pub fn publish(&self, name: &str, json: String) {
        let _ = self.tx.send(Emitted {
            name: name.to_string(),
            json,
        });
    }

    fn subscribe(&self) -> broadcast::Receiver<Emitted> {
        self.tx.subscribe()
    }

    async fn snapshot(&self) -> Option<String> {
        (self.snapshot)().await
    }
}

/// What one subscriber brings: who it is and how to learn it was
/// revoked.
pub struct Subscriber {
    /// The peer's certificate fingerprint, from the `Peer` extension.
    pub fingerprint: String,
    /// `PairingState::subscribe_revocations()`; a message naming
    /// `fingerprint` ends the stream.
    pub revocations: broadcast::Receiver<String>,
    /// Consulted only when the revocation channel lagged, since the
    /// missed message may have been this device's.
    pub paired: Arc<dyn PairedCerts>,
}

/// Open the stream for one paired phone. `None` when the device is not
/// paired -- which the listener's gate has already checked, but the
/// gate ran before `sub.revocations` existed and a revocation in that
/// gap would otherwise go unseen.
pub async fn subscribe(
    hub: &Hub,
    sub: Subscriber,
) -> Option<Sse<impl Stream<Item = Result<sse::Event, Infallible>>>> {
    // Subscribe BEFORE reading the snapshot: an update landing between
    // the two is then queued behind the snapshot rather than lost.
    let events = hub.subscribe();
    if !sub.paired.is_paired(&sub.fingerprint) {
        return None;
    }
    let first = hub.snapshot().await.map(|json| Emitted {
        name: SNAPSHOT_EVENT.to_string(),
        json,
    });
    let frames = stream::iter(first)
        .chain(live(events, sub))
        .map(|e| Ok(sse::Event::default().event(e.name).data(e.json)));
    Some(Sse::new(frames).keep_alive(KeepAlive::new().interval(KEEP_ALIVE)))
}

/// Events after the snapshot, until one of the exits in the module docs.
fn live(
    events: broadcast::Receiver<Emitted>,
    sub: Subscriber,
) -> impl Stream<Item = Emitted> + Send {
    stream::unfold((events, sub), |(mut events, mut sub)| async move {
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    Ok(e) => return Some((e, (events, sub))),
                    Err(RecvError::Lagged(n)) => {
                        log::info!(
                            "remote: an event subscriber fell {n} frames behind; \
                             ending its stream so it reconnects and gets a snapshot"
                        );
                        return None;
                    }
                    Err(RecvError::Closed) => return None,
                },
                revoked = sub.revocations.recv() => match revoked {
                    Ok(fp) if fp == sub.fingerprint => {
                        log::info!("remote: ending the event stream of a revoked device");
                        return None;
                    }
                    Ok(_) => {}
                    Err(RecvError::Lagged(_)) => {
                        if !sub.paired.is_paired(&sub.fingerprint) {
                            return None;
                        }
                    }
                    // The pairing state is gone, so nothing could tell
                    // us about a revocation: better no stream than one
                    // that can never be cut.
                    Err(RecvError::Closed) => return None,
                },
            }
        }
    })
}

/// `pub(crate)`: the loopback test in `remote/loopback_tests.rs` reads
/// the event stream through the same client as the tests here.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::remote::identity::Identity;
    use crate::remote::listener::tests::{client_config, serve_with, MemoryCerts};
    use serde_json::json;
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio_rustls::client::TlsStream;

    fn fixed_snapshot(json: Option<String>) -> SnapshotSource {
        Arc::new(move || {
            let json = json.clone();
            Box::pin(async move { json })
        })
    }

    fn subscriber(fp: &str, certs: &Arc<MemoryCerts>) -> (Subscriber, broadcast::Sender<String>) {
        let (tx, rx) = broadcast::channel(16);
        let sub = Subscriber {
            fingerprint: fp.to_string(),
            revocations: rx,
            paired: certs.clone(),
        };
        (sub, tx)
    }

    /// What Tauri's `emit` hands the webview for a payload: one compact
    /// `serde_json::to_string`. The hub must deliver exactly this.
    fn webview_json(payload: &serde_json::Value) -> String {
        serde_json::to_string(payload).unwrap()
    }

    // ---- the stream, without a socket ------------------------------

    #[tokio::test]
    async fn a_published_event_reaches_a_subscriber_unchanged() {
        let hub = Hub::new(fixed_snapshot(None));
        let certs = Arc::new(MemoryCerts::default());
        certs.pair("ab");
        let (sub, _keep) = subscriber("ab", &certs);
        let mut s = Box::pin(live(hub.subscribe(), sub));

        let payload = json!({"repoPath": "/tmp/octocat/hello-world", "applied": 2, "failed": 0});
        hub.publish("update-run-done", webview_json(&payload));
        assert_eq!(
            s.next().await,
            Some(Emitted {
                name: "update-run-done".into(),
                json: webview_json(&payload),
            })
        );
    }

    #[tokio::test]
    async fn revoking_this_device_ends_the_stream_and_revoking_another_does_not() {
        let hub = Hub::new(fixed_snapshot(None));
        let certs = Arc::new(MemoryCerts::default());
        certs.pair("ab");
        let (sub, revocations) = subscriber("ab", &certs);
        let mut s = Box::pin(live(hub.subscribe(), sub));

        revocations.send("cd".into()).unwrap();
        hub.publish("poll-state", "\"idle\"".into());
        assert_eq!(s.next().await.map(|e| e.name), Some("poll-state".into()));

        revocations.send("ab".into()).unwrap();
        assert_eq!(s.next().await, None);
    }

    /// A subscriber that fell behind is cut rather than fed a gap: its
    /// reconnect replays the snapshot, which a gap could never do.
    #[tokio::test]
    async fn a_lagging_subscriber_is_ended() {
        let hub = Hub::new(fixed_snapshot(None));
        let certs = Arc::new(MemoryCerts::default());
        certs.pair("ab");
        let (sub, _keep) = subscriber("ab", &certs);
        let mut s = Box::pin(live(hub.subscribe(), sub));

        for i in 0..=CAPACITY {
            hub.publish("worktree-removal-progress", format!("[{i},{CAPACITY}]"));
        }
        assert_eq!(s.next().await, None);
    }

    /// The gate ran before the revocation watch existed; `subscribe`
    /// re-checks so a device revoked in between gets nothing.
    #[tokio::test]
    async fn subscribe_refuses_a_device_that_is_no_longer_paired() {
        let hub = Hub::new(fixed_snapshot(Some("[]".into())));
        let certs = Arc::new(MemoryCerts::default());
        let (sub, _keep) = subscriber("ab", &certs);
        assert!(subscribe(&hub, sub).await.is_none());
    }

    // ---- over the wire ---------------------------------------------

    /// A phone's view of the stream: one mTLS connection, the HTTP/1.1
    /// response head, then SSE frames out of hyper's chunked body.
    pub(crate) struct SseClient {
        tls: TlsStream<TcpStream>,
        raw: Vec<u8>,
        pub(crate) status: u16,
        pub(crate) content_type: String,
        body_start: usize,
        delivered: usize,
    }

    impl SseClient {
        pub(crate) async fn connect(addr: SocketAddr, phone: &Identity, server_fp: &str) -> Self {
            let connector = tokio_rustls::TlsConnector::from(client_config(Some(phone), server_fp));
            let tcp = TcpStream::connect(addr).await.unwrap();
            let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
            let mut tls = connector.connect(name, tcp).await.unwrap();
            tls.write_all(
                format!(
                    "GET {PATH} HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\r\n"
                )
                .as_bytes(),
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
                raw,
                status,
                content_type: header("content-type"),
                body_start: head_end + 4,
                delivered: 0,
            }
        }

        /// The SSE body decoded so far, plus whether the final chunk
        /// has arrived.
        pub(crate) fn body(&self) -> (Vec<u8>, bool) {
            dechunk(&self.raw[self.body_start..])
        }

        /// The next `(event, data)` frame, or `None` once the server has
        /// finished the body. Panics on silence, so a hang is a failure
        /// rather than a wait.
        pub(crate) async fn next_frame(&mut self) -> Option<(String, String)> {
            loop {
                let (body, done) = self.body();
                let frames = parse_frames(&body);
                if let Some(f) = frames.get(self.delivered) {
                    self.delivered += 1;
                    return Some(f.clone());
                }
                if done {
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
        match tokio::time::timeout(Duration::from_secs(5), tls.read(buf)).await {
            Ok(Ok(n)) => n,
            // The server's close_notify or a reset after it dropped the
            // connection both read as "no more bytes".
            Ok(Err(_)) => 0,
            Err(_) => panic!("no bytes from the event stream within 5s"),
        }
    }

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    /// Decode the complete chunks of an HTTP/1.1 chunked body; the
    /// second value is whether the terminating zero-length chunk arrived.
    fn dechunk(mut b: &[u8]) -> (Vec<u8>, bool) {
        let mut out = Vec::new();
        loop {
            let Some(nl) = find(b, b"\r\n") else {
                return (out, false);
            };
            let size = usize::from_str_radix(std::str::from_utf8(&b[..nl]).unwrap().trim(), 16)
                .expect("a chunk-size line");
            if size == 0 {
                return (out, true);
            }
            let start = nl + 2;
            let end = start + size;
            if b.len() < end + 2 {
                return (out, false);
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

    /// The checklist test: the JSON the webview received arrives on the
    /// phone byte for byte, under the same event name, after the
    /// snapshot frame that lets it paint before the next poll.
    #[tokio::test]
    async fn a_phone_gets_the_snapshot_then_each_emit_with_the_webviews_json() {
        let snapshot = json!([{
            "id": "PR_kwDOA", "number": 1347, "title": "Amazing new feature",
            "url": "https://github.com/octocat/hello-world/pull/1347",
            "repo": "octocat/hello-world", "author": "octocat", "isDraft": false
        }]);
        let hub = Arc::new(Hub::new(fixed_snapshot(Some(webview_json(&snapshot)))));
        let certs = Arc::new(MemoryCerts::default());
        let server = serve_with(certs, hub.clone()).await;
        let phone = Identity::generate().unwrap();
        server.certs.pair(&phone.fingerprint());

        let mut client = SseClient::connect(server.handle.local_addr(), &phone, &server.fp).await;
        assert_eq!(client.status, 200);
        assert!(
            client.content_type.starts_with("text/event-stream"),
            "content-type was {}",
            client.content_type
        );

        // Snapshot first, before any poll.
        assert_eq!(
            client.next_frame().await,
            Some(("prs-updated".into(), webview_json(&snapshot)))
        );

        // Then what the poll loop emits, exactly as the webview sees it.
        let update = json!([{
            "id": "PR_kwDOB", "number": 1348, "title": "Fix the thing\nwith a newline",
            "url": "https://github.com/octocat/spoon-knife/pull/1348",
            "repo": "octocat/spoon-knife", "author": "octocat", "isDraft": true
        }]);
        hub.publish("prs-updated", webview_json(&update));
        assert_eq!(
            client.next_frame().await,
            Some(("prs-updated".into(), webview_json(&update)))
        );
        hub.publish("poll-state", webview_json(&json!("fetching")));
        assert_eq!(
            client.next_frame().await,
            Some(("poll-state".into(), "\"fetching\"".into()))
        );

        // And the framing on the wire is the plain SSE form, one data
        // line per frame.
        let (body, _) = client.body();
        let expected = format!(
            "event: prs-updated\ndata: {}\n\nevent: prs-updated\ndata: {}\n\nevent: poll-state\ndata: \"fetching\"\n\n",
            webview_json(&snapshot),
            webview_json(&update)
        );
        assert_eq!(String::from_utf8(body).unwrap(), expected);
        server.handle.stop().await;
    }

    #[tokio::test]
    async fn a_revoked_phones_stream_ends() {
        let hub = Arc::new(Hub::new(fixed_snapshot(Some("[]".into()))));
        let certs = Arc::new(MemoryCerts::default());
        let server = serve_with(certs, hub.clone()).await;
        let phone = Identity::generate().unwrap();
        server.certs.pair(&phone.fingerprint());
        let other = Identity::generate().unwrap();
        server.certs.pair(&other.fingerprint());

        let mut client = SseClient::connect(server.handle.local_addr(), &phone, &server.fp).await;
        assert_eq!(
            client.next_frame().await,
            Some(("prs-updated".into(), "[]".into()))
        );

        // Someone else's revocation is not ours.
        server.certs.revoke(&other.fingerprint());
        server.revocations.send(other.fingerprint()).unwrap();
        hub.publish("poll-state", "\"idle\"".into());
        assert_eq!(
            client.next_frame().await,
            Some(("poll-state".into(), "\"idle\"".into()))
        );

        // Ours is.
        server.certs.revoke(&phone.fingerprint());
        server.revocations.send(phone.fingerprint()).unwrap();
        assert_eq!(client.next_frame().await, None, "the stream must end");
        assert!(
            client.body().1,
            "the body must be finished, not cut mid-chunk"
        );
        server.handle.stop().await;
    }

    #[tokio::test]
    async fn no_snapshot_means_no_opening_frame() {
        let hub = Arc::new(Hub::new(fixed_snapshot(None)));
        let certs = Arc::new(MemoryCerts::default());
        let server = serve_with(certs, hub.clone()).await;
        let phone = Identity::generate().unwrap();
        server.certs.pair(&phone.fingerprint());

        let mut client = SseClient::connect(server.handle.local_addr(), &phone, &server.fp).await;
        hub.publish("poll-error", "\"rate limited\"".into());
        assert_eq!(
            client.next_frame().await,
            Some(("poll-error".into(), "\"rate limited\"".into()))
        );
        server.handle.stop().await;
    }

    #[test]
    fn keep_alive_is_fifteen_seconds() {
        assert_eq!(KEEP_ALIVE, Duration::from_secs(15));
    }
}

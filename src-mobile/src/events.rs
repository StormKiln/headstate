//! The event subscriber: `GET /v1/events` on the paired desktop,
//! re-emitting each frame as a Tauri event under the same name, so the
//! frontend's hooks see `prs-updated` and the rest exactly as the
//! desktop's own webview does.
//!
//! # The wire (desktop `remote/events.rs`)
//!
//! ```text
//! event: <tauri event name>\n
//! data: <one-line JSON, as serde_json wrote it>\n
//! \n
//! ```
//!
//! A bare `:` comment line every 15 seconds is the keep-alive. The first
//! frame after connecting is always `prs-updated` with the cached
//! snapshot. The stream ends when the device is revoked, when it fell
//! too far behind, or when the listener stops; the phone reconnects and
//! gets a fresh snapshot. Only the names in [`EVENT_NAMES`] are
//! re-emitted; anything else is dropped, so the desktop cannot fire an
//! arbitrary event in the webview. The list is deliberately short, and
//! [`tests::the_allowlist_matches_the_desktops`] pins it to the
//! desktop's own copy so neither side can widen it alone.
//!
//! # The loop
//!
//! `/v1/hello` first, so the connection state carries the desktop's
//! protocol version, then the stream. Any failure that is not a
//! handshake refusal is `unreachable` and retried with backoff (1s
//! doubling to 30s); a handshake refusal, or a 403 from the path gate,
//! is `revoked` and the loop ends -- only re-pairing gets past that.
//! [`Handle::resume`] wakes a sleeping loop at once, for the frontend to
//! call when the app returns to the foreground: iOS kills the stream
//! when the app suspends, and this is how the phone catches up.
//!
//! The most recent `prs-updated` payload is kept in the store as the
//! snapshot, so the list renders (stale) while the desktop is away.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

use crate::client::{Client, ClientError, PROTOCOL_VERSION};
use crate::connection::{Connection, EventSink, State};
use crate::pairing;
use crate::store::{get_json, put_json, Store, StoreError};

/// The desktop events a phone receives, under these exact names.
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
    // Widening this list widens a security boundary: the whole point of
    // it is that the desktop cannot fire an arbitrary event in this
    // webview. Added deliberately (#657) so the Branches page fills in
    // as branches classify instead of showing nothing for ten seconds.
    // It carries branch names and deletability verdicts -- what the
    // page is about to render anyway -- and no filesystem paths.
    "branch-scan-progress",
    // Also deliberate (#724): a bulk deletion showed nothing for over
    // ten minutes, and a phone feels that hardest -- it has no window
    // to leave open and watch. Counts only: no branch names, no paths.
    "branch-delete-progress",
    // The twelfth, and the first that carries a PATH (#754). Weighed
    // rather than waved through, because the two entries above make a
    // point of carrying counts only.
    //
    // What tips it: `size_worktrees`, an already-allowlisted command,
    // RETURNS these very `(path, bytes)` pairs to this phone over this
    // transport. The event is the same data arriving earlier, so it
    // widens no boundary that the command has not already opened -- and
    // withholding it would not hide a path, only delay it.
    //
    // It has to be here at all because the walk is unbounded in
    // wall-clock terms (MEASURED: 21.40s for one 200 GB checkout), and
    // the phone is the client that suffers most: it cannot leave a
    // window open, so what it can show is what arrives while it is in
    // the foreground.
    "worktree-size",
    // The thirteenth, and the richest payload here: a whole `Worktree` --
    // path, branch name, and safety verdict (#830).
    //
    // Same test as the entry above, same answer. `classify_worktrees` is
    // already an allowlisted Read that RETURNS a `Vec<Worktree>` of these
    // values to this phone over this transport; the event is that data
    // arriving per worktree rather than in one batch, so it widens no
    // boundary the command has not already opened.
    //
    // Needed because classification is unbounded in a way a per-call
    // timeout cannot see: `content_landed` on the desktop spends up to
    // four git calls per CHANGED FILE, so one branch is an unbounded
    // number of individually-bounded calls. #830 is a 111-worktree
    // repository whose safety column never resolved at all.
    "worktree-safety",
    // The fourteenth: how much of a stats window has been collected so far
    // (#1093). Counts, a day tally, and the scope key this phone just
    // asked about -- no repository names, no logins, no titles.
    //
    // Same test as the entries above, same answer: `stats_board` is
    // already an allowlisted Read returning a whole leaderboard for this
    // scope over this transport, so counts about that scope widen nothing.
    //
    // Needed because a backfill is unbounded in time BY DESIGN -- it walks
    // a horizon a point at a time over minutes or hours -- and a phone
    // cannot leave a window open to wait it out. Without this the caveat
    // on the page never changes, and a number that never moves reads as
    // broken rather than as progressing.
    "stats-backfill-progress",
];

/// The event whose payload is the PR list, cached as the snapshot.
pub const SNAPSHOT_EVENT: &str = "prs-updated";

/// Store key for the snapshot.
pub const SNAPSHOT_KEY: &str = "snapshot";

pub const MIN_BACKOFF: Duration = Duration::from_secs(1);
pub const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// The cached PR list.
#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub v: u32,
    /// ISO 8601: when the frame arrived.
    pub received_at: String,
    /// The `prs-updated` payload verbatim.
    pub prs: Box<RawValue>,
}

const SNAPSHOT_VERSION: u32 = 1;

impl Snapshot {
    pub fn received_at(&self) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(&self.received_at)
            .ok()
            .map(|t| t.with_timezone(&Utc))
    }
}

pub fn cached_snapshot(store: &dyn Store) -> Result<Option<Snapshot>, StoreError> {
    get_json(store, SNAPSHOT_KEY)
}

pub fn save_snapshot(
    store: &dyn Store,
    prs_json: &str,
    at: DateTime<Utc>,
) -> Result<(), StoreError> {
    let prs = RawValue::from_string(prs_json.to_string()).map_err(|e| StoreError::Corrupt {
        key: SNAPSHOT_KEY.into(),
        what: "JSON",
        message: e.to_string(),
    })?;
    put_json(
        store,
        SNAPSHOT_KEY,
        &Snapshot {
            v: SNAPSHOT_VERSION,
            received_at: at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            prs,
        },
    )
}

pub fn forget_snapshot(store: &dyn Store) -> Result<(), StoreError> {
    store.remove(SNAPSHOT_KEY)
}

// ---------------------------------------------------------------------
// SSE parsing
// ---------------------------------------------------------------------

/// One event off the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub name: String,
    pub data: String,
}

/// An incremental parser for the event-stream format: feed it bytes as
/// they arrive, take the frames that completed. Handles frames split
/// across chunks, `\r\n` line ends, comment lines, and multi-line
/// `data:` (joined with `\n`, per the spec, though the desktop never
/// sends one).
///
/// # Bounds
///
/// Both accumulators are capped. A phone has a hard per-app memory
/// limit and the OS kills the app outright when it is crossed -- no
/// dialog, no log, and to the user an app that "just closes". The
/// desktop is a trusted peer, so this is not an attack path; it is a
/// resource path, and a stream that never sends a newline (or a blank
/// line) would grow `buf` (or `data`) without limit until jetsam took
/// the process.
///
/// Crossing a cap drops the buffered bytes and resets the parser rather
/// than trying to resynchronise mid-frame: the frame is already lost,
/// and the reconnect that follows replays the desktop's snapshot.
#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

/// The longest single line to buffer before giving up on it. Generous
/// against the real traffic -- a `prs-updated` frame for a large account
/// is tens of kilobytes on one `data:` line -- and still far below what
/// a phone can absorb.
const MAX_LINE: usize = 8 * 1024 * 1024;

/// The most `data:` to accumulate across one frame's lines, before the
/// blank line that dispatches it.
const MAX_FRAME: usize = 8 * 1024 * 1024;

impl SseParser {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Frame> {
        self.buf.extend_from_slice(bytes);
        // No newline in this much data means the peer is not speaking
        // the framing; nothing further in `buf` can be parsed.
        if self.buf.len() > MAX_LINE && !self.buf.contains(&b'\n') {
            log::warn!(
                "companion: dropping {} buffered bytes with no line end; resetting the parser",
                self.buf.len()
            );
            self.reset();
            return Vec::new();
        }
        let mut out = Vec::new();
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=nl).collect();
            let mut line = &line[..nl];
            if line.ends_with(b"\r") {
                line = &line[..line.len() - 1];
            }
            let line = String::from_utf8_lossy(line);
            if line.is_empty() {
                out.extend(self.dispatch());
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = match line.split_once(':') {
                Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
                None => (line.as_ref(), ""),
            };
            match field {
                "event" => self.event = Some(value.to_string()),
                "data" => {
                    self.data.push(value.to_string());
                    if self.data.iter().map(String::len).sum::<usize>() > MAX_FRAME {
                        log::warn!("companion: dropping an oversized frame; resetting the parser");
                        self.reset();
                        return out;
                    }
                }
                // `id` and `retry` are not used by the desktop.
                _ => {}
            }
        }
        out
    }

    /// Forget everything buffered. Whatever was mid-parse is gone, and
    /// the frames already returned from this `feed` are still good.
    fn reset(&mut self) {
        self.buf.clear();
        self.buf.shrink_to_fit();
        self.event = None;
        self.data.clear();
    }

    fn dispatch(&mut self) -> Option<Frame> {
        let event = self.event.take();
        let data = std::mem::take(&mut self.data);
        if data.is_empty() {
            return None;
        }
        Some(Frame {
            name: event.unwrap_or_else(|| "message".to_string()),
            data: data.join("\n"),
        })
    }
}

// ---------------------------------------------------------------------
// The subscriber
// ---------------------------------------------------------------------

/// Control over a running [`run`]: wake it or end it.
#[derive(Clone, Default)]
pub struct Handle {
    wake: Arc<Notify>,
    stopped: Arc<AtomicBool>,
}

impl Handle {
    pub fn new() -> Self {
        Self::default()
    }
    /// Reconnect now: drop the stream we have and open a fresh one.
    ///
    /// Called when the app returns to the foreground, which is the case
    /// this whole module exists for -- iOS ends the stream when the app
    /// suspends, and this is how the phone catches up.
    ///
    /// It used to only wake the select, and the wake arm did nothing on
    /// the reasoning that the stream was still up. It is not: after a
    /// suspension the `reqwest::Response` handle is alive while the
    /// socket underneath is dead, and Rust cannot tell. So the loop went
    /// straight back to awaiting `chunk()` on a corpse, and recovery
    /// waited out `STREAM_READ_TIMEOUT` (45s) -- longer still on a
    /// network change, where the old socket black-holes instead of
    /// resetting. The user saw a stale list under a banner that said
    /// "connected".
    pub fn resume(&self) {
        self.wake.notify_one();
    }
    /// End the loop at its next opportunity.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

/// Everything one subscriber needs.
pub struct Subscriber {
    pub client: Arc<Client>,
    pub sink: Arc<dyn EventSink>,
    pub store: Arc<dyn Store>,
    pub conn: Arc<Connection>,
    /// The desktop's fingerprint, to file its `/v1/hello` under.
    pub desktop_fp: String,
}

/// Sleep `d` or until woken. `false` when the handle was stopped.
async fn wait(handle: &Handle, d: Duration) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(d) => {}
        _ = handle.wake.notified() => {}
    }
    !handle.is_stopped()
}

fn next_backoff(d: Duration) -> Duration {
    (d * 2).min(MAX_BACKOFF)
}

/// Whether an error means the desktop no longer recognises this phone.
/// A handshake refusal is the spec's signal; a 403 is the desktop's
/// path gate saying "not paired" to a certificate the handshake let
/// through because a pairing window happened to be open.
fn is_revocation(e: &ClientError) -> bool {
    e.is_handshake() || matches!(e, ClientError::Status { status: 403, .. })
}

/// The loop described in the module docs. Runs until stopped or revoked.
pub async fn run(sub: Subscriber, handle: Handle) {
    let mut backoff = MIN_BACKOFF;
    loop {
        if handle.is_stopped() {
            return;
        }
        sub.conn.set_state(State::Connecting);

        let hello = match sub.client.hello().await {
            Ok(h) => h,
            Err(e) if is_revocation(&e) => {
                log::warn!("companion: the desktop refused this phone: {e}");
                sub.conn.set_state(State::Revoked);
                return;
            }
            Err(e) => {
                log::info!("companion: desktop unreachable: {e}");
                sub.conn.set_state(State::Unreachable);
                if !wait(&handle, backoff).await {
                    return;
                }
                backoff = next_backoff(backoff);
                continue;
            }
        };
        if hello.protocol_version != PROTOCOL_VERSION {
            // Reported through `connection_state.protocol_version` for
            // the frontend to say "desktop too old/new"; the stream is
            // still opened, since the event names are the stable part.
            log::warn!(
                "companion: desktop speaks protocol {} and this app speaks {PROTOCOL_VERSION}",
                hello.protocol_version
            );
        }
        if let Err(e) = pairing::record_hello(sub.store.as_ref(), &sub.desktop_fp, &hello) {
            log::warn!("companion: could not record hello: {e}");
        }
        sub.conn.connected(hello.protocol_version);

        match sub.client.events().await {
            Ok(mut resp) => {
                backoff = MIN_BACKOFF;
                let mut parser = SseParser::default();
                // Whether we left the read loop because the app came
                // back, rather than because the desktop went away. The
                // two deserve different handling below.
                let mut resumed = false;
                loop {
                    tokio::select! {
                        chunk = resp.chunk() => match chunk {
                            Ok(Some(bytes)) => {
                                for frame in parser.feed(&bytes) {
                                    deliver(&sub, frame);
                                }
                            }
                            Ok(None) => {
                                log::info!("companion: the event stream ended; reconnecting");
                                break;
                            }
                            Err(e) => {
                                log::info!("companion: the event stream failed: {e}");
                                break;
                            }
                        },
                        _ = handle.wake.notified() => {
                            if handle.is_stopped() {
                                return;
                            }
                            // Break, which drops `resp` and its socket,
                            // so the outer loop redoes `hello()` and
                            // `events()` on a fresh connection. The
                            // alternative -- carrying on with the
                            // handle we have -- is what made a resume a
                            // no-op: the only thing that can prove a
                            // suspended socket is dead is trying a new
                            // one.
                            log::info!("companion: resumed; reconnecting the event stream");
                            resumed = true;
                            break;
                        }
                    }
                }
                // A resume is not a failure. Reconnect immediately, and
                // do NOT report the desktop unreachable on the way: the
                // phone has been asleep, not the desktop, and a banner
                // that flashes "unreachable" every time the app is
                // opened teaches people to ignore it.
                if resumed {
                    continue;
                }
                // Ended streams reconnect after the minimum backoff, not
                // instantly, so a desktop that keeps ending them is not
                // hammered; a revocation shows on the next hello.
                sub.conn.set_state(State::Unreachable);
                if !wait(&handle, MIN_BACKOFF).await {
                    return;
                }
            }
            Err(e) if is_revocation(&e) => {
                log::warn!("companion: the desktop refused the event stream: {e}");
                sub.conn.set_state(State::Revoked);
                return;
            }
            Err(e) => {
                log::info!("companion: could not open the event stream: {e}");
                sub.conn.set_state(State::Unreachable);
                if !wait(&handle, backoff).await {
                    return;
                }
                backoff = next_backoff(backoff);
            }
        }
    }
}

fn deliver(sub: &Subscriber, frame: Frame) {
    if !EVENT_NAMES.contains(&frame.name.as_str()) {
        log::debug!("companion: dropping unknown event {:?}", frame.name);
        return;
    }
    if frame.name == SNAPSHOT_EVENT {
        let now = Utc::now();
        match save_snapshot(sub.store.as_ref(), &frame.data, now) {
            Ok(()) => sub.conn.mark_poll(now),
            Err(e) => log::warn!("companion: could not cache the snapshot: {e}"),
        }
    }
    sub.sink.emit(&frame.name, &frame.data);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::tests::Recorder;
    use crate::keys::{DeviceKeys, SoftwareKeys};
    use crate::store::MemoryStore;
    use crate::testing::{Reply, TestServer};

    /// The allowlist is copied, so nothing but a test stops the two
    /// halves drifting -- and drift here is silent in exactly the way
    /// that matters: a name the desktop emits and the phone drops
    /// produces a page that never fills in, with no error anywhere.
    ///
    /// `include_str!` ties this to the desktop file at compile time, so
    /// the lists are compared as they are checked in rather than as
    /// someone remembers them. The same shape as
    /// `surface::tests::table_is_identical_to_the_desktop_table`.
    ///
    /// It also guards the boundary in the widening direction: adding a
    /// name to one side alone fails here, so nobody can quietly grow
    /// the set of events the desktop may fire in this webview.
    #[test]
    fn the_allowlist_matches_the_desktops() {
        let src = include_str!("../../src-tauri/src/remote/events.rs");
        let start = src
            .find("pub const EVENT_NAMES")
            .expect("desktop events.rs must define EVENT_NAMES");
        let body = &src[start..];
        let end = body.find("];").expect("EVENT_NAMES must close");
        let desktop: Vec<&str> = body[..end]
            .lines()
            .filter_map(|l| {
                l.trim()
                    .strip_prefix('"')?
                    .split_once("\",")
                    .map(|(n, _)| n)
            })
            .collect();
        assert!(
            desktop.len() > 5,
            "parsed only {} names from the desktop's events.rs; the parser is broken",
            desktop.len()
        );
        assert_eq!(
            EVENT_NAMES.to_vec(),
            desktop,
            "src-mobile/src/events.rs EVENT_NAMES differs from \
             src-tauri/src/remote/events.rs; copy the desktop's list verbatim"
        );
    }

    fn feed_all(chunks: &[&str]) -> Vec<Frame> {
        let mut p = SseParser::default();
        chunks.iter().flat_map(|c| p.feed(c.as_bytes())).collect()
    }

    fn frame(name: &str, data: &str) -> Frame {
        Frame {
            name: name.into(),
            data: data.into(),
        }
    }

    #[test]
    fn parses_the_desktops_framing() {
        let frames = feed_all(&["event: prs-updated\ndata: [{\"number\":1347}]\n\nevent: poll-state\ndata: \"idle\"\n\n"]);
        assert_eq!(
            frames,
            vec![
                frame("prs-updated", "[{\"number\":1347}]"),
                frame("poll-state", "\"idle\"")
            ]
        );
    }

    #[test]
    fn frames_split_across_chunks_and_crlf_and_keepalives() {
        let frames = feed_all(&[
            "event: prs-upd",
            "ated\r\ndata: [",
            "1,2]\r\n",
            ":\n\n",
            "\r\n",
            ": keep-alive\n\nevent: poll-error\ndata: \"boom\"\n",
            "\n",
        ]);
        assert_eq!(
            frames,
            vec![
                frame("prs-updated", "[1,2]"),
                frame("poll-error", "\"boom\"")
            ]
        );
    }

    #[test]
    fn multi_line_data_joins_and_a_blank_line_without_data_is_nothing() {
        assert_eq!(
            feed_all(&["data:a\ndata: b\n\n\n\nevent: x\n\ndata: y\n\n"]),
            vec![frame("message", "a\nb"), frame("message", "y")]
        );
    }

    #[test]
    fn the_snapshot_round_trips_verbatim() {
        let store = MemoryStore::default();
        assert!(cached_snapshot(&store).unwrap().is_none());
        let at: DateTime<Utc> = "2026-09-05T12:00:00Z".parse().unwrap();
        save_snapshot(&store, r#"[{"number":1347,"title":"Add spoon"}]"#, at).unwrap();
        let snap = cached_snapshot(&store).unwrap().unwrap();
        assert_eq!(snap.prs.get(), r#"[{"number":1347,"title":"Add spoon"}]"#);
        assert_eq!(snap.received_at(), Some(at));
        assert!(save_snapshot(&store, "not json", at).is_err());
        forget_snapshot(&store).unwrap();
        assert!(cached_snapshot(&store).unwrap().is_none());
    }

    /// A stream that never sends a line end must not grow the buffer
    /// until the OS kills the app. There is no dialog and no log when
    /// jetsam takes a process; the user sees an app that "just closes".
    #[test]
    fn a_line_that_never_ends_is_dropped_rather_than_buffered() {
        let mut p = SseParser::default();
        // Well past MAX_LINE, in chunks, with no `\n` anywhere.
        let chunk = vec![b'x'; 1024 * 1024];
        for _ in 0..9 {
            assert!(p.feed(&chunk).is_empty());
        }
        assert!(p.buf.len() <= 1024 * 1024, "buffer grew to {}", p.buf.len());

        // And the parser still works afterwards: the reset is a
        // recovery, not a poisoning.
        let frames = p.feed(b"event: prs-updated\ndata: []\n\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].name, "prs-updated");
    }

    /// The same bound on the other accumulator: `data:` lines pile up
    /// until a blank line dispatches them, so a frame that never ends
    /// is the same unbounded growth by another route.
    #[test]
    fn a_frame_that_never_dispatches_is_dropped() {
        let mut p = SseParser::default();
        let line = format!("data: {}\n", "x".repeat(1024 * 1024));
        for _ in 0..9 {
            assert!(p.feed(line.as_bytes()).is_empty());
        }
        assert!(p.data.is_empty(), "data held {} lines", p.data.len());

        let frames = p.feed(b"event: prs-updated\ndata: []\n\n");
        assert_eq!(frames.len(), 1);
    }

    /// The bounds are generous against real traffic: a `prs-updated`
    /// frame for a large account is tens of kilobytes on one line, and
    /// must pass through untouched.
    #[test]
    fn an_ordinary_large_frame_is_not_dropped() {
        let mut p = SseParser::default();
        let big = "y".repeat(256 * 1024);
        let frames = p.feed(format!("event: prs-updated\ndata: {big}\n\n").as_bytes());
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data.len(), big.len());
    }

    #[test]
    fn backoff_doubles_to_a_ceiling() {
        let mut d = MIN_BACKOFF;
        let mut seen = vec![];
        for _ in 0..7 {
            seen.push(d.as_secs());
            d = next_backoff(d);
        }
        assert_eq!(seen, vec![1, 2, 4, 8, 16, 30, 30]);
    }

    struct Rig {
        server: TestServer,
        store: Arc<MemoryStore>,
        rec: Arc<Recorder>,
        conn: Arc<Connection>,
        handle: Handle,
        fp: String,
    }

    async fn rig(frames: &[(&str, &str)]) -> Rig {
        let store = Arc::new(MemoryStore::default());
        let keys = SoftwareKeys::new(store.clone());
        keys.generate().unwrap();
        let id = keys.session_identity().unwrap();
        let server = TestServer::start().await;
        server.pair(&id.fingerprint());
        server.reply("/v1/events", Reply::sse(frames, true));
        let client =
            Arc::new(Client::new(&id, &server.fp, vec![server.addr()], server.port()).unwrap());
        let rec = Arc::new(Recorder::default());
        let conn = Arc::new(Connection::new(rec.clone()));
        let handle = Handle::new();
        tokio::spawn(run(
            Subscriber {
                client,
                sink: rec.clone(),
                store: store.clone(),
                conn: conn.clone(),
                desktop_fp: server.fp.clone(),
            },
            handle.clone(),
        ));
        Rig {
            fp: id.fingerprint(),
            server,
            store,
            rec,
            conn,
            handle,
        }
    }

    /// Poll until `cond` holds, or give up.
    ///
    /// The budget is deliberately far larger than the ~10ms these
    /// conditions take when the machine is healthy. It is not a
    /// measurement of how long the work should take -- it is the point
    /// at which "still false" stops meaning "not yet" and starts
    /// meaning "never", and only a hang produces the latter.
    ///
    /// It was 10s, and that failed a release build (`mobile-v0.1.0`):
    /// the runner was slow enough that this suite took 302s against the
    /// 2s it takes locally, and one condition did not land inside its
    /// window. A polling loop costs nothing while it waits, so a budget
    /// tight enough to lose that race buys nothing and blocks a
    /// release. `HEADSTATE_TEST_TIMEOUT_SECS` overrides it for anyone
    /// debugging a genuine hang who wants to fail sooner.
    async fn until(mut cond: impl FnMut() -> bool) {
        let secs = std::env::var("HEADSTATE_TEST_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(120);
        tokio::time::timeout(Duration::from_secs(secs), async {
            while !cond() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("condition within {secs}s"));
    }

    #[tokio::test]
    async fn events_are_re_emitted_by_name_and_the_snapshot_is_cached() {
        let r = rig(&[
            ("prs-updated", r#"[{"number":1347}]"#),
            ("poll-state", r#"{"polling":true}"#),
            ("reveal-secrets", r#"{"nope":1}"#),
            ("worktree-removal-progress", r#"{"done":1,"total":3}"#),
        ])
        .await;
        until(|| r.rec.last("worktree-removal-progress").is_some()).await;
        let names: Vec<String> = r
            .rec
            .names()
            .into_iter()
            .filter(|n| n != crate::connection::STATE_EVENT)
            .collect();
        assert_eq!(
            names,
            vec!["prs-updated", "poll-state", "worktree-removal-progress"],
            "unknown names are dropped"
        );
        assert_eq!(r.rec.last("prs-updated").unwrap(), r#"[{"number":1347}]"#);
        assert_eq!(
            cached_snapshot(r.store.as_ref())
                .unwrap()
                .unwrap()
                .prs
                .get(),
            r#"[{"number":1347}]"#
        );
        let report = r.conn.report();
        assert_eq!(report.state, State::Connected);
        assert_eq!(report.protocol_version, Some(PROTOCOL_VERSION));
        assert!(report.last_poll.is_some());
        // The hello was filed with the desktop record it belongs to.
        pairing::save_desktops(
            r.store.as_ref(),
            &[pairing::Desktop {
                name: "d".into(),
                addrs: vec![],
                port: 1,
                fp: r.server.fp.clone(),
                paired_at: "x".into(),
                hello: None,
            }],
        )
        .unwrap();
        r.handle.stop();
    }

    #[tokio::test]
    async fn an_ended_stream_reconnects_and_a_revocation_ends_the_loop() {
        let r = rig(&[("prs-updated", "[]")]).await;
        until(|| r.conn.state() == State::Connected).await;
        let streams = |r: &Rig| {
            r.server
                .requests()
                .iter()
                .filter(|q| q.path == "/v1/events")
                .count()
        };
        until(|| streams(&r) == 1).await;
        r.server.end_streams();
        until(|| streams(&r) == 2).await;
        assert_eq!(r.conn.state(), State::Connected);

        r.server.revoke(&r.fp);
        r.server.end_streams();
        until(|| r.conn.state() == State::Revoked).await;
        assert_eq!(r.conn.report().protocol_version, None);
        // Revoked is terminal: a resume does not bring it back.
        r.handle.resume();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(r.conn.state(), State::Revoked);
        assert_eq!(streams(&r), 2);
    }

    /// The suspend/resume case, which had no test at all -- which is
    /// why `resume` shipped as a no-op while its own module doc said
    /// "iOS kills the stream when the app suspends, and this is how the
    /// phone catches up".
    ///
    /// The shape that matters is a stream the loop still HOLDS. The
    /// existing reconnect test ends the stream server-side, so
    /// `chunk()` returns and the loop notices on its own; that never
    /// exercised `resume`. Here the stream is left open and idle,
    /// exactly as it is across an iOS suspension, and the only thing
    /// that can make the phone reconnect is the resume itself.
    #[tokio::test]
    async fn a_resume_reconnects_a_stream_the_loop_is_still_holding() {
        let r = rig(&[("prs-updated", "[]")]).await;
        until(|| r.conn.state() == State::Connected).await;
        let streams = |r: &Rig| {
            r.server
                .requests()
                .iter()
                .filter(|q| q.path == "/v1/events")
                .count()
        };
        until(|| streams(&r) == 1).await;

        // Nothing has happened to the stream: it is open, idle, and as
        // far as the loop knows perfectly healthy. Before the fix this
        // resume did nothing and the count stayed at 1 until
        // `STREAM_READ_TIMEOUT` expired.
        //
        // So the assertion is on the CLOCK, not just the reconnect.
        // Measured against the unfixed code this test still passed --
        // in 46s rather than 0.08s -- because `until` waited out that
        // timeout, and a test that only proves "reconnects eventually"
        // would not have caught the bug it exists for. What the user
        // experiences is the delay.
        let started = std::time::Instant::now();
        r.handle.resume();
        until(|| streams(&r) == 2).await;
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a resume took {:?}; it must not wait out STREAM_READ_TIMEOUT",
            started.elapsed()
        );

        // And the desktop is never reported unreachable on the way: the
        // phone was asleep, not the desktop, so a banner flashing
        // "unreachable" on every app open would be both wrong and
        // trained-away.
        assert_eq!(r.conn.state(), State::Connected);
        r.handle.stop();
    }

    #[tokio::test]
    async fn an_unreachable_desktop_is_reported_and_stop_ends_the_loop() {
        let r = rig(&[]).await;
        until(|| r.conn.state() == State::Connected).await;
        let port = r.server.port();
        drop(r.server);
        // The held stream dies with the server; the next hello cannot
        // connect.
        until(|| r.conn.state() == State::Unreachable).await;
        r.handle.stop();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(r.handle.is_stopped());
        let _ = port;
    }
}

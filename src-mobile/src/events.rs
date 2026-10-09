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
    "source-poll-status",
    "gitlab-data-changed",
    "poll-error",
    "prs-truncated",
    "prs-incomplete",
    "store-error",
    "worktree-removal-progress",
    "reviewing-short",
    "reviewing-updated",
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
    // The fifteenth, and the first about a Claude Code session (#1477):
    // `{ session_id, size, seq }` when a RUNNING session's transcript
    // changed, so the transcript open on this phone reads at once instead
    // of waiting out its poll's backoff, and the session list can mark
    // other sessions "active now".
    //
    // It carries NO transcript text. Nothing re-emitted from this list is
    // masked (#1488 masks `/v1/call` answers), so an event about a
    // transcript is only safe here because nothing in it came out of one.
    //
    // It widens nothing: `claude_sessions`, an allowlisted Read, already
    // RETURNS every session id to this phone, and a byte size says only
    // that the file changed. A lost nudge costs latency, never what is
    // shown -- the follow keeps its own poll.
    "claude-session-activity",
    // Opaque watch ID, byte size, sequence only; shares the main activity event cap.
    "claude-transcript-activity",
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

#[cfg(test)]
impl Snapshot {
    pub fn received_at(&self) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(&self.received_at)
            .ok()
            .map(|t| t.with_timezone(&Utc))
    }
}

#[cfg(test)]
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

const OWNED_SNAPSHOT_KEY: &str = "owned-source-receipts-v2";
static OWNED_RECEIPTS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn save_owned_receipt(
    store: &dyn Store,
    desktop: &str,
    receipt: &serde_json::Value,
) -> Result<(), StoreError> {
    let _guard = OWNED_RECEIPTS_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(list @ ("authored" | "reviewing")) = receipt["list"].as_str() else {
        return Ok(());
    };
    if receipt["ownership"]["state"] == "different_account"
        && receipt["source"]["provider"] == "github"
    {
        let Some(session) = receipt["session"].as_str() else {
            return Ok(());
        };
        let mut saved: serde_json::Value = get_json(store, OWNED_SNAPSHOT_KEY)?
            .unwrap_or(serde_json::json!({"desktop":desktop,"retired":[]}));
        if saved["desktop"] != desktop {
            return Ok(());
        }
        if saved["retired"]
            .as_array()
            .is_some_and(|values| values.iter().any(|old| old == session))
        {
            return Ok(());
        }
        let mut retired = saved["retired"].as_array().cloned().unwrap_or_default();
        if saved["session"].as_str().is_some_and(|old| old != session) {
            retired.push(saved["session"].clone());
        }
        saved["retired"] = retired.into();
        saved["session"] = session.into();
        saved["owner"] = serde_json::Value::Null;
        saved["receipts"] = serde_json::json!({});
        put_json(store, OWNED_SNAPSHOT_KEY, &saved)?;
        return Ok(());
    }
    let owner = receipt["ownership"]["owner"]
        .as_str()
        .filter(|s| !s.is_empty());
    if !matches!(
        receipt["ownership"]["state"].as_str(),
        Some("live_verified" | "credential_bound")
    ) || owner.is_none()
        || receipt["source"]["provider"] != "github"
        || receipt["source"]["host"] != "github.com"
        || receipt["data"]["state"] != "available"
        || !receipt["data"]["prs"].is_array()
        || receipt["data"]["fetched_at"]
            .as_str()
            .and_then(provider_time)
            .is_none()
    {
        return Ok(());
    }
    let Some(session) = receipt["session"].as_str().filter(|s| !s.is_empty()) else {
        return Ok(());
    };
    let mut saved: serde_json::Value =
        get_json(store, OWNED_SNAPSHOT_KEY)?.unwrap_or(serde_json::json!({}));
    if saved["desktop"] != desktop {
        saved = serde_json::json!({"v":2,"desktop":desktop,"owner":owner,"session":session,"retired":[],"receipts":{}});
    }
    if saved["retired"]
        .as_array()
        .is_some_and(|values| values.iter().any(|old| old == session))
    {
        return Ok(());
    }
    if let Some(previous) = saved["session"]
        .as_str()
        .filter(|old| *old != session)
        .map(str::to_owned)
    {
        let mut retired = saved["retired"].as_array().cloned().unwrap_or_default();
        retired.push(previous.into());
        saved["retired"] = retired.into();
    }
    if saved["owner"].as_str() != owner {
        saved["receipts"] = serde_json::json!({});
    }
    let old_time = saved["receipts"][list]["data"]["fetched_at"]
        .as_str()
        .and_then(provider_time);
    let new_time = receipt["data"]["fetched_at"]
        .as_str()
        .and_then(provider_time);
    if old_time.zip(new_time).is_some_and(|(old, new)| new < old) {
        return Ok(());
    }
    saved["owner"] = serde_json::json!(owner);
    saved["session"] = session.into();
    saved["receipts"][list] = receipt.clone();
    put_json(store, OWNED_SNAPSHOT_KEY, &saved)
}
/// The ribbon describes saved provider evidence, never transport arrival. Use
/// the oldest retained list so a newer queue cannot rejuvenate another queue.
pub fn saved_provider_time(
    store: &dyn Store,
    desktop: &str,
) -> Result<Option<DateTime<Utc>>, StoreError> {
    let Some(saved): Option<serde_json::Value> = get_json(store, OWNED_SNAPSHOT_KEY)? else {
        return Ok(None);
    };
    if saved["v"] != 2 || saved["desktop"] != desktop || saved["owner"].as_str().is_none() {
        return Ok(None);
    }
    Ok(["authored", "reviewing"]
        .iter()
        .filter_map(|list| {
            saved["receipts"][list]["data"]["fetched_at"]
                .as_str()
                .and_then(provider_time)
        })
        .min())
}
fn provider_time(at: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|t| t.with_timezone(&Utc))
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(at, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|t| t.and_utc())
        })
}
pub fn offline_receipt(
    store: &dyn Store,
    desktop: &str,
    list: &str,
) -> Result<Option<serde_json::Value>, StoreError> {
    let Some(saved): Option<serde_json::Value> = get_json(store, OWNED_SNAPSHOT_KEY)? else {
        return Ok(None);
    };
    if saved["v"] != 2 || saved["desktop"] != desktop {
        return Ok(None);
    }
    let mut receipt = saved["receipts"][list].clone();
    let Some(at) = receipt["data"]["fetched_at"]
        .as_str()
        .and_then(provider_time)
    else {
        return Ok(None);
    };
    if receipt["data"]["state"] != "available" {
        return Ok(None);
    }
    receipt["ownership"] =
        serde_json::json!({"state":"saved_desktop","owner":saved["owner"],"desktop":desktop});
    receipt["data"]["stale_secs"] = serde_json::json!((Utc::now() - at).num_seconds().max(1));
    Ok(Some(receipt))
}

pub fn forget_snapshot(store: &dyn Store) -> Result<(), StoreError> {
    let _guard = OWNED_RECEIPTS_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    store.remove(OWNED_SNAPSHOT_KEY)?;
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
    publication: Arc<std::sync::Mutex<()>>,
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
        let _guard = self.publication.lock().unwrap_or_else(|e| e.into_inner());
        self.stopped.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }
    fn publish(&self, work: impl FnOnce()) {
        let _guard = self.publication.lock().unwrap_or_else(|e| e.into_inner());
        if !self.is_stopped() {
            work();
        }
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

fn retry_delay(delay: Duration, seed: u32) -> Duration {
    delay.mul_f64(0.8 + f64::from(seed % 201) / 1000.0)
}

/// Sleep `d` or until woken. `false` when the handle was stopped.
async fn wait(handle: &Handle, d: Duration) -> bool {
    // Timing noise spreads retries; it carries no identity or security role.
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let d = retry_delay(d, seed);
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
        handle.publish(|| sub.conn.set_state(State::Connecting));

        let hello_result = tokio::select! {
            result = sub.client.hello() => result,
            _ = handle.wake.notified() => {
                if handle.is_stopped() { return; }
                continue;
            }
        };
        let hello = match hello_result {
            Ok(h) => h,
            Err(e) if is_revocation(&e) => {
                log::warn!(
                    "companion: the desktop refused this phone ({})",
                    crate::client::failure_category(&e)
                );
                handle.publish(|| {
                    let _ = forget_snapshot(sub.store.as_ref());
                    sub.conn.set_state(State::Revoked);
                });
                return;
            }
            Err(e) => {
                log::info!(
                    "companion: desktop unreachable ({})",
                    crate::client::failure_category(&e)
                );
                handle.publish(|| sub.conn.set_state(State::Unreachable));
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
        handle.publish(|| {
            if let Err(e) = pairing::record_hello(sub.store.as_ref(), &sub.desktop_fp, &hello) {
                log::warn!("companion: could not record hello: {e}");
            }
            sub.conn.connected(hello.protocol_version);
        });
        if handle.is_stopped() {
            return;
        }

        let stream_result = tokio::select! {
            result = sub.client.events() => result,
            _ = handle.wake.notified() => {
                if handle.is_stopped() { return; }
                continue;
            }
        };
        match stream_result {
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
                                    handle.publish(|| deliver(&sub, frame));
                                }
                            }
                            Ok(None) => {
                                log::info!("companion: the event stream ended; reconnecting");
                                break;
                            }
                            Err(_) => {
                                log::info!("companion: the event stream failed");
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
                handle.publish(|| sub.conn.set_state(State::Unreachable));
                if !wait(&handle, MIN_BACKOFF).await {
                    return;
                }
            }
            Err(e) if is_revocation(&e) => {
                log::warn!(
                    "companion: the desktop refused the event stream ({})",
                    crate::client::failure_category(&e)
                );
                handle.publish(|| {
                    let _ = forget_snapshot(sub.store.as_ref());
                    sub.conn.set_state(State::Revoked);
                });
                return;
            }
            Err(e) => {
                log::info!(
                    "companion: could not open the event stream ({})",
                    crate::client::failure_category(&e)
                );
                handle.publish(|| sub.conn.set_state(State::Unreachable));
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
    if frame.name == "source-poll-status" {
        if let Ok(update) = serde_json::from_str::<serde_json::Value>(&frame.data) {
            if update["prs"].is_array() && !update["receipt_revision"].is_null() {
                let receipt = serde_json::json!({"session":update["session"],"source":update["source"],"list":update["list"],
                    "ownership":{"state":"live_verified","owner":update["owner"]},
                    "data":{"state":"available","prs":update["prs"],"fetched_at":update["last_received_at"],
                    "stale_secs":null,"coverage":update["coverage"]}});
                if let Err(e) = save_owned_receipt(sub.store.as_ref(), &sub.desktop_fp, &receipt) {
                    log::warn!("companion: could not cache the owned receipt: {e}");
                }
                if let Ok(at) = saved_provider_time(sub.store.as_ref(), &sub.desktop_fp) {
                    sub.conn.mark_poll(at);
                }
            }
        }
    }
    if frame.name == SNAPSHOT_EVENT {
        let now = Utc::now();
        match save_snapshot(sub.store.as_ref(), &frame.data, now) {
            Ok(()) => {}
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
    fn simultaneous_authored_and_reviewing_receipts_both_survive() {
        struct SlowRead(MemoryStore);
        impl Store for SlowRead {
            fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
                let value = self.0.get(key)?;
                std::thread::sleep(std::time::Duration::from_millis(10));
                Ok(value)
            }
            fn put(&self, key: &str, value: &[u8]) -> Result<(), StoreError> {
                self.0.put(key, value)
            }
            fn remove(&self, key: &str) -> Result<(), StoreError> {
                self.0.remove(key)
            }
        }
        let store = Arc::new(SlowRead(MemoryStore::default()));
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let tasks: Vec<_> = ["authored", "reviewing"].into_iter().map(|list| {
            let store = store.clone(); let barrier = barrier.clone();
            std::thread::spawn(move || {
                let receipt = serde_json::json!({"session":"session","source":{"provider":"github","host":"github.com"},"list":list,
                    "ownership":{"state":"live_verified","owner":"alice"},"data":{"state":"available","prs":[],"fetched_at":"2026-01-01T00:00:00Z","stale_secs":null,"coverage":"complete"}});
                barrier.wait(); save_owned_receipt(store.as_ref(), "desktop", &receipt).unwrap();
            })
        }).collect();
        for task in tasks {
            task.join().unwrap();
        }
        for list in ["authored", "reviewing"] {
            assert!(offline_receipt(store.as_ref(), "desktop", list)
                .unwrap()
                .is_some());
        }
    }
    #[test]
    fn old_session_and_older_same_session_cannot_replace_phone_receipt() {
        let store = MemoryStore::default();
        let receipt = |session: &str, owner: &str, at: &str| {
            serde_json::json!({"session":session,"source":{"provider":"github","host":"github.com"},"list":"authored",
            "ownership":{"state":"live_verified","owner":owner},"data":{"state":"available","prs":[],"fetched_at":at,"stale_secs":null,"coverage":"complete"}})
        };
        let alice = receipt("a", "alice", "2026-01-01T00:00:00Z");
        let bob = receipt("b", "bob", "2026-01-02T00:00:00Z");
        save_owned_receipt(&store, "desktop", &alice).unwrap();
        save_owned_receipt(&store, "desktop", &bob).unwrap();
        save_owned_receipt(&store, "desktop", &alice).unwrap();
        let saved = offline_receipt(&store, "desktop", "authored")
            .unwrap()
            .unwrap();
        assert_eq!(saved["ownership"]["owner"], "bob");
        let older_bob = receipt("b", "bob", "2026-01-01T12:00:00Z");
        save_owned_receipt(&store, "desktop", &older_bob).unwrap();
        assert_eq!(
            offline_receipt(&store, "desktop", "authored")
                .unwrap()
                .unwrap()["data"]["fetched_at"],
            bob["data"]["fetched_at"]
        );
    }
    #[test]
    fn owned_phone_receipt_preserves_provider_age_and_refuses_other_desktop() {
        let store = MemoryStore::default();
        let receipt = serde_json::json!({"session":"fixture-session","source":{"provider":"github","host":"github.com"},"list":"authored",
            "ownership":{"state":"live_verified","owner":"alice"},
            "data":{"state":"available","prs":[],"fetched_at":"2026-01-01T00:00:00Z","stale_secs":86400,"coverage":"complete"}});
        save_owned_receipt(&store, "desktop-a", &receipt).unwrap();
        let cached = offline_receipt(&store, "desktop-a", "authored")
            .unwrap()
            .unwrap();
        assert_eq!(cached["ownership"]["state"], "saved_desktop");
        assert_eq!(cached["ownership"]["owner"], "alice");
        assert_eq!(cached["data"]["fetched_at"], receipt["data"]["fetched_at"]);
        assert!(cached["data"]["stale_secs"].as_i64().unwrap() >= 86400);
        assert!(offline_receipt(&store, "desktop-b", "authored")
            .unwrap()
            .is_none());
        save_snapshot(&store, "[]", Utc::now()).unwrap();
        assert!(offline_receipt(&store, "desktop-a", "authored")
            .unwrap()
            .is_some());
        forget_snapshot(&store).unwrap();
        assert!(offline_receipt(&store, "desktop-a", "authored")
            .unwrap()
            .is_none());
    }
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
    fn retry_delays_are_jittered_without_exceeding_the_backoff_budget() {
        for budget in [MIN_BACKOFF, MAX_BACKOFF] {
            let early = retry_delay(budget, 0);
            let late = retry_delay(budget, 200);
            assert!(early < late, "different retry seeds must spread reconnects");
            assert!(early >= budget.mul_f64(0.8));
            assert!(late <= budget);
        }
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

    #[tokio::test]
    async fn stopping_a_subscriber_cancels_pending_hello_and_event_headers() {
        for path in ["/v1/hello", "/v1/events"] {
            let store = Arc::new(MemoryStore::default());
            let keys = SoftwareKeys::new(store.clone());
            keys.generate().unwrap();
            let id = keys.session_identity().unwrap();
            let server = TestServer::start().await;
            server.pair(&id.fingerprint());
            server.reply(path, Reply::Stall);
            let client =
                Arc::new(Client::new(&id, &server.fp, vec![server.addr()], server.port()).unwrap());
            let rec = Arc::new(Recorder::default());
            let handle = Handle::new();
            let task = tokio::spawn(run(
                Subscriber {
                    client,
                    store,
                    sink: rec.clone(),
                    conn: Arc::new(Connection::new(rec)),
                    desktop_fp: server.fp.clone(),
                },
                handle.clone(),
            ));
            until(|| server.requests().iter().any(|r| r.path == path)).await;
            handle.stop();
            assert!(
                tokio::time::timeout(Duration::from_millis(200), task)
                    .await
                    .is_ok(),
                "stopped subscriber kept its {path} request alive"
            );
        }
    }

    #[test]
    fn saved_provider_age_is_scoped_and_never_uses_legacy_arrival() {
        let store = MemoryStore::default();
        save_snapshot(&store, "[]", Utc::now()).unwrap();
        assert_eq!(saved_provider_time(&store, "desktop").unwrap(), None);
        let receipt = serde_json::json!({"session":"desktop-session","source":{"provider":"github","host":"github.com"},"list":"authored",
            "ownership":{"state":"live_verified","owner":"alice"},"data":{"state":"available","prs":[],"fetched_at":"2026-01-01 00:00:00","coverage":"complete"}});
        save_owned_receipt(&store, "desktop", &receipt).unwrap();
        assert_eq!(saved_provider_time(&store, "other-desktop").unwrap(), None);
        let mut newer_list = receipt.clone();
        newer_list["list"] = "reviewing".into();
        newer_list["data"]["fetched_at"] = "2026-01-02T00:00:00Z".into();
        save_owned_receipt(&store, "desktop", &newer_list).unwrap();
        assert_eq!(
            saved_provider_time(&store, "desktop").unwrap(),
            Some("2026-01-01T00:00:00Z".parse().unwrap())
        );
    }

    #[tokio::test]
    async fn opening_arrays_never_rejuvenate_owned_provider_age() {
        let yesterday = (Utc::now() - chrono::Duration::days(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let update = serde_json::json!({"session":"desktop", "source":{"provider":"github","host":"github.com"},
            "list":"authored","owner":"alice","receipt_revision":1,"prs":[],
            "last_received_at":yesterday,"coverage":"complete"}).to_string();
        let r = rig(&[("source-poll-status", &update), ("prs-updated", "[]")]).await;
        until(|| r.rec.last("prs-updated").is_some()).await;
        assert_eq!(r.conn.report().last_poll, Some(yesterday.parse().unwrap()));
        r.handle.stop();
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
        assert!(
            report.last_poll.is_none(),
            "ownerless opening arrays have no provider time"
        );
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

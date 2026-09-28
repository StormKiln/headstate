//! Paged reads of a transcript: a bounded page before or after a cursor
//! (#1220, epic #1473).
//!
//! `transcript_model::tail` answers "show me the newest 256 KB". The
//! viewer also has to answer "what happened earlier" in a 70 MB file,
//! and #1487's harness measured the only way to get there today --
//! `follow` from offset 0 -- reading all 70.1 MiB into one buffer. This
//! module is the read that replaces it.
//!
//! # The promises
//!
//! - **Every read is bounded**, the first one included. A page holds at
//!   most [`PAGE_BYTES`] of records (plus the one record that crossed the
//!   line, itself at most [`RECORD_HOLD_BYTES`]) and at most `limit`
//!   messages, wherever in the file it lands. [`PAGE_READ_BOUND`] is the
//!   whole of what one call reads into memory, and `read_bench.rs`
//!   asserts it on the 70 MB fixture at the start, middle and end.
//! - **O(page), not O(file).** A page before a cursor reads BACKWARDS
//!   from it, a chunk at a time, and stops when the page is full. A page
//!   at the start of a 70 MB file costs what a page at its end does.
//! - **A record larger than a page is one message, never a hole.** Over
//!   [`RECORD_HOLD_BYTES`], a record is streamed through
//!   `transcript_skim` rather than held: its message carries
//!   `oversized_bytes`, its blocks are clipped, and each clip states the
//!   record's true length. #1487's `huge-result-5mb` tail showed only the
//!   reply AFTER a 10 MiB record; a page shows the record.
//! - **Cursors land on record boundaries.** A page's `start` and `end`
//!   are always the first byte of a record (or the end of the last
//!   complete one), never a raw offset, and each carries a digest of the
//!   bytes behind it so a cursor into a rewritten file is noticed rather
//!   than trusted ([`PageCursor`]).
//! - **Positions are honest.** "messages ~4,200-4,400 of ~16,700
//!   (estimate)", never a count a byte index cannot support: see
//!   [`TranscriptPosition`].
//!
//! # Where pages are merged, and why there
//!
//! Pages are parsed separately, so a tool call at the end of one page
//! and its result at the start of the next arrive apart, and so do a
//! turn's opener and its later messages, and two assistant messages whose
//! model differs. A single read pairs, groups and marks these in
//! `transcript_model::settle`.
//!
//! **The client merges, and the server hands it everything it needs to do
//! that by id.** The client holds the loaded pages -- the server keeps no
//! per-viewer state, and a paired phone must not make the desktop keep
//! any -- so the merge happens where the pages are. It does not
//! re-implement `settle`: every decision that needs the record format is
//! taken HERE, and the page carries the result:
//!
//! - **Pairing**: an unanswered call is a `tool_call` block whose
//!   `result` is null; a result whose call was not in its page is a
//!   standing `tool_result` block. Both carry the `tool_use_id`, so the
//!   client moves a standing result into its call BY ID.
//! - **Turns**: a message whose turn began before its page has
//!   `turn_id: null`; it belongs to the turn in effect at the end of the
//!   page before it.
//! - **Model changes**: [`PageSeam`] names the page's first real
//!   assistant message and its last model, so the client inserts the one
//!   derived model-change marker a seam can need without deciding what a
//!   "real" assistant message is.
//!
//! `src/lib/transcriptPages.ts` is that merge, and its test replays
//! pages this module produced (`transcriptPages.golden.json`, generated
//! and checked by `the_merge_golden_file_is_current` below) against one
//! read of the same file: merged pages must equal the single read,
//! message for message.
//!
//! # Read-only
//!
//! Like the rest of `claude/`, nothing here writes under `~/.claude`.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use super::transcript_model::{
    build, IdSource, Line, MessageKind, Seed, TranscriptMessage, TranscriptPage, WindowStart,
};
use super::transcript_skim;
use crate::remote::privacy::{self, Matching};

/// The most bytes of records a page holds.
///
/// `preview::TAIL_BYTES`, for its reason: 256 KB is the whole
/// conversation for the 97.4% of transcripts under 1 MB, and the
/// measured tail read of it is ~1.4 ms on the 70 MB fixture.
pub const PAGE_BYTES: u64 = 256 * 1024;

/// The most messages a page returns, and the default `limit`.
///
/// `preview::MAX_MESSAGES`, for its reason: the byte bound does not bound
/// the RENDER, and 256 KB of short records is thousands of messages.
pub const PAGE_MESSAGES: usize = 200;

/// A record larger than this is streamed rather than held.
///
/// Half a page, so a page always has room for at least one record held
/// whole and the record that crosses the budget costs at most half again.
/// The real corpus's largest record is 1.3 MB; #1487's fixture reaches
/// 10.3 MiB.
pub const RECORD_HOLD_BYTES: u64 = PAGE_BYTES / 2;

/// How much a page reads at a time.
pub const CHUNK_BYTES: u64 = 64 * 1024;

/// How many bytes behind a cursor its digest covers.
///
/// Records average ~2 KB and each carries a uuid and a timestamp, so
/// 4 KB behind a boundary cannot survive a rewrite byte-for-byte unless
/// the records behind it did. Smaller than `preview::FINGERPRINT_BYTES`
/// (64 KB) on purpose: a page checks one cursor per call and returns two,
/// and every page is a phone's round trip.
pub const CURSOR_FINGERPRINT_BYTES: u64 = 4 * 1024;

/// How far behind a page's first record the id anchor is looked for.
///
/// A uuid-less record's id is `"<previous uuid>+<n>"` ([`transcript_model::IdSource`]),
/// so a page that STARTS with one needs the nearest uuid behind it to
/// give the id a whole read would. Nearly every record carries a uuid;
/// the fixture's longest uuid-less run is four bookkeeping records.
pub const SEED_LOOKBACK_BYTES: u64 = 64 * 1024;

/// What a skimmed record may keep.
const SKIM_KEEP_BYTES: usize = 64 * 1024;

/// Everything one page call can read into memory.
///
/// The page's records; one record-in-progress plus one chunk of overshoot
/// for the gather and again for the anchor lookback; the lookback's own
/// budget; and three cursor digests (the one checked, the two returned)
/// with a boundary byte each. Oversized records are STREAMED, and count
/// towards [`TranscriptWindow::bytes_scanned`] instead.
pub const PAGE_READ_BOUND: u64 = PAGE_BYTES
    + 2 * (RECORD_HOLD_BYTES + CHUNK_BYTES)
    + SEED_LOOKBACK_BYTES
    + 3 * (CURSOR_FINGERPRINT_BYTES + 1);

/// Where a page is read from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageAnchor {
    /// Byte 0.
    Start,
    /// The end of the last complete record. A record still being written
    /// is not read, `preview::follow`'s case 3.
    End,
    /// A cursor a previous page returned, handed back unread.
    Cursor { offset: u64, behind_digest: String },
}

/// Which way from the anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageDirection {
    /// The records ending at the anchor: older messages.
    Before,
    /// The records starting at the anchor: newer messages.
    After,
}

/// A record boundary, and what the file looked like behind it.
///
/// Opaque to the client, like `preview::Cursor`: it is handed back as
/// [`PageAnchor::Cursor`] to read the next page. `offset` is also a
/// position the client may compare, since pages never overlap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageCursor {
    pub offset: u64,
    /// Lowercase hex SHA256 of the [`CURSOR_FINGERPRINT_BYTES`] behind
    /// `offset` (fewer near the start; empty at 0).
    pub behind_digest: String,
}

/// What a position figure rests on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionBasis {
    /// The page is the whole file: every figure is a count.
    WholeFile,
    /// An index over the whole file counted the records that render. The
    /// figures are close, and still estimates: pairing a result into its
    /// call, and model-change markers, move the count a little.
    Index,
    /// An index over PART of the file, extrapolated by bytes beyond it.
    PartialIndex,
    /// No index yet: extrapolated from this page's own messages per byte.
    Bytes,
}

/// Where a page sits in the whole transcript.
///
/// Rendered as "messages ~4,200-4,400 of ~16,700 (estimate)". `exact`
/// is true only when the page is the whole file; any other figure is an
/// estimate and MUST be labelled one -- a scrubber must not imply a
/// precision a byte offset does not have.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptPosition {
    /// 1-based ordinal of the page's first message. `None` when the page
    /// holds no messages -- nothing to place, which is not position 0.
    pub first: Option<u64>,
    /// Of its last message.
    pub last: Option<u64>,
    /// Messages in the whole transcript. `None` when there was nothing
    /// to estimate from (an empty page with no index).
    pub total: Option<u64>,
    pub exact: bool,
    pub basis: PositionBasis,
}

/// A page's first real assistant message: the one a model-change marker
/// is inserted before when the page before it ended on another model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeamModel {
    pub message_id: String,
    pub model: String,
    pub timestamp: Option<String>,
}

/// What the client needs to join this page to its neighbours by id. See
/// the module docs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageSeam {
    /// The page's first "real" assistant message: not a sidechain, and a
    /// model that is not Claude Code's `<synthetic>`.
    pub first_model: Option<SeamModel>,
    /// The model of its last one.
    pub last_model: Option<String>,
}

/// One page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptWindow {
    /// The messages, settled within the page. `page.truncated` is whether
    /// anything precedes the page; `page.bytes_read` is everything this
    /// call read into memory, cursor digests and anchor lookback included.
    pub page: TranscriptPage,
    /// The page covers exactly the records in `[start, end)`. Pages never
    /// overlap: the next page before this one ENDS at `start`.
    pub start: PageCursor,
    pub end: PageCursor,
    /// `start` is byte 0: nothing older exists.
    pub at_start: bool,
    /// `end` is the end of the last complete record at the time of the
    /// read: nothing newer exists yet.
    pub at_end: bool,
    /// The anchor's cursor no longer described the file -- it was
    /// rewritten, truncated or replaced -- so this page was read from the
    /// END instead, and every page the client holds is stale. Not an
    /// error: the answer that still works, stated.
    pub rewritten: bool,
    pub position: TranscriptPosition,
    pub seam: PageSeam,
    /// Bytes passed over without being held: oversized records streamed
    /// through the skim, and scans for a record's far boundary. Stated so
    /// that an O(record) cost is visible rather than hidden in the bound.
    pub bytes_scanned: u64,
}

/// A page of `path`.
///
/// Uses the process-wide position index ([`index_for`]), building it off
/// the calling thread on first use; until it exists the position is
/// estimated from the page itself and says so.
///
/// # Errors
///
/// Only when the file cannot be opened, sized, sought or read.
pub fn page(
    path: &Path,
    anchor: &PageAnchor,
    direction: PageDirection,
    limit: Option<usize>,
) -> Result<TranscriptWindow, String> {
    read_page(path, anchor, direction, limit, IndexUse::Cached)
}

/// Which position index a read consults. `None` and `Given` are the
/// deterministic choices tests make.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum IndexUse<'a> {
    /// None: estimate from the page (deterministic, for tests).
    None,
    /// This one.
    Given(&'a PositionIndex),
    /// The process-wide cache, building on a worker thread when missing.
    Cached,
}

pub(crate) fn read_page(
    path: &Path,
    anchor: &PageAnchor,
    direction: PageDirection,
    limit: Option<usize>,
    index: IndexUse,
) -> Result<TranscriptWindow, String> {
    let limit = limit.unwrap_or(PAGE_MESSAGES).clamp(1, PAGE_MESSAGES);
    let mut r = Reader::open(path)?;
    let boundary = r.boundary_end()?;

    let (at, direction, rewritten) = match anchor {
        PageAnchor::Start => (0, direction, false),
        PageAnchor::End => (boundary, direction, false),
        PageAnchor::Cursor {
            offset,
            behind_digest,
        } => {
            if *offset <= boundary
                && r.is_boundary(*offset)?
                && r.digest_behind(*offset)? == *behind_digest
            {
                (*offset, direction, false)
            } else {
                (boundary, PageDirection::Before, true)
            }
        }
    };

    let mut back = Backward::new(at);
    let (mut lines, mut start, mut end) = match direction {
        PageDirection::Before => {
            let lines = gather_before(&mut r, &mut back, limit)?;
            (lines, back.end, at)
        }
        PageDirection::After => {
            let (lines, end) = gather_after(&mut r, at, boundary, limit)?;
            (lines, at, end)
        }
    };

    // The message bound is checked against records before parsing; a
    // derived model-change marker can still tip a page over. Trim whole
    // records from the far side -- the oldest when paging back, the
    // newest when paging forward -- until it fits, moving the boundary
    // with them so the next page picks them up.
    let mut page = build(&lines, Seed::default(), Some(path));
    while page.messages.len() > limit && lines.len() > 1 {
        let over = page.messages.len() - limit;
        let mut dropped = 0usize;
        while dropped < over.max(1) && lines.len() > 1 {
            let gone = match direction {
                PageDirection::Before => lines.remove(0),
                PageDirection::After => lines.pop().expect("more than one line"),
            };
            if gone.may_render() {
                dropped += 1;
            }
            match direction {
                PageDirection::Before => {
                    start = lines[0].offset.expect("a page line has an offset");
                }
                PageDirection::After => {
                    end = gone.offset.expect("a page line has an offset");
                }
            }
            // The records dropped from the front are the lookback's
            // nearest records now: hand them back to it.
            if direction == PageDirection::Before {
                back.push_front_line(gone);
            }
        }
        page = build(&lines, Seed::default(), Some(path));
    }

    // Ids: a page that starts with uuid-less records is seeded from the
    // nearest uuid behind it, so they get the ids a whole read gives.
    if needs_seed(&lines) {
        let seed = if start == 0 {
            Seed::at(WindowStart::FileStart)
        } else {
            // Paging back, `back` already stands at `start` with what it
            // read beyond the page (and any records a trim handed back)
            // in hand; paging forward, it starts fresh there.
            if direction == PageDirection::After {
                back = Backward::new(start);
            }
            lookback_seed(&mut r, &mut back)?
        };
        page = build(&lines, seed, Some(path));
    }

    let records = lines.len();
    drop(lines);
    page.truncated = start > 0;
    page.file_bytes = r.size;
    let start_cursor = PageCursor {
        offset: start,
        behind_digest: r.digest_behind(start)?,
    };
    let end_cursor = PageCursor {
        offset: end,
        behind_digest: r.digest_behind(end)?,
    };
    page.bytes_read = r.held;

    let at_start = start == 0;
    let at_end = end >= boundary;
    let cached: Option<Arc<PositionIndex>>;
    let ix: Option<&PositionIndex> = match index {
        IndexUse::None => None,
        IndexUse::Given(i) => Some(i),
        IndexUse::Cached => {
            cached = index_for(path, &mut r, boundary);
            cached.as_deref()
        }
    };
    let position = position(
        page.messages.len() as u64,
        records as u64,
        start,
        end,
        boundary,
        at_start && at_end,
        ix,
    );
    let seam = seam(&page.messages);
    Ok(TranscriptWindow {
        page,
        start: start_cursor,
        end: end_cursor,
        at_start,
        at_end,
        rewritten,
        position,
        seam,
        bytes_scanned: r.scanned,
    })
}

/// Whether the page's first parseable record has no uuid, so its id
/// depends on what is behind the page.
fn needs_seed(lines: &[Line]) -> bool {
    lines
        .iter()
        .find(|l| l.value.is_some())
        .is_some_and(|l| l.uuid().is_none())
}

/// The id seed at `back.end`: the nearest uuid behind it and the
/// uuid-less records since, read backwards within
/// [`SEED_LOOKBACK_BYTES`]. Past that bound, no anchor: the page's
/// leading uuid-less records are [`transcript_model::IdSource::Unanchored`],
/// which says the id may move -- not an anchored id that might be wrong.
fn lookback_seed(r: &mut Reader, back: &mut Backward) -> Result<Seed, String> {
    let held_before = r.held;
    let mut since = 0usize;
    loop {
        if r.held - held_before >= SEED_LOOKBACK_BYTES {
            return Ok(Seed::default());
        }
        let Some(line) = back.prev_line(r)? else {
            // Reached byte 0 without a uuid: the file-start anchor.
            return Ok(Seed {
                anchor: Some("^".to_owned()),
                since,
            });
        };
        if line.value.is_none() {
            continue;
        }
        match line.uuid() {
            Some(u) => {
                return Ok(Seed {
                    anchor: Some(u),
                    since,
                })
            }
            None => since += 1,
        }
    }
}

fn gather_before(r: &mut Reader, back: &mut Backward, limit: usize) -> Result<Vec<Line>, String> {
    let held_before = r.held;
    let mut out: Vec<Line> = Vec::new();
    let mut renders = 0usize;
    while renders < limit {
        if !out.is_empty() && r.held - held_before >= PAGE_BYTES {
            break;
        }
        let Some(line) = back.prev_line(r)? else {
            break;
        };
        if line.may_render() {
            renders += 1;
        }
        out.push(line);
    }
    out.reverse();
    Ok(out)
}

fn gather_after(
    r: &mut Reader,
    from: u64,
    boundary: u64,
    limit: usize,
) -> Result<(Vec<Line>, u64), String> {
    let held_before = r.held;
    let mut fwd = Forward {
        start: from,
        buf: Vec::new(),
    };
    let mut out: Vec<Line> = Vec::new();
    let mut renders = 0usize;
    while renders < limit {
        if !out.is_empty() && r.held - held_before >= PAGE_BYTES {
            break;
        }
        let Some(line) = fwd.next_line(r, boundary)? else {
            break;
        };
        if line.may_render() {
            renders += 1;
        }
        out.push(line);
    }
    // Bytes read ahead but not consumed belong to the next page.
    Ok((out, fwd.start))
}

// ---------------------------------------------------------------------
// Reading records at a boundary, in either direction
// ---------------------------------------------------------------------

/// One open transcript, and what reading it has cost.
struct Reader {
    file: std::fs::File,
    path: PathBuf,
    size: u64,
    /// Bytes read into buffers that are kept: records, digests.
    held: u64,
    /// Bytes streamed past in fixed-size chunks and not kept.
    scanned: u64,
}

/// A record located but not yet parsed.
enum Raw {
    /// Held whole: `[start, start + bytes.len())`.
    Held { start: u64, bytes: Vec<u8> },
    /// Too large to hold: `[start, end)`, skimmed.
    Big { start: u64, end: u64 },
}

impl Reader {
    fn open(path: &Path) -> Result<Reader, String> {
        let file = std::fs::File::open(path)
            .map_err(|e| format!("{}: could not open it: {e}", path.display()))?;
        let size = file
            .metadata()
            .map_err(|e| format!("{}: could not read its size: {e}", path.display()))?
            .len();
        Ok(Reader {
            file,
            path: path.to_path_buf(),
            size,
            held: 0,
            scanned: 0,
        })
    }

    fn err(&self, what: &str, e: std::io::Error) -> String {
        format!("{}: could not {what} it: {e}", self.path.display())
    }

    fn read_raw(&mut self, from: u64, len: u64) -> Result<Vec<u8>, String> {
        self.file
            .seek(SeekFrom::Start(from))
            .map_err(|e| self.err("seek in", e))?;
        let mut buf = vec![0u8; len as usize];
        self.file
            .read_exact(&mut buf)
            .map_err(|e| self.err("read", e))?;
        Ok(buf)
    }

    /// Read `[from, from + len)` into a kept buffer.
    fn read_held(&mut self, from: u64, len: u64) -> Result<Vec<u8>, String> {
        let buf = self.read_raw(from, len)?;
        self.held += len;
        Ok(buf)
    }

    /// Read `[from, from + len)` to look at and discard.
    fn read_scanned(&mut self, from: u64, len: u64) -> Result<Vec<u8>, String> {
        let buf = self.read_raw(from, len)?;
        self.scanned += len;
        Ok(buf)
    }

    /// The end of the last complete record: one past the last newline, or
    /// 0. A trailing record still being written is not a record yet.
    fn boundary_end(&mut self) -> Result<u64, String> {
        let mut to = self.size;
        while to > 0 {
            let from = to.saturating_sub(CHUNK_BYTES);
            let chunk = self.read_scanned(from, to - from)?;
            if let Some(i) = chunk.iter().rposition(|b| *b == b'\n') {
                return Ok(from + i as u64 + 1);
            }
            to = from;
        }
        Ok(0)
    }

    /// Whether `offset` is the first byte of a record (or 0).
    fn is_boundary(&mut self, offset: u64) -> Result<bool, String> {
        if offset == 0 {
            return Ok(true);
        }
        if offset > self.size {
            return Ok(false);
        }
        Ok(self.read_held(offset - 1, 1)?[0] == b'\n')
    }

    fn digest_behind(&mut self, offset: u64) -> Result<String, String> {
        use sha2::{Digest, Sha256};
        let want = offset.min(CURSOR_FINGERPRINT_BYTES).min(self.size);
        if want == 0 {
            return Ok(String::new());
        }
        let buf = self.read_held(offset - want, want)?;
        Ok(Sha256::digest(&buf)
            .iter()
            .fold(String::with_capacity(64), |mut acc, b| {
                use std::fmt::Write;
                let _ = write!(acc, "{b:02x}");
                acc
            }))
    }

    /// Where the record containing byte `before - 1` starts, found by
    /// streaming backwards: one past the nearest newline, or 0.
    fn scan_back(&mut self, before: u64) -> Result<u64, String> {
        let mut to = before;
        while to > 0 {
            let from = to.saturating_sub(CHUNK_BYTES);
            let chunk = self.read_scanned(from, to - from)?;
            if let Some(i) = chunk.iter().rposition(|b| *b == b'\n') {
                return Ok(from + i as u64 + 1);
            }
            to = from;
        }
        Ok(0)
    }

    /// One past the first newline at or after `from`, streaming forwards.
    /// `limit` is a boundary, so a newline exists before it.
    fn scan_forward(&mut self, from: u64, limit: u64) -> Result<u64, String> {
        let mut at = from;
        while at < limit {
            let to = (at + CHUNK_BYTES).min(limit);
            let chunk = self.read_scanned(at, to - at)?;
            if let Some(i) = chunk.iter().position(|b| *b == b'\n') {
                return Ok(at + i as u64 + 1);
            }
            at = to;
        }
        Ok(limit)
    }

    /// Parse a located record: whole when held, skimmed when not.
    fn line(&mut self, raw: Raw) -> Result<Option<Line>, String> {
        match raw {
            Raw::Held { start, bytes } => {
                let text = String::from_utf8_lossy(&bytes);
                Ok(Line::whole(&text, Some(start)))
            }
            Raw::Big { start, end } => {
                self.file
                    .seek(SeekFrom::Start(start))
                    .map_err(|e| self.err("seek in", e))?;
                let len = end - start;
                let mut head = vec![0u8; 256.min(len as usize)];
                self.file
                    .read_exact(&mut head)
                    .map_err(|e| self.err("read", e))?;
                self.scanned += head.len() as u64;
                self.file
                    .seek(SeekFrom::Start(start))
                    .map_err(|e| self.err("seek in", e))?;
                let skim = transcript_skim::skim(
                    Read::take(&mut self.file, len),
                    super::preview::MAX_TEXT_CHARS,
                    SKIM_KEEP_BYTES,
                );
                self.scanned += len;
                Ok(Some(Line::skimmed(
                    skim,
                    start,
                    len,
                    &String::from_utf8_lossy(&head),
                )))
            }
        }
    }
}

/// Records read backwards from a boundary.
///
/// `buf` holds the bytes `[end - buf.len(), end)` already read and not
/// yet handed out, so consecutive records cost one chunk read per
/// [`CHUNK_BYTES`], not one per record.
struct Backward {
    end: u64,
    buf: Vec<u8>,
    /// Lines a trim handed back, nearest last: served before `buf`.
    returned: Vec<Line>,
}

impl Backward {
    fn new(end: u64) -> Backward {
        Backward {
            end,
            buf: Vec::new(),
            returned: Vec::new(),
        }
    }

    fn push_front_line(&mut self, line: Line) {
        self.returned.push(line);
    }

    /// The record ending at `self.end`, parsed; `None` at byte 0.
    fn prev_line(&mut self, r: &mut Reader) -> Result<Option<Line>, String> {
        if let Some(line) = self.returned.pop() {
            return Ok(Some(line));
        }
        loop {
            let Some(raw) = self.prev_raw(r)? else {
                return Ok(None);
            };
            if let Some(line) = r.line(raw)? {
                return Ok(Some(line));
            }
            // A blank line: no record, keep going.
        }
    }

    fn prev_raw(&mut self, r: &mut Reader) -> Result<Option<Raw>, String> {
        if self.end == 0 {
            return Ok(None);
        }
        loop {
            // `buf`'s last byte ends the record wanted (its newline, at a
            // boundary), so the record's START is after the newline
            // before that.
            let upto = self.buf.len().saturating_sub(1);
            if let Some(i) = self.buf[..upto].iter().rposition(|b| *b == b'\n') {
                let bytes = self.buf.split_off(i + 1);
                let start = self.end - bytes.len() as u64;
                self.end = start;
                return Ok(Some(Raw::Held { start, bytes }));
            }
            let buf_start = self.end - self.buf.len() as u64;
            if buf_start == 0 {
                let bytes = std::mem::take(&mut self.buf);
                self.end = 0;
                return Ok(Some(Raw::Held { start: 0, bytes }));
            }
            if self.buf.len() as u64 > RECORD_HOLD_BYTES {
                // Too large to hold: find where it starts without
                // holding it, then skim it.
                let end = self.end;
                let start = r.scan_back(buf_start)?;
                self.buf.clear();
                self.end = start;
                return Ok(Some(Raw::Big { start, end }));
            }
            let from = buf_start.saturating_sub(CHUNK_BYTES);
            let mut chunk = r.read_held(from, buf_start - from)?;
            chunk.append(&mut self.buf);
            self.buf = chunk;
        }
    }
}

/// Records read forwards from a boundary.
struct Forward {
    /// The next record's first byte.
    start: u64,
    /// The bytes `[start, start + buf.len())` already read.
    buf: Vec<u8>,
}

impl Forward {
    fn next_line(&mut self, r: &mut Reader, limit: u64) -> Result<Option<Line>, String> {
        loop {
            let Some(raw) = self.next_raw(r, limit)? else {
                return Ok(None);
            };
            if let Some(line) = r.line(raw)? {
                return Ok(Some(line));
            }
        }
    }

    fn next_raw(&mut self, r: &mut Reader, limit: u64) -> Result<Option<Raw>, String> {
        loop {
            if let Some(i) = self.buf.iter().position(|b| *b == b'\n') {
                let rest = self.buf.split_off(i + 1);
                let bytes = std::mem::replace(&mut self.buf, rest);
                let start = self.start;
                self.start += bytes.len() as u64;
                return Ok(Some(Raw::Held { start, bytes }));
            }
            let buf_end = self.start + self.buf.len() as u64;
            if buf_end >= limit {
                // Everything before a boundary is complete records, so
                // this is only ever an empty buffer at the boundary.
                return Ok(None);
            }
            if self.buf.len() as u64 > RECORD_HOLD_BYTES {
                let start = self.start;
                let end = r.scan_forward(buf_end, limit)?;
                self.buf.clear();
                self.start = end;
                return Ok(Some(Raw::Big { start, end }));
            }
            let to = (buf_end + CHUNK_BYTES).min(limit);
            let mut chunk = r.read_held(buf_end, to - buf_end)?;
            self.buf.append(&mut chunk);
        }
    }
}

// ---------------------------------------------------------------------
// The seam
// ---------------------------------------------------------------------

/// `transcript_model::settle`'s definition of an assistant message whose
/// model counts, taken here so the client never re-decides it.
fn real_assistant(m: &TranscriptMessage) -> Option<&str> {
    (m.kind == MessageKind::Assistant && !m.is_sidechain)
        .then_some(m.model.as_deref())
        .flatten()
        .filter(|x| *x != "<synthetic>")
}

fn seam(messages: &[TranscriptMessage]) -> PageSeam {
    let first_model = messages.iter().find_map(|m| {
        real_assistant(m).map(|model| SeamModel {
            message_id: m.id.clone(),
            model: model.to_owned(),
            timestamp: m.timestamp.clone(),
        })
    });
    let last_model = messages
        .iter()
        .rev()
        .find_map(|m| real_assistant(m).map(str::to_owned));
    PageSeam {
        first_model,
        last_model,
    }
}

// ---------------------------------------------------------------------
// Position
// ---------------------------------------------------------------------

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn position(
    in_page: u64,
    records: u64,
    start: u64,
    end: u64,
    boundary: u64,
    whole: bool,
    index: Option<&PositionIndex>,
) -> TranscriptPosition {
    let place = |before: u64, total: Option<u64>, exact: bool, basis| {
        let (first, last) = if in_page == 0 {
            (None, None)
        } else {
            (Some(before + 1), Some(before + in_page))
        };
        // An estimate can come out below what this page alone proves;
        // never state a total smaller than the last message shown.
        let total = match (total, last) {
            (Some(t), Some(l)) => Some(t.max(l)),
            (t, _) => t,
        };
        TranscriptPosition {
            first,
            last,
            total,
            exact,
            basis,
        }
    };
    if whole {
        return place(0, Some(in_page), true, PositionBasis::WholeFile);
    }
    if let Some(ix) = index.filter(|ix| ix.covered > 0) {
        let density = ix.count as f64 / ix.covered as f64;
        let at = |offset: u64| -> u64 {
            if offset <= ix.covered {
                ix.before(offset)
            } else {
                ix.count + ((offset - ix.covered) as f64 * density).round() as u64
            }
        };
        let before = if start == 0 { 0 } else { at(start) };
        let total = at(boundary);
        let basis = if ix.covered >= boundary {
            PositionBasis::Index
        } else {
            PositionBasis::PartialIndex
        };
        return place(before, Some(total), false, basis);
    }
    // No index: this page's messages per byte, extrapolated. Crude, and
    // labelled so; a page of any records at all gives a figure.
    let span = end.saturating_sub(start);
    if span == 0 || records == 0 {
        return place(0, None, false, PositionBasis::Bytes);
    }
    let density = in_page as f64 / span as f64;
    let before = if start == 0 {
        0
    } else {
        (start as f64 * density).round() as u64
    };
    let after = (boundary.saturating_sub(end) as f64 * density).round() as u64;
    place(
        before,
        Some(before + in_page + after),
        false,
        PositionBasis::Bytes,
    )
}

/// Every record's offset in one transcript, with how many messages a
/// whole read would show before it.
///
/// # Why an index, and what it costs
///
/// A page knows where it is in BYTES. "Message 4,200 of 16,700" needs a
/// count of what lies before it, which only a read of what lies before it
/// can give. The index is that read, done once per file version, OFF the
/// request path (a worker thread, [`index_for`]), and kept: a page
/// consults it in O(log n) and never waits for it.
///
/// - **Memory**: 16 bytes a record -- about 512 KB for the 70 MB
///   fixture's 32,741 records. At most [`INDEX_FILES`] are kept.
/// - **Time**: one streamed pass, records over [`RECORD_HOLD_BYTES`]
///   skimmed, bounded by [`INDEX_DEADLINE`]. Past the deadline it keeps
///   what it counted (partial is not nothing) and positions beyond it are
///   extrapolated, [`PositionBasis::PartialIndex`].
/// - **Keyed** by path, and checked on every use against the file's
///   size, mtime, first bytes and the bytes behind its covered end. An
///   append EXTENDS it from where it stopped; anything else (a rewrite,
///   a truncation) rebuilds it.
/// - **What it counts** is records that render, less tool-result records
///   whose call is in the file (a whole read absorbs those into the
///   call). Close to a whole read's message count and not equal to it,
///   which is why an index figure is still labelled an estimate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionIndex {
    /// `(record offset, messages before it)`, in file order.
    records: Vec<(u64, u64)>,
    /// The index covers `[0, covered)`; `covered` is a boundary.
    covered: u64,
    /// Messages in `[0, covered)`.
    count: u64,
    head_digest: String,
    tail_digest: String,
    mtime: Option<SystemTime>,
    size: u64,
    /// Tool-call ids seen, so a result record is known to be absorbed.
    /// Dropped once built.
    calls: Option<std::collections::HashSet<String>>,
}

impl PositionIndex {
    /// How many records it covers.
    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    /// Messages before the record at or after `offset`.
    fn before(&self, offset: u64) -> u64 {
        let i = self.records.partition_point(|(o, _)| *o < offset);
        self.records.get(i).map_or(self.count, |(_, n)| *n)
    }
}

/// How many transcripts' indexes are kept.
pub const INDEX_FILES: usize = 8;

/// The most one index build may take before it keeps what it has.
pub const INDEX_DEADLINE: Duration = Duration::from_secs(10);

enum Slot {
    /// A worker is building or extending it; the index it started from,
    /// if any, still serves meanwhile.
    Building(Option<Arc<PositionIndex>>),
    Ready(Arc<PositionIndex>, Instant),
}

static INDEXES: LazyLock<Mutex<HashMap<PathBuf, Slot>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The cached index for `path`, if one still describes the file --
/// possibly covering only part of it. When it is missing, stale or short,
/// a worker thread builds or extends it; the page never waits for that.
fn index_for(path: &Path, r: &mut Reader, boundary: u64) -> Option<Arc<PositionIndex>> {
    let (cached, building) = {
        let map = INDEXES.lock().ok()?;
        match map.get(path) {
            Some(Slot::Ready(ix, _)) => (Some(Arc::clone(ix)), false),
            Some(Slot::Building(ix)) => (ix.clone(), true),
            None => (None, false),
        }
    };
    let usable = cached.filter(|ix| still_describes(ix, r));
    let complete = usable.as_ref().is_some_and(|ix| ix.covered >= boundary);
    if !complete && !building {
        let mut map = INDEXES.lock().ok()?;
        if map.len() >= INDEX_FILES && !map.contains_key(path) {
            // Evict the least recently built.
            let oldest = map
                .iter()
                .filter_map(|(p, s)| match s {
                    Slot::Ready(_, at) => Some((p.clone(), *at)),
                    Slot::Building(_) => None,
                })
                .min_by_key(|(_, at)| *at)
                .map(|(p, _)| p);
            if let Some(p) = oldest {
                map.remove(&p);
            }
        }
        map.insert(path.to_path_buf(), Slot::Building(usable.clone()));
        drop(map);
        let owned = path.to_path_buf();
        let from = usable.as_deref().cloned();
        let spawned = std::thread::Builder::new()
            .name("transcript-index".into())
            .spawn(move || {
                let built = build_index(&owned, from, Instant::now() + INDEX_DEADLINE);
                if let Ok(mut map) = INDEXES.lock() {
                    match built {
                        Ok(ix) => {
                            map.insert(owned, Slot::Ready(Arc::new(ix), Instant::now()));
                        }
                        Err(_) => {
                            map.remove(&owned);
                        }
                    }
                }
            });
        if spawned.is_err() {
            if let Ok(mut map) = INDEXES.lock() {
                map.remove(path);
            }
        }
    }
    usable
}

/// Whether an index built earlier still describes the file: nothing it
/// covered has changed, as far as a bounded check can tell.
fn still_describes(ix: &PositionIndex, r: &mut Reader) -> bool {
    if r.size < ix.covered {
        return false;
    }
    let mtime = r.file.metadata().ok().and_then(|m| m.modified().ok());
    if r.size == ix.size && mtime == ix.mtime {
        return true;
    }
    let head = r.digest_behind(ix.covered.min(CURSOR_FINGERPRINT_BYTES));
    let tail = r.digest_behind(ix.covered);
    head.is_ok_and(|h| h == ix.head_digest) && tail.is_ok_and(|t| t == ix.tail_digest)
}

/// Build (or extend) the index of `path`.
///
/// # Errors
///
/// The file cannot be read.
pub(crate) fn build_index(
    path: &Path,
    from: Option<PositionIndex>,
    deadline: Instant,
) -> Result<PositionIndex, String> {
    let mut r = Reader::open(path)?;
    let boundary = r.boundary_end()?;
    let mtime = r.file.metadata().ok().and_then(|m| m.modified().ok());
    let mut ix = from.unwrap_or(PositionIndex {
        records: Vec::new(),
        covered: 0,
        count: 0,
        head_digest: String::new(),
        tail_digest: String::new(),
        mtime: None,
        size: 0,
        calls: None,
    });
    let mut calls = ix.calls.take().unwrap_or_default();
    let mut fwd = Forward {
        start: ix.covered,
        buf: Vec::new(),
    };
    loop {
        if Instant::now() >= deadline {
            break;
        }
        let at = fwd.start;
        let Some(line) = fwd.next_line(&mut r, boundary)? else {
            break;
        };
        // Held bytes are released per record here; the bound that
        // matters is the per-record one, not the total.
        r.held = 0;
        ix.records.push((at, ix.count));
        if counts_as_message(&line, &mut calls) {
            ix.count += 1;
        }
        ix.covered = fwd.start;
    }
    ix.head_digest = r.digest_behind(ix.covered.min(CURSOR_FINGERPRINT_BYTES))?;
    ix.tail_digest = r.digest_behind(ix.covered)?;
    ix.size = r.size;
    ix.mtime = mtime;
    ix.calls = (ix.covered < boundary).then_some(calls);
    Ok(ix)
}

/// Whether a record will be a message in a whole read.
fn counts_as_message(line: &Line, calls: &mut std::collections::HashSet<String>) -> bool {
    if !line.may_render() {
        return false;
    }
    let Some(v) = &line.value else {
        return false;
    };
    let t = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let content = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array());
    let Some(items) = content else {
        return true;
    };
    if t == "assistant" {
        for b in items {
            if b.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                if let Some(id) = b.get("id").and_then(|i| i.as_str()) {
                    calls.insert(id.to_owned());
                }
            }
        }
        return true;
    }
    // A user record of nothing but results whose calls were all seen is
    // absorbed into those calls.
    let absorbed = t == "user"
        && v.get("isCompactSummary").and_then(|b| b.as_bool()) != Some(true)
        && !items.is_empty()
        && items.iter().all(|b| {
            b.get("type").and_then(|t| t.as_str()) == Some("tool_result")
                && b.get("tool_use_id")
                    .and_then(|i| i.as_str())
                    .is_some_and(|i| calls.contains(i))
        });
    !absorbed
}

// ---------------------------------------------------------------------
// Finding messages in the whole file (#1484)
// ---------------------------------------------------------------------

/// The most hits one find returns, and the default `limit`.
pub const FIND_HITS: usize = 500;

/// The most one find may take before it keeps what it found.
///
/// A find streams the whole file, as the position index does, and is
/// bounded the same way: past the deadline it answers with what it
/// found (partial is not nothing) and says how far it got.
pub const FIND_DEADLINE: Duration = Duration::from_secs(8);

/// How much text either side of a match a snippet keeps.
const FIND_CONTEXT_CHARS: usize = 60;

/// One message a find located: where it is, and enough to name it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindHit {
    /// The message's id, as a page read gives it. A result record's id
    /// is its own even though a merge absorbs it into its call.
    pub message_id: String,
    /// The record's start, handed back as a page anchor to read from it.
    pub cursor: PageCursor,
    pub timestamp: Option<String>,
    /// The text around the match, or an opener's first text.
    pub snippet: String,
    /// The message opens a turn: a prompt, slash command or shell input.
    pub opener: bool,
}

/// What a find over the whole file located.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptFind {
    /// Oldest first.
    pub hits: Vec<FindHit>,
    /// The hit limit was reached: more matches may exist after the last.
    pub more: bool,
    /// The scan reached the end of the file. `false` when the deadline
    /// stopped it: nothing after `scanned_to` was looked at.
    pub complete: bool,
    /// Where the scan stopped, a record boundary.
    pub scanned_to: u64,
    pub file_bytes: u64,
    /// Records too large to hold, searched only in the part a skim kept.
    pub skimmed_records: usize,
}

/// Find messages in `path`: every turn opener when `query` is `None`
/// (the outline), otherwise every message whose text contains `query`,
/// ignoring case.
///
/// Streams the file record by record, holding one at a time, bounded by
/// `limit` hits (clamped to [`FIND_HITS`]) and [`FIND_DEADLINE`]. Each
/// hit carries a cursor at its record, so the caller reads the page
/// holding it with an ordinary [`PageAnchor::Cursor`] -- the cursor's
/// digest guards it against a rewrite exactly as a page's own does.
///
/// `matching` says which text `query` is matched against (#1519): the
/// real text for the desktop's window, the masked text for a phone --
/// so a phone's query can only hit what it could see, and a hit or a
/// miss says nothing about a secret. Masked, each string is masked
/// WHOLE before it is matched and clipped to a snippet, so a snippet's
/// clip can never cut a secret short of its recognisable shape.
///
/// # Errors
///
/// Only when the file cannot be opened, sized, sought or read.
pub fn find(
    path: &Path,
    query: Option<&str>,
    limit: Option<usize>,
    matching: Matching,
) -> Result<TranscriptFind, String> {
    find_until(path, query, limit, matching, Instant::now() + FIND_DEADLINE)
}

pub(crate) fn find_until(
    path: &Path,
    query: Option<&str>,
    limit: Option<usize>,
    matching: Matching,
    deadline: Instant,
) -> Result<TranscriptFind, String> {
    let limit = limit.unwrap_or(FIND_HITS).clamp(1, FIND_HITS);
    let needle = query
        .map(|q| q.replace("\r\n", "\n").trim().to_lowercase())
        .filter(|q| !q.is_empty())
        .map(|q| Needle::new(&q, matching));
    let mut r = Reader::open(path)?;
    let boundary = r.boundary_end()?;
    let mut fwd = Forward {
        start: 0,
        buf: Vec::new(),
    };
    let mut out = TranscriptFind {
        hits: Vec::new(),
        more: false,
        complete: false,
        scanned_to: 0,
        file_bytes: r.size,
        skimmed_records: 0,
    };
    loop {
        if Instant::now() >= deadline {
            break;
        }
        let Some(line) = fwd.next_line(&mut r, boundary)? else {
            out.complete = true;
            break;
        };
        // One record held at a time; the bound is per record.
        r.held = 0;
        out.scanned_to = fwd.start;
        if !line.may_render() {
            continue;
        }
        if line.oversized.is_some() {
            out.skimmed_records += 1;
        }
        let Some(v) = &line.value else { continue };
        let found = match &needle {
            Some(n) => match snippet_in(v, n) {
                Some(s) => Some(s),
                None => continue,
            },
            // Only a user record can open a turn.
            None if v.get("type").and_then(|t| t.as_str()) == Some("user") => None,
            None => continue,
        };
        let Some(at) = line.offset else { continue };
        let page = build(std::slice::from_ref(&line), Seed::default(), Some(path));
        let Some(m) = page
            .messages
            .into_iter()
            .find(|m| m.id_source != IdSource::Derived)
        else {
            continue;
        };
        let opener = m.turn_id.as_deref() == Some(m.id.as_str()) && !m.is_meta && !m.is_sidechain;
        if needle.is_none() && !opener {
            continue;
        }
        if out.hits.len() == limit {
            out.more = true;
            break;
        }
        let snippet = found.unwrap_or_else(|| opener_text(&m));
        out.hits.push(FindHit {
            message_id: m.id,
            cursor: PageCursor {
                offset: at,
                behind_digest: r.digest_behind(at)?,
            },
            timestamp: m.timestamp,
            snippet,
            opener,
        });
    }
    if out.complete {
        out.scanned_to = boundary;
    }
    Ok(out)
}

/// What an opener says, in one line: its first text, or the command.
fn opener_text(m: &TranscriptMessage) -> String {
    use super::transcript_model::TranscriptBlock;
    let text = m.blocks.iter().find_map(|b| match b {
        TranscriptBlock::Text { text, .. } => Some(text.as_str()),
        _ => None,
    });
    let s = match (&m.kind, text) {
        (MessageKind::SlashCommand { name }, Some(t)) => format!("{name} {t}"),
        (MessageKind::SlashCommand { name }, None) => name.clone(),
        (MessageKind::ShellInput { command }, _) => format!("! {command}"),
        (_, Some(t)) => t.to_owned(),
        (_, None) => String::new(),
    };
    clip_chars(&one_line(&s), 2 * FIND_CONTEXT_CHARS)
}

/// The text of a record a person could read: prompt and reply text,
/// thinking, tool arguments and tool output -- each string separately,
/// so a match never spans two of them.
fn texts_of<'a>(v: &'a serde_json::Value, out: &mut Vec<&'a str>) {
    match v.get("message").and_then(|m| m.get("content")) {
        Some(serde_json::Value::String(s)) => out.push(s),
        Some(serde_json::Value::Array(items)) => {
            for b in items {
                for key in ["text", "thinking"] {
                    if let Some(s) = b.get(key).and_then(|s| s.as_str()) {
                        out.push(s);
                    }
                }
                if let Some(input) = b.get("input") {
                    strings_in(input, out);
                }
                match b.get("content") {
                    Some(serde_json::Value::String(s)) => out.push(s),
                    Some(serde_json::Value::Array(parts)) => {
                        for p in parts {
                            if let Some(s) = p.get("text").and_then(|s| s.as_str()) {
                                out.push(s);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    // System records and summaries carry their text at the top level.
    for key in ["content", "summary"] {
        if let Some(s) = v.get(key).and_then(|s| s.as_str()) {
            out.push(s);
        }
    }
}

fn strings_in<'a>(v: &'a serde_json::Value, out: &mut Vec<&'a str>) {
    match v {
        serde_json::Value::String(s) => out.push(s),
        serde_json::Value::Array(a) => a.iter().for_each(|x| strings_in(x, out)),
        serde_json::Value::Object(o) => o.values().for_each(|x| strings_in(x, out)),
        _ => {}
    }
}

/// A find's query, prepared once per find rather than once per record.
struct Needle {
    /// The query, lowercased, as chars.
    want: Vec<char>,
    matching: Matching,
    /// Masked, and the query could match inside a marker: every string
    /// must be masked to be matched, not only those it occurs in.
    marker_reach: bool,
}

impl Needle {
    fn new(lower: &str, matching: Matching) -> Needle {
        Needle {
            want: lower.chars().collect(),
            matching,
            marker_reach: matching == Matching::Masked
                && privacy::needle_could_touch_a_marker(lower),
        }
    }
}

/// The first match of `needle` (already lowercase) in the record's
/// text, with [`FIND_CONTEXT_CHARS`] either side; `None` when it has none.
///
/// [`Matching::Masked`] matches each string as `privacy::mask_text`
/// leaves it, and the snippet is cut from that masked string. Only the
/// strings the needle occurs in are masked, unless the needle could
/// match inside a marker itself: masking elsewhere in a string cannot
/// create an occurrence that was not there
/// ([`privacy::needle_could_touch_a_marker`] argues it), so the cost of
/// masking follows the matches rather than the file.
fn snippet_in(v: &serde_json::Value, needle: &Needle) -> Option<String> {
    let Needle {
        want,
        matching,
        marker_reach,
    } = needle;
    let mut texts = Vec::new();
    texts_of(v, &mut texts);
    for t in texts {
        let normal = t.replace("\r\n", "\n");
        let found = snippet_of(&normal, want);
        match matching {
            Matching::Unmasked => {
                if found.is_some() {
                    return found;
                }
            }
            Matching::Masked => {
                if found.is_none() && !marker_reach {
                    continue;
                }
                match privacy::mask_text(&normal) {
                    // Nothing hidden: the real text is the masked text.
                    (_, 0) => {
                        if found.is_some() {
                            return found;
                        }
                    }
                    (masked, _) => {
                        if let Some(s) = snippet_of(&masked, want) {
                            return Some(s);
                        }
                    }
                }
            }
        }
    }
    None
}

/// The first match of `want` (lowercase) in `text`, with
/// [`FIND_CONTEXT_CHARS`] either side.
fn snippet_of(text: &str, want: &[char]) -> Option<String> {
    // Char by char, lowercased one to one, so an index into `lower`
    // is an index into `chars` whatever the script.
    let chars: Vec<char> = text.chars().collect();
    if want.len() > chars.len() {
        return None;
    }
    let lower: Vec<char> = chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();
    let i = lower.windows(want.len()).position(|w| w == want)?;
    let from = i.saturating_sub(FIND_CONTEXT_CHARS);
    let to = (i + want.len() + FIND_CONTEXT_CHARS).min(chars.len());
    let mut s = one_line(&chars[from..to].iter().collect::<String>());
    if from > 0 {
        s.insert(0, '…');
    }
    if to < chars.len() {
        s.push('…');
    }
    Some(s)
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip_chars(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(n).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::transcript_model::{self, IdSource, TranscriptBlock};

    fn user(uuid: &str, text: &str) -> serde_json::Value {
        serde_json::json!({"type": "user", "uuid": uuid, "timestamp": "2026-01-01T00:00:00Z",
            "message": {"role": "user", "content": text}})
    }

    fn assistant(uuid: &str, model: &str, text: &str) -> serde_json::Value {
        serde_json::json!({"type": "assistant", "uuid": uuid, "timestamp": "2026-01-01T00:00:01Z",
            "message": {"id": format!("msg_{uuid}"), "model": model, "role": "assistant",
                "content": [{"type": "text", "text": text}]}})
    }

    fn call(uuid: &str, tool: &str) -> serde_json::Value {
        serde_json::json!({"type": "assistant", "uuid": uuid, "timestamp": "2026-01-01T00:00:02Z",
            "message": {"id": format!("msg_{uuid}"), "model": "model-a", "role": "assistant",
                "content": [{"type": "tool_use", "id": tool, "name": "Bash",
                    "input": {"command": "ls", "description": "list"}}]}})
    }

    fn result(uuid: &str, tool: &str, text: &str) -> serde_json::Value {
        serde_json::json!({"type": "user", "uuid": uuid, "timestamp": "2026-01-01T00:00:03Z",
            "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": tool,
                "content": text}]}})
    }

    fn mode() -> serde_json::Value {
        serde_json::json!({"type": "permission-mode", "permissionMode": "default"})
    }

    fn bookkeeping() -> serde_json::Value {
        serde_json::json!({"type": "queue-operation", "operation": "enqueue"})
    }

    fn write(dir: &Path, name: &str, records: &[serde_json::Value]) -> PathBuf {
        let p = dir.join(name);
        let mut body = String::new();
        for r in records {
            body.push_str(&r.to_string());
            body.push('\n');
        }
        std::fs::write(&p, body).unwrap();
        p
    }

    /// A conversation of `turns` turns with calls, results, uuid-less
    /// records and a model switch -- everything a seam can split.
    pub(crate) fn conversation(turns: usize) -> Vec<serde_json::Value> {
        let mut out = vec![mode()];
        for t in 0..turns {
            out.push(bookkeeping());
            out.push(mode());
            out.push(user(&format!("u{t}"), &format!("prompt {t}")));
            let model = if t % 3 == 2 { "model-b" } else { "model-a" };
            out.push(assistant(
                &format!("a{t}"),
                model,
                &format!("thinking about {t}"),
            ));
            out.push(call(&format!("c{t}"), &format!("t{t}")));
            out.push(mode());
            out.push(result(
                &format!("r{t}"),
                &format!("t{t}"),
                &"out ".repeat(40),
            ));
            out.push(assistant(&format!("d{t}"), model, &format!("done {t}")));
        }
        out
    }

    /// Every page before the end, walked back to the start.
    fn walk_back(p: &Path, limit: usize) -> Vec<TranscriptWindow> {
        let mut pages = Vec::new();
        let mut anchor = PageAnchor::End;
        loop {
            let w = read_page(
                p,
                &anchor,
                PageDirection::Before,
                Some(limit),
                IndexUse::None,
            )
            .unwrap();
            let done = w.at_start;
            anchor = PageAnchor::Cursor {
                offset: w.start.offset,
                behind_digest: w.start.behind_digest.clone(),
            };
            pages.push(w);
            if done {
                break;
            }
            assert!(pages.len() < 10_000, "walk does not terminate");
        }
        pages.reverse();
        pages
    }

    fn whole(p: &Path) -> Vec<TranscriptMessage> {
        let bytes = std::fs::read(p).unwrap();
        let lines = transcript_model::lines_of(&bytes, 0);
        build(&lines, Seed::at(WindowStart::FileStart), Some(p)).messages
    }

    /// Pages tile the file exactly: each ends where the next starts, the
    /// first at 0 and the last at the end, both directions.
    #[test]
    fn pages_tile_the_file_in_both_directions() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "t.jsonl", &conversation(30));
        let size = std::fs::metadata(&p).unwrap().len();
        for limit in [1, 3, 7, 50, 200] {
            let back = walk_back(&p, limit);
            assert_eq!(back[0].start.offset, 0);
            assert_eq!(back.last().unwrap().end.offset, size);
            for pair in back.windows(2) {
                assert_eq!(pair[0].end.offset, pair[1].start.offset, "limit {limit}");
            }
            // Forward from the start.
            let mut anchor = PageAnchor::Start;
            let mut fwd = Vec::new();
            loop {
                let w = read_page(
                    &p,
                    &anchor,
                    PageDirection::After,
                    Some(limit),
                    IndexUse::None,
                )
                .unwrap();
                let done = w.at_end;
                anchor = PageAnchor::Cursor {
                    offset: w.end.offset,
                    behind_digest: w.end.behind_digest.clone(),
                };
                fwd.push(w);
                if done {
                    break;
                }
            }
            assert_eq!(fwd.last().unwrap().end.offset, size);
            for pair in fwd.windows(2) {
                assert_eq!(pair[0].end.offset, pair[1].start.offset);
            }
            for w in back.iter().chain(&fwd) {
                assert!(w.page.messages.len() <= limit, "limit {limit}");
            }
        }
    }

    /// Every record lands in exactly one page: the messages of all pages,
    /// by id, are the whole read's -- none lost, none twice.
    ///
    /// Sabotaged by making `gather_before` skip every tenth line: this
    /// fails on the missing ids.
    #[test]
    fn no_record_is_lost_or_repeated_across_pages() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "t.jsonl", &conversation(40));
        let whole_ids: std::collections::HashSet<String> = whole(&p)
            .iter()
            .filter(|m| m.id_source != IdSource::Derived)
            .flat_map(|m| {
                let mut ids = vec![m.id.clone()];
                for b in &m.blocks {
                    if let TranscriptBlock::ToolCall {
                        result: Some(o), ..
                    } = b
                    {
                        ids.push(o.message_id.clone());
                    }
                }
                ids
            })
            .collect();
        for limit in [2, 5, 13] {
            let mut ids = Vec::new();
            for w in walk_back(&p, limit) {
                for m in &w.page.messages {
                    if m.id_source == IdSource::Derived {
                        continue;
                    }
                    ids.push(m.id.clone());
                    for b in &m.blocks {
                        if let TranscriptBlock::ToolCall {
                            result: Some(o), ..
                        } = b
                        {
                            ids.push(o.message_id.clone());
                        }
                    }
                }
            }
            let unique: std::collections::HashSet<String> = ids.iter().cloned().collect();
            assert_eq!(unique.len(), ids.len(), "a record appeared twice");
            assert_eq!(unique, whole_ids, "limit {limit}");
        }
    }

    /// A page that starts with uuid-less records gives them the ids a
    /// whole read does, by looking behind itself for the anchor.
    ///
    /// Sabotaged by skipping `lookback_seed`: the leading
    /// `permission-mode` records come back `unanchored`.
    #[test]
    fn a_page_starting_on_uuid_less_records_anchors_them_as_a_whole_read_does() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "t.jsonl", &conversation(20));
        let whole: HashMap<Option<u64>, (String, IdSource)> = whole(&p)
            .into_iter()
            .map(|m| (m.offset, (m.id, m.id_source)))
            .collect();
        let mut checked = 0;
        for limit in [1, 2, 3, 4, 5, 6] {
            for w in walk_back(&p, limit) {
                for m in &w.page.messages {
                    // A derived marker has no record; a standing result
                    // is absorbed into its call in the whole read.
                    if m.id_source == IdSource::Derived || m.kind == MessageKind::ToolResults {
                        continue;
                    }
                    let (id, source) = &whole[&m.offset];
                    assert_eq!((&m.id, &m.id_source), (id, source), "limit {limit}");
                    if m.id_source == IdSource::Anchored {
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 20, "only {checked} anchored ids were compared");
    }

    /// A record larger than a page is one message with its size and
    /// clipped content -- never dropped, never held whole.
    ///
    /// Sabotaged by returning `None` for `Raw::Big` in `Reader::line`:
    /// the result disappears and this fails.
    #[test]
    fn a_record_larger_than_a_page_is_one_honest_message() {
        let dir = tempfile::tempdir().unwrap();
        let huge = "x".repeat(3 * PAGE_BYTES as usize);
        let mut recs = conversation(3);
        recs.push(call("big-call", "tbig"));
        recs.push(result("big-result", "tbig", &huge));
        recs.push(assistant("after", "model-a", "the reply"));
        let p = write(dir.path(), "t.jsonl", &recs);
        for dir in [PageDirection::Before, PageDirection::After] {
            let anchor = match dir {
                PageDirection::Before => PageAnchor::End,
                PageDirection::After => PageAnchor::Start,
            };
            let mut all = Vec::new();
            let mut a = anchor;
            loop {
                let w = read_page(&p, &a, dir, Some(PAGE_MESSAGES), IndexUse::None).unwrap();
                assert!(
                    w.page.bytes_read <= PAGE_READ_BOUND,
                    "{} > {PAGE_READ_BOUND}",
                    w.page.bytes_read
                );
                let (done, next) = match dir {
                    PageDirection::Before => (w.at_start, w.start.clone()),
                    PageDirection::After => (w.at_end, w.end.clone()),
                };
                a = PageAnchor::Cursor {
                    offset: next.offset,
                    behind_digest: next.behind_digest,
                };
                all.push(w);
                if done {
                    break;
                }
            }
            let found: Vec<&transcript_model::TranscriptToolOutput> = all
                .iter()
                .flat_map(|w| &w.page.messages)
                .flat_map(|m| &m.blocks)
                .filter_map(|b| match b {
                    TranscriptBlock::ToolCall {
                        result: Some(o), ..
                    } if o.message_id == "big-result" => Some(o),
                    TranscriptBlock::ToolResult(o) if o.message_id == "big-result" => Some(o),
                    _ => None,
                })
                .collect();
            assert_eq!(found.len(), 1, "{dir:?}");
            let clip = found[0].clip.expect("clipped");
            assert_eq!(clip.total_chars, huge.len(), "the true length is stated");
            assert_eq!(clip.shown_chars, crate::claude::preview::MAX_TEXT_CHARS);
            let msg = all
                .iter()
                .flat_map(|w| &w.page.messages)
                .find(|m| {
                    m.id == "big-result"
                        || m.blocks.iter().any(|b| {
                            matches!(b, TranscriptBlock::ToolCall { result: Some(o), .. } if o.message_id == "big-result")
                        })
                })
                .unwrap();
            // Oversized is carried by the record's own message when it
            // stands; when it merged into its call the call's message is
            // a different record.
            if msg.id == "big-result" {
                assert!(msg.oversized_bytes.unwrap() > 3 * PAGE_BYTES);
            }
            // And by the OUTPUT either way (#1476): merged into its call,
            // the record's message is gone, and this is what still says
            // the output was streamed and clipped.
            assert!(
                found[0].oversized_bytes.expect("carried onto the output") > 3 * PAGE_BYTES,
                "{dir:?}"
            );
            let scanned: u64 = all.iter().map(|w| w.bytes_scanned).sum();
            assert!(scanned >= 3 * PAGE_BYTES, "the big record was streamed");
        }
    }

    /// A page of large records stops at its byte budget, not at its
    /// message count, in both directions -- and at the start of the file
    /// as at the end.
    ///
    /// Sabotaged by removing the byte check from `gather_before` and
    /// `gather_after`: the pages run to 200 messages, ~2 MB, and this
    /// fails.
    #[test]
    fn a_page_of_large_records_stops_at_its_byte_budget() {
        let dir = tempfile::tempdir().unwrap();
        let mut recs = Vec::new();
        for t in 0..300 {
            recs.push(user(&format!("u{t}"), &"p".repeat(8 * 1024)));
            recs.push(assistant(
                &format!("a{t}"),
                "model-a",
                &"r".repeat(8 * 1024),
            ));
        }
        let p = write(dir.path(), "t.jsonl", &recs);
        for (anchor, dir) in [
            (PageAnchor::End, PageDirection::Before),
            (PageAnchor::Start, PageDirection::After),
        ] {
            let w = read_page(&p, &anchor, dir, None, IndexUse::None).unwrap();
            let span = w.end.offset - w.start.offset;
            assert!(
                span <= PAGE_BYTES + RECORD_HOLD_BYTES,
                "{dir:?}: the page spans {span} bytes"
            );
            assert!(w.page.bytes_read <= PAGE_READ_BOUND);
            assert!(w.page.messages.len() < PAGE_MESSAGES, "bytes bound first");
            assert!(w.page.messages.len() > 10);
        }
    }

    /// A cursor into a file that was rewritten is noticed, and the page
    /// comes from the end with `rewritten` set -- not spliced onto
    /// history that no longer exists.
    #[test]
    fn a_cursor_into_a_rewritten_file_is_not_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "t.jsonl", &conversation(20));
        let w = read_page(
            &p,
            &PageAnchor::End,
            PageDirection::Before,
            Some(5),
            IndexUse::None,
        )
        .unwrap();
        let cursor = PageAnchor::Cursor {
            offset: w.start.offset,
            behind_digest: w.start.behind_digest.clone(),
        };
        // Untouched: honoured.
        let again = read_page(&p, &cursor, PageDirection::Before, Some(5), IndexUse::None).unwrap();
        assert!(!again.rewritten);
        assert_eq!(again.end.offset, w.start.offset);
        // Rewritten with different content of the same shape.
        let mut recs = conversation(20);
        recs[3] = user("u0", "a different prompt entirely");
        write(dir.path(), "t.jsonl", &recs);
        let after = read_page(&p, &cursor, PageDirection::Before, Some(5), IndexUse::None).unwrap();
        assert!(after.rewritten);
        assert!(after.at_end);
        // Truncated below the cursor.
        std::fs::write(&p, "").unwrap();
        let empty = read_page(&p, &cursor, PageDirection::After, Some(5), IndexUse::None).unwrap();
        assert!(empty.rewritten);
        assert!(empty.page.messages.is_empty());
        assert_eq!(empty.position.first, None, "nothing to place is not 0");
    }

    /// A record still being written is not read, and a cursor never lands
    /// mid-record.
    #[test]
    fn a_partial_trailing_record_is_left_for_later() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "t.jsonl", &conversation(3));
        let complete = std::fs::metadata(&p).unwrap().len();
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        std::io::Write::write_all(&mut f, br#"{"type":"user","uuid":"half"#).unwrap();
        let w = read_page(
            &p,
            &PageAnchor::End,
            PageDirection::Before,
            None,
            IndexUse::None,
        )
        .unwrap();
        assert_eq!(w.end.offset, complete);
        assert!(w.at_end);
        assert!(w.page.messages.iter().all(|m| m.id != "half"));
    }

    /// The position of a page is a count when the page is the whole file,
    /// and an estimate -- said to be one -- otherwise.
    #[test]
    fn positions_are_counts_only_when_they_can_be() {
        let dir = tempfile::tempdir().unwrap();
        let small = write(dir.path(), "s.jsonl", &conversation(2));
        let w = read_page(
            &small,
            &PageAnchor::End,
            PageDirection::Before,
            None,
            IndexUse::None,
        )
        .unwrap();
        assert!(w.position.exact);
        assert_eq!(w.position.basis, PositionBasis::WholeFile);
        assert_eq!(w.position.first, Some(1));
        assert_eq!(w.position.total, Some(w.page.messages.len() as u64));

        let big = write(dir.path(), "b.jsonl", &conversation(60));
        let whole_count = whole(&big).len() as u64;
        let pages = walk_back(&big, 10);
        let mid = &pages[pages.len() / 2];
        assert!(!mid.position.exact);
        assert_eq!(mid.position.basis, PositionBasis::Bytes);

        let ix = build_index(&big, None, Instant::now() + INDEX_DEADLINE).unwrap();
        assert_eq!(ix.covered, std::fs::metadata(&big).unwrap().len());
        let anchor = PageAnchor::Cursor {
            offset: mid.end.offset,
            behind_digest: mid.end.behind_digest.clone(),
        };
        let with = read_page(
            &big,
            &anchor,
            PageDirection::Before,
            Some(10),
            IndexUse::Given(&ix),
        )
        .unwrap();
        assert_eq!(with.position.basis, PositionBasis::Index);
        assert!(!with.position.exact);
        // The index's total is within the model-change markers of a
        // whole read's count, which is why it is an estimate and why it
        // is a good one.
        let total = with.position.total.unwrap();
        let changes = whole(&big)
            .iter()
            .filter(|m| matches!(m.kind, MessageKind::ModelChange { .. }))
            .count() as u64;
        assert_eq!(total + changes, whole_count, "index total {total}");
        // And the page's first ordinal agrees with where its first
        // message sits in the whole read, give or take those markers.
        let first_id = &with.page.messages[0].id;
        let at = whole(&big).iter().position(|m| &m.id == first_id).unwrap() as u64 + 1;
        let first = with.position.first.unwrap();
        assert!(
            first <= at && at - first <= changes,
            "first {first}, actual {at}"
        );
    }

    /// An append extends the index from where it stopped; a rewrite is
    /// noticed and not trusted.
    #[test]
    fn the_index_extends_on_append_and_is_distrusted_on_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "t.jsonl", &conversation(10));
        let ix = build_index(&p, None, Instant::now() + INDEX_DEADLINE).unwrap();
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        for r in conversation(2).iter().skip(1) {
            let mut r = r.clone();
            if let Some(u) = r
                .get("uuid")
                .and_then(|u| u.as_str())
                .map(|u| format!("x{u}"))
            {
                r["uuid"] = serde_json::json!(u);
            }
            std::io::Write::write_all(&mut f, format!("{r}\n").as_bytes()).unwrap();
        }
        drop(f);
        let mut r = Reader::open(&p).unwrap();
        assert!(
            still_describes(&ix, &mut r),
            "an append keeps the prefix valid"
        );
        let extended = build_index(&p, Some(ix.clone()), Instant::now() + INDEX_DEADLINE).unwrap();
        let fresh = build_index(&p, None, Instant::now() + INDEX_DEADLINE).unwrap();
        assert_eq!(extended.records, fresh.records);
        assert_eq!(extended.count, fresh.count);

        let mut recs = conversation(10);
        recs[4] = user("u0", "rewritten");
        write(dir.path(), "t.jsonl", &recs);
        let mut r = Reader::open(&p).unwrap();
        assert!(!still_describes(&extended, &mut r));
    }

    /// Past its deadline an index keeps what it counted.
    #[test]
    fn an_index_out_of_time_keeps_what_it_counted() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "t.jsonl", &conversation(10));
        let ix = build_index(&p, None, Instant::now()).unwrap();
        assert_eq!(ix.covered, 0);
        let w = read_page(
            &p,
            &PageAnchor::End,
            PageDirection::Before,
            Some(5),
            IndexUse::Given(&ix),
        )
        .unwrap();
        // An empty index is no basis at all: fall back to bytes.
        assert_eq!(w.position.basis, PositionBasis::Bytes);
    }

    /// The seam names the page's first real assistant message and its
    /// last model, ignoring sidechains and `<synthetic>`.
    #[test]
    fn the_seam_names_the_models_at_the_page_edges() {
        let mut side = assistant("s1", "model-z", "side");
        side["isSidechain"] = serde_json::json!(true);
        let synth = assistant("y1", "<synthetic>", "x");
        let msgs = transcript_model::parse(
            &[
                side,
                synth,
                assistant("a1", "model-a", "one"),
                assistant("a2", "model-b", "two"),
            ]
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
            WindowStart::FileStart,
            None,
        )
        .messages;
        let s = seam(&msgs);
        let f = s.first_model.unwrap();
        assert_eq!((f.message_id.as_str(), f.model.as_str()), ("a1", "model-a"));
        assert_eq!(s.last_model.as_deref(), Some("model-b"));
    }

    /// The golden file `src/lib/transcriptPages.golden.json` is what this
    /// build produces: pages of a conversation at several limits, and one
    /// read of it. `src/lib/transcriptPages.test.ts` merges the pages and
    /// requires the single read -- the cross-language proof that merging
    /// by id reproduces `settle`.
    ///
    /// Regenerate with `HEADSTATE_WRITE_GOLDEN=1 cargo test --lib
    /// the_merge_golden_file_is_current`.
    #[test]
    fn the_merge_golden_file_is_current() {
        let dir = tempfile::tempdir().unwrap();
        // Written under a fixed name so no temporary path reaches the
        // golden file: subagent links resolve against it.
        let p = write(dir.path(), "golden.jsonl", &conversation(6));
        let mut cases = Vec::new();
        for limit in [1, 3, 200] {
            cases.push(serde_json::json!({
                "limit": limit,
                "windows": walk_back(&p, limit),
            }));
        }
        let golden = serde_json::json!({
            "note": "Generated by transcript_page.rs `the_merge_golden_file_is_current`; do not edit.",
            "whole": whole(&p),
            "cases": cases,
        });
        // Compact: generated, and checked by this test rather than read.
        let text = serde_json::to_string(&golden).unwrap() + "\n";
        let at =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/lib/transcriptPages.golden.json");
        if std::env::var_os("HEADSTATE_WRITE_GOLDEN").is_some() {
            std::fs::write(&at, &text).unwrap();
        }
        // A Windows checkout may turn the trailing newline into CRLF.
        let current = std::fs::read_to_string(&at)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        assert!(
            current == text,
            "{} is stale: regenerate it with HEADSTATE_WRITE_GOLDEN=1",
            at.display()
        );
    }

    /// #1484: with no query, a find is the outline -- every turn opener
    /// in the file, oldest first, each with a cursor that reads the page
    /// starting at it.
    #[test]
    fn a_find_without_a_query_lists_every_turn_opener() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "t.jsonl", &conversation(30));
        let found = find(&p, None, None, Matching::Unmasked).unwrap();
        assert!(found.complete);
        assert!(!found.more);
        let ids: Vec<&str> = found.hits.iter().map(|h| h.message_id.as_str()).collect();
        let want: Vec<String> = (0..30).map(|t| format!("u{t}")).collect();
        assert_eq!(ids, want.iter().map(String::as_str).collect::<Vec<_>>());
        assert!(found.hits.iter().all(|h| h.opener));
        assert_eq!(found.hits[7].snippet, "prompt 7");

        // The cursor is an ordinary page anchor: the page after it
        // starts with the hit.
        let h = &found.hits[7];
        let w = read_page(
            &p,
            &PageAnchor::Cursor {
                offset: h.cursor.offset,
                behind_digest: h.cursor.behind_digest.clone(),
            },
            PageDirection::After,
            None,
            IndexUse::None,
        )
        .unwrap();
        assert!(!w.rewritten);
        assert_eq!(w.page.messages[0].id, "u7");
    }

    /// A query matches any readable text -- here a tool result -- case
    /// insensitively, and the snippet shows the match in context.
    #[test]
    fn a_find_with_a_query_matches_text_anywhere_ignoring_case() {
        let dir = tempfile::tempdir().unwrap();
        let mut recs = conversation(3);
        recs.push(result("rx", "t2", "a line with Needle in it"));
        let p = write(dir.path(), "t.jsonl", &recs);
        let found = find(&p, Some("needle"), None, Matching::Unmasked).unwrap();
        assert_eq!(found.hits.len(), 1);
        assert_eq!(found.hits[0].message_id, "rx");
        assert!(!found.hits[0].opener);
        assert_eq!(found.hits[0].snippet, "a line with Needle in it");

        let prompts = find(&p, Some("PROMPT 1"), None, Matching::Unmasked).unwrap();
        assert_eq!(
            prompts
                .hits
                .iter()
                .map(|h| h.message_id.as_str())
                .collect::<Vec<_>>(),
            vec!["u1"]
        );
        assert!(find(&p, Some("absent words"), None, Matching::Unmasked)
            .unwrap()
            .hits
            .is_empty());
    }

    /// Bounded both ways, and each bound says so: the hit limit sets
    /// `more`, and a deadline that has passed keeps what was found and
    /// reports the scan incomplete rather than failing.
    #[test]
    fn a_find_says_when_it_stopped_short() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "t.jsonl", &conversation(10));
        let limited = find(&p, None, Some(4), Matching::Unmasked).unwrap();
        assert_eq!(limited.hits.len(), 4);
        assert!(limited.more);

        let stopped = find_until(&p, None, None, Matching::Unmasked, Instant::now()).unwrap();
        assert!(!stopped.complete);
        assert!(stopped.hits.is_empty());
        assert!(stopped.scanned_to < stopped.file_bytes);
    }

    /// Masked matching (#1519) is held to the same two bounds: the hit
    /// limit sets `more`, and a passed deadline keeps what was found --
    /// including for a needle that reaches into a marker, the one case
    /// that masks every string rather than only those it occurs in.
    #[test]
    fn a_masked_find_keeps_the_same_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let mut recs = conversation(10);
        recs.push(user("sx", "token sk-ant-api03-AAAAbbbbCCCCddddEEEE0000"));
        let p = write(dir.path(), "t.jsonl", &recs);
        let limited = find(&p, Some("prompt"), Some(4), Matching::Masked).unwrap();
        assert_eq!(limited.hits.len(), 4);
        assert!(limited.more);

        for needle in ["prompt", "hidden"] {
            let stopped =
                find_until(&p, Some(needle), None, Matching::Masked, Instant::now()).unwrap();
            assert!(!stopped.complete, "{needle}");
            assert!(stopped.hits.is_empty(), "{needle}");
        }
        let whole = find(&p, Some("hidden"), None, Matching::Masked).unwrap();
        assert!(whole.complete);
        assert_eq!(
            whole
                .hits
                .iter()
                .map(|h| h.message_id.as_str())
                .collect::<Vec<_>>(),
            vec!["sx"]
        );
    }
}

//! Incremental PR links with durable parser continuation (#1557, #1864).
//!
//! The live pass reuses the corpus listing and resumes accepted transcript
//! checkpoints. Offsets, pending create IDs, partial JSONL records, source
//! evidence and links commit in one SQLite transaction. Import resets the
//! baseline in its accepted graph transaction; absent a baseline, this lane
//! does not run. Old offset-only settings replay once to establish evidence.
//!
//! # Source evidence (#1865, #1875)
//!
//! Before seeking an existing offset, compare source identity/metadata and
//! the trailing accepted block. Replacements replay with empty parser state.
//! A matching trailing block alone does NOT prove append-only continuity:
//! the suffix from the earliest pending create or partial record (including
//! its preceding delimiter) is verified before combining old and new records.
//! This proves the absence of intervening terminal results as well as the call.
//! A long-unresolved call can therefore require a large, resumable suffix read. Remaining history
//! gets a bounded rotating checksum audit to detect middle rewrites eventually.
//! Audits finish despite ordinary appends; completed unchanged files return to
//! zero body reads. Opened-handle and path metadata must agree after reading.
//!
//! # Bounds and persistence
//!
//! Parsing, trailing checks, semantic-state proofs and historical audits all
//! spend the same per-pass byte budget, including reads preceding an error.
//! Partial checks and records resume across ticks/restart. Records above16MiB
//! discard through their newline. Pending create IDs have count/length limits;
//! overflow reports deferred/unreadable without advancing the prior checkpoint.
//!
//! A small baseline and per-source rows replace the legacy settings cursor.
//! Only changed sources serialize and write their evidence. Checksums grow
//! with accepted bytes (one per4KiB), and each file retains one bounded partial
//! record. Complete listings prune missing paths; partial listings retain them.
//!
//! Incremental ingestion only ADDS links, preserving existing first-seen
//! timestamps. Authoritative graph replacement owns removing associations
//! whose source changed or disappeared. No transcript is opened for writing.

use std::collections::BTreeMap;
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::Path;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(test)]
use crate::store::settings;
mod storage;
#[cfg(test)]
pub(crate) use storage::checkpoint_cursor;
pub(crate) use storage::migrate_legacy;

/// How many transcript bytes one pass may read.
///
/// 16 MiB. The import's `subagent::build` reads 0.86 GB with the same
/// per-line gates in 969 ms (measured, see `transcript::scan`), so this is
/// about 18 ms of reading at that rate -- an ESTIMATE from that
/// measurement, not a measurement of this pass. In steady state a pass
/// reads only what sessions appended in the last minute, far below it;
/// the bound is for the first pass after an import, which re-reads every
/// file modified since the import began from the start.
pub const BYTES_PER_PASS: u64 = 16 * 1024 * 1024;

/// mtime slack when deciding a file predates the import.
///
/// Filesystem mtimes are coarser than the clock on some platforms, and a
/// file appended in the same second the import started must be read
/// again rather than assumed covered. Over-reading is harmless -- every
/// insert is idempotent -- while under-reading loses a link until the
/// next import.
const MTIME_SLACK_MS: i64 = 2_000;

/// Where the incremental read has got to, persisted between passes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    /// When the last import BEGAN, in ms since the epoch. A file last
    /// modified before this was read whole by that import.
    pub since_ms: i64,
    /// Transcript path -> accepted bytes (partial lines live in checkpoints).
    pub offsets: BTreeMap<String, u64>,
    #[serde(default)]
    checkpoints: BTreeMap<String, Checkpoint>,
}

/// What one pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Refreshed {
    /// No import has set a cursor yet, so nothing was read.
    pub not_started: bool,
    /// Cancellation/deadline or a newer generation discarded this pass's staged
    /// work. Previously committed links and parser checkpoints are unchanged.
    pub stopped: Option<super::transcript::PopulationStatus>,
    /// Files read from, in whole or in part.
    pub files_read: usize,
    /// Transcript bytes consumed.
    pub bytes_read: u64,
    /// Links not already in the table.
    pub links_added: usize,
    /// Files with unread bytes the budget did not reach. They are read on
    /// a later pass; nothing is lost.
    pub deferred: usize,
    /// Files that could not be stat'ed or read, with why.
    pub unreadable: Vec<String>,
}

/// Start the cursor over from an import that began at `since_ms`.
///
/// Called in the accepted graph transaction. Anything a pass wrote before
/// that is forgotten safely: files modified since `since_ms` replay, and
/// graph publication cannot commit without its matching continuation.
/// Standalone callers receive the same atomicity through an owned transaction.
pub fn reset(conn: &Connection, since_ms: i64) -> Result<(), crate::store::StoreError> {
    if conn.is_autocommit() {
        #[cfg(feature = "enterprise-harness")]
        let tx = crate::enterprise_harness::resources::transaction(
            conn,
            rusqlite::TransactionBehavior::Immediate,
            "corpus-links",
        )?;
        #[cfg(not(feature = "enterprise-harness"))]
        let tx =
            rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
        reset(&tx, since_ms)?;
        tx.commit()?;
        return Ok(());
    }
    migrate_legacy(conn)?;
    super::transcript::population_generation(conn, "links")?;
    conn.execute("DELETE FROM claude_link_checkpoint", [])?;
    conn.execute("INSERT INTO claude_link_baseline VALUES(1, ?1) ON CONFLICT(singleton) DO UPDATE SET since_ms=excluded.since_ms", [since_ms])?;
    Ok(())
}

/// One bounded pass over the listed corpus.
///
/// `scan` is a listing (`transcript::corpus`): only each session's `path`
/// and `session_id` are read off it.
pub fn refresh(
    conn: &mut Connection,
    scan: &super::Scan,
    budget: u64,
) -> Result<Refreshed, crate::store::StoreError> {
    refresh_controlled(
        conn,
        scan,
        budget,
        &super::transcript::PopulationControl::discovery(),
    )
}

/// Cancellation is cooperative, including evidence reads and oversized records.
/// Read outside a write transaction, then fence atomic publication against both
/// pause/cancel and an accepted graph reset. A commit that wins the cancellation
/// race remains valid; stale work cannot publish after its database fence.
pub(crate) fn refresh_controlled(
    conn: &mut Connection,
    scan: &super::Scan,
    budget: u64,
    control: &super::transcript::PopulationControl,
) -> Result<Refreshed, crate::store::StoreError> {
    let mut out = Refreshed::default();
    if let Some(reason) = control.stop_reason() {
        out.stopped = Some(reason);
        return Ok(out);
    }
    let generation = super::transcript::population_generation(conn, "links")?;
    migrate_legacy(conn)?;
    let Some(since_ms) = storage::baseline(conn)? else {
        out.not_started = true;
        return Ok(out);
    };
    let offsets = storage::offsets(conn)?;
    // Decode one source at a time. Staged strings still scale with the changed
    // sources (and one large source remains large), never every warm source.
    let mut changed = Vec::new();

    let mut todo = Vec::new();
    for t in &scan.sessions {
        if let Some(reason) = control.stop_reason() {
            out.stopped = Some(reason);
            return Ok(out);
        }
        match std::fs::metadata(&t.path) {
            Ok(meta) => {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(i64::MAX);
                if !offsets.contains_key(&t.path) && mtime.saturating_add(MTIME_SLACK_MS) < since_ms
                {
                    continue;
                }
                todo.push((mtime, t));
            }
            Err(e) => out
                .unreadable
                .push(format!("{}: could not stat it: {e}", t.path)),
        }
    }
    todo.sort_by_key(|a| std::cmp::Reverse(a.0));
    let mut links = Vec::new();
    let mut left = budget;
    for (_, t) in todo {
        if let Some(reason) = stopped(conn, control, generation)? {
            out.stopped = Some(reason);
            return Ok(out);
        }
        if left == 0 {
            out.deferred += 1;
            continue;
        }
        // Old offset-only settings replay once: they cannot establish source
        // continuity or reconstruct an unfinished create/result pairing.
        let original = storage::checkpoint(conn, &t.path)?;
        let checkpoint = original.clone().unwrap_or_default();
        match read_from_controlled(
            Path::new(&t.path),
            &t.session_id,
            checkpoint,
            left,
            control,
            || {},
        ) {
            Ok(read) => {
                left = left.saturating_sub(read.consumed);
                out.bytes_read += read.consumed;
                out.files_read += usize::from(read.consumed > 0);
                out.deferred += usize::from(read.deferred);
                if let Some(reason) = control.stop_reason() {
                    out.stopped = Some(reason);
                    return Ok(out);
                }
                if let Some(error) = read.error {
                    out.unreadable.push(format!("{}: {error}", t.path));
                }
                links.extend(read.links);
                if original.as_ref() != Some(&read.checkpoint)
                    || offsets.get(&t.path) != Some(&read.checkpoint.to)
                {
                    changed.push((
                        t.path.clone(),
                        read.checkpoint.to,
                        serde_json::to_string(&read.checkpoint)?,
                    ));
                }
            }
            Err(e) => out
                .unreadable
                .push(format!("{}: could not read it: {e}", t.path)),
        }
    }
    let mut removed = Vec::new();
    if !scan.is_partial() {
        let listed: std::collections::HashSet<&str> =
            scan.sessions.iter().map(|t| t.path.as_str()).collect();
        removed.extend(offsets.keys().filter(|p| !listed.contains(p.as_str())));
    }

    // The links and the cursor that says they were read, in ONE
    // transaction: a cursor committed without its links would skip them
    // for good.
    observe("before_publish");
    if let Some(reason) = control.stop_reason() {
        out.stopped = Some(reason);
        return Ok(out);
    }
    #[cfg(feature = "enterprise-harness")]
    let tx = crate::enterprise_harness::resources::transaction(
        conn,
        rusqlite::TransactionBehavior::Immediate,
        "corpus-links",
    )?;
    #[cfg(not(feature = "enterprise-harness"))]
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if let Some(reason) = stopped(&tx, control, generation)? {
        out.stopped = Some(reason);
        return Ok(out);
    }
    for l in &links {
        out.links_added += tx.execute(
            "INSERT OR IGNORE INTO claude_session_pr
                 (session_id, repo, number, url, first_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                l.session_id,
                l.repo,
                l.number as i64,
                l.url,
                l.first_seen_at
            ],
        )?;
    }
    for (path, offset, payload) in changed {
        storage::put(&tx, &path, offset, Some(&payload))?;
    }
    for path in removed {
        tx.execute("DELETE FROM claude_link_checkpoint WHERE path=?1", [path])?;
    }
    observe("before_commit");
    if let Some(reason) = stopped(&tx, control, generation)? {
        out.links_added = 0;
        out.stopped = Some(reason);
        return Ok(out);
    }
    tx.commit()?;
    Ok(out)
}

fn stopped(
    conn: &Connection,
    control: &super::transcript::PopulationControl,
    generation: i64,
) -> Result<Option<super::transcript::PopulationStatus>, crate::store::StoreError> {
    if let Some(reason) = control.stop_reason() {
        return Ok(Some(reason));
    }
    Ok(
        (!super::transcript::generation_current(conn, "links", generation)?)
            .then_some(super::transcript::PopulationStatus::Invalidated),
    )
}

#[cfg(test)]
thread_local! {
    #[allow(clippy::type_complexity)]
    static OBSERVER: std::cell::RefCell<Option<Box<dyn FnMut(&str)>>> = const { std::cell::RefCell::new(None) };
}
fn observe(_stage: &str) {
    #[cfg(test)]
    OBSERVER.with(|observer| {
        if let Some(callback) = observer.borrow_mut().as_mut() {
            callback(_stage);
        }
    });
}

// Checksums cover ALL accepted bytes. Growth checks the last block before
// seeking; a rotating bounded audit detects middle rewrites eventually. It
// finishes even while appends continue, and stops reading unchanged bodies
// once that audit is complete. Incremental ingestion still only ADDS links;
// authoritative import owns removing associations from replaced transcripts.
const BLOCK: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Source {
    len: u64,
    modified: Option<std::time::SystemTime>,
    created: Option<std::time::SystemTime>,
    // Older Windows checkpoints lacked identity. None differs from a fresh
    // handle identity, forcing replay instead of trusting timestamp collisions.
    #[serde(default)]
    handle_identity: Option<HandleIdentity>,
    #[cfg(unix)]
    identity: (u64, u64),
    #[cfg(unix)]
    changed: (i64, i64),
}
impl Source {
    fn read(file: &std::fs::File) -> std::io::Result<Self> {
        let source = Self::of(&file.metadata()?);
        #[cfg(windows)]
        {
            Ok(Self {
                handle_identity: Some(windows_handle_identity(file)?),
                ..source
            })
        }
        #[cfg(not(windows))]
        {
            Ok(source)
        }
    }

    fn of(meta: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            len: meta.len(),
            modified: meta.modified().ok(),
            created: meta.created().ok(),
            handle_identity: None,
            #[cfg(unix)]
            identity: (meta.dev(), meta.ino()),
            #[cfg(unix)]
            changed: (meta.ctime(), meta.ctime_nsec()),
        }
    }
    fn same_file(&self, other: &Self) -> bool {
        if self.handle_identity != other.handle_identity {
            return false;
        }
        #[cfg(unix)]
        if self.identity != other.identity {
            return false;
        }
        self.created == other.created
    }
}

// FILE_ID_INFO uses the volume plus all 128 file-ID bits. The older 64-bit
// GetFileInformationByHandle index is not guaranteed unique on ReFS.
#[repr(C)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct HandleIdentity {
    volume: u64,
    id: [u8; 16],
}

#[cfg(windows)]
fn windows_handle_identity(file: &std::fs::File) -> std::io::Result<HandleIdentity> {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(
            handle: *mut std::ffi::c_void,
            class: i32,
            info: *mut std::ffi::c_void,
            size: u32,
        ) -> i32;
    }
    let mut identity = HandleIdentity {
        volume: 0,
        id: [0; 16],
    };
    // SAFETY: file keeps its OS handle alive. FILE_ID_INFO is repr(C), 24 bytes;
    // the API writes at most that size through this live exclusive pointer.
    // FileIdInfo is 0x12. Failure is an unreadable source, never a weaker
    // timestamp-only fallback. No pointer or handle ownership is transferred.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            0x12,
            std::ptr::from_mut(&mut identity).cast(),
            std::mem::size_of::<HandleIdentity>() as u32,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(identity)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Checkpoint {
    to: u64,
    source: Option<Source>,
    parser: super::subagent::LinkReader,
    /// Complete source record supporting each still-unresolved create.
    #[serde(default)]
    pending_origins: BTreeMap<String, (u64, u64)>,
    #[serde(default)]
    provenance: Provenance,
    discarding_line: bool,
    line_buffer: Vec<u8>,
    hashes: Vec<String>,
    tail: Vec<u8>,
    last_block: Vec<u8>,
    // Validation of the trailing anchor may span tiny-budget ticks.
    anchor_checked: usize,
    anchor_source: Option<Source>,
    audit_next: usize,
    audit_end: usize,
    audit_buffer: Vec<u8>,
    audit_source: Option<Source>,
}
/// Source proof for carried semantic state is stronger than the eventual
/// historical audit: it MUST finish before any new bytes reach the parser.
/// One suffix from the earliest pending call/partial record, with one bounded
/// block buffer, resumes across ticks/restarts. Its intervening records matter:
/// a terminal result inserted there would invalidate the saved pending state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Provenance {
    source: Option<Source>,
    ranges: Vec<(usize, usize)>,
    range: usize,
    block: usize,
    buffer: Vec<u8>,
}
impl Provenance {
    fn complete(&self) -> bool {
        self.range == self.ranges.len()
    }
}

impl Checkpoint {
    fn prepare_provenance(&mut self, source: &Source) {
        if self.provenance.source.as_ref() == Some(source) {
            return;
        }
        let mut start = self.pending_origins.values().map(|&(start, _)| start).min();
        if !self.line_buffer.is_empty() {
            let partial_start = self.to - self.line_buffer.len() as u64;
            start = Some(start.map_or(partial_start, |start| start.min(partial_start)));
        }
        // Pending means BOTH a create was seen and no later terminal result
        // consumed it. Prove that entire suffix, including the record delimiter.
        // The trailing partial block is already checked by the anchor.
        let merged: Vec<_> = start
            .into_iter()
            .filter_map(|start| {
                let first = (start.saturating_sub(1) / BLOCK as u64) as usize;
                (first < self.hashes.len()).then_some((first, self.hashes.len()))
            })
            .collect();
        self.provenance = Provenance {
            source: Some(source.clone()),
            block: merged.first().map_or(0, |r| r.0),
            ranges: merged,
            range: 0,
            buffer: Vec::new(),
        };
    }

    fn note_pending_origins(&mut self, start: u64) {
        let parser = &self.parser;
        self.pending_origins.retain(|id, _| parser.has_pending(id));
        for id in parser.pending_ids() {
            self.pending_origins
                .entry(id.to_string())
                .or_insert((start, self.to));
        }
    }

    fn anchor(&self) -> &[u8] {
        if self.tail.is_empty() {
            &self.last_block
        } else {
            &self.tail
        }
    }
    fn accept_bytes(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let n = (BLOCK - self.tail.len()).min(bytes.len());
            self.tail.extend_from_slice(&bytes[..n]);
            bytes = &bytes[n..];
            if self.tail.len() == BLOCK {
                self.hashes.push(digest(&self.tail));
                self.last_block = std::mem::take(&mut self.tail);
            }
        }
    }
}
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

struct FileRead {
    links: Vec<super::subagent::PrLink>,
    consumed: u64,
    checkpoint: Checkpoint,
    deferred: bool,
    error: Option<String>,
}

#[cfg(test)]
fn read_from(
    path: &Path,
    session_id: &str,
    original: Checkpoint,
    budget: u64,
) -> std::io::Result<FileRead> {
    read_from_observed(path, session_id, original, budget, || {})
}

#[cfg(test)]
fn read_from_observed(
    path: &Path,
    session_id: &str,
    original: Checkpoint,
    budget: u64,
    after_read: impl FnOnce(),
) -> std::io::Result<FileRead> {
    read_from_controlled(
        path,
        session_id,
        original,
        budget,
        &super::transcript::PopulationControl::discovery(),
        after_read,
    )
}

fn read_from_controlled(
    path: &Path,
    session_id: &str,
    original: Checkpoint,
    budget: u64,
    control: &super::transcript::PopulationControl,
    after_read: impl FnOnce(),
) -> std::io::Result<FileRead> {
    let mut file = std::fs::File::open(path)?;
    let before = Source::read(&file)?;
    let mut cp = original.clone();
    let mut out = FileRead {
        links: Vec::new(),
        consumed: 0,
        checkpoint: original,
        deferred: false,
        error: None,
    };
    let attempt = (|| -> std::io::Result<Checkpoint> {
        let changed = cp.source.as_ref() != Some(&before);
        if cp
            .parser
            .pending_ids()
            .any(|id| !cp.pending_origins.contains_key(id))
            || cp.source.as_ref().is_some_and(|s| {
                !s.same_file(&before) || before.len < cp.to || (changed && before.len == s.len)
            })
        {
            cp = Checkpoint::default();
        }
        if changed && cp.audit_next == cp.audit_end {
            cp.audit_next = 0;
            cp.audit_end = cp.hashes.len();
            cp.audit_source = Some(before.clone());
        }
        if cp.anchor_source.as_ref() != Some(&before) {
            cp.anchor_checked = 0;
            cp.anchor_source = Some(before.clone());
            cp.audit_buffer.clear();
        }
        // Validate the anchor in bounded pieces, retaining progress on a stable
        // source. No offset or pending parser state is trusted before it matches.
        let anchor_len = cp.anchor().len();
        if changed && cp.anchor_checked < anchor_len {
            let n = (anchor_len - cp.anchor_checked).min(budget as usize);
            file.seek(SeekFrom::Start(
                cp.to - anchor_len as u64 + cp.anchor_checked as u64,
            ))?;
            let mut bytes = vec![0; n];
            read_exact_counted(&mut file, &mut bytes, &mut out.consumed, control)?;
            if bytes != cp.anchor()[cp.anchor_checked..cp.anchor_checked + n] {
                cp = Checkpoint::default();
            } else {
                cp.anchor_checked += n;
            }
        }
        let anchor_ready = !changed || cp.anchor_checked == cp.anchor().len();
        if anchor_ready && changed {
            cp.prepare_provenance(&before);
            while !cp.provenance.complete() && out.consumed < budget {
                let proof = &mut cp.provenance;
                let n = (BLOCK - proof.buffer.len()).min((budget - out.consumed) as usize);
                file.seek(SeekFrom::Start(
                    (proof.block * BLOCK + proof.buffer.len()) as u64,
                ))?;
                let mut bytes = vec![0; n];
                read_exact_counted(&mut file, &mut bytes, &mut out.consumed, control)?;
                proof.buffer.extend(bytes);
                if proof.buffer.len() == BLOCK {
                    if digest(&proof.buffer) != cp.hashes[proof.block] {
                        cp = Checkpoint::default();
                        break;
                    }
                    proof.buffer.clear();
                    proof.block += 1;
                    if proof.block == proof.ranges[proof.range].1 {
                        proof.range += 1;
                        if !proof.complete() {
                            proof.block = proof.ranges[proof.range].0;
                        }
                    }
                }
            }
        }
        let parser_ready = anchor_ready && (!changed || cp.provenance.complete());
        if parser_ready && cp.audit_next < cp.audit_end && out.consumed < budget {
            let n = (BLOCK - cp.audit_buffer.len()).min((budget - out.consumed) as usize);
            file.seek(SeekFrom::Start(
                (cp.audit_next * BLOCK + cp.audit_buffer.len()) as u64,
            ))?;
            let mut bytes = vec![0; n];
            read_exact_counted(&mut file, &mut bytes, &mut out.consumed, control)?;
            cp.audit_buffer.extend(bytes);
            if cp.audit_buffer.len() == BLOCK {
                if digest(&cp.audit_buffer) != cp.hashes[cp.audit_next] {
                    cp = Checkpoint::default();
                } else {
                    cp.audit_next += 1;
                    cp.audit_buffer.clear();
                }
            }
        }
        if parser_ready && out.consumed < budget && cp.to < before.len {
            file.seek(SeekFrom::Start(cp.to))?;
            let limit = (before.len - cp.to).min(budget - out.consumed);
            let already_read = out.consumed;
            let mut reader = std::io::BufReader::new(CountedRead {
                inner: (&mut file).take(limit),
                consumed: 0,
                control,
            });
            let mut buf = Vec::new();
            loop {
                buf.clear();
                let result = reader.read_until(b'\n', &mut buf);
                out.consumed = already_read + reader.get_ref().consumed;
                let n = result? as u64;
                if n == 0 {
                    break;
                }
                let complete = buf.last() == Some(&b'\n');
                cp.accept_bytes(&buf);
                cp.to += n;
                if cp.discarding_line {
                    cp.discarding_line = !complete;
                    continue;
                }
                if cp.line_buffer.len().saturating_add(buf.len()) > BYTES_PER_PASS as usize {
                    cp.line_buffer.clear();
                    cp.discarding_line = !complete;
                    continue;
                }
                cp.line_buffer.extend_from_slice(&buf);
                if !complete {
                    break;
                }
                if let Ok(line) = std::str::from_utf8(&cp.line_buffer) {
                    out.links.extend(
                        cp.parser
                            .line_links(line.trim_end_matches(['\n', '\r']), session_id),
                    );
                }
                cp.note_pending_origins(cp.to - cp.line_buffer.len() as u64);
                cp.line_buffer.clear();
                if cp.parser.overflowed() {
                    return Err(std::io::Error::other("pending PR-create checkpoint exceeds 256 IDs or a 512-byte ID; retained previous checkpoint"));
                }
            }
        }
        after_read();
        let after = Source::read(&file)?;
        // Re-open the path: metadata alone cannot distinguish a same-size
        // Windows rename with colliding creation/modification timestamps.
        let path_after = Source::read(&std::fs::File::open(path)?)?;
        if before != after || before != path_after {
            return Err(std::io::Error::other(
                "source changed while reading; retained previous checkpoint",
            ));
        }
        // A cycle started before later appends must finish, then audit again:
        // restarting it on each append would starve its later blocks.
        if cp.audit_next == cp.audit_end && cp.audit_source.as_ref().is_some_and(|s| s != &before) {
            cp.audit_next = 0;
            cp.audit_end = cp.hashes.len();
            cp.audit_source = Some(before.clone());
        }
        out.deferred = !parser_ready || cp.to < before.len || cp.audit_next < cp.audit_end;
        // Do not bless new metadata before ALL carried parser state is proved.
        if parser_ready {
            cp.source = Some(before);
        }
        Ok(cp)
    })();
    match attempt {
        Ok(cp) => out.checkpoint = cp,
        Err(error) => {
            out.links.clear();
            out.error = Some(error.to_string());
            out.deferred = true;
        }
    }
    Ok(out)
}

struct CountedRead<'a, R> {
    inner: R,
    consumed: u64,
    control: &'a super::transcript::PopulationControl,
}
impl<R: Read> Read for CountedRead<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        observe("read");
        if self.control.stopped() {
            return Err(std::io::Error::other("link population stopped"));
        }
        let limit = buffer.len().min(64 * 1024);
        let n = self.inner.read(&mut buffer[..limit])?;
        #[cfg(feature = "enterprise-harness")]
        crate::enterprise_harness::resources::read_bytes("links", n);
        self.consumed += n as u64;
        Ok(n)
    }
}
fn read_exact_counted(
    file: &mut std::fs::File,
    buffer: &mut [u8],
    consumed: &mut u64,
    control: &super::transcript::PopulationControl,
) -> std::io::Result<()> {
    let mut reader = CountedRead {
        inner: file,
        consumed: 0,
        control,
    };
    let result = reader.read_exact(buffer);
    *consumed += reader.consumed;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn checkpoint_rows_do_not_rewrite_unchanged_sources() {
        let (dir, project, mut conn) = fixture();
        append(&project.join("s1.jsonl"), &[filler()]);
        append(&project.join("s2.jsonl"), &[filler()]);
        reset(&conn, 0).unwrap();
        let scan = crate::claude::corpus(&dir.path().join("projects"));
        refresh(&mut conn, &scan, BYTES_PER_PASS).unwrap();
        conn.execute_batch("CREATE TEMP TABLE changed(path TEXT); CREATE TEMP TRIGGER checkpoint_changes AFTER UPDATE ON claude_link_checkpoint BEGIN INSERT INTO changed VALUES(new.path); END;").unwrap();
        let warm = refresh(&mut conn, &scan, BYTES_PER_PASS).unwrap();
        assert_eq!(warm.bytes_read, 0);
        assert_eq!(
            conn.query_row("SELECT count(*) FROM changed", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        append(&project.join("s1.jsonl"), &[filler()]);
        refresh(&mut conn, &scan, BYTES_PER_PASS).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM changed", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn partial_listing_preserves_checkpoint_and_malformed_cursor_is_an_error() {
        let (dir, project, mut conn) = fixture();
        append(&project.join("s1.jsonl"), &[create_line("pending")]);
        reset(&conn, 0).unwrap();
        refresh(
            &mut conn,
            &crate::claude::corpus(&dir.path().join("projects")),
            BYTES_PER_PASS,
        )
        .unwrap();
        let before = checkpoint_cursor(&conn).unwrap();
        let partial = super::super::Scan {
            unreadable_dirs: vec!["injected discovery failure".into()],
            ..Default::default()
        };
        refresh(&mut conn, &partial, BYTES_PER_PASS).unwrap();
        assert_eq!(checkpoint_cursor(&conn).unwrap(), before);
        drop(conn);
        let mut conn = crate::store::open_db(&dir.path().join("t.db")).unwrap();
        append(&project.join("s1.jsonl"), &[result_line("pending", false)]);
        refresh(
            &mut conn,
            &crate::claude::corpus(&dir.path().join("projects")),
            BYTES_PER_PASS,
        )
        .unwrap();
        assert_eq!(linked(&conn, 77), ["s1"]);
        // Only a complete listing may remove an absent source's continuation.
        refresh(&mut conn, &super::super::Scan::default(), BYTES_PER_PASS).unwrap();
        assert!(checkpoint_cursor(&conn)
            .unwrap()
            .unwrap()
            .checkpoints
            .is_empty());
        settings::set(
            &conn,
            settings::keys::CLAUDE_PR_LINK_CURSOR,
            &"not a cursor",
        )
        .unwrap();
        assert!(refresh(&mut conn, &partial, BYTES_PER_PASS).is_err());
    }

    #[test]
    fn stopped_refresh_retains_cursor_and_links() {
        for timed_out in [false, true] {
            let (dir, project, mut conn) = fixture();
            append(
                &project.join("s1.jsonl"),
                &[link_line("s1", "acme/api", 77)],
            );
            reset(&conn, 0).unwrap();
            let before = checkpoint_cursor(&conn).unwrap();
            let control = super::super::transcript::PopulationControl::discovery();
            let control = if timed_out {
                control.with_budget(std::time::Duration::ZERO)
            } else {
                control.cancel();
                control
            };
            let result = refresh_controlled(
                &mut conn,
                &crate::claude::corpus(&dir.path().join("projects")),
                BYTES_PER_PASS,
                &control,
            )
            .unwrap();
            assert!(result.stopped.is_some());
            assert_eq!(result.bytes_read, 0);
            assert!(linked(&conn, 77).is_empty());
            assert_eq!(checkpoint_cursor(&conn).unwrap(), before);
        }
    }

    struct ObserveGuard;
    impl Drop for ObserveGuard {
        fn drop(&mut self) {
            OBSERVER.with(|observer| *observer.borrow_mut() = None);
        }
    }
    fn observing(callback: impl FnMut(&str) + 'static) -> ObserveGuard {
        OBSERVER.with(|observer| *observer.borrow_mut() = Some(Box::new(callback)));
        ObserveGuard
    }

    #[test]
    fn cancellation_at_read_or_publication_rolls_back_staged_links() {
        for stage in ["read", "before_publish", "before_commit"] {
            let (dir, project, mut conn) = fixture();
            let path = project.join("s1.jsonl");
            append(&path, &[link_line("s1", "acme/api", 77)]);
            // No newline: cancellation must be checked within read_until too.
            std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(&vec![b'x'; 256 * 1024])
                .unwrap();
            reset(&conn, 0).unwrap();
            let before = checkpoint_cursor(&conn).unwrap();
            let control = super::super::transcript::PopulationControl::discovery();
            let cancel = control.clone();
            let mut reads = 0;
            let observer = observing(move |event| {
                if event == "read" {
                    reads += 1;
                }
                if event == stage && (stage != "read" || reads == 2) {
                    cancel.cancel();
                }
            });
            let result = refresh_controlled(
                &mut conn,
                &crate::claude::corpus(&dir.path().join("projects")),
                BYTES_PER_PASS,
                &control,
            )
            .unwrap();
            drop(observer);
            assert_eq!(
                result.stopped,
                Some(super::super::transcript::PopulationStatus::Cancelled)
            );
            assert_eq!(result.links_added, 0);
            assert!(result.unreadable.is_empty());
            if stage == "read" {
                assert!(result.bytes_read > 0 && result.bytes_read <= 64 * 1024);
            }
            assert!(linked(&conn, 77).is_empty());
            assert_eq!(checkpoint_cursor(&conn).unwrap(), before);
            refresh(
                &mut conn,
                &crate::claude::corpus(&dir.path().join("projects")),
                BYTES_PER_PASS,
            )
            .unwrap();
            assert_eq!(linked(&conn, 77), ["s1"]);
        }
    }

    #[test]
    fn newer_generation_or_graph_reset_wins_over_inflight_reader() {
        for reset_graph in [false, true] {
            let (dir, project, mut conn) = fixture();
            append(
                &project.join("s1.jsonl"),
                &[link_line("s1", "acme/api", 77)],
            );
            reset(&conn, 0).unwrap();
            let mut writer = crate::store::open_db(&dir.path().join("t.db")).unwrap();
            writer.busy_timeout(std::time::Duration::ZERO).unwrap();
            let mut once = false;
            let observer = observing(move |stage| {
                if stage == "read" && !once {
                    once = true;
                    let tx = writer
                        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                        .unwrap();
                    if reset_graph {
                        reset(&tx, 12345).unwrap();
                    } else {
                        super::super::transcript::population_generation(&tx, "links").unwrap();
                    }
                    tx.commit().unwrap();
                }
            });
            let result = refresh_controlled(
                &mut conn,
                &crate::claude::corpus(&dir.path().join("projects")),
                BYTES_PER_PASS,
                &super::super::transcript::PopulationControl::discovery(),
            )
            .unwrap();
            drop(observer);
            assert_eq!(
                result.stopped,
                Some(super::super::transcript::PopulationStatus::Invalidated)
            );
            assert!(linked(&conn, 77).is_empty());
            let cursor = checkpoint_cursor(&conn).unwrap().unwrap();
            assert!(cursor.offsets.is_empty());
            assert_eq!(cursor.since_ms, if reset_graph { 12345 } else { 0 });
        }
    }

    #[test]
    fn interrupted_proof_and_audit_retain_checkpoint_across_restart() {
        for pending in [false, true] {
            let (dir, project, mut conn) = fixture();
            let root = dir.path().join("projects");
            let path = project.join("s1.jsonl");
            if pending {
                append(&path, &[create_line("pending")]);
            }
            append(&path, &[" ".repeat(32 * 1024)]);
            reset(&conn, 0).unwrap();
            refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
            let before = checkpoint_cursor(&conn).unwrap();
            append(
                &path,
                &[if pending {
                    result_line("pending", false)
                } else {
                    link_line("s1", "acme/api", 77)
                }],
            );
            let control = super::super::transcript::PopulationControl::discovery();
            let cancel = control.clone();
            let mut reads = 0;
            let observer = observing(move |stage| {
                if stage == "read" {
                    reads += 1;
                    if reads == 2 {
                        cancel.cancel();
                    }
                }
            });
            let result = refresh_controlled(
                &mut conn,
                &crate::claude::corpus(&root),
                BYTES_PER_PASS,
                &control,
            )
            .unwrap();
            drop(observer);
            assert_eq!(
                result.stopped,
                Some(super::super::transcript::PopulationStatus::Cancelled)
            );
            assert!(result.bytes_read > 0);
            assert_eq!(checkpoint_cursor(&conn).unwrap(), before);
            assert!(linked(&conn, 77).is_empty());
            drop(conn);
            let mut conn = crate::store::open_db(&dir.path().join("t.db")).unwrap();
            refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
            assert_eq!(linked(&conn, 77), ["s1"]);
        }
    }

    #[test]
    fn cursor_missing_malformed_and_database_failure_are_distinct() {
        let (_dir, _project, mut conn) = fixture();
        let scan = super::super::Scan::default();
        assert!(
            refresh(&mut conn, &scan, BYTES_PER_PASS)
                .unwrap()
                .not_started
        );
        settings::set(
            &conn,
            settings::keys::CLAUDE_PR_LINK_CURSOR,
            &"malformed cursor",
        )
        .unwrap();
        assert!(matches!(
            refresh(&mut conn, &scan, BYTES_PER_PASS),
            Err(crate::store::StoreError::Json(_))
        ));
        conn.execute_batch("DROP TABLE settings").unwrap();
        assert!(matches!(
            refresh(&mut conn, &scan, BYTES_PER_PASS),
            Err(crate::store::StoreError::Db(_))
        ));
    }

    fn create_line(id: &str) -> String {
        serde_json::json!({"message":{"content":[{"type":"tool_use","id":id,"name":"Bash","input":{"command":"gh pr create --fill"}}]}}).to_string()
    }

    fn result_line(id: &str, error: bool) -> String {
        serde_json::json!({"message":{"content":[{"type":"tool_result","tool_use_id":id,"is_error":error,"content":"https://github.com/acme/api/pull/77"}]}}).to_string()
    }

    #[test]
    fn pairing_survives_tick_restart_and_budget_boundary() {
        for boundary in [false, true] {
            let (dir, project, mut conn) = fixture();
            let root = dir.path().join("projects");
            let path = project.join("s1.jsonl");
            let call = create_line("create-1");
            append(&path, std::slice::from_ref(&call));
            if boundary {
                append(&path, &[result_line("create-1", false)]);
            }
            reset(&conn, 0).unwrap();
            refresh(
                &mut conn,
                &crate::claude::corpus(&root),
                call.len() as u64 + 1,
            )
            .unwrap();
            drop(conn);
            let mut conn = crate::store::open_db(&dir.path().join("t.db")).unwrap();
            if !boundary {
                append(&path, &[result_line("create-1", false)]);
            }
            refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
            assert_eq!(linked(&conn, 77), ["s1"], "boundary={boundary}");
            assert_eq!(
                refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS)
                    .unwrap()
                    .links_added,
                0
            );
        }
    }

    #[test]
    fn replacement_replays_same_size_larger_and_truncate_regrow() {
        for mode in 0..3 {
            let (dir, project, mut conn) = fixture();
            let root = dir.path().join("projects");
            let path = project.join("s1.jsonl");
            append(&path, &[link_line("s1", "acme/api", 11)]);
            reset(&conn, 0).unwrap();
            refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
            let mut replacement = format!("{}\n", link_line("s1", "acme/api", 22));
            if mode > 0 {
                replacement.push_str(&format!("{}\n", filler()));
            }
            if mode == 1 {
                let other = project.join("replacement");
                std::fs::write(&other, replacement).unwrap();
                std::fs::rename(other, &path).unwrap();
            } else {
                std::fs::write(&path, replacement).unwrap();
            }
            refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
            assert_eq!(linked(&conn, 22), ["s1"], "mode={mode}");
        }
    }

    #[test]
    fn invalid_results_cannot_reuse_a_pending_create_after_restart() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let path = project.join("s1.jsonl");
        append(
            &path,
            &[
                create_line("failed"),
                create_line("empty"),
                create_line("valid"),
            ],
        );
        reset(&conn, 0).unwrap();
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        drop(conn);
        let mut conn = crate::store::open_db(&dir.path().join("t.db")).unwrap();
        append(
            &path,
            &[
                result_line("unrelated", false),
                result_line("failed", true),
                result_line("empty", false)
                    .replace("https://github.com/acme/api/pull/77", "no URL"),
                "{torn tool_result".into(),
            ],
        );
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert!(linked(&conn, 77).is_empty());
        append(
            &path,
            &[result_line("failed", false), result_line("empty", false)],
        );
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert!(
            linked(&conn, 77).is_empty(),
            "terminal error/no-URL results consume their IDs"
        );
        append(&path, &[result_line("valid", false)]);
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert_eq!(linked(&conn, 77), ["s1"]);
    }

    #[test]
    fn failed_checkpoint_commit_rolls_back_links_and_parser_together() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let path = project.join("s1.jsonl");
        append(&path, &[create_line("create-1")]);
        reset(&conn, 0).unwrap();
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        let before = checkpoint_cursor(&conn).unwrap();
        append(&path, &[result_line("create-1", false)]);
        conn.execute_batch("CREATE TRIGGER fail_checkpoint BEFORE UPDATE ON claude_link_checkpoint BEGIN SELECT RAISE(ABORT, 'injected checkpoint failure'); END;").unwrap();
        assert!(refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).is_err());
        assert!(linked(&conn, 77).is_empty());
        assert_eq!(checkpoint_cursor(&conn).unwrap(), before);
        conn.execute_batch("DROP TRIGGER fail_checkpoint;").unwrap();
        drop(conn);
        let mut conn = crate::store::open_db(&dir.path().join("t.db")).unwrap();
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert_eq!(linked(&conn, 77), ["s1"]);
    }

    #[test]
    fn legacy_offsets_replay_to_reconstruct_pending_pairing() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let path = project.join("s1.jsonl");
        append(&path, &[create_line("old"), result_line("old", false)]);
        let offsets = BTreeMap::from([(
            path.to_string_lossy().to_string(),
            std::fs::metadata(&path).unwrap().len(),
        )]);
        settings::set(
            &conn,
            settings::keys::CLAUDE_PR_LINK_CURSOR,
            &serde_json::json!({"since_ms":i64::MAX,"offsets":offsets}),
        )
        .unwrap();
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert_eq!(linked(&conn, 77), ["s1"]);
    }

    #[test]
    fn replacement_drops_pending_create_from_previous_source() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let path = project.join("s1.jsonl");
        append(&path, &[create_line("old")]);
        reset(&conn, 0).unwrap();
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        std::fs::write(
            &path,
            format!("{}\n{}\n", filler(), result_line("old", false)),
        )
        .unwrap();
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert!(linked(&conn, 77).is_empty());
    }

    #[test]
    fn middle_rewrite_with_preserved_tail_and_growth_is_eventually_replayed() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let path = project.join("s1.jsonl");
        let padding = format!("{}\n", filler()).repeat(40);
        let original = format!("{padding}{}\n{padding}", link_line("s1", "acme/api", 11));
        std::fs::write(&path, &original).unwrap();
        reset(&conn, 0).unwrap();
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        let replacement = original.replace(
            &link_line("s1", "acme/api", 11),
            &link_line("s1", "acme/api", 22),
        );
        std::fs::write(&path, format!("{replacement}{}\n", filler())).unwrap();
        for _ in 0..100 {
            let got = refresh(&mut conn, &crate::claude::corpus(&root), 300).unwrap();
            assert!(got.bytes_read <= 300);
            drop(conn);
            conn = crate::store::open_db(&dir.path().join("t.db")).unwrap();
            if !linked(&conn, 22).is_empty() {
                break;
            }
        }
        assert_eq!(
            linked(&conn, 22),
            ["s1"],
            "middle rewrite must not evade anchor checks forever"
        );
        for _ in 0..100 {
            if refresh(&mut conn, &crate::claude::corpus(&root), 1024)
                .unwrap()
                .deferred
                == 0
            {
                break;
            }
        }
        assert_eq!(
            refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS)
                .unwrap()
                .bytes_read,
            0
        );
    }

    #[test]
    fn ordinary_append_reads_bounded_evidence_then_warm_ticks_read_nothing() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let path = project.join("s1.jsonl");
        append(&path, &vec![filler(); 1000]);
        reset(&conn, 0).unwrap();
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        let link = link_line("s1", "acme/api", 88);
        append(&path, std::slice::from_ref(&link));
        let got = refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert_eq!(linked(&conn, 88), ["s1"]);
        assert!(got.bytes_read <= 2 * BLOCK as u64 + link.len() as u64 + 1);
        for _ in 0..100 {
            if refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS)
                .unwrap()
                .deferred
                == 0
            {
                break;
            }
        }
        assert_eq!(
            refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS)
                .unwrap()
                .bytes_read,
            0
        );
    }

    #[test]
    fn tiny_budgets_retain_partial_records_and_anchor_validation() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let path = project.join("s1.jsonl");
        append(&path, &[create_line("tiny")]);
        reset(&conn, 0).unwrap();
        for _ in 0..200 {
            refresh(&mut conn, &crate::claude::corpus(&root), 1).unwrap();
        }
        append(&path, &[result_line("tiny", false)]);
        for _ in 0..500 {
            let got = refresh(&mut conn, &crate::claude::corpus(&root), 1).unwrap();
            assert!(got.bytes_read <= 1);
            if !linked(&conn, 77).is_empty() {
                break;
            }
        }
        assert_eq!(linked(&conn, 77), ["s1"]);
    }

    #[test]
    fn pending_overflow_is_explicit_and_retains_previous_checkpoint() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let path = project.join("s1.jsonl");
        append(
            &path,
            &(0..257)
                .map(|i| create_line(&format!("id-{i}")))
                .collect::<Vec<_>>(),
        );
        reset(&conn, 0).unwrap();
        let got = refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert_eq!(got.unreadable.len(), 1);
        assert!(got.unreadable[0].contains("256 IDs"));
        assert_eq!(got.deferred, 1);
        let cursor = checkpoint_cursor(&conn).unwrap().unwrap();
        assert_eq!(cursor.offsets.get(path.to_str().unwrap()), Some(&0));
    }

    #[test]
    fn oversized_line_suffix_is_not_parsed_as_a_new_record() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let path = project.join("s1.jsonl");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(&vec![b'x'; BYTES_PER_PASS as usize + 1])
            .unwrap();
        writeln!(file, "{}", link_line("s1", "acme/api", 55)).unwrap();
        writeln!(file, "{}", link_line("s1", "acme/api", 66)).unwrap();
        reset(&conn, 0).unwrap();
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS + 1).unwrap();
        drop(conn);
        let mut conn = crate::store::open_db(&dir.path().join("t.db")).unwrap();
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert!(linked(&conn, 55).is_empty());
        assert_eq!(linked(&conn, 66), ["s1"]);
    }

    #[test]
    fn concurrent_append_or_rename_retains_old_checkpoint_and_retries() {
        for rename in [false, true] {
            let (_dir, project, _conn) = fixture();
            let path = project.join("s1.jsonl");
            append(&path, &[link_line("s1", "acme/api", 11)]);
            let original = Checkpoint::default();
            let read = read_from_observed(&path, "s1", original.clone(), BYTES_PER_PASS, || {
                if rename {
                    let replacement = project.join("replacement");
                    append(&replacement, &[link_line("s1", "acme/api", 22)]);
                    std::fs::rename(replacement, &path).unwrap();
                } else {
                    append(&path, &[link_line("s1", "acme/api", 22)]);
                }
            })
            .unwrap();
            assert!(
                read.error
                    .as_deref()
                    .is_some_and(|error| error.contains("source changed")),
                "source mutation must reject the read: rename={rename}, error={:?}",
                read.error
            );
            assert_eq!(read.checkpoint, original);
            assert!(read.links.is_empty());
            assert!(read.consumed > 0 && read.consumed <= BYTES_PER_PASS);
            let retry = read_from(&path, "s1", read.checkpoint, BYTES_PER_PASS).unwrap();
            assert!(retry.links.iter().any(|l| l.number == 22));
        }
    }

    #[test]
    fn every_successful_result_in_one_record_is_linked_by_live_and_full_scan() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let path = project.join("s1.jsonl");
        append(&path, &[create_line("first"), create_line("second")]);
        reset(&conn, 0).unwrap();
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        let first: serde_json::Value = serde_json::from_str(&result_line("first", false)).unwrap();
        let second: serde_json::Value =
            serde_json::from_str(&result_line("second", false).replace("pull/77", "pull/78"))
                .unwrap();
        append(&path, &[serde_json::json!({"message":{"content":[first["message"]["content"][0], second["message"]["content"][0]]}}).to_string()]);
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert_eq!(linked(&conn, 77), ["s1"]);
        assert_eq!(linked(&conn, 78), ["s1"]);
        let map = super::super::subagent::build(&[path]);
        assert_eq!(map.pr_links.len(), 2);
    }

    #[test]
    fn pending_create_cannot_pair_across_a_middle_rewrite() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let path = project.join("s1.jsonl");
        let padding = format!("{}\n", filler()).repeat(40);
        let original = format!("{padding}{}\n{padding}", create_line("reused"));
        std::fs::write(&path, &original).unwrap();
        reset(&conn, 0).unwrap();
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        let rewritten = original.replace("gh pr create", "gh pr status");
        std::fs::write(
            &path,
            format!("{rewritten}{}\n", result_line("reused", false)),
        )
        .unwrap();
        for _ in 0..100 {
            let got = refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
            assert!(
                linked(&conn, 77).is_empty(),
                "no source version proves this pairing"
            );
            if got.deferred == 0 {
                break;
            }
        }
    }

    #[test]
    fn partial_record_cannot_combine_bytes_across_a_middle_rewrite() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let path = project.join("s1.jsonl");
        let padding = format!("{}\n", filler()).repeat(40);
        let partial = format!(
            r#"{{"type":"pr-link","prNumber":77,"prUrl":"u","prRepository":"acme/api","padding":"{}"#,
            "x".repeat(BLOCK * 3)
        );
        let original = format!("{padding}{partial}");
        std::fs::write(&path, &original).unwrap();
        reset(&conn, 0).unwrap();
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        let rewritten = original.replace("pr-link", "no-link");
        std::fs::write(&path, format!("{rewritten}\"}}\n")).unwrap();
        for _ in 0..100 {
            let got = refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
            assert!(
                linked(&conn, 77).is_empty(),
                "the completed source contains no PR-link record"
            );
            if got.deferred == 0 {
                break;
            }
        }
    }

    #[test]
    fn pending_origin_proof_resumes_with_small_budgets_and_restart() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let path = project.join("s1.jsonl");
        let padding = format!("{}\n", filler()).repeat(40);
        std::fs::write(
            &path,
            format!("{padding}{}\n{padding}", create_line("stable")),
        )
        .unwrap();
        reset(&conn, 0).unwrap();
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        append(&path, &[result_line("stable", false)]);
        let cursor = checkpoint_cursor(&conn).unwrap().unwrap();
        let checkpoint = cursor
            .checkpoints
            .get(path.to_str().unwrap())
            .unwrap()
            .clone();
        let ordinary = read_from(&path, "s1", checkpoint, BYTES_PER_PASS).unwrap();
        assert!(ordinary.links.iter().any(|l| l.number == 77));
        assert!(
            ordinary.consumed < 4 * BLOCK as u64,
            "proof must skip history before the earliest pending record"
        );
        for _ in 0..200 {
            let got = refresh(&mut conn, &crate::claude::corpus(&root), 257).unwrap();
            assert!(got.bytes_read <= 257);
            drop(conn);
            conn = crate::store::open_db(&dir.path().join("t.db")).unwrap();
            if !linked(&conn, 77).is_empty() {
                break;
            }
        }
        assert_eq!(linked(&conn, 77), ["s1"]);
    }

    #[test]
    fn resolved_then_reused_id_gets_the_new_create_origin() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let path = project.join("s1.jsonl");
        let padding = format!("{}\n", filler()).repeat(40);
        let before_last = format!(
            "{}\n{}\n{padding}",
            create_line("reuse"),
            result_line("reuse", true)
        );
        let original = format!("{before_last}{}\n{padding}", create_line("reuse"));
        std::fs::write(&path, original).unwrap();
        reset(&conn, 0).unwrap();
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        let rewritten_last = create_line("reuse").replace("gh pr create", "gh pr status");
        std::fs::write(
            &path,
            format!(
                "{before_last}{rewritten_last}\n{padding}{}\n",
                result_line("reuse", false)
            ),
        )
        .unwrap();
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert!(linked(&conn, 77).is_empty());
    }

    #[test]
    fn inserted_terminal_result_invalidates_old_pending_state() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let path = project.join("s1.jsonl");
        let padding = format!("{}\n", filler()).repeat(40);
        let slot = filler();
        let prefix = format!("{padding}{}\n{padding}", create_line("old"));
        std::fs::write(&path, format!("{prefix}{slot}\n{padding}")).unwrap();
        reset(&conn, 0).unwrap();
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        let terminal = result_line("old", true);
        assert!(terminal.len() <= slot.len());
        let replacement_slot = format!("{terminal}{}", " ".repeat(slot.len() - terminal.len()));
        std::fs::write(
            &path,
            format!(
                "{prefix}{replacement_slot}\n{padding}{}\n",
                result_line("old", false)
            ),
        )
        .unwrap();
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert!(
            linked(&conn, 77).is_empty(),
            "replacement's terminal result already consumed this ID"
        );
    }

    #[test]
    fn rewritten_record_delimiter_invalidates_carried_parser_state() {
        for partial in [false, true] {
            let (dir, project, mut conn) = fixture();
            let root = dir.path().join("projects");
            let path = project.join("s1.jsonl");
            let prefix = format!("{}\n", "x".repeat(BLOCK * 2 - 1));
            let (record, continuation) = if partial {
                (
                    format!(
                        r#"{{"type":"pr-link","prNumber":77,"prUrl":"u","prRepository":"acme/api","padding":"{}"#,
                        "x".repeat(BLOCK * 3)
                    ),
                    "\"}\n".to_string(),
                )
            } else {
                (
                    format!(
                        "{}\n{}",
                        create_line("old"),
                        format!("{}\n", filler()).repeat(40)
                    ),
                    format!("{}\n", result_line("old", false)),
                )
            };
            std::fs::write(&path, format!("{prefix}{record}")).unwrap();
            reset(&conn, 0).unwrap();
            refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
            let rewritten_prefix = "x".repeat(prefix.len());
            std::fs::write(&path, format!("{rewritten_prefix}{record}{continuation}")).unwrap();
            refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
            assert!(
                linked(&conn, 77).is_empty(),
                "record's preceding delimiter changed, partial={partial}"
            );
        }
    }

    #[test]
    fn equal_metadata_does_not_make_distinct_handle_identities_the_same_source() {
        let (_dir, project, _conn) = fixture();
        let path = project.join("s1.jsonl");
        append(&path, &[filler()]);
        let mut a = serde_json::to_value(Source::of(&std::fs::metadata(&path).unwrap())).unwrap();
        let mut b = a.clone();
        a["handle_identity"] =
            serde_json::json!({"volume":7,"id":[1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]});
        b["handle_identity"] =
            serde_json::json!({"volume":7,"id":[2,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]});
        let a: Source = serde_json::from_value(a).unwrap();
        let b: Source = serde_json::from_value(b).unwrap();
        assert!(
            !a.same_file(&b),
            "timestamps and length cannot identify a file object"
        );
        assert_ne!(
            a, b,
            "post-read acceptance must detect a different path handle"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_handles_distinguish_files_when_other_source_fields_collide() {
        let (_dir, project, _conn) = fixture();
        let first_path = project.join("first.jsonl");
        let second_path = project.join("second.jsonl");
        std::fs::write(&first_path, b"identical bytes").unwrap();
        std::fs::write(&second_path, b"identical bytes").unwrap();
        let first = std::fs::File::open(&first_path).unwrap();
        let second = std::fs::File::open(&second_path).unwrap();
        let a = Source::read(&first).unwrap();
        let mut b = Source::read(&second).unwrap();
        assert!(a.handle_identity.is_some());
        assert_ne!(
            a.handle_identity, b.handle_identity,
            "actual kernel file identities must differ"
        );
        // Force a deterministic timestamp collision while retaining real OS
        // identities. The concurrent-rename test exercises final-path reopening.
        b.created = a.created;
        b.modified = a.modified;
        assert_eq!(a.len, b.len);
        assert!(!a.same_file(&b));
        assert_ne!(a, b);
        let mut legacy_json = serde_json::to_value(&a).unwrap();
        legacy_json
            .as_object_mut()
            .unwrap()
            .remove("handle_identity");
        let legacy: Source = serde_json::from_value(legacy_json).unwrap();
        assert!(!legacy.same_file(&a), "legacy Windows cursor must replay");
        assert_eq!(std::mem::size_of::<HandleIdentity>(), 24);
    }

    fn link_line(session: &str, repo: &str, n: u64) -> String {
        format!(
            r#"{{"type":"pr-link","sessionId":"{session}","prNumber":{n},"prUrl":"https://example.invalid/{repo}/pull/{n}","prRepository":"{repo}","timestamp":"2026-09-11T12:00:00.000Z"}}"#
        )
    }

    fn filler() -> String {
        format!(r#"{{"type":"user","text":"{}"}}"#, "x".repeat(200))
    }

    fn append(path: &Path, lines: &[String]) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    /// A corpus root with one project directory, and a database.
    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("projects").join("-work-example");
        std::fs::create_dir_all(&project).unwrap();
        let conn = crate::store::open_db(&dir.path().join("t.db")).unwrap();
        (dir, project, conn)
    }

    fn linked(conn: &Connection, n: u64) -> Vec<String> {
        crate::claude::store::sessions_for_pr_number(conn, n)
            .unwrap()
            .into_iter()
            .map(|l| l.session_id)
            .collect()
    }

    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    /// A PR opened after the import is found by the next pass, without
    /// another import (#1557).
    #[test]
    fn a_link_written_after_the_import_is_found_by_the_next_pass() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let t = project.join("s1.jsonl");
        append(&t, &[filler(), link_line("s1", "acme/api", 7)]);

        // The import, as `claude_import_transcripts` runs it: the scan,
        // the rewrite, then the cursor reset.
        let since = now_ms();
        let scan = crate::claude::scan(&root);
        crate::claude::store::import(&mut conn, scan).unwrap();
        reset(&conn, since).unwrap();
        assert_eq!(linked(&conn, 7), ["s1"]);

        // The session opens PR 8 after the import.
        append(&t, &[filler(), link_line("s1", "acme/api", 8)]);
        assert!(
            linked(&conn, 8).is_empty(),
            "stale until something reads it"
        );

        let got = refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert_eq!(linked(&conn, 8), ["s1"], "the stale link is found");
        assert_eq!(got.links_added, 1, "PR 7 was already there");
        assert_eq!(got.deferred, 0);

        // A second pass with nothing new reads nothing.
        let again = refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert_eq!(again.bytes_read, 0, "the cursor holds between passes");
        assert_eq!(again.links_added, 0);
    }

    /// A file the import read whole, and untouched since, is not read.
    #[test]
    fn a_file_older_than_the_import_is_not_read_again() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let t = project.join("old.jsonl");
        append(&t, &[link_line("old", "acme/api", 3)]);
        let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&t)
            .unwrap()
            .set_modified(hour_ago)
            .unwrap();

        reset(&conn, now_ms()).unwrap();
        let got = refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert_eq!(got.bytes_read, 0);
        assert!(linked(&conn, 3).is_empty(), "the import owns that file");
    }

    /// Before any import, a pass does nothing -- and says so.
    #[test]
    fn no_pass_runs_before_the_first_import() {
        let (dir, project, mut conn) = fixture();
        append(&project.join("s1.jsonl"), &[link_line("s1", "acme/api", 7)]);
        let got = refresh(
            &mut conn,
            &crate::claude::corpus(&dir.path().join("projects")),
            BYTES_PER_PASS,
        )
        .unwrap();
        assert!(got.not_started);
        assert!(linked(&conn, 7).is_empty());
    }

    /// Partial is not nothing: a pass that runs out of budget keeps the
    /// links it read and resumes where it stopped.
    #[test]
    fn a_pass_out_of_budget_keeps_what_it_found_and_resumes() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let t = project.join("s1.jsonl");
        let first = link_line("s1", "acme/api", 1);
        let second = link_line("s1", "acme/api", 2);
        append(&t, &[first.clone(), filler(), filler(), second]);
        reset(&conn, now_ms() - 60_000).unwrap();

        // Room for the first record and part of the filler, not the
        // second record.
        let budget = first.len() as u64 + 50;
        let got = refresh(&mut conn, &crate::claude::corpus(&root), budget).unwrap();
        assert_eq!(linked(&conn, 1), ["s1"], "what it read is kept");
        assert!(linked(&conn, 2).is_empty());
        assert_eq!(got.deferred, 1, "the unread rest is counted, not dropped");
        assert_eq!(got.bytes_read, budget);

        // The cursor stopped at the last complete line, so nothing is
        // half-read: the next passes finish the file.
        let mut guard = 0;
        while linked(&conn, 2).is_empty() {
            refresh(&mut conn, &crate::claude::corpus(&root), budget * 4).unwrap();
            guard += 1;
            assert!(guard < 10, "the passes must make progress");
        }
        assert_eq!(linked(&conn, 1), ["s1"]);
    }

    /// A record still being written is not consumed half-way.
    #[test]
    fn an_unfinished_line_is_left_for_the_next_pass() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let t = project.join("s1.jsonl");
        let line = link_line("s1", "acme/api", 9);
        let (head, tail) = line.split_at(40);
        std::fs::write(&t, head).unwrap();
        reset(&conn, now_ms() - 60_000).unwrap();

        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert!(linked(&conn, 9).is_empty());

        append(&t, &[tail.to_string()]);
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert_eq!(linked(&conn, 9), ["s1"], "read whole once it was complete");
    }

    /// The pass adds; it never overwrites the import's earlier timestamp.
    #[test]
    fn an_existing_link_keeps_its_first_seen_time() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        conn.execute(
            "INSERT INTO claude_session_pr (session_id, repo, number, url, first_seen_at)
             VALUES ('s1', 'acme/api', 7, 'u', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        append(&project.join("s1.jsonl"), &[link_line("s1", "acme/api", 7)]);
        reset(&conn, now_ms() - 60_000).unwrap();
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        let got = crate::claude::store::sessions_for_pr_number(&conn, 7).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].first_seen_at.as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
    }
}

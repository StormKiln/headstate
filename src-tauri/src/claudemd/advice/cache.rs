//! A repository's advice [`Report`], persisted and served from the
//! store, recomputed only when a tracked input has changed (#1293).
//!
//! # What this is NOT
//!
//! `claude_advice_ledger` and `claude_advice_signal` (migration 24) are
//! the TRANSCRIPTS producer's internal cache: they let that one pass
//! skip a session whose transcript has not moved. They hold no report.
//! Nothing before this module ever stored a [`Report`], so every open of
//! the panel re-ran all eight producers, including the whole-body read
//! of every session under the repository that #1246 put on its own
//! `spawn_blocking`. This module stores the assembled report and the
//! fingerprint of what it was computed from.
//!
//! # ONE blob, not one entry per check
//!
//! Weighed, and rejected, per #1293's ask. Per-check entries would only
//! pay if the checks' inputs were mostly disjoint. They are not:
//!
//! - All eight producers read `Context::scan`, the effective CLAUDE.md
//!   walk. Seven of the eight answer a question ABOUT those files, so an
//!   edit to any CLAUDE.md invalidates seven of eight entries -- and the
//!   walk itself, the shared cost, has to be redone for any one of them.
//! - The genuinely separable producer is `transcripts`, and it ALREADY
//!   has a per-session incremental cache of its own one layer down. A
//!   per-check entry for it would cache the assembly of rows it can
//!   already re-assemble without re-reading a transcript.
//! - [`Report::brief`] is rendered over the whole finding set by
//!   `brief::render_report`, and [`Report::checks`] is derived from
//!   `Check::ALL`. Serving a mix of eight per-check entries means
//!   re-deriving both on every read, so the "cheap" path is not cheap and
//!   gains a way to assemble a report no run ever produced.
//!
//! The cost of the choice is stated rather than hidden: a change that
//! touches one producer's input alone still recomputes all eight. On the
//! shared walk that is most of the work regardless.
//!
//! # What is tracked, and what is knowingly not
//!
//! [`Fingerprint::of`] hashes, in a fixed order:
//!
//! 1. **Every CLAUDE.md the effective scan found** -- repo, global and
//!    local scope -- by absolute path and SHA256 of its BYTES. Content,
//!    not `mtime`: the input set is bounded (the scan already holds the
//!    list) and `mtime` lies in both directions here. A `git checkout`
//!    rewrites timestamps on files whose content did not change, which
//!    would recompute for nothing; clock skew and a same-second write can
//!    hide a change, which is the direction that serves stale advice as
//!    current. The files are small -- this page exists to say how small.
//! 2. **Every scope the scan could not read**, verbatim. An unreadable
//!    CLAUDE.md that becomes readable is a change.
//! 3. **Every definition in the inventory** (skills, agents, commands
//!    from the user root, the repository's `.claude` and installed
//!    plugins) by path and SHA256, plus every `ScopeRefusal`. This is
//!    what makes "a new skill leaves stale advice showing" not happen:
//!    #1293 offered dropping it as an acceptable trade, and it is not
//!    taken, because the skills and rot producers read the inventory and
//!    a skill added or removed silently changes their answer.
//! 4. **Every session under the repository**, by `session_id`,
//!    `size_bytes` and `mtime_ms` -- NOT content.
//!
//! Point 4 is the one deliberate weakening, and it is deliberate because
//! hashing session bodies is precisely the whole-body read this cache
//! exists to avoid; hashing to verify would cost what recomputing costs.
//! `(size, mtime)` is the same change key `claude_advice_ledger` and
//! `claude_index_ledger` already use for these same files, and they are
//! append-mostly logs written by another process, the case where a size
//! change is the reliable signal. An edit that leaves a transcript
//! byte-identical in length AND timestamp is missed. Stated, not hidden.
//!
//! # Unverifiable is not current (#846, #1042)
//!
//! If any tracked file's bytes or metadata cannot be read, or the
//! session rows cannot be queried, [`Fingerprint::of`] does NOT quietly
//! omit it and hash the rest -- that would produce a digest that happens
//! to match the stored one and report the report current. It returns
//! [`Verification::Unverified`] with the reason, and a cached report
//! served under it says so. A caller may still show the cached report --
//! a previous run is a real answer -- but it must never claim currency it
//! could not establish.
//!
//! # A cache entry that will not decode is not an error
//!
//! [`Report`]'s shape changes across releases. A row whose JSON no
//! longer deserializes -- a renamed field, a new `Check` variant, a
//! truncated write -- is treated exactly as a miss: recompute, then
//! overwrite. Never a crash, never a half-decoded report, and never a
//! row left behind to fail the same way on every open.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

use super::{AdviceResult, Context, Freshness, Mode, Report};
use crate::claude::definitions::Inventory;
use crate::claudemd::EffectiveScan;

/// The schema of the cached payload, bumped when [`Report`]'s meaning
/// changes in a way a successful `serde` decode would not catch.
///
/// A decode failure already covers a shape change. This covers the
/// nastier case: a report that still decodes but no longer MEANS the
/// same thing, because a producer's rule changed. A row at a different
/// version is a miss, the same as a decode failure.
///
/// 2: the transcripts producer re-roots every linked worktree and no
/// longer counts a read of a file the session edits (#1324). A report
/// stored before it fingerprints identically -- no tracked input moved --
/// and would otherwise be served as current with the old findings.
pub const PAYLOAD_VERSION: i64 = 2;

/// Whether a fingerprint is a statement about every tracked input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verification {
    /// Every tracked input was read. The digest is a complete statement:
    /// equal digests mean equal inputs (modulo the session `(size,
    /// mtime)` caveat in the module docs).
    Complete,
    /// At least one tracked input could not be read, in its own words.
    /// The digest covers what DID read and is therefore not a claim
    /// about currency in either direction.
    Unverified { reason: String },
}

/// A digest of every tracked input, and whether it covers all of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    /// Lowercase hex SHA256 over the ordered input record.
    pub digest: String,
    pub verification: Verification,
}

impl Fingerprint {
    /// Hash the tracked inputs for one run.
    ///
    /// Infallible by construction: a read that fails becomes an
    /// `Unverified` reason rather than an `Err`, because the caller's
    /// answer to "could not fingerprint" is never "refuse" -- it is
    /// "recompute, and do not claim the result is verified".
    ///
    /// `conn` is `None` for a store-less run (tests, a bare checkout).
    /// That is NOT unverified: with no store there are no session rows
    /// to track, and the transcripts producer will report itself Unknown
    /// for the same reason. It is a complete statement about an input
    /// set that does not include sessions. What WOULD be unverified is a
    /// store that is present and whose rows will not read.
    pub fn of(
        repo: &Path,
        home: Option<&Path>,
        scan: &EffectiveScan,
        definitions: Option<&Inventory>,
        conn: Option<&Connection>,
    ) -> Fingerprint {
        let mut h = Sha256::new();
        let mut refusals: Vec<String> = Vec::new();

        // Domain-separated and length-prefixed: without it, two
        // different input sets could serialise to the same byte stream
        // and a change would hash equal. `feed` writes the length before
        // the bytes for the same reason.
        feed(&mut h, b"headstate/claudemd/advice/cache/v1");

        // 1 + 2: the CLAUDE.md files, and the scopes that would not read.
        feed(&mut h, b"scan");
        let mut files: Vec<&str> = scan
            .repo
            .files
            .iter()
            .map(|f| f.path.as_str())
            .chain(scan.extra.iter().map(|s| s.file.path.as_str()))
            .collect();
        // Sorted, so a walk that returns a directory's entries in a
        // different order does not present as a changed input.
        files.sort_unstable();
        files.dedup();
        for path in files {
            feed(&mut h, path.as_bytes());
            match content_digest(Path::new(path)) {
                Ok(d) => feed(&mut h, d.as_bytes()),
                Err(reason) => {
                    // Fed as a distinct marker rather than skipped: a
                    // file that goes from readable to unreadable IS a
                    // change, and the reason below records that this
                    // digest is not a complete statement.
                    feed(&mut h, b"<unreadable>");
                    refusals.push(reason);
                }
            }
        }
        // 2: what the scan PROVED exists and could not read.
        //
        // These are refusals, not merely more bytes to hash. An
        // unreadable directory hides an unknown number of CLAUDE.md
        // files, and an unreadable CLAUDE.md hides its content -- in
        // both cases the digest below describes a smaller input set than
        // the producers were asked about, so it cannot prove currency.
        // Hashing these strings and calling the result Complete is
        // exactly the "quietly omit it and hash the rest" failure the
        // module docs rule out: the digest would match on the next open
        // and report a report computed without them as current.
        //
        // Hashed AS WELL, so a wall that comes down is a change.
        feed(&mut h, b"scan-unreadable");
        let mut walls = scan.repo.unreadable_dirs.clone();
        walls.extend(scan.repo.unreadable_files.iter().cloned());
        walls.sort_unstable();
        for u in &walls {
            feed(&mut h, u.as_bytes());
            refusals.push(u.clone());
        }

        // `EffectiveScan::unreadable` is the one list that mixes two
        // different facts, and they must not be treated alike (#1050:
        // never report "we did not ask" as "they did not answer"). With
        // a home directory its entries are real read failures of the
        // global or local scope, and they are refusals. With NO home,
        // `scan_effective_opt` puts one stated-absence entry here --
        // "no home directory is set, so the global scope could not be
        // read" -- which is not a failed read at all; it is a complete
        // statement about an input set that has no global scope in it,
        // and the producers already treat it that way. Counting it as a
        // refusal would make every store-less and home-less run
        // permanently unverified, which would make the third state mean
        // nothing.
        feed(&mut h, b"scan-scopes");
        let mut scopes = scan.unreadable.clone();
        scopes.sort_unstable();
        for u in &scopes {
            feed(&mut h, u.as_bytes());
            if home.is_some() {
                refusals.push(u.clone());
            }
        }

        // 3: the definitions inventory.
        //
        // `None` is fed as its own marker and is NOT a refusal: the
        // store-less callers pass `None` deliberately, and the producers
        // that need it report themselves Unknown. A run with an
        // inventory and a run without are different input sets, which is
        // what the marker keeps apart.
        feed(&mut h, b"definitions");
        match definitions {
            None => feed(&mut h, b"<none>"),
            Some(inv) => {
                let mut paths: Vec<&str> =
                    inv.definitions.iter().map(|d| d.path.as_str()).collect();
                paths.sort_unstable();
                paths.dedup();
                for path in paths {
                    feed(&mut h, path.as_bytes());
                    match content_digest(Path::new(path)) {
                        Ok(d) => feed(&mut h, d.as_bytes()),
                        Err(reason) => {
                            feed(&mut h, b"<unreadable>");
                            refusals.push(reason);
                        }
                    }
                }
                // A `ScopeRefusal` is a scope that exists and would not
                // list, hiding an unknown number of definitions. Same
                // reasoning as the scan's unreadable lists above: fed to
                // the digest so it becoming readable is a change, AND
                // recorded as a refusal so the digest does not claim to
                // cover what it could not see.
                let mut refused: Vec<String> = inv
                    .unreadable
                    .iter()
                    .map(|r| format!("{:?}: {}", r.source, r.detail))
                    .collect();
                refused.sort_unstable();
                for r in &refused {
                    feed(&mut h, r.as_bytes());
                    refusals.push(r.clone());
                }
            }
        }

        // 4: the sessions, by `(size, mtime)`. See the module docs for
        // why this one input is not content-hashed.
        feed(&mut h, b"sessions");
        match conn {
            None => feed(&mut h, b"<no-store>"),
            Some(conn) => match session_keys(conn, repo) {
                Ok(keys) => {
                    for (id, size, mtime) in &keys {
                        feed(&mut h, id.as_bytes());
                        feed(&mut h, &size.to_le_bytes());
                        feed(&mut h, &mtime.to_le_bytes());
                    }
                }
                Err(reason) => {
                    feed(&mut h, b"<unreadable>");
                    refusals.push(reason);
                }
            },
        }

        let digest = hex(h.finalize().as_slice());
        let verification = match refusals.first() {
            None => Verification::Complete,
            Some(first) => Verification::Unverified {
                // The first reason verbatim, and a count for the rest:
                // one sentence a reader can act on beats a wall, and the
                // count stops "1 input" being read as "the only one".
                reason: if refusals.len() == 1 {
                    first.clone()
                } else {
                    format!("{first} (and {} more inputs)", refusals.len() - 1)
                },
            },
        };
        Fingerprint {
            digest,
            verification,
        }
    }
}

/// Serve a repository's report: from the store when the tracked inputs
/// still match, else by running every producer.
///
/// The one entry point the command calls, so the cache's three states
/// are decided in one place rather than assembled by each caller.
///
/// # Order of operations, and why
///
/// The fingerprint is taken BEFORE the producers run, not after. Taken
/// after, a CLAUDE.md edited while the transcript pass was reading would
/// be hashed into the digest that gets stored -- and the next open would
/// match it and serve a report computed from the file's PREVIOUS
/// contents as current. Taken first, that edit makes the stored digest
/// disagree on the next open and the report recomputes. The failure this
/// ordering chooses is one wasted recompute; the other ordering's
/// failure is stale advice presented as fresh, which is the one thing
/// #1293 exists to prevent.
///
/// `conn` being `None` -- no store, or a store that would not open -- is
/// not a rejection: the producers run and nothing is filed. A run that
/// happened is a real answer whether or not it could be cached, which is
/// the same reasoning that keeps the command `Ok` when one producer
/// fails (#1044).
pub fn serve(cx: &Context, mode: Mode, now: &str) -> AdviceResult {
    let fingerprint = Fingerprint::of(cx.repo, cx.home, cx.scan, cx.definitions, cx.conn);

    if mode == Mode::Cached {
        // A load failure is a miss, never a rejection: the producers can
        // answer without the store.
        let stored = match cx.conn {
            None => None,
            Some(conn) => match load(conn, cx.repo) {
                Ok(v) => v,
                Err(e) => {
                    log::warn!("advice cache: could not read the stored report: {e}");
                    None
                }
            },
        };
        if let Some(hit) = stored {
            let freshness = match (&fingerprint.verification, &hit.stored_unverified) {
                // The report was stored under a digest that already
                // omitted an input. Matching that digest now proves
                // nothing, so the STORED reason is what a reader is
                // owed -- it names the input that was never covered.
                (_, Some(stored_reason)) => Freshness::Unverified {
                    reason: stored_reason.clone(),
                    recomputed: false,
                },
                // We could not read an input NOW. The stored digest may
                // be complete; this comparison is not.
                (Verification::Unverified { reason }, None) => Freshness::Unverified {
                    reason: reason.clone(),
                    recomputed: false,
                },
                (Verification::Complete, None) => {
                    if fingerprint.digest == hit.digest {
                        // Verified current: every tracked input read,
                        // both then and now, and they agree.
                        Freshness::Fresh { recomputed: false }
                    } else {
                        Freshness::Cached { stale: true }
                    }
                }
            };
            // The stored report is returned even when it is STALE, and
            // that is the point of the label rather than a shortcut
            // around it.
            //
            // The alternative -- recompute silently on a stale hit --
            // makes `Freshness::Cached` a variant no caller can ever
            // observe, which would mean the API does not in fact expose
            // the epic's second state. It would also put the expensive
            // run back on the path #1293 exists to take it off: a
            // repository whose CLAUDE.md was edited a second ago is
            // exactly when the panel most wants to show SOMETHING now.
            //
            // A previous run is a real answer (#1044: partial is not
            // nothing, and out-of-date is not nothing either). What the
            // user is owed is not the withholding of it -- it is being
            // told. `stale: true` is that telling, and it is what a
            // caller composes "from cache, refreshing" out of: show
            // this, fire `Mode::Fresh`, replace it when that lands.
            return AdviceResult {
                report: hit.report,
                freshness,
                computed_at: hit.computed_at,
            };
        }
    }

    let report = super::run(cx);
    if let Some(conn) = cx.conn {
        if let Err(e) = store(conn, cx.repo, &report, &fingerprint, now) {
            // Not fatal, and not silent. The report stands; the next
            // open pays for the run again.
            log::warn!("advice cache: could not store the report: {e}");
        }
    }
    let freshness = match &fingerprint.verification {
        Verification::Complete => Freshness::Fresh { recomputed: true },
        // A run that just finished and STILL cannot be called current,
        // because the fingerprint that will be compared against it omits
        // something. Reporting this as Fresh would put the lie one open
        // later instead of removing it.
        Verification::Unverified { reason } => Freshness::Unverified {
            reason: reason.clone(),
            recomputed: true,
        },
    };
    AdviceResult {
        report,
        freshness,
        computed_at: now.to_string(),
    }
}

/// One stored report, as [`load`] returns it.
#[derive(Debug, Clone, PartialEq)]
pub struct Cached {
    pub report: Report,
    /// The fingerprint of the inputs the report was computed from.
    pub digest: String,
    /// Whether that fingerprint covered every tracked input. A report
    /// stored under an unverified fingerprint is unverified FOREVER, not
    /// only on the open that stored it: the digest it was compared
    /// against was never a complete statement.
    pub stored_unverified: Option<String>,
    /// RFC 3339, when the producers ran.
    pub computed_at: String,
}

/// Read this repository's stored report, if one decodes.
///
/// `Ok(None)` for every kind of miss, including a row that will not
/// decode: the caller's action is identical -- recompute -- and forcing
/// it to distinguish "no row" from "a row from a previous release" would
/// only invite one of the two to be handled as an error page. A decode
/// failure is logged and the row is left to be overwritten by [`store`].
///
/// `Err` is reserved for the database itself refusing, which the caller
/// treats as a miss too but which is worth not swallowing silently.
pub fn load(conn: &Connection, repo: &Path) -> Result<Option<Cached>, String> {
    let key = repo.to_string_lossy().to_string();
    let row: Option<(i64, String, String, Option<String>, String)> = conn
        .query_row(
            "SELECT payload_version, digest, payload, unverified, computed_at
               FROM claude_advice_report WHERE repo = ?1",
            [&key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(format!("claude_advice_report: {other}")),
        })?;

    let Some((version, digest, payload, unverified, computed_at)) = row else {
        return Ok(None);
    };

    if version != PAYLOAD_VERSION {
        log::info!(
            "advice cache: stored payload version {version} is not {PAYLOAD_VERSION}; recomputing"
        );
        return Ok(None);
    }

    // The upgrade path. A `Report` whose shape changed under this row is
    // a MISS, never a rejection and never a partial decode: `serde_json`
    // gives all or nothing, and the caller recomputes.
    match serde_json::from_str::<Report>(&payload) {
        Ok(report) => Ok(Some(Cached {
            report,
            digest,
            stored_unverified: unverified,
            computed_at,
        })),
        Err(e) => {
            log::info!("advice cache: stored report did not decode ({e}); recomputing");
            Ok(None)
        }
    }
}

/// Write this repository's report and the fingerprint it was computed
/// from, replacing whatever was there.
///
/// A failure is returned rather than swallowed so the caller can log it,
/// but the caller must not turn it into a rejection: a report that was
/// computed is a real answer whether or not it could be filed.
pub fn store(
    conn: &Connection,
    repo: &Path,
    report: &Report,
    fingerprint: &Fingerprint,
    computed_at: &str,
) -> Result<(), String> {
    let payload = serde_json::to_string(report).map_err(|e| format!("advice cache: {e}"))?;
    let unverified = match &fingerprint.verification {
        Verification::Complete => None,
        Verification::Unverified { reason } => Some(reason.clone()),
    };
    conn.execute(
        "INSERT INTO claude_advice_report
            (repo, payload_version, digest, payload, unverified, computed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(repo) DO UPDATE SET
            payload_version = ?2, digest = ?3, payload = ?4,
            unverified = ?5, computed_at = ?6",
        rusqlite::params![
            repo.to_string_lossy().to_string(),
            PAYLOAD_VERSION,
            fingerprint.digest,
            payload,
            unverified,
            computed_at,
        ],
    )
    .map_err(|e| format!("claude_advice_report: {e}"))?;
    Ok(())
}

/// `(session_id, size_bytes, mtime_ms)` for every session recorded under
/// `repo`, in a stable order.
///
/// The same `claude_session` rows and the same worktree re-rooting
/// (`transcripts::reroot_cwd`, one resolver per fingerprint) the
/// transcripts producer selects by, so the fingerprint covers
/// exactly the set that producer will read. A session row with no
/// transcript path contributes its id alone: it is still an input (the
/// producer reports it unreadable) and it can gain a path later.
fn session_keys(conn: &Connection, repo: &Path) -> Result<Vec<(String, i64, i64)>, String> {
    let mut q = conn
        .prepare(
            "SELECT session_id, cwd, transcript_path
               FROM claude_session
              WHERE cwd IS NOT NULL
              ORDER BY session_id",
        )
        .map_err(|e| format!("claude_session: {e}"))?;
    let rows = q
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|e| format!("claude_session: {e}"))?;

    let mut worktrees = super::transcripts::Worktrees::new(repo);
    let mut out = Vec::new();
    for row in rows {
        let (session_id, cwd, transcript_path) = row.map_err(|e| format!("claude_session: {e}"))?;
        if !super::transcripts::reroot_cwd(&cwd, &mut worktrees).starts_with(repo) {
            continue;
        }
        // A transcript we cannot stat is NOT a refusal that makes the
        // whole fingerprint unverified: the transcripts producer records
        // exactly that per session and still answers. It is fed as a
        // sentinel so the file appearing later reads as a change.
        let (size, mtime) = transcript_path
            .as_deref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| {
                let mtime = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                (m.len() as i64, mtime)
            })
            .unwrap_or((-1, -1));
        out.push((session_id, size, mtime));
    }
    Ok(out)
}

/// SHA256 of a file's bytes, as lowercase hex.
///
/// Bytes, not text: a CLAUDE.md the scan could not decode as UTF-8 still
/// has a content identity, and a fingerprint that skipped it would call
/// an edit to it no change at all.
fn content_digest(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(hex(Sha256::digest(&bytes).as_slice()))
}

/// Length-prefixed feed, so `["ab", "c"]` and `["a", "bc"]` differ.
fn feed(h: &mut Sha256, bytes: &[u8]) {
    h.update((bytes.len() as u64).to_le_bytes());
    h.update(bytes);
}

/// Hex by fold, matching `claude::preview` and `remote::identity`.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[cfg(test)]
mod tests;

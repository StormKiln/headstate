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
//! 0. **The build that will compute the report** ([`Build::id`], #1333):
//!    the version the release workflow stamps from the tag, plus the
//!    running executable's `(size, mtime)`. The producers ARE an input:
//!    in 7.3.0 five PRs changed what a producer reports and one bumped
//!    [`PAYLOAD_VERSION`]. A report stored by another build is a miss, not
//!    a stale hit, and the build that computed a report is filed with it
//!    and shown beside `computed_at`. No git SHA is baked in at build
//!    time; the executable's stat is what makes a `0.1.0` dev build of
//!    changed code a different build, and it catches uncommitted changes
//!    a SHA would not. An executable that cannot be stat'ed is a refusal.
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
//! 5. **The repository tree, as git sees it** (#1334): `git rev-parse
//!    HEAD`, the bytes of `git status --porcelain=v1 -z
//!    --untracked-files=all`, and the bytes of every file that status
//!    lists. Rot resolves paths against the tree and reads `.gitignore`
//!    (#1318); toolchain and rot read `package.json` and `Makefile`;
//!    placement probes the tree. HEAD covers every tracked file as
//!    committed; the status covers every tracked file that differs from
//!    it and every untracked file git does not ignore. The listed files'
//!    BYTES are hashed as well, because a status line does not move on a
//!    second edit: ` M Makefile` reads the same after a target is added
//!    to an already-modified Makefile. `all` rather than `normal` for the
//!    same reason: `normal` collapses an untracked directory to one
//!    `?? dir/` line, so a second file in it would be no change. A nested
//!    repository (a linked worktree kept under the checkout) stays one
//!    `?? dir/` entry: git does not descend into it, and its files are
//!    another repository's. `--no-optional-locks`, so opening the panel
//!    never takes `index.lock` from the user's own git.
//! 6. **`.claude/rules/`**, by relative path and bytes of every entry
//!    (#1321: placement probes it). Hashed directly, not only through
//!    git, because a repository that ignores `.claude/` hides it from
//!    point 5. Absent is a statement, not a refusal.
//!
//! What is knowingly NOT tracked, beyond point 4's weakening:
//!
//! - **An ignored file changing.** A local `.env` edited, or an ignored
//!   build output appearing, does not invalidate. Git does not list it,
//!   and hashing every ignored file would mean walking `node_modules`. A
//!   change to the ignore RULES does count: `.gitignore` is a tracked or
//!   untracked file in point 5, and `.git/info/exclude` or a global
//!   excludes file reaches the digest through what status then lists.
//! - **A dirty file over 8 MiB** is keyed on `(size, mtime)`, not bytes
//!   ([`DIRTY_CONTENT_CAP`]), so one large untracked artifact cannot make
//!   every open read it whole. The producers' inputs are source and
//!   manifests, far below it.
//!
//! If git cannot answer -- not installed, not a repository, a timeout, an
//! exit other than an unborn HEAD's, or a status that answered and
//! complained on stderr -- the fingerprint is [`Verification::Unverified`]
//! with git's words. Never an empty tree hashed as complete.
//!
//! # What it costs
//!
//! Measured on this repository (832 tracked files, clean tree, macOS,
//! release build, five runs): **29.6 to 31.1 ms** per fingerprint, of
//! which the two git calls are all but under 1 ms; the CLAUDE.md hashing
//! and the `.claude/rules` walk are microseconds. Points 5 and 6 did not exist
//! before, so this is what every panel open now pays before it can say
//! "current". It scales with the tree git has to stat and with the bytes
//! of the dirty files, not with the number of sessions.
//!
//! Point 4 is the one deliberate weakening among the inputs a producer
//! reads directly, and it is deliberate because hashing session bodies is precisely the whole-body read this cache
//! exists to avoid; hashing to verify would cost what recomputing costs.
//! `(size, mtime)` is the same change key `claude_advice_ledger` and
//! `claude_index_ledger` already use for these same files, and they are
//! append-mostly logs written by another process, the case where a size
//! change is the reliable signal. An edit that leaves a transcript
//! byte-identical in length AND timestamp is missed. Stated, not hidden.
//!
//! # Unverifiable is not current (#846, #1042)
//!
//! If any tracked file's bytes or metadata cannot be read, the session
//! rows cannot be queried, git cannot answer, or the running executable
//! cannot be identified, [`Fingerprint::of`] does NOT quietly
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
/// A decode failure already covers a shape change. This covers a report
/// that still decodes but no longer MEANS the same thing. Since #1333 a
/// producer's rule change needs no bump: it ships in a new build, and a
/// row from another [`Build`] is already a miss. A bump is still the
/// tool for a meaning change within one build's lifetime of stored rows
/// that the build identity cannot see. A row at a different version is a
/// miss, the same as a decode failure.
///
/// 2: the transcripts producer re-roots every linked worktree and no
/// longer counts a read of a file the session edits (#1324). A report
/// stored before it fingerprints identically -- no tracked input moved --
/// and would otherwise be served as current with the old findings.
///
/// 3: the transcripts producer's coverage counts, census and "already
/// written" hits are `Severity::Note`, not `Advice` (#1339). A stored
/// report still decodes, and would render those rows as advice.
pub const PAYLOAD_VERSION: i64 = 3;

/// The build of Headstate a report was computed by (#1333).
///
/// Two fields because they answer two questions. `label` is what a
/// reader is shown: the version, which the release workflow stamps into
/// `Cargo.toml` from the tag, so `CARGO_PKG_VERSION` is the release's
/// own number in a release build. `id` is what the cache compares, and
/// it is stricter than the label: the label plus the running
/// executable's `(size, mtime)`, so a dev build -- which reads `0.1.0`
/// forever -- rebuilt from changed producer code is a different build
/// too. No git SHA is baked in at build time and none is added: the
/// executable's own stat answers "is this the same code" for committed
/// and uncommitted changes alike, with no build-script mechanism.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    /// Shown beside `computed_at`. `7.4.0`, or `0.1.0-debug` for a debug
    /// build so a developer's report is never read as a release's.
    pub label: String,
    /// Compared: a stored row whose `id` differs is a miss.
    pub id: String,
    /// Why the executable could not be identified, when it could not.
    /// The fingerprint is then Unverified: two builds with the same
    /// version and an unknown executable cannot be told apart.
    pub refusal: Option<String>,
}

impl Build {
    /// The running build, identified once per process.
    ///
    /// Once, not per call: an updater that replaces the executable while
    /// this process still runs would otherwise make the OLD code store
    /// its report under the NEW build's identity, and the new build
    /// would then serve it as its own. `lib.rs` touches this at startup
    /// so the stat is taken before any update can land.
    pub fn current() -> Build {
        static CURRENT: std::sync::OnceLock<Build> = std::sync::OnceLock::new();
        CURRENT.get_or_init(Build::identify).clone()
    }

    fn identify() -> Build {
        let mut label = env!("CARGO_PKG_VERSION").to_string();
        if cfg!(debug_assertions) {
            label.push_str("-debug");
        }
        let stat = std::env::current_exe()
            .and_then(std::fs::metadata)
            .map(|m| {
                let mtime = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis())
                    .unwrap_or(0);
                format!("{}.{mtime}", m.len())
            });
        match stat {
            Ok(stat) => Build {
                id: format!("{label}+{stat}"),
                label,
                refusal: None,
            },
            Err(e) => Build {
                id: format!("{label}+<unknown>"),
                label,
                refusal: Some(format!("the running executable could not be read: {e}")),
            },
        }
    }
}

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
    /// The build the digest was taken under, filed beside the report.
    pub build: Build,
}

impl Fingerprint {
    /// [`Fingerprint::of_with`] for the running build and the resolved
    /// git binary.
    pub fn of(
        repo: &Path,
        home: Option<&Path>,
        scan: &EffectiveScan,
        definitions: Option<&Inventory>,
        conn: Option<&Connection>,
    ) -> Fingerprint {
        Fingerprint::of_with(
            &Build::current(),
            crate::auth::git_program(),
            repo,
            home,
            scan,
            definitions,
            conn,
        )
    }

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
    ///
    /// `build` and `git` are parameters so a test can compare two builds
    /// in one process and prove what a git that cannot run produces.
    pub fn of_with(
        build: &Build,
        git: &Path,
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
        // the bytes for the same reason. v2: the build and the git state
        // joined the record (#1333, #1334).
        feed(&mut h, b"headstate/claudemd/advice/cache/v2");

        // 0: the build that will compute the report.
        feed(&mut h, b"build");
        feed(&mut h, build.id.as_bytes());
        if let Some(reason) = &build.refusal {
            refusals.push(reason.clone());
        }

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

        // 5: the repository tree, as git sees it. See the module docs.
        feed(&mut h, b"git");
        git_state(git, repo, &mut h, &mut refusals);

        // 6: `.claude/rules/`, by its own listing and bytes.
        feed(&mut h, b"rules");
        rules_state(&repo.join(".claude").join("rules"), &mut h, &mut refusals);

        // One input can be refused twice -- an unreadable CLAUDE.md that
        // git also lists as untracked -- and `content_digest` words both
        // refusals identically. Counted once, so "and N more" is a count
        // of inputs, not of attempts.
        let mut seen = std::collections::HashSet::new();
        refusals.retain(|r| seen.insert(r.clone()));

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
            build: build.clone(),
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
    serve_with(cx, mode, now, &Build::current(), crate::auth::git_program())
}

/// [`serve`] for a given build and git binary. See [`Fingerprint::of_with`].
pub fn serve_with(cx: &Context, mode: Mode, now: &str, build: &Build, git: &Path) -> AdviceResult {
    let fingerprint = Fingerprint::of_with(
        build,
        git,
        cx.repo,
        cx.home,
        cx.scan,
        cx.definitions,
        cx.conn,
    );

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
        // A report another build computed is a MISS, not a stale hit
        // (#1333). Stale means "the right rules over old inputs"; this is
        // other rules, which is what `PAYLOAD_VERSION` has always made a
        // miss. The digest differs too, since the build is hashed, but
        // serving it labelled stale would still show the previous
        // release's findings while this one's run.
        let stored = stored.filter(|hit| {
            let same = hit.build_id == build.id;
            if !same {
                log::info!(
                    "advice cache: stored report is from build {}, this is {}; recomputing",
                    hit.build,
                    build.label
                );
            }
            same
        });
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
                build: hit.build,
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
        build: build.label.clone(),
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
    /// [`Build::label`] of the build that computed it, for display.
    pub build: String,
    /// [`Build::id`] of that build, compared against the running one.
    pub build_id: String,
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
    type Row = (
        i64,
        String,
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
    );
    let row: Option<Row> = conn
        .query_row(
            "SELECT payload_version, digest, payload, unverified, computed_at, build, build_id
               FROM claude_advice_report WHERE repo = ?1",
            [&key],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(format!("claude_advice_report: {other}")),
        })?;

    let Some((version, digest, payload, unverified, computed_at, build, build_id)) = row else {
        return Ok(None);
    };

    if version != PAYLOAD_VERSION {
        log::info!(
            "advice cache: stored payload version {version} is not {PAYLOAD_VERSION}; recomputing"
        );
        return Ok(None);
    }

    // A row filed before builds were recorded (migration 27) names no
    // build. A miss, not a report attributed to whoever reads it.
    let (Some(build), Some(build_id)) = (build, build_id) else {
        log::info!("advice cache: stored report records no build; recomputing");
        return Ok(None);
    };

    // The upgrade path. A `Report` whose shape changed under this row is
    // a MISS, never a rejection and never a partial decode: `serde_json`
    // gives all or nothing, and the caller recomputes.
    match serde_json::from_str::<Report>(&payload) {
        Ok(report) => Ok(Some(Cached {
            report,
            digest,
            stored_unverified: unverified,
            computed_at,
            build,
            build_id,
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
            (repo, payload_version, digest, payload, unverified, computed_at, build, build_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(repo) DO UPDATE SET
            payload_version = ?2, digest = ?3, payload = ?4,
            unverified = ?5, computed_at = ?6, build = ?7, build_id = ?8",
        rusqlite::params![
            repo.to_string_lossy().to_string(),
            PAYLOAD_VERSION,
            fingerprint.digest,
            payload,
            unverified,
            computed_at,
            fingerprint.build.label,
            fingerprint.build.id,
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

/// The largest dirty file hashed by content. Above it a file is keyed on
/// `(size, mtime)`, the same weakening the sessions take, so one large
/// untracked artifact cannot make every panel open read it whole.
const DIRTY_CONTENT_CAP: u64 = 8 * 1024 * 1024;

/// Point 5: `HEAD`, the porcelain status, and the bytes of every file the
/// status lists. See the module docs for what this covers and what not.
///
/// Every way git can fail to answer is a refusal, never an empty tree: a
/// git that did not run did not say "nothing changed" (#1050).
fn git_state(git: &Path, repo: &Path, h: &mut Sha256, refusals: &mut Vec<String>) {
    use crate::worktrees::scan::git_output_with;

    // `-q --verify` so an unborn HEAD -- a repository with no commits --
    // is exit 1 and silence, which is a statement, not a failure.
    match git_output_with(git, repo, &["rev-parse", "-q", "--verify", "HEAD"]) {
        Err(e) => {
            feed(h, b"<no-head>");
            refusals.push(format!("git rev-parse HEAD could not run: {e}"));
        }
        Ok(out) => match out.status.code() {
            Some(0) => feed(h, String::from_utf8_lossy(&out.stdout).trim().as_bytes()),
            Some(1) if out.stderr.is_empty() => feed(h, b"<unborn>"),
            code => {
                feed(h, b"<no-head>");
                refusals.push(git_refusal("git rev-parse HEAD", code, &out.stderr));
            }
        },
    }

    // `--untracked-files=all`, not `normal`: `normal` collapses an
    // untracked directory to one `?? dir/` line, so a second file added
    // under it reads as no change. `--no-optional-locks` so a panel open
    // never takes `index.lock` out from under the user's own git.
    let out = match git_output_with(
        git,
        repo,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ],
    ) {
        Ok(out) => out,
        Err(e) => {
            feed(h, b"<no-status>");
            refusals.push(format!("git status could not run: {e}"));
            return;
        }
    };
    if !out.status.success() {
        feed(h, b"<no-status>");
        refusals.push(git_refusal("git status", out.status.code(), &out.stderr));
        return;
    }
    // A status that answered AND complained -- "could not open
    // directory" -- listed less than the tree holds. Not complete.
    if !out.stderr.is_empty() {
        refusals.push(git_refusal("git status", Some(0), &out.stderr));
    }
    feed(h, &out.stdout);

    // The status line of a modified file reads the same after a second
    // edit, so every listed file's bytes are hashed as well.
    let mut fields = out.stdout.split(|b| *b == 0).filter(|f| !f.is_empty());
    while let Some(entry) = fields.next() {
        // `XY path`. A rename or copy is followed by its source path as
        // its own field, which is not a file in the tree now.
        let (Some(xy), Some(rel)) = (entry.get(..2), entry.get(3..)) else {
            refusals.push("git status: an entry did not parse".to_string());
            continue;
        };
        if xy.iter().any(|c| matches!(c, b'R' | b'C')) {
            fields.next();
        }
        let Ok(rel) = std::str::from_utf8(rel) else {
            refusals.push(format!(
                "git status: a path is not UTF-8: {}",
                String::from_utf8_lossy(rel)
            ));
            continue;
        };
        feed(h, rel.as_bytes());
        let path = repo.join(rel);
        match tree_file_key(&path) {
            Ok(key) => feed(h, key.as_bytes()),
            Err(reason) => {
                feed(h, b"<unreadable>");
                // An untracked CLAUDE.md the scan already refused is one
                // input, not two, so "and N more" stays a count of inputs.
                if !already_refused(refusals, &path) {
                    refusals.push(reason);
                }
            }
        }
    }
}

/// Whether a refusal already names `path`. The scan words its refusals
/// `path (error)` and this module `path: error`, so the path followed by
/// either separator is the match.
fn already_refused(refusals: &[String], path: &Path) -> bool {
    let p = path.display().to_string();
    refusals.iter().any(|r| {
        r.strip_prefix(p.as_str())
            .is_some_and(|rest| rest.starts_with(':') || rest.starts_with(" ("))
    })
}

/// `git <what> exit status N: <stderr>`, the shape `rot` and `shape` use.
fn git_refusal(what: &str, code: Option<i32>, stderr: &[u8]) -> String {
    format!(
        "{what} exit status {}: {}",
        code.map(|c| c.to_string()).unwrap_or_else(|| "none".into()),
        String::from_utf8_lossy(stderr).trim()
    )
}

/// One file `git status` listed: its bytes, or what stands in for them.
fn tree_file_key(path: &Path) -> Result<String, String> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        // Deleted: the status line already says so, and absence is the
        // content.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok("<absent>".into()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if meta.file_type().is_symlink() {
        return std::fs::read_link(path)
            .map(|t| format!("<link>{}", t.to_string_lossy()))
            .map_err(|e| format!("{}: {e}", path.display()));
    }
    // A nested repository or submodule. Its own state is not this tree's;
    // git reports it changed through the status line.
    if meta.is_dir() {
        return Ok("<dir>".into());
    }
    if meta.len() > DIRTY_CONTENT_CAP {
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis())
            .unwrap_or(0);
        return Ok(format!("<large>{}.{mtime}", meta.len()));
    }
    content_digest(path)
}

/// Point 6: every entry under `.claude/rules/`, by relative path and
/// bytes. Absent is a statement; a directory that will not list is a
/// refusal. Symlinks are recorded by target and not followed, so a loop
/// cannot hang the fingerprint.
fn rules_state(dir: &Path, h: &mut Sha256, refusals: &mut Vec<String>) {
    match std::fs::metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => feed(h, b"<absent>"),
        Err(e) => {
            feed(h, b"<unreadable>");
            refusals.push(format!("{}: {e}", dir.display()));
        }
        Ok(_) => rules_walk(dir, dir, h, refusals),
    }
}

fn rules_walk(root: &Path, dir: &Path, h: &mut Sha256, refusals: &mut Vec<String>) {
    let entries = std::fs::read_dir(dir).and_then(|rd| rd.collect::<Result<Vec<_>, _>>());
    let mut entries = match entries {
        Ok(e) => e,
        Err(e) => {
            feed(h, b"<unreadable>");
            refusals.push(format!("{}: {e}", dir.display()));
            return;
        }
    };
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap_or(&path);
        feed(h, rel.to_string_lossy().as_bytes());
        match std::fs::symlink_metadata(&path) {
            Ok(m) if m.is_dir() => {
                feed(h, b"<dir>");
                rules_walk(root, &path, h, refusals);
            }
            Ok(m) if m.file_type().is_symlink() => match std::fs::read_link(&path) {
                Ok(t) => feed(h, format!("<link>{}", t.to_string_lossy()).as_bytes()),
                Err(e) => {
                    feed(h, b"<unreadable>");
                    refusals.push(format!("{}: {e}", path.display()));
                }
            },
            Ok(_) => match content_digest(&path) {
                Ok(d) => feed(h, d.as_bytes()),
                Err(reason) => {
                    feed(h, b"<unreadable>");
                    refusals.push(reason);
                }
            },
            Err(e) => {
                feed(h, b"<unreadable>");
                refusals.push(format!("{}: {e}", path.display()));
            }
        }
    }
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

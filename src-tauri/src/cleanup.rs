//! Automatic cleanup: what the app WOULD remove, recorded for review.
//!
//! Phase 1 of #382, and it deliberately cannot delete anything. There is
//! no removal path in this module.
//!
//! The reason is not caution for its own sake. Enabling this feature asks
//! the user to trust a predicate they have never seen run against their
//! own machine, and no amount of description substitutes for a list of
//! what it actually picked. Preview turns "trust this rule" into "confirm
//! this list", and it generates that list from real machines before any
//! code has the power to act on it.
//!
//! Nothing here talks to GitHub.

use crate::store::settings;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// One thing the cleanup pass considered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub at: String,
    /// `artifact` or `venv`.
    pub kind: String,
    /// The path.
    pub target: String,
    /// What it is, in the terms the view uses: an artifact's rebuild
    /// command, or a virtualenv's project.
    pub detail: Option<String>,
    pub bytes: Option<u64>,
    /// `proposed`, `removed`, `refused`, or `skipped`.
    pub action: String,
    pub error: Option<String>,
}

/// What automatic cleanup is allowed to consider, and whether it may act.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupMode {
    /// Record what WOULD be removed. The only mode Phase 1 ships.
    Preview,
    /// Reserved for Phase 2. Present in the type so the ledger's `action`
    /// column and the settings shape do not need changing later, and
    /// absent from every code path that could act on it.
    Remove,
}

/// Preferences for the automatic pass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CleanupPrefs {
    /// Master switch. OFF, because an upgrade must never start deleting
    /// -- and in Phase 1, must not start scanning on a timer either.
    #[serde(default)]
    pub enabled: bool,
    /// Preview even once enabled. Flipping to Remove is a SECOND,
    /// separate decision, made after reading a week of real proposals.
    #[serde(default = "preview")]
    pub mode: CleanupMode,
    /// Whether build artifacts are considered.
    #[serde(default)]
    pub artifacts: bool,
    /// Whether virtualenvs are considered at all.
    #[serde(default)]
    pub venvs: bool,
    /// Whether STALE virtualenvs count too, not just orphans.
    ///
    /// The distinction is the whole reason this is separate from
    /// `venvs`. An orphan is a FACT: nothing on the machine hashes to
    /// it, so the project that made it is gone and it can never be used
    /// again. Stale is a THRESHOLD -- 90 days by default -- about a
    /// project that still exists, and a threshold is a guess at intent.
    ///
    /// Manual removal does not need this: ticking a row and confirming a
    /// dialog IS the intent. An unattended pass has no such signal, so
    /// this is where the opt-in belongs and where it now lives.
    ///
    /// Defaults OFF: an upgrade must never widen what runs by itself.
    #[serde(default)]
    pub venvs_stale: bool,
    /// Whether merged BRANCHES are considered.
    ///
    /// Parent of the two below. Off by default for the same reason as
    /// everything else here: an upgrade must never widen what runs
    /// unattended.
    #[serde(default)]
    pub branches: bool,
    /// Branches merged by ANCESTRY -- the tip is reachable from the
    /// default branch. A graph fact, and the strongest claim available.
    #[serde(default)]
    pub branches_ancestor: bool,
    /// Branches merged by SQUASH, found by comparing patch-ids.
    ///
    /// Separate from `branches_ancestor` for the reason `venvs_stale`
    /// is separate from `venvs`: this is a CONTENT comparison, not a
    /// graph one. Two different changes producing an identical diff are
    /// indistinguishable to it. That is vanishingly rare and it is
    /// still a weaker claim, so an unattended pass gets its own opt-in.
    ///
    /// It is also the common case -- measured at 489 of 536 merged
    /// branches on a real repository -- so turning it off is not a
    /// small restriction, and turning it on should be deliberate.
    #[serde(default)]
    pub branches_squash: bool,
    /// Whether WORKTREES are considered.
    #[serde(default)]
    pub worktrees: bool,
    /// Worktrees whose branch is merged, clean, and pushed -- the
    /// `Safety::Safe` case. Nothing is lost by removing one.
    #[serde(default)]
    pub worktrees_safe: bool,
    /// Whether DOCKER IMAGES are considered.
    #[serde(default)]
    pub docker: bool,
    /// Dangling images -- untagged and referenced by nothing.
    #[serde(default)]
    pub docker_dangling: bool,
    /// Most entries a single run may propose.
    ///
    /// A blast radius, and it matters in Preview too: a run that proposed
    /// all 178 directories would produce a ledger nobody reads, which is
    /// the same as no ledger.
    #[serde(default)]
    pub max_per_run: u32,
}

fn preview() -> CleanupMode {
    CleanupMode::Preview
}

impl Default for CleanupPrefs {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: CleanupMode::Preview,
            artifacts: false,
            venvs: false,
            venvs_stale: false,
            branches: false,
            branches_ancestor: false,
            branches_squash: false,
            worktrees: false,
            worktrees_safe: false,
            docker: false,
            docker_dangling: false,
            max_per_run: 0,
        }
    }
}

/// Entries per run, honouring the setting.
///
/// Clamped rather than trusted, and 0 means "use the default" rather
/// than "propose nothing" -- a stored 0 from a bad write should not
/// silently disable a feature the user turned on.
pub fn max_per_run(prefs: &CleanupPrefs) -> usize {
    match prefs.max_per_run {
        0 => 25,
        n => n.clamp(1, 500) as usize,
    }
}

/// How many ledger rows are kept, newest first.
///
/// The ledger is read with a limit of 200 (`commands::cleanup_log`), so
/// everything past the newest 200 rows is storage nothing can display.
/// This bound is deliberately 10x that read limit: it leaves room for
/// the read limit to grow several times over before the trim could ever
/// remove a row the view would have shown, and it is still a hard
/// ceiling instead of the unbounded growth #837 found.
///
/// Before this, `cleanup_log` was the only table in the store that was
/// never pruned: `grep 'DELETE FROM cleanup_log'` returned nothing.
/// Growth is one row per considered entry per run, and `max_per_run`
/// clamps a run to 500 ([`max_per_run`]) -- so a daily run at the cap
/// wrote ~180,000 rows a year, of which 200 were ever displayable.
///
/// Why a ROW count and not an age window, which is what
/// `store::health::RETENTION_HOURS` uses: a health sample is written on
/// a timer, so "the last 24 hours" is a predictable number of rows. A
/// cleanup run is user-triggered and bursty -- a week of no runs then
/// one run of 500 -- so an age window would either drop a ledger the
/// user has not read yet (short window) or bound nothing in practice
/// (long window). A row count bounds the table regardless of how the
/// runs are spaced, which is the property #837 asked for.
pub const MAX_ROWS: i64 = 2_000;

/// Append entries to the ledger, then trim it to [`MAX_ROWS`].
///
/// Failure is logged and swallowed: the ledger is a record of work, and
/// losing a row must never take down the pass that produced it. Same
/// reasoning as `notify_breakage` in `poll`.
///
/// The trim runs beside the insert rather than on a timer, for the
/// reason `store::health::record` gives: a timer is a second thing to
/// schedule and to get wrong, and deleting a handful of rows next to an
/// insert costs nothing.
pub fn record(conn: &Connection, entries: &[LedgerEntry]) {
    for e in entries {
        let r = conn.execute(
            "INSERT INTO cleanup_log (at, kind, target, detail, bytes, action, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                e.at,
                e.kind,
                e.target,
                e.detail,
                e.bytes.map(|b| b as i64),
                e.action,
                e.error
            ],
        );
        if let Err(err) = r {
            log::warn!("could not record a cleanup entry: {err}");
        }
    }
    trim(conn);
}

/// Drop all but the newest [`MAX_ROWS`] ledger rows.
///
/// Ordered by `id`, not by `at`. `at` is supplied by the CALLER --
/// `propose` stamps every entry in a run with the `now` string it was
/// handed, and `record` does not check it -- so a clock change, a
/// timezone-less write, or a caller passing a literal (the tests pass
/// `"now"`) would make `at` non-monotonic and trim the wrong end.
/// `id` is `INTEGER PRIMARY KEY`, so it is the rowid and strictly
/// increasing per insert; newest-by-`id` is exactly newest-by-insert.
///
/// Note this means the trim does not use the `cleanup_log_at` index.
/// It does not need to: the subquery orders by the rowid, which is the
/// table's own physical order, and a `LIMIT`ed scan from the end of it
/// needs no index at all.
///
/// The two orderings are therefore not identical -- [`recent`] reads
/// `ORDER BY at DESC, id DESC`, putting `at` first. They agree whenever
/// `at` is monotonic, which is every normal run. If a clock moved
/// backwards they could disagree: a row with a later `id` but an earlier
/// `at` is kept here while sorting late there. That cannot cost a
/// displayable row at these numbers -- the trim keeps 2,000 and the read
/// asks for 200, so the disagreement would have to span ten times the
/// read limit to reach the boundary -- and the alternative, trimming by
/// `at`, would hand the decision to the same untrusted clock rather than
/// to insert order. Recorded rather than fixed, because the fix would be
/// the worse of the two.
///
/// Failure is logged and swallowed for the same reason the insert's is:
/// a ledger that cannot be trimmed is a table that grows, which is
/// strictly better than a cleanup pass that fails.
fn trim(conn: &Connection) {
    let r = conn.execute(
        "DELETE FROM cleanup_log WHERE id NOT IN
           (SELECT id FROM cleanup_log ORDER BY id DESC LIMIT ?1)",
        rusqlite::params![MAX_ROWS],
    );
    if let Err(err) = r {
        log::warn!("could not trim the cleanup ledger: {err}");
    }
}

/// The most recent entries, newest first.
pub fn recent(conn: &Connection, limit: u32) -> Result<Vec<LedgerEntry>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT at, kind, target, detail, bytes, action, error
         FROM cleanup_log ORDER BY at DESC, id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], |r| {
        Ok(LedgerEntry {
            at: r.get(0)?,
            kind: r.get(1)?,
            target: r.get(2)?,
            detail: r.get(3)?,
            bytes: r.get::<_, Option<i64>>(4)?.map(|b| b as u64),
            action: r.get(5)?,
            error: r.get(6)?,
        })
    })?;
    rows.collect()
}

/// Read the stored preferences.
pub fn prefs(conn: &Connection) -> CleanupPrefs {
    settings::get(conn, settings::keys::CLEANUP_PREFS)
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// Build the proposal list for one run.
///
/// Reuses the SAME scanners and the same verdicts the Artifacts view
/// renders, rather than re-deriving what counts as reclaimable. One
/// implementation means the preview cannot drift from what the view
/// shows, and a user comparing the two will never find them disagreeing.
///
/// Pure apart from the filesystem reads the scanners do: it proposes,
/// and the caller decides what to do with the list. In Phase 1 the only
/// caller writes it to the ledger.
pub fn propose(prefs: &CleanupPrefs, roots: &[String], now: &str) -> Vec<LedgerEntry> {
    if !prefs.enabled {
        return Vec::new();
    }

    let cap = max_per_run(prefs);
    let mut out: Vec<LedgerEntry> = Vec::new();

    if prefs.venvs {
        // Orphans always; stale only with `venvs_stale`.
        //
        // An orphan is a FACT -- nothing on the machine hashes to it, so
        // the project that made it is gone. Stale is a THRESHOLD about a
        // project that still exists, and an unattended pass is the last
        // place to act on a threshold without being told to.
        //
        // The manual path deliberately has no such gate: ticking a row
        // and confirming a dialog is already the user's intent. Here
        // there is no such signal, which is why the opt-in lives here.
        let dirs = crate::caches::project_dirs(roots);
        for v in crate::caches::scan_poetry(&dirs) {
            if out.len() >= cap {
                break;
            }
            // MEASURED first, because staleness needs the idle time and
            // `scan_poetry` reports only what it can decide without a
            // walk. The size is needed for the ledger either way, so
            // this costs nothing extra.
            let (bytes, idle) = crate::caches::measure(std::path::Path::new(&v.path));
            let state = crate::caches::classify_measured(v.state, idle);
            let eligible = match state {
                crate::caches::VenvState::Orphaned => true,
                crate::caches::VenvState::Stale => prefs.venvs_stale,
                // Live is never removed unattended. Its project exists
                // and something touched it recently.
                crate::caches::VenvState::Live => false,
                // A truncated walk cannot support any proposal. The
                // unattended pass is the worst place to guess: nobody
                // is watching it decide (#747).
                crate::caches::VenvState::Unknown => false,
            };
            if !eligible {
                continue;
            }
            out.push(LedgerEntry {
                at: now.to_string(),
                kind: "venv".into(),
                target: v.path,
                detail: Some(v.project),
                bytes: Some(bytes),
                action: "proposed".into(),
                error: None,
            });
        }
    }

    if prefs.artifacts {
        for a in crate::artifacts::scan(roots) {
            if out.len() >= cap {
                break;
            }
            let (bytes, idle) = crate::artifacts::measure(std::path::Path::new(&a.path));
            // Skip anything a build may be writing to, AND anything whose
            // age could not be read at all. Recorded as `skipped` rather
            // than omitted: a directory that keeps being passed over is
            // something the user should be able to see, not a silent gap
            // in the list.
            //
            // The None arm was missing (#841, same defect as
            // `remove_artifact`'s delete gate). `idle.is_some_and(..)`
            // maps None to false, so a directory nothing could measure
            // was `proposed` for UNATTENDED removal. `measure` returns
            // None when no mtime was readable anywhere in the tree, or
            // when the newest mtime is in the future and `elapsed()`
            // fails -- an NTP correction mid-build produces the latter.
            //
            // This is the same rule the venv branch above already
            // applies to `VenvState::Unknown`, for the reason stated
            // there: the unattended pass is the worst place to guess,
            // because nobody is watching it decide (#747). The reasons
            // are reported separately because they are different facts
            // about the directory and a user acting on the ledger needs
            // to know which one they are looking at.
            let skip = match idle {
                None => Some("could not tell when it was last written to"),
                Some(secs) if secs < ACTIVE_WINDOW_SECS => Some("written to recently"),
                Some(_) => None,
            };
            if let Some(reason) = skip {
                out.push(LedgerEntry {
                    at: now.to_string(),
                    kind: "artifact".into(),
                    target: a.path,
                    detail: Some(a.kind.regenerated_by().to_string()),
                    bytes: Some(bytes),
                    action: "skipped".into(),
                    error: Some(reason.into()),
                });
                continue;
            }
            out.push(LedgerEntry {
                at: now.to_string(),
                kind: "artifact".into(),
                target: a.path,
                detail: Some(a.kind.regenerated_by().to_string()),
                bytes: Some(bytes),
                action: "proposed".into(),
                error: None,
            });
        }
    }

    if prefs.worktrees {
        propose_worktrees(prefs, roots, now, cap, &mut out);
    }

    if prefs.branches {
        propose_branches(prefs, roots, now, cap, &mut out);
    }

    out
}

/// Whether an unattended pass may propose removing this worktree.
///
/// Extracted so the decision is testable as itself, the pattern
/// `venv_eligibility` establishes: the alternative is building real git
/// repositories in a unit test to assert a boolean.
///
/// Only `Safety::Safe`. Not `MergedUpstreamDeleted`, not
/// `DetachedMerged` -- each is a separate CLAIM and would need its own
/// opt-in field. "Merged and pushed" is checkable against a remote that
/// still has the branch; "the upstream is gone" means the evidence went
/// with it; a detached HEAD has no branch to have been merged at all.
///
/// The main checkout is excluded EXPLICITLY rather than relying on
/// `MainCheckout` never being `Safe`. That happens to be true and is
/// the kind of property that quietly stops being true.
fn worktree_eligible(wt: &crate::worktrees::Worktree) -> bool {
    !wt.is_main && matches!(wt.safety, crate::worktrees::Safety::Safe)
}

/// Whether an unattended pass may propose deleting this branch.
///
/// The two merge proofs are separately gated because they do not
/// deserve equal confidence: `Ancestor` is certain -- the tip IS in the
/// default branch's history -- while `Squash` is an inference from a
/// diff appearing under a different commit, which is the shape a
/// squash-merged PR leaves AND the shape a cherry-picked-then-reverted
/// branch leaves.
///
/// Every other `Deletable` is a refusal, and an unattended pass must
/// not reinterpret one.
fn branch_eligible(d: &crate::branches::Deletable, prefs: &CleanupPrefs) -> bool {
    use crate::branches::{Deletable, MergedHow};
    match d {
        Deletable::Merged {
            how: MergedHow::Ancestor,
        } => prefs.branches_ancestor,
        Deletable::Merged {
            how: MergedHow::Squash,
        } => prefs.branches_squash,
        _ => false,
    }
}

/// Merged worktrees, proposed for removal (#1141).
///
/// # Only `Safety::Safe`
///
/// Not `MergedUpstreamDeleted`, not `DetachedMerged`. Each is a separate
/// CLAIM and would need its own opt-in field: "merged and pushed" is
/// checkable against a remote that still has the branch, while "the
/// upstream is gone" means the evidence went with it, and a detached
/// HEAD has no branch to have been merged at all. `worktrees_safe`
/// gates exactly the one this proposes.
///
/// An unattended pass is the worst place to widen a claim, because
/// nobody is watching it decide (#747) -- the same reasoning the venv
/// proposer gives for gating staleness.
///
/// # The main checkout is never proposed
///
/// It is the repository. `Safety::MainCheckout` is its own verdict and
/// never `Safe`, so this falls out of the gate above -- but it is
/// asserted in a test, because "the gate happens to exclude it" is a
/// property that could quietly stop being true.
fn propose_worktrees(
    prefs: &CleanupPrefs,
    roots: &[String],
    now: &str,
    cap: usize,
    out: &mut Vec<LedgerEntry>,
) {
    // The opt-in for the one claim this makes. Without it the category
    // is enabled and proposes nothing, which is the state #1141 is
    // about -- so this is a real gate, not a formality.
    if !prefs.worktrees_safe {
        return;
    }
    for repo in crate::worktrees::scan_dirs_fast_reporting(roots).repos {
        for wt in repo.worktrees {
            if out.len() >= cap {
                return;
            }
            if !worktree_eligible(&wt) {
                continue;
            }
            out.push(LedgerEntry {
                at: now.to_string(),
                kind: "worktree".into(),
                target: wt.path,
                // The BRANCH, which is what the user recognises. A path
                // under `.worktrees/` is a directory name; the branch
                // is the work.
                detail: Some(wt.branch),
                // `size_bytes` is whatever the fast scan knew. NOT
                // measured here: the venv and artifact proposers
                // measure because staleness needs the idle time, and
                // this gate needs no such thing -- a walk per worktree
                // on a machine with 295 of them would be minutes of
                // disk for a number the ledger only displays.
                bytes: wt.size_bytes,
                action: "proposed".into(),
                error: None,
            });
        }
    }
}

/// Merged branches, proposed for deletion (#1141).
///
/// # LOCAL branches only, and that is not a default
///
/// #1141's own scope says it plainly and it is worth restating where
/// the code is: **no remote branch deletion, ever, from an unattended
/// pass.** `branches::scan` reports local refs and this proposes
/// exactly those; a remote delete is irreversible for everyone else on
/// the repository, not just the person whose machine ran the timer.
///
/// # The two merge proofs are separately gated
///
/// `Ancestor` is certain -- the tip IS in the default branch's history.
/// `Squash` is an inference from a diff appearing under a different
/// commit, which is the shape a squash-merged PR leaves AND the shape a
/// cherry-picked-then-reverted branch leaves. They do not deserve equal
/// confidence, so `branches_ancestor` and `branches_squash` gate them
/// separately, exactly as the issue specifies.
fn propose_branches(
    prefs: &CleanupPrefs,
    roots: &[String],
    now: &str,
    cap: usize,
    out: &mut Vec<LedgerEntry>,
) {
    use crate::branches::{Deletable, MergedHow};

    // Neither proof enabled: the category is on and has nothing it is
    // allowed to propose.
    if !prefs.branches_ancestor && !prefs.branches_squash {
        return;
    }
    for repo in crate::worktrees::scan_dirs_fast_reporting(roots).repos {
        if out.len() >= cap {
            return;
        }
        // A repository whose branches could not be read proposes
        // nothing, and says so rather than looking like one with no
        // merged branches (#846). One row per repository, not per
        // branch -- the failure is about the repository.
        let branches = match crate::branches::scan(std::path::Path::new(&repo.path)) {
            Ok(b) => b,
            Err(e) => {
                out.push(LedgerEntry {
                    at: now.to_string(),
                    kind: "branch".into(),
                    target: repo.path.clone(),
                    detail: None,
                    bytes: None,
                    action: "skipped".into(),
                    error: Some(e),
                });
                continue;
            }
        };
        for b in branches {
            if out.len() >= cap {
                return;
            }
            if !branch_eligible(&b.deletable, prefs) {
                continue;
            }
            out.push(LedgerEntry {
                at: now.to_string(),
                kind: "branch".into(),
                target: b.name,
                // WHICH proof, carried into the ledger. A user reading
                // "proposed" later needs to know whether it rested on
                // an ancestor check or on a diff inference -- they are
                // not the same evidence.
                detail: Some(
                    match &b.deletable {
                        Deletable::Merged {
                            how: MergedHow::Squash,
                        } => "squash-merged",
                        _ => "merged",
                    }
                    .to_string(),
                ),
                // A branch is a ref. Reporting 0 would claim a
                // measurement of nothing (#846).
                bytes: None,
                action: "proposed".into(),
                error: None,
            });
        }
    }
}

/// Mirrors the artifact view's rule, and the backend's delete-time one.
///
/// All three are now asserted equal by
/// `src/lib/mirroredConstants.test.ts`, which reads this literal and
/// `artifacts/mod.rs`' via Vite's `?raw` and compares both to
/// `ArtifactsPage.tsx`' `ACTIVE_SECS`. This comment claimed the mirror
/// while the UI's copy was an hour (#850); `artifacts/mod.rs` carries the
/// argument for fifteen.
const ACTIVE_WINDOW_SECS: u64 = 15 * 60;

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> CleanupPrefs {
        CleanupPrefs {
            enabled: true,
            artifacts: true,
            venvs: false,
            ..Default::default()
        }
    }

    /// #1141: which worktrees and branches an UNATTENDED pass may
    /// propose.
    ///
    /// Tested as the decision itself rather than through `propose`, for
    /// the reason `venv_eligibility` below gives: reaching those arms
    /// means building real git repositories on the machine running the
    /// tests.
    mod worktree_eligibility {
        use super::*;
        use crate::worktrees::{Safety, Worktree};

        fn wt(safety: Safety, is_main: bool) -> Worktree {
            Worktree {
                path: "/code/proj/.worktrees/x".into(),
                branch: "feature".into(),
                head: "abc".into(),
                size_bytes: Some(1024),
                safety,
                is_main,
                merged_at: None,
                upstream: None,
                last_commit: None,
                submodules: None,
                locked: None,
                prunable: None,
            }
        }

        #[test]
        fn a_safe_worktree_is_proposed() {
            assert!(worktree_eligible(&wt(Safety::Safe, false)));
        }

        #[test]
        fn the_main_checkout_is_never_proposed_even_if_it_reads_safe() {
            // Excluded EXPLICITLY rather than relying on
            // `MainCheckout` never being `Safe`. That happens to be
            // true and is the kind of property that quietly stops
            // being true -- and the cost of being wrong is proposing
            // the repository itself for removal.
            assert!(!worktree_eligible(&wt(Safety::Safe, true)));
            assert!(!worktree_eligible(&wt(Safety::MainCheckout, true)));
        }

        #[test]
        fn a_merged_upstream_deleted_worktree_is_not_proposed() {
            // A separate CLAIM needing its own opt-in: "merged and
            // pushed" is checkable against a remote that still has the
            // branch, while "the upstream is gone" means the evidence
            // went with it.
            assert!(!worktree_eligible(&wt(
                Safety::MergedUpstreamDeleted,
                false
            )));
        }

        #[test]
        fn a_detached_merged_worktree_is_not_proposed() {
            // No branch to have been merged at all.
            assert!(!worktree_eligible(&wt(
                Safety::DetachedMerged("abc".into()),
                false
            )));
        }

        #[test]
        fn nothing_with_work_in_it_is_proposed() {
            for s in [
                Safety::Dirty(3),
                Safety::Unpushed(2),
                Safety::NeverPushed,
                Safety::Unmerged,
                Safety::Prunable("gone".into()),
                Safety::Orphaned,
                Safety::Unknown("could not check".into()),
            ] {
                assert!(!worktree_eligible(&wt(s.clone(), false)), "{s:?}");
            }
        }
    }

    mod branch_eligibility {
        use super::*;
        use crate::branches::{Deletable, MergedHow};

        fn prefs(ancestor: bool, squash: bool) -> CleanupPrefs {
            CleanupPrefs {
                enabled: true,
                branches: true,
                branches_ancestor: ancestor,
                branches_squash: squash,
                ..Default::default()
            }
        }

        #[test]
        fn each_merge_proof_is_gated_separately() {
            // They do not deserve equal confidence: `Ancestor` is
            // certain, `Squash` is an inference from a diff shape that
            // a cherry-picked-then-reverted branch also leaves.
            let anc = Deletable::Merged {
                how: MergedHow::Ancestor,
            };
            let sq = Deletable::Merged {
                how: MergedHow::Squash,
            };

            assert!(branch_eligible(&anc, &prefs(true, false)));
            assert!(!branch_eligible(&sq, &prefs(true, false)));

            assert!(!branch_eligible(&anc, &prefs(false, true)));
            assert!(branch_eligible(&sq, &prefs(false, true)));
        }

        #[test]
        fn neither_proof_enabled_proposes_nothing() {
            // The state #1141 is about: the category is ticked and
            // nothing happens. With both sub-options off that is
            // CORRECT rather than the bug -- the bug was that it was
            // true however they were set.
            for d in [
                Deletable::Merged {
                    how: MergedHow::Ancestor,
                },
                Deletable::Merged {
                    how: MergedHow::Squash,
                },
            ] {
                assert!(!branch_eligible(&d, &prefs(false, false)));
            }
        }

        #[test]
        fn a_refusal_is_never_reinterpreted_as_permission() {
            // Every other `Deletable` means "no", and an unattended
            // pass is the last place to read one as a maybe.
            let all_on = prefs(true, true);
            for d in [
                Deletable::DefaultBranch,
                Deletable::CheckedOut {
                    path: "/code/proj".into(),
                },
                Deletable::Unmerged { ahead: Some(4) },
                // `None` is the UNREAD count, not zero (#967). It must
                // not become permission either.
                Deletable::Unmerged { ahead: None },
            ] {
                assert!(!branch_eligible(&d, &all_on), "{d:?}");
            }
        }
    }

    /// #453: which virtualenv states an UNATTENDED pass may propose.
    ///
    /// Tested as the decision itself rather than through `propose`,
    /// which would need a real Poetry cache on the machine running the
    /// tests -- and writing into a developer's actual cache to assert a
    /// boolean is the wrong trade.
    mod venv_eligibility {
        use super::*;
        use crate::caches::VenvState;

        /// Mirrors the arm in `propose`. If that changes shape, this
        /// test must be updated with it -- which is the point: the rule
        /// is small enough that duplicating it beats not testing it.
        fn eligible(state: VenvState, venvs_stale: bool) -> bool {
            match state {
                VenvState::Orphaned => true,
                VenvState::Stale => venvs_stale,
                VenvState::Live => false,
                VenvState::Unknown => false,
            }
        }

        /// An orphan is a FACT -- nothing hashes to it, so the project
        /// that made it is gone. No opt-in needed.
        #[test]
        fn orphans_need_no_opt_in() {
            assert!(eligible(VenvState::Orphaned, false));
            assert!(eligible(VenvState::Orphaned, true));
        }

        /// Stale is a THRESHOLD about a project that still exists, and
        /// an unattended pass is the last place to act on one uninvited.
        #[test]
        fn stale_requires_the_opt_in() {
            assert!(!eligible(VenvState::Stale, false));
            assert!(eligible(VenvState::Stale, true));
        }

        /// #747: a truncated walk proposes NOTHING, whatever the opt-in
        /// says. `Unknown` means the scan did not finish, so "nothing
        /// owns this venv" was never established -- and the unattended
        /// pass is the one place where a wrong call is unwitnessed.
        #[test]
        fn an_unfinished_scan_proposes_nothing() {
            assert!(!eligible(VenvState::Unknown, false));
            assert!(!eligible(VenvState::Unknown, true));
        }

        /// Live is never proposed, whatever the settings say. Its
        /// project exists and something touched it recently.
        #[test]
        fn live_is_never_proposed() {
            assert!(!eligible(VenvState::Live, false));
            assert!(!eligible(VenvState::Live, true));
        }

        /// The default must not widen what runs by itself.
        #[test]
        fn the_default_is_orphans_only() {
            let p = CleanupPrefs::default();
            assert!(!p.venvs_stale);
            assert!(!eligible(VenvState::Stale, p.venvs_stale));
        }
    }

    /// The master switch is the outermost guard. An upgrade must not
    /// start scanning, let alone proposing, for someone who never opened
    /// Settings.
    /// The fixture must contain something the pass WOULD propose,
    /// otherwise an empty result proves nothing -- an earlier version of
    /// this test used a bare temp directory and passed with the master
    /// switch removed entirely.
    #[test]
    fn a_disabled_pass_proposes_nothing() {
        let t = tempfile::TempDir::new().unwrap();
        std::fs::write(t.path().join("Cargo.toml"), "[package]").unwrap();
        let target = t.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("x.bin"), "x").unwrap();
        let roots = vec![t.path().to_string_lossy().to_string()];

        // Enabled, this fixture yields an entry...
        assert!(!propose(&on(), &roots, "now").is_empty());

        // ...and disabled, it must yield none.
        let prefs = CleanupPrefs {
            enabled: false,
            artifacts: true,
            ..Default::default()
        };
        assert!(propose(&prefs, &roots, "now").is_empty());
    }

    /// Defaults are conservative in every direction, so a settings row
    /// written before this feature existed enables nothing.
    #[test]
    fn the_defaults_enable_nothing() {
        let d = CleanupPrefs::default();
        assert!(!d.enabled);
        assert!(!d.artifacts);
        assert!(!d.venvs);
        assert_eq!(d.mode, CleanupMode::Preview);
    }

    /// Phase 1 CANNOT delete. This pins the property that makes the
    /// phase reviewable on the predicate's merits alone: if the pass
    /// gains a removal path, this test is where that shows up.
    #[test]
    fn nothing_in_a_proposal_is_an_action() {
        let t = tempfile::TempDir::new().unwrap();
        std::fs::write(t.path().join("Cargo.toml"), "[package]").unwrap();
        let target = t.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("x.bin"), "x").unwrap();

        let out = propose(&on(), &[t.path().to_string_lossy().to_string()], "now");
        assert!(
            out.iter().all(|e| e.action != "removed"),
            "a preview run must never record a removal"
        );
        assert!(target.exists(), "and must never actually remove anything");
    }

    /// A directory a build may be writing to is recorded as SKIPPED
    /// rather than omitted: one that keeps being passed over is
    /// something the user should be able to see.
    #[test]
    fn an_active_directory_is_skipped_visibly() {
        let t = tempfile::TempDir::new().unwrap();
        std::fs::write(t.path().join("Cargo.toml"), "[package]").unwrap();
        let target = t.path().join("target");
        std::fs::create_dir(&target).unwrap();
        // Written just now.
        std::fs::write(target.join("fresh.bin"), "x").unwrap();

        let out = propose(&on(), &[t.path().to_string_lossy().to_string()], "now");
        let e = out
            .iter()
            .find(|e| e.target == target.to_string_lossy())
            .expect("the directory must appear in the ledger, not vanish from it");
        assert_eq!(
            e.action, "skipped",
            "a fresh directory must not be proposed"
        );
        assert_ne!(
            e.action, "proposed",
            "removing the active check would propose a directory a build may be writing to"
        );
        assert!(e.error.is_some(), "and says why");
    }

    /// The same fail-open hole as `remove_artifact`'s delete gate (#841),
    /// on the pass where it matters more: an unattended one.
    ///
    /// `measure` returns `None` when no mtime was readable anywhere in
    /// the tree, or when the newest mtime is in the FUTURE -- it ends in
    /// `newest.and_then(|n| n.elapsed().ok())`, and `elapsed()` is `Err`
    /// for a future timestamp, which an NTP correction during a build
    /// produces. `idle.is_some_and(..)` mapped that None to false, so a
    /// directory nothing could measure was `proposed` for removal with
    /// nobody watching.
    ///
    /// This is the rule `an_unfinished_scan_proposes_nothing` already
    /// states for venvs, asserted for artifacts: the unattended pass is
    /// the worst place to guess (#747).
    ///
    /// `#[cfg(unix)]` because setting an mtime needs `touch -t` -- the
    /// trade `artifacts::removal_tests::make_old` already documents, and
    /// the status is asserted here rather than ignored so a silently-unset
    /// mtime cannot leave this passing for the wrong reason. The guard it
    /// covers is platform-independent.
    #[test]
    #[cfg(unix)]
    fn a_directory_of_unreadable_age_is_skipped_not_proposed() {
        let t = tempfile::TempDir::new().unwrap();
        std::fs::write(t.path().join("Cargo.toml"), "[package]").unwrap();
        let target = t.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("artifact.bin"), "x").unwrap();
        // A future mtime, so `elapsed()` fails and the age is unreadable.
        // Chosen over unreadable permissions because a permissions test
        // cannot run as root and would be skipped in the CI that needs it.
        let status = std::process::Command::new("touch")
            .args(["-t", "209901010000"])
            .arg(target.join("artifact.bin"))
            .status()
            .expect("touch should be available");
        assert!(status.success(), "could not set a future mtime");
        assert!(
            crate::artifacts::measure(&target).1.is_none(),
            "premise: a future mtime must make the age unreadable"
        );

        let out = propose(&on(), &[t.path().to_string_lossy().to_string()], "now");
        let e = out
            .iter()
            .find(|e| e.target == target.to_string_lossy())
            .expect("the directory must appear in the ledger, not vanish from it");
        assert_eq!(
            e.action, "skipped",
            "an unmeasurable directory must not be proposed"
        );
        assert_eq!(
            e.error.as_deref(),
            Some("could not tell when it was last written to"),
            "and says which of the two reasons it was skipped for"
        );
    }

    /// The cap bounds a run. It matters in Preview too: a ledger listing
    /// every directory on the machine is one nobody reads, which is the
    /// same as no ledger.
    #[test]
    fn a_run_is_capped() {
        let t = tempfile::TempDir::new().unwrap();
        for i in 0..6 {
            let d = t.path().join(format!("p{i}"));
            std::fs::create_dir(&d).unwrap();
            std::fs::write(d.join("Cargo.toml"), "[package]").unwrap();
            std::fs::create_dir(d.join("target")).unwrap();
        }
        let prefs = CleanupPrefs {
            max_per_run: 2,
            ..on()
        };
        let out = propose(&prefs, &[t.path().to_string_lossy().to_string()], "now");
        assert_eq!(out.len(), 2);
    }

    /// Zero means "use the default", not "propose nothing": a stored 0
    /// from a bad write must not silently disable a feature the user
    /// turned on.
    #[test]
    fn a_zero_cap_means_the_default() {
        assert_eq!(max_per_run(&CleanupPrefs::default()), 25);
        assert_eq!(
            max_per_run(&CleanupPrefs {
                max_per_run: 3,
                ..Default::default()
            }),
            3
        );
        // And a wild value is clamped rather than trusted.
        assert_eq!(
            max_per_run(&CleanupPrefs {
                max_per_run: 9_999,
                ..Default::default()
            }),
            500
        );
    }

    /// Phase 1 must not be able to STORE Remove mode.
    ///
    /// The variant exists so the settings and ledger shapes do not change
    /// in Phase 2, but a setting that can be turned on and does nothing
    /// is worse than one that does not exist -- the user believes it.
    /// The command refuses it; this pins that the default never is it.
    #[test]
    fn remove_mode_is_never_the_stored_default() {
        assert_eq!(CleanupPrefs::default().mode, CleanupMode::Preview);
        let round: CleanupPrefs = serde_json::from_str("{}").unwrap();
        assert_eq!(
            round.mode,
            CleanupMode::Preview,
            "a settings row written before this field existed must read as Preview"
        );
    }

    #[test]
    fn the_ledger_round_trips() {
        let dir = tempfile::TempDir::new().unwrap();
        let conn = crate::store::open_db(&dir.path().join("t.db")).unwrap();
        record(
            &conn,
            &[LedgerEntry {
                at: "2026-09-02T00:00:00Z".into(),
                kind: "artifact".into(),
                target: "/code/x/target".into(),
                detail: Some("cargo build".into()),
                bytes: Some(1234),
                action: "proposed".into(),
                error: None,
            }],
        );
        let back = recent(&conn, 10).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].target, "/code/x/target");
        assert_eq!(back[0].bytes, Some(1234));
    }

    /// The ledger is bounded, so the table cannot grow without bound.
    ///
    /// #837: `cleanup_log` was the only table in the store never pruned.
    /// The same property `health::samples_older_than_a_day_are_dropped`
    /// pins for `health_samples`, by row count instead of by age -- see
    /// [`MAX_ROWS`] for why that axis.
    ///
    /// Writes `MAX_ROWS + 50` in ONE `record` call rather than one per
    /// call: the trim runs at the end of `record`, so a per-call loop
    /// would only ever test the steady state where the table is already
    /// at the bound. A single over-cap batch tests the case where the
    /// trim has to remove more than one row, which is what a user who
    /// upgrades with an already-large ledger hits.
    #[test]
    fn the_ledger_is_trimmed_to_its_bound() {
        let dir = tempfile::TempDir::new().unwrap();
        let conn = crate::store::open_db(&dir.path().join("t.db")).unwrap();

        let over = (MAX_ROWS + 50) as usize;
        let entries: Vec<LedgerEntry> = (0..over)
            .map(|i| LedgerEntry {
                // Every row carries the SAME `at`, which is what a real
                // run does -- `propose` stamps one `now` across the
                // whole batch. If the trim ordered by `at` it would have
                // no order to work with here and could keep any 2,000 of
                // these; ordering by `id` makes "newest" well defined.
                at: "2026-09-02T00:00:00Z".into(),
                kind: "artifact".into(),
                target: format!("/code/p{i}/target"),
                detail: None,
                bytes: Some(i as u64),
                action: "proposed".into(),
                error: None,
            })
            .collect();
        record(&conn, &entries);

        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM cleanup_log", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, MAX_ROWS, "the ledger must be bounded by MAX_ROWS");

        // And the rows kept are the NEWEST ones, not an arbitrary
        // 2,000: the last entry written must still be readable.
        let back = recent(&conn, 1).unwrap();
        assert_eq!(
            back[0].target,
            format!("/code/p{}/target", over - 1),
            "the trim must drop the oldest rows, not the newest"
        );

        // The bound is comfortably above the 200 rows `cleanup_log`
        // reads (`commands::cleanup_log`), so nothing displayable is
        // ever trimmed. Pinned rather than left to the doc: a future
        // edit that lowers MAX_ROWS below the read limit would start
        // deleting rows the view was about to show.
        //
        // A `const` block, so it fails at COMPILE time rather than when
        // this test runs -- both operands are constants, and clippy is
        // right that an assertion over two constants belongs in one.
        const { assert!(MAX_ROWS >= 200 * 5) };
        assert_eq!(
            recent(&conn, 200).unwrap().len(),
            200,
            "a full read must still be satisfiable after a trim"
        );
    }
}

#[cfg(test)]
mod granularity {
    use super::*;

    /// #493 added six fields to a PERSISTED struct.
    ///
    /// A user upgrading has prefs on disk written before they existed.
    /// Every field carries `#[serde(default)]`, so the old shape must
    /// still load and must not silently turn anything on.
    #[test]
    fn prefs_saved_before_these_fields_existed_still_load() {
        let old = r#"{
            "enabled": true,
            "mode": "remove",
            "artifacts": true,
            "venvs": true,
            "venvs_stale": true,
            "max_per_run": 25
        }"#;
        let p: CleanupPrefs = serde_json::from_str(old).expect("the old shape must still load");

        // What they set is preserved.
        assert!(p.enabled);
        assert!(p.artifacts);
        assert!(p.venvs);
        assert!(p.venvs_stale);
        assert_eq!(p.max_per_run, 25);

        // What they never saw is OFF. An upgrade must not widen what an
        // unattended pass may delete -- and this user has `mode:
        // remove`, so a default of `true` would start deleting branches
        // they never opted into.
        assert!(!p.branches, "an upgrade must not enable branch cleanup");
        assert!(!p.branches_ancestor);
        assert!(!p.branches_squash);
        assert!(!p.worktrees, "an upgrade must not enable worktree cleanup");
        assert!(!p.worktrees_safe);
        assert!(!p.docker, "an upgrade must not enable image cleanup");
        assert!(!p.docker_dangling);
    }

    /// A fresh install starts with nothing enabled.
    #[test]
    fn every_new_category_defaults_off() {
        let d = CleanupPrefs::default();
        assert!(!d.branches && !d.branches_ancestor && !d.branches_squash);
        assert!(!d.worktrees && !d.worktrees_safe);
        assert!(!d.docker && !d.docker_dangling);
    }

    /// The round trip must not lose a field, or saving prefs would
    /// quietly reset whatever the writer did not know about.
    #[test]
    fn every_field_survives_a_round_trip() {
        let p = CleanupPrefs {
            enabled: true,
            mode: CleanupMode::Preview,
            artifacts: true,
            venvs: true,
            venvs_stale: true,
            branches: true,
            branches_ancestor: true,
            branches_squash: true,
            worktrees: true,
            worktrees_safe: true,
            docker: true,
            docker_dangling: true,
            max_per_run: 12,
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: CleanupPrefs = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}

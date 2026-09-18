//! Bringing every repository level with its remote default branch
//! (#1012, #1013, #1014, #1016, epic #1011).
//!
//! One button, N repositories, and every refusal carries a reason.
//!
//! # This is a loop over `pull_checkout`, not a second pull
//!
//! `remove_worktrees_with_progress` states the shape this follows:
//!
//! > Deliberately a loop over `remove_worktree` rather than a bulk git
//! > call: that keeps the per-worktree safety gate exactly as it is. A
//! > bulk path that evaluated safety once and then deleted N directories
//! > would be a different and much more dangerous thing than N safe
//! > deletions.
//!
//! Substitute "pull" for "delete" and the sentence is unchanged in
//! force. Every one of `pull_checkout`'s three constraints -- refuses on
//! a dirty checkout, `--ff-only`, returns git's own message -- gets
//! STRONGER at N repositories, because the click is further from the
//! repository than it has ever been and the user is watching a progress
//! bar rather than any one of them. So nothing here relaxes them, and
//! nothing here inlines a laxer copy: `pull_checkout` is called, as it
//! is, once per repository.
//!
//! # Every precondition is re-derived at the moment of acting
//!
//! `caches/mod.rs` states the rule at its deletion gate:
//!
//! > Re-derived NOW: if the project directory came back since the scan,
//! > this is no longer an orphan and must not be removed on the strength
//! > of a stale verdict.
//!
//! The All Repositories table's "up to date with the ref it tracks"
//! verdict is exactly a stale verdict. It was computed when the table was
//! drawn -- possibly minutes ago -- and across 45 rows some of them have
//! certainly stopped being true. It may be used to decide what to OFFER
//! and never to decide what to DO. So `classify_for_update` reads
//! `symbolic-ref`, `status --porcelain` and `rev-list --left-right`
//! freshly, per repository, inside the loop. None of it is hoisted out as
//! an optimisation, because hoisting is precisely the "evaluated safety
//! once and then acted N times" shape the comment above rejects.
//!
//! # Five outcomes, never one aggregate
//!
//! `RemovalOutcome`'s `Option<String>` two-state is not enough here, and
//! this is the one place the existing shape is extended rather than
//! copied. Removal has two outcomes: removed, or refused. Updating has
//! five, and collapsing them is the bug #941 spent a day removing:
//! "Updated 12 of 45" hides the only distinction that matters, which is
//! could-not versus did-not. See [`UpdateResult`].

use super::model::Upstream;
use super::scan::{git, pull_checkout};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a whole run may take before it stops by itself.
///
/// A ceiling around the RUN, not around each call, and the distinction is
/// the one #830 records at `scan.rs`:
///
/// > `GIT_TIMEOUT` bounds a single `git` invocation and was believed to
/// > bound this pass through it. It does not... A per-call ceiling cannot
/// > see that; only a ceiling around the worktree can.
///
/// Same failure one level up. `GIT_TIMEOUT` (30s) bounds each
/// repository's pull; nothing bounds 45 of them. MEASURED on the
/// reporting machine, `git fetch --dry-run origin` across the scan root
/// runs 25ms--1055ms per repository, so 45 repositories is ~40s when
/// every remote is reachable. On a dropped VPN or a captive portal every
/// call instead burns the full `GIT_TIMEOUT`, and 45 x 30s is **22
/// minutes** of a button that looks hung.
///
/// 20 minutes rather than something tighter, for `CLASSIFY_TIMEOUT`'s
/// stated reason: *"The bound exists to convert 'never' into 'could not
/// classify', NOT to tighten a latency target."* A run that is genuinely
/// updating 45 slow repositories must be allowed to finish; what must not
/// happen is an unattended run with no end at all. Cancellation is the
/// tool for "I did not mean this" and fires in seconds; this is only the
/// floor under an unattended one.
const RUN_CEILING: Duration = Duration::from_secs(20 * 60);

/// What happened to one repository.
///
/// # Why five variants and not `Option<String>`
///
/// `RemovalOutcome` carries a path and an optional error because removal
/// has two answers. Updating has five, and an aggregate that sums them
/// answers no question a user has:
///
/// - **Updated** -- fast-forwarded. Carries git's own output.
/// - **AlreadyLevel** -- nothing to do. A SUCCESS, and it must never
///   appear in a failure list.
/// - **Skipped** -- a deliberate non-action with a named reason: on a
///   branch that is not the default, ahead of the remote, dirty,
///   detached. The user may want to act on these later; none is a
///   malfunction. On a developer's machine this is the EXPECTED MAJORITY,
///   because most repositories sit on a feature branch, and 30 of these
///   rendered as errors would be a report nobody reads twice.
/// - **Failed** -- could not be read or reached: unreachable remote,
///   unreadable repository, a git refusal. **These are the rows that need
///   attention**, and they are the ones an aggregate buries.
/// - **NotAttempted** -- the run stopped before reaching this repository,
///   because the user cancelled or the ceiling fired. Honestly distinct
///   from both skipped and failed: nothing was decided about it at all.
///
/// A typed multi-state outcome rather than a boolean plus a string, on
/// `RunState`'s precedent (`packages/runs.rs`): `#[serde(tag = "state")]`
/// so a client gets one answer whatever happened.
///
/// **Git's own message survives**, per repository, in every variant that
/// has one. The categorisation is ADDITIONAL, never a replacement:
/// `pull_checkout`'s third constraint is that "could not update" says
/// nothing while git's refusal usually names the problem exactly, and at
/// N repositories that is the only thing making a failure list
/// actionable.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum UpdateResult {
    /// Fast-forwarded, carrying git's own output.
    Updated { message: String },
    /// Already level with the ref it tracks. Nothing to do.
    AlreadyLevel,
    /// Deliberately not acted on, with the reason.
    Skipped { reason: String },
    /// Could not be read or reached, with git's own message.
    Failed { error: String },
    /// The run ended before reaching this repository.
    NotAttempted,
}

/// One repository's outcome in a bulk update.
///
/// Every input gets exactly one of these, in the input's order. Partial
/// completion is not the exception here, it is the design: most
/// repositories on a working machine are on a feature branch and will be
/// skipped, and a run that returned only the ones it changed would be
/// silent about the majority.
///
/// **Paths in outcomes, counts in progress events.** `remove_worktrees`
/// draws exactly this line -- its `RemovalOutcome` carries `path` while
/// its progress event carries only `(done, total)` -- and this follows
/// it: the user needs to know WHICH repository to go to, and a progress
/// event is not a place to leak what they are working on.
///
/// `Repo`-prefixed because `packages::apply::UpdateOutcome` already
/// exists and means something entirely different -- one PACKAGE's
/// outcome in a dependency-update run. Two same-named types in one crate,
/// both about "an update", is the kind of collision a reader resolves
/// wrongly once and then trusts.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RepoUpdateOutcome {
    pub path: String,
    pub result: UpdateResult,
}

/// What the run as a whole did.
///
/// # `unreadable` is here because "all" is a claim about a SET (#1025)
///
/// The scan already reports what it could not read -- `RepoScan` carries
/// `unreadable`, and `PartialScanNotice` renders it. A button labelled
/// "Update All Repositories" over a list assembled from a short census is
/// a lie about scope, and nothing in the sequence is false: the table
/// lists 42, the button updates 42, the report says 42 of 42, and the
/// user closes the app believing all 45 are level.
///
/// Where this DIFFERS from `remove_venv`, which refuses outright on a
/// truncated walk (#747), and the distinction is written here so the next
/// reader does not "harmonise" the two and get one wrong:
///
/// - `remove_venv`'s VERDICT is unsound over an incomplete set. "This is
///   an orphan" means "no live project claims it", which a walk that
///   never reached a project answers wrongly. So it refuses.
/// - Update All's per-repository decision is sound regardless of what the
///   scan missed. Fast-forwarding one repository is correct whether or
///   not some other directory was readable. The incompleteness corrupts
///   the SCOPE CLAIM, not the action.
///
/// So the run is not refused. The word "all" is. The count is carried to
/// the report so a user who dismissed the notice before clicking -- or
/// who reads only the result -- still learns the set was short.
///
/// Unreadable directories are NOT counted as skipped or failed
/// repositories. They are not repositories; they are a hole in the list,
/// and summing them into a per-repository outcome would invent rows for
/// things that were never enumerated.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAllReport {
    /// One entry per input, in the input's order.
    pub outcomes: Vec<RepoUpdateOutcome>,
    /// Whether the user stopped it. Distinct from an error: the
    /// repositories that were fast-forwarded before the stop really were.
    pub cancelled: bool,
    /// Whether [`RUN_CEILING`] fired. Also distinct from an error: the
    /// run did not fail, it ran out of the time it was given.
    pub timed_out: bool,
    /// Paths the SCAN could not read, carried through verbatim (#1025).
    ///
    /// Never folded into `outcomes`, and never dropped: this is what
    /// stops the report claiming a complete census it never had.
    pub unreadable: Vec<String>,
}

impl UpdateAllReport {
    /// Whether the set this report covers was short.
    ///
    /// `RepoScan::is_partial`'s spelling, so the two read the same way.
    pub fn is_partial(&self) -> bool {
        !self.unreadable.is_empty()
    }
}

/// What a fresh look at one repository says should happen to it.
///
/// Deliberately separate from performing the update, so the refusal set
/// is one readable thing that can be tested without a remote. Every
/// branch is re-derived here, at the moment of acting -- never read from
/// the scan, and never from the table's row.
enum Verdict {
    /// Nothing stands in the way; go and pull.
    Pull,
    /// Do not act, and say why. Not a malfunction.
    Skip(String),
    /// Could not be read. Needs attention.
    Fail(String),
}

/// Decide what to do with one repository, reading nothing from the scan.
///
/// The order of the checks is the order of the cheapest, most definite
/// facts first: a missing directory, then a detached HEAD, then the
/// branch, then the working tree, then the ahead/behind counts.
///
/// # The refusal set, and why each is refused rather than handled
///
/// **Dirty working tree -> SKIP, and never stash.** `pull_checkout`
/// refuses this itself, freshly, and this reaches the same verdict one
/// step earlier so the report can phrase it as a deliberate skip rather
/// than as git's failure. Stash is the tempting option and is ruled out
/// explicitly: `CONTRIBUTING.md` records that `git stash` is shared
/// repo-wide, so stashing N dirty trees puts N entries into one stack the
/// user never created, and `stash pop` can conflict -- a button meant to
/// SAVE the user from conflicts manufacturing one in a repository nobody
/// is watching. `pull_checkout`'s own doc says a half-recovered pull is
/// "exactly the situation a GUI button should not create"; a stash/pop
/// cycle creates two more places to land in it.
///
/// **Detached HEAD -> SKIP, and never check out the default branch.**
/// `git pull --ff-only` on a detached HEAD already fails with git's own
/// message, which is acceptable for one repository and not good at 45,
/// where the failure list is the entire output. "not on a branch" is a
/// one-line actionable answer where git's raw text is not. Moving off the
/// detached HEAD is refused outright: it may be the only reference to
/// commits created during a bisect or an interactive rebase, and checking
/// out the default branch can strand them where only the reflog knows.
///
/// **Not on the default branch -> SKIP, counted separately.** The common
/// case, not an error. `git pull --ff-only` on a feature branch pulls
/// THAT branch's upstream, which is a different operation from "bring
/// this repository level with its remote default branch" and not what the
/// button promises. Checking out the default branch is refused for a
/// blunter reason: it silently relocates the user's working position in
/// every one of 45 repositories, and they return to find themselves on
/// `main` in a repository they left on `feature/x` with no record of
/// where they were. That is a larger and more surprising mutation than
/// the update itself.
///
/// **Purely ahead -> SKIP.** There is nothing to fast-forward TO. "N
/// commits ahead, nothing to pull" is information, not a failure, and
/// must not be counted as one. The `NOT PUSHED` reasoning in `assess.rs`
/// applies directly: commits the remote does not have may exist only on
/// this machine.
///
/// **Diverged -> SKIP with the counts named.** `--ff-only` would refuse,
/// correctly, and that refusal stays. Splitting this from "ahead" is the
/// point: `--ff-only` collapses two genuinely different states into one
/// failure message, and `rev-list --left-right --count` -- which the scan
/// already uses -- tells them apart for one cheap local call.
///
/// **Untracked branch -> SKIP.** No upstream means no comparison and
/// nothing to pull. `Upstream::Untracked`'s own doc refuses the other
/// reading: *"Normal, not an error -- and distinctly not 'up to date'."*
///
/// **Anything git could not answer -> FAIL**, carrying git's message.
///
/// Merge conflicts are absent from this list because they cannot arise.
/// With `--ff-only` there is no merge: git either moves the ref or
/// refuses, and a refusal leaves the working tree byte-identical. That is
/// the single strongest reason to hold the line at fast-forward only, and
/// every proposal to make the button "try harder" gives it up.
fn classify_for_update(path: &str) -> Verdict {
    let dir = Path::new(path);
    if !dir.is_dir() {
        // Not "it vanished, never mind": a repository the table listed and
        // that is now gone is a real thing to report. `is_dir` on a path
        // that cannot be stat'd is false for a permission error too, and
        // both belong on the side that needs attention.
        return Verdict::Fail("that directory is missing".into());
    }

    // A detached HEAD FIRST, because every check below asks a question
    // about a branch and there is none.
    let head = match git(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        Ok(s) => s.trim().to_string(),
        Err(_) => {
            return Verdict::Skip(
                "not on a branch (detached HEAD) -- check out a branch first".into(),
            )
        }
    };

    // The default ref, resolved the ONE way this codebase resolves it.
    //
    // Never a hardcoded `origin/main`: that is the wrong answer for over
    // 10% of the repositories on the reporting machine, and #757 measured
    // what a second spelling of this costs -- a nine-row verdict swing in
    // 34, every affected row carrying a confident, false reason.
    // `invariants.rs` already guards four functions named `default_branch`
    // against drifting apart, and this makes no fifth.
    let default_ref = super::scan::default_branch(dir);
    // The SHORT name, because `default_branch` may return either a
    // remote-tracking ref (`origin/master`) or a bare local one, and HEAD
    // is always a local branch name.
    let default_short = default_ref.rsplit('/').next().unwrap_or(&default_ref);
    if head != default_short {
        return Verdict::Skip(format!(
            "on {head}, not the default branch ({default_short})"
        ));
    }

    // The working tree, read FRESHLY. `pull_checkout` does this again
    // itself and that duplication is deliberate: this one exists so the
    // report can categorise, that one is the guarantee, and removing
    // either would be wrong. The scan's answer is not consulted at all.
    //
    // `--untracked-files=no`, matching `pull_checkout` exactly (#653).
    // Untracked files do not block a fast-forward -- git refuses on its
    // own when an incoming commit would OVERWRITE one, and says so -- and
    // counting them here would refuse to update a checkout whose only
    // change is one untracked file. That regression already shipped once.
    let status = match git(dir, &["status", "--porcelain", "--untracked-files=no"]) {
        Ok(s) => s,
        Err(e) => return Verdict::Fail(format!("could not read the checkout's state: {e}")),
    };
    let dirty = status.lines().filter(|l| !l.trim().is_empty()).count();
    if dirty > 0 {
        // `pull_checkout`'s wording, verbatim: it names the count and it
        // names the fix, and one fact should not have two spellings.
        return Verdict::Skip(format!(
            "{dirty} uncommitted change{} -- commit or stash first",
            if dirty == 1 { "" } else { "s" }
        ));
    }

    // Ahead, behind, diverged or level -- one local call, no network.
    match upstream_now(dir) {
        Upstream::Behind(_) => Verdict::Pull,
        // Level as of the refs on disk. The pull is still what confirms
        // it, because those refs may be old -- see `update_all_with`.
        Upstream::Current => Verdict::Pull,
        Upstream::Ahead(n) => Verdict::Skip(format!(
            "{n} commit{} ahead of the remote, nothing to pull",
            if n == 1 { "" } else { "s" }
        )),
        Upstream::Diverged(a, b) => Verdict::Skip(format!(
            "diverged: {a} commit{} ahead, {b} behind -- cannot fast-forward",
            if a == 1 { "" } else { "s" },
        )),
        Upstream::Untracked => {
            Verdict::Skip("no upstream to pull from -- this branch is local only".into())
        }
        // Unreachable in practice: the detached check above returned
        // already. Handled rather than `unreachable!()`, because a panic
        // in a loop over 45 repositories would take the whole run down.
        Upstream::Detached => {
            Verdict::Skip("not on a branch (detached HEAD) -- check out a branch first".into())
        }
        Upstream::Unknown(why) => Verdict::Fail(why),
    }
}

/// How this checkout stands against its upstream, RIGHT NOW.
///
/// A local read of refs already on disk, the same three calls
/// `scan::upstream_state` makes, and the same conclusions. Spelled here
/// rather than calling that function because it is private to the scan
/// and taking it public would invite the scan's cached answer to be used
/// in its place -- which is the one thing this module must not do.
fn upstream_now(dir: &Path) -> Upstream {
    if git(dir, &["symbolic-ref", "--quiet", "HEAD"]).is_err() {
        return Upstream::Detached;
    }
    if git(dir, &["rev-parse", "--abbrev-ref", "@{u}"]).is_err() {
        return Upstream::Untracked;
    }
    match git(dir, &["rev-list", "--left-right", "--count", "@{u}...HEAD"]) {
        Ok(s) => {
            let mut it = s.split_whitespace();
            let behind = it.next().and_then(|v| v.parse::<u64>().ok());
            let ahead = it.next().and_then(|v| v.parse::<u64>().ok());
            match (ahead, behind) {
                (Some(0), Some(0)) => Upstream::Current,
                (Some(a), Some(0)) => Upstream::Ahead(a),
                (Some(0), Some(b)) => Upstream::Behind(b),
                (Some(a), Some(b)) => Upstream::Diverged(a, b),
                // Unparsable output must not become a confident zero
                // (#967): a missing count read as 0 is what rendered "0
                // commits not on the default branch" beside a delete box.
                _ => Upstream::Unknown("could not read ahead/behind counts".into()),
            }
        }
        Err(e) => Upstream::Unknown(e),
    }
}

/// Fast-forward every repository that can be, reporting each one.
///
/// # Cancellation is checked BETWEEN repositories, never mid-pull
///
/// The same granularity `run_on_branch_cancellable` uses between
/// packages, and clean by construction: a repository is either
/// fast-forwarded or it is not, so stopping between two of them leaves no
/// repository half-updated. Killing a `git pull` mid-write is how you get
/// a repository the app has no story for.
///
/// A cancelled run still returns per-repository outcomes for everything
/// it attempted. Cancelling must not discard the report -- the
/// repositories that were fast-forwarded before the stop really were, and
/// the user needs to know which.
///
/// # A CALLBACK, not a Tauri emit
///
/// `remove_worktrees_with_progress` states why, and it holds here:
///
/// > A CALLBACK rather than emitting Tauri events here: this module is
/// > pure git plumbing and has no AppHandle, which is also what keeps it
/// > testable without a running app.
///
/// `on_progress` is called with `(done, total)` AFTER each repository, so
/// the count means "done" and not "started" -- a progress bar that
/// reaches 100% before the work finishes is worse than none.
///
/// # Why a level repository is still pulled
///
/// `Upstream::Current` means level with the refs ON DISK, and those refs
/// are only as fresh as the last fetch -- on the reporting machine one of
/// 38 repositories is fresh by the app's own one-hour threshold, thirteen
/// have never been fetched at all, and sampling found half of eight
/// "up to date" verdicts were actually behind. So a repository the local
/// refs call level is still handed to `pull_checkout`, whose `git pull`
/// fetches and finds out. `AlreadyLevel` is then reported from what git
/// ACTUALLY did, not from what the stale refs predicted -- which is the
/// whole of "the table's verdict may decide what to offer, never what to
/// do".
pub fn update_all_with(
    paths: &[String],
    unreadable: Vec<String>,
    stop: &Arc<AtomicBool>,
    mut on_progress: impl FnMut(usize, usize),
) -> UpdateAllReport {
    let total = paths.len();
    let started = Instant::now();
    let mut outcomes = Vec::with_capacity(total);
    let mut cancelled = false;
    let mut timed_out = false;

    for (i, path) in paths.iter().enumerate() {
        // Both ends checked BEFORE the repository is touched, so a stop
        // never lands halfway through a pull. Once either fires, the rest
        // of the input still gets an outcome -- `NotAttempted` -- because
        // "every input produces exactly one outcome" is the invariant a
        // missing row would break silently.
        if stop.load(Ordering::SeqCst) {
            cancelled = true;
        } else if started.elapsed() >= RUN_CEILING {
            timed_out = true;
        }
        if cancelled || timed_out {
            outcomes.push(RepoUpdateOutcome {
                path: path.clone(),
                result: UpdateResult::NotAttempted,
            });
            continue;
        }

        let result = match classify_for_update(path) {
            Verdict::Skip(reason) => UpdateResult::Skipped { reason },
            Verdict::Fail(error) => UpdateResult::Failed { error },
            // The ONE place this module writes. `pull_checkout` as it is,
            // with its own fresh dirty check and its own `--ff-only`: no
            // force flag, no bulk variant, nothing inlined.
            Verdict::Pull => match pull_checkout(path) {
                Ok(message) => {
                    // git's own words decide which success this was.
                    // "Already up to date." is what `git pull` prints
                    // when the fetch found nothing, and it is the honest
                    // AlreadyLevel -- taken from what git did, not from
                    // the refs that were on disk before it ran.
                    if message.to_ascii_lowercase().contains("already up to date") {
                        UpdateResult::AlreadyLevel
                    } else {
                        UpdateResult::Updated { message }
                    }
                }
                Err(error) => UpdateResult::Failed { error },
            },
        };
        outcomes.push(RepoUpdateOutcome {
            path: path.clone(),
            result,
        });
        // AFTER the work, and INCLUDING failures -- a batch where several
        // fail would otherwise appear to stall.
        on_progress(i + 1, total);
    }

    UpdateAllReport {
        outcomes,
        cancelled,
        timed_out,
        unreadable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    /// Every path built with `PathBuf::join`, never `format!("{}/...")`.
    /// Six Windows-only failures have cost this repository, all of them a
    /// separator baked into a string.
    fn run(dir: &PathBuf, args: &[&str]) {
        let out = Command::new(crate::auth::git_program())
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git must run");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// An origin with one commit on `main`, and a clone of it.
    ///
    /// Returns (tempdir, origin, clone) -- the tempdir is returned so the
    /// caller holds it and the directories outlive the test body.
    fn origin_and_clone() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).expect("origin dir");
        run(&origin, &["init", "--initial-branch=main", "--quiet"]);
        run(
            &origin,
            &["config", "user.email", "headstate@users.noreply.github.com"],
        );
        run(&origin, &["config", "user.name", "T"]);
        std::fs::write(origin.join("a.txt"), "one\n").expect("write");
        run(&origin, &["add", "."]);
        run(&origin, &["commit", "-qm", "one"]);

        let clone = tmp.path().join("clone");
        run(
            &origin,
            &[
                "clone",
                "--quiet",
                origin.to_str().expect("utf8"),
                clone.to_str().expect("utf8"),
            ],
        );
        run(
            &clone,
            &["config", "user.email", "headstate@users.noreply.github.com"],
        );
        run(&clone, &["config", "user.name", "T"]);
        (tmp, origin, clone)
    }

    /// One more commit on the origin, so the clone is behind.
    fn advance(origin: &PathBuf) {
        std::fs::write(origin.join("b.txt"), "two\n").expect("write");
        run(origin, &["add", "."]);
        run(origin, &["commit", "-qm", "two"]);
    }

    fn never_stop() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    fn only(report: &UpdateAllReport) -> &UpdateResult {
        assert_eq!(report.outcomes.len(), 1, "one input, one outcome");
        &report.outcomes[0].result
    }

    /// The happy path, so the refusal tests below cannot be satisfied by
    /// a function that refuses everything.
    ///
    /// Paired with `a_dirty_checkout_is_refused` deliberately: a guard
    /// proved only by what it blocks is indistinguishable from a guard
    /// that blocks everything.
    #[test]
    fn a_clean_behind_checkout_is_fast_forwarded() {
        let (_tmp, origin, clone) = origin_and_clone();
        advance(&origin);

        let report = update_all_with(
            &[clone.to_string_lossy().into_owned()],
            vec![],
            &never_stop(),
            |_, _| {},
        );

        assert!(
            matches!(only(&report), UpdateResult::Updated { .. }),
            "a clean checkout behind its remote must fast-forward, got {:?}",
            only(&report)
        );
        assert!(clone.join("b.txt").is_file(), "the new commit's file lands");
    }

    /// THE dirty-tree guard (#1013 case 1).
    ///
    /// The tree is left byte-identical, which is the property `--ff-only`
    /// plus a refusal buys and the reason no conflict can arise.
    #[test]
    fn a_dirty_checkout_is_refused() {
        let (_tmp, origin, clone) = origin_and_clone();
        advance(&origin);
        // A TRACKED file modified -- untracked files deliberately do not
        // count (#653), and the next test proves that separately.
        std::fs::write(clone.join("a.txt"), "edited locally\n").expect("write");

        let report = update_all_with(
            &[clone.to_string_lossy().into_owned()],
            vec![],
            &never_stop(),
            |_, _| {},
        );

        match only(&report) {
            UpdateResult::Skipped { reason } => {
                assert!(
                    reason.contains("uncommitted change"),
                    "the refusal must name what is in the way, got {reason:?}"
                );
            }
            other => panic!("a dirty tree must be skipped, got {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(clone.join("a.txt")).expect("read"),
            "edited locally\n",
            "the working tree must be byte-identical after a refusal"
        );
        assert!(
            !clone.join("b.txt").exists(),
            "nothing may be pulled into a dirty tree"
        );
    }

    /// The pair to the dirty test: one untracked file must not make the
    /// button silent (#653). That regression shipped once already.
    #[test]
    fn an_untracked_file_does_not_block_the_update() {
        let (_tmp, origin, clone) = origin_and_clone();
        advance(&origin);
        std::fs::write(clone.join("scratch.md"), "notes\n").expect("write");

        let report = update_all_with(
            &[clone.to_string_lossy().into_owned()],
            vec![],
            &never_stop(),
            |_, _| {},
        );

        assert!(
            matches!(only(&report), UpdateResult::Updated { .. }),
            "an untracked file must not block a fast-forward, got {:?}",
            only(&report)
        );
        assert!(
            clone.join("scratch.md").is_file(),
            "and the untracked file survives"
        );
    }

    /// A detached HEAD is skipped and, critically, still detached
    /// afterwards (#1013 case 2). Checking out the default branch can
    /// strand bisect or rebase commits where only the reflog knows.
    #[test]
    fn a_detached_head_is_refused_and_stays_detached() {
        let (_tmp, origin, clone) = origin_and_clone();
        advance(&origin);
        run(&clone, &["checkout", "--quiet", "--detach", "HEAD"]);
        let before = Command::new(crate::auth::git_program())
            .args(["rev-parse", "HEAD"])
            .current_dir(&clone)
            .output()
            .expect("git");

        let report = update_all_with(
            &[clone.to_string_lossy().into_owned()],
            vec![],
            &never_stop(),
            |_, _| {},
        );

        match only(&report) {
            UpdateResult::Skipped { reason } => assert!(
                reason.contains("detached"),
                "the reason must say detached, got {reason:?}"
            ),
            other => panic!("a detached HEAD must be skipped, got {other:?}"),
        }
        assert!(
            Command::new(crate::auth::git_program())
                .args(["symbolic-ref", "--quiet", "HEAD"])
                .current_dir(&clone)
                .output()
                .expect("git")
                .status
                .code()
                != Some(0),
            "it must still be detached -- never helpfully checked out"
        );
        let after = Command::new(crate::auth::git_program())
            .args(["rev-parse", "HEAD"])
            .current_dir(&clone)
            .output()
            .expect("git");
        assert_eq!(before.stdout, after.stdout, "HEAD must not have moved");
    }

    /// A feature branch is SKIPPED and not checked out (#1013 case 3).
    /// The largest expected bucket, and it must not read as an error.
    #[test]
    fn a_feature_branch_is_skipped_and_stays_checked_out() {
        let (_tmp, origin, clone) = origin_and_clone();
        advance(&origin);
        run(&clone, &["checkout", "--quiet", "-b", "feature/x"]);

        let report = update_all_with(
            &[clone.to_string_lossy().into_owned()],
            vec![],
            &never_stop(),
            |_, _| {},
        );

        match only(&report) {
            UpdateResult::Skipped { reason } => {
                assert!(
                    reason.contains("feature/x") && reason.contains("default"),
                    "the reason must name the branch and the default, got {reason:?}"
                );
            }
            other => panic!("a feature branch must be skipped, got {other:?}"),
        }
        let head = Command::new(crate::auth::git_program())
            .args(["symbolic-ref", "--short", "HEAD"])
            .current_dir(&clone)
            .output()
            .expect("git");
        assert_eq!(
            String::from_utf8_lossy(&head.stdout).trim(),
            "feature/x",
            "the user's working position must not be relocated"
        );
    }

    /// Ahead and diverged are told apart, which `--ff-only` alone cannot
    /// do (#1013 case 5). Both are skips, and neither is a failure.
    #[test]
    fn ahead_and_diverged_are_distinct_skips() {
        let (_tmp, _origin, clone) = origin_and_clone();
        std::fs::write(clone.join("local.txt"), "mine\n").expect("write");
        run(&clone, &["add", "."]);
        run(&clone, &["commit", "-qm", "local"]);

        let ahead = update_all_with(
            &[clone.to_string_lossy().into_owned()],
            vec![],
            &never_stop(),
            |_, _| {},
        );
        match only(&ahead) {
            UpdateResult::Skipped { reason } => assert!(
                reason.contains("ahead") && reason.contains("nothing to pull"),
                "purely ahead must say so, got {reason:?}"
            ),
            other => panic!("ahead must be a skip, got {other:?}"),
        }

        // Now move the remote too, so the two have genuinely diverged.
        advance(&_origin);
        run(&clone, &["fetch", "--quiet"]);
        let diverged = update_all_with(
            &[clone.to_string_lossy().into_owned()],
            vec![],
            &never_stop(),
            |_, _| {},
        );
        match only(&diverged) {
            UpdateResult::Skipped { reason } => assert!(
                reason.contains("diverged") && reason.contains("cannot fast-forward"),
                "diverged must be its own message, got {reason:?}"
            ),
            other => panic!("diverged must be a skip, got {other:?}"),
        }
    }

    /// A repository already level reports `AlreadyLevel`, which is a
    /// SUCCESS and must never land in a failure list (#1014).
    #[test]
    fn a_level_repository_reports_already_level() {
        let (_tmp, _origin, clone) = origin_and_clone();

        let report = update_all_with(
            &[clone.to_string_lossy().into_owned()],
            vec![],
            &never_stop(),
            |_, _| {},
        );

        assert_eq!(
            only(&report),
            &UpdateResult::AlreadyLevel,
            "nothing to do is not a failure"
        );
    }

    /// THE four-outcomes test (#1014). Four repositories in one run, each
    /// landing in a different state, and the report keeps them apart.
    ///
    /// This is the test that fails if anything collapses the enum into a
    /// count or an `Option<String>`.
    #[test]
    fn the_outcomes_stay_distinct_rather_than_collapsing_to_a_count() {
        let tmp = tempfile::tempdir().expect("a temp dir");

        // 1. Updated.
        let (_a, origin_a, updated) = origin_and_clone();
        advance(&origin_a);
        // 2. Already level.
        let (_b, _origin_b, level) = origin_and_clone();
        // 3. Skipped -- dirty.
        let (_c, origin_c, skipped) = origin_and_clone();
        advance(&origin_c);
        std::fs::write(skipped.join("a.txt"), "edited\n").expect("write");
        // 4. Failed -- the directory is not there.
        let missing = tmp.path().join("gone");

        let paths = vec![
            updated.to_string_lossy().into_owned(),
            level.to_string_lossy().into_owned(),
            skipped.to_string_lossy().into_owned(),
            missing.to_string_lossy().into_owned(),
        ];
        let report = update_all_with(&paths, vec![], &never_stop(), |_, _| {});

        assert_eq!(
            report.outcomes.len(),
            paths.len(),
            "every input produces exactly one outcome"
        );
        assert!(
            matches!(report.outcomes[0].result, UpdateResult::Updated { .. }),
            "got {:?}",
            report.outcomes[0].result
        );
        assert_eq!(report.outcomes[1].result, UpdateResult::AlreadyLevel);
        assert!(
            matches!(report.outcomes[2].result, UpdateResult::Skipped { .. }),
            "got {:?}",
            report.outcomes[2].result
        );
        assert!(
            matches!(report.outcomes[3].result, UpdateResult::Failed { .. }),
            "got {:?}",
            report.outcomes[3].result
        );

        // The distinction an aggregate destroys: skipped is NOT failed,
        // and already-level is NOT a failure.
        let failed = report
            .outcomes
            .iter()
            .filter(|o| matches!(o.result, UpdateResult::Failed { .. }))
            .count();
        assert_eq!(
            failed, 1,
            "exactly one row needs attention -- a skip and a level repo are not failures"
        );
        // And the paths are carried, so the user knows where to go.
        assert_eq!(report.outcomes[3].path, paths[3]);
    }

    /// THE cancellation test (#1016). The flag is set after the first
    /// repository, and the second is never touched.
    ///
    /// Proven by the FILESYSTEM, not only by the report: the second
    /// clone's incoming file must not exist. A report that said
    /// `NotAttempted` while the pull had actually run would pass a
    /// report-only assertion.
    #[test]
    fn cancellation_stops_the_run_and_still_reports() {
        let (_a, origin_a, first) = origin_and_clone();
        advance(&origin_a);
        let (_b, origin_b, second) = origin_and_clone();
        advance(&origin_b);

        let stop = never_stop();
        let flag = stop.clone();
        let paths = vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ];
        let report = update_all_with(&paths, vec![], &stop, move |done, _| {
            // Cancelled from OUTSIDE, between repositories, exactly as
            // the command's stop flag is set by `cancel`.
            if done == 1 {
                flag.store(true, Ordering::SeqCst);
            }
        });

        assert!(report.cancelled, "the report must say it was cancelled");
        assert!(
            matches!(report.outcomes[0].result, UpdateResult::Updated { .. }),
            "work done before the stop still counts, got {:?}",
            report.outcomes[0].result
        );
        assert_eq!(
            report.outcomes[1].result,
            UpdateResult::NotAttempted,
            "the repository after the stop is not attempted"
        );
        assert!(
            !second.join("b.txt").exists(),
            "the second repository must not have been pulled at all"
        );
        assert_eq!(
            report.outcomes.len(),
            2,
            "a cancelled run still returns an outcome per input"
        );
    }

    /// Cancellation must not be a permanent brake: an uncancelled run
    /// over the same two repositories updates both.
    ///
    /// The pair to the test above, so "stops the run" cannot be satisfied
    /// by a loop that never runs.
    #[test]
    fn an_uncancelled_run_updates_every_repository() {
        let (_a, origin_a, first) = origin_and_clone();
        advance(&origin_a);
        let (_b, origin_b, second) = origin_and_clone();
        advance(&origin_b);

        let report = update_all_with(
            &[
                first.to_string_lossy().into_owned(),
                second.to_string_lossy().into_owned(),
            ],
            vec![],
            &never_stop(),
            |_, _| {},
        );

        assert!(!report.cancelled);
        assert!(!report.timed_out);
        assert!(first.join("b.txt").is_file(), "the first is pulled");
        assert!(second.join("b.txt").is_file(), "the second is pulled");
    }

    /// Progress counts AFTER each repository, including the ones that
    /// were skipped -- a batch where several are skipped would otherwise
    /// appear to stall.
    #[test]
    fn progress_counts_every_repository_including_skips() {
        let (_a, origin_a, updated) = origin_and_clone();
        advance(&origin_a);
        let (_b, _origin_b, skipped) = origin_and_clone();
        run(&skipped, &["checkout", "--quiet", "-b", "feature/y"]);

        let mut frames = Vec::new();
        update_all_with(
            &[
                updated.to_string_lossy().into_owned(),
                skipped.to_string_lossy().into_owned(),
            ],
            vec![],
            &never_stop(),
            |done, total| frames.push((done, total)),
        );

        assert_eq!(
            frames,
            vec![(1, 2), (2, 2)],
            "one frame per repository, counting DONE not started"
        );
    }

    /// The scan's shortfall reaches the report (#1025), and is never
    /// counted as a repository outcome.
    #[test]
    fn the_scans_shortfall_is_carried_and_not_counted_as_a_repository() {
        let (_tmp, _origin, clone) = origin_and_clone();

        let report = update_all_with(
            &[clone.to_string_lossy().into_owned()],
            vec!["/work: permission denied".into()],
            &never_stop(),
            |_, _| {},
        );

        assert!(report.is_partial(), "a short census must say so");
        assert_eq!(report.unreadable, vec!["/work: permission denied"]);
        assert_eq!(
            report.outcomes.len(),
            1,
            "an unreadable directory is a hole in the list, not a row in it"
        );
    }

    /// An empty input is an empty report, not a panic and not a claim.
    #[test]
    fn no_repositories_is_an_empty_report() {
        let report = update_all_with(&[], vec![], &never_stop(), |_, _| {});
        assert!(report.outcomes.is_empty());
        assert!(!report.cancelled);
        assert!(!report.is_partial());
    }

    /// Nothing on this path may reach for stash, reset, rebase, merge,
    /// clean or `checkout -f` (#1012's last constraint, which the issue
    /// asks for as a test rather than a convention).
    ///
    /// A source-level assertion, because the behavioural tests above can
    /// only prove the cases they construct: this catches a future edit
    /// that adds an escape hatch for a case nobody wrote a test for.
    #[test]
    fn this_module_never_reaches_for_a_destructive_git_verb() {
        let src = include_str!("update.rs");
        // The PRODUCTION code only -- everything above `mod tests`, and
        // with the comments stripped. Both exclusions are load-bearing:
        // the doc comments legitimately NAME these verbs in order to say
        // they are refused, and this test's own list of forbidden strings
        // would otherwise match itself.
        let production = src
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .expect("this module has a test module");
        let code = production
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                !t.starts_with("//") && !t.starts_with("///") && !t.starts_with("//!")
            })
            .collect::<String>();
        for verb in [
            "\"stash\"",
            "\"reset\"",
            "\"rebase\"",
            "\"merge\"",
            "\"clean\"",
            "--force",
            "--hard",
        ] {
            assert!(
                !code.contains(verb),
                "{verb} must not appear in this module: the refusal set is the feature"
            );
        }
        // And the one verb that must: the pull goes through
        // `pull_checkout`, never a second spelling of it.
        assert!(
            code.contains("pull_checkout(path)"),
            "the update must go through pull_checkout"
        );
        assert!(
            !code.contains("\"pull\""),
            "no second `git pull` invocation may be spelled here"
        );
    }
}

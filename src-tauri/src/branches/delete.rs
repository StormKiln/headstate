//! Deleting branches, local and remote.
//!
//! The contract `remove_worktree` established, kept here: the safety
//! gate is RE-EVALUATED at delete time. What the UI last saw may be
//! minutes old and a branch can be merged, checked out, or advanced in
//! between -- so the list the user ticked is a list of names, never a
//! list of permissions.
//!
//! # Two phases, reported separately (#724)
//!
//! A deletion is a re-check and then a deletion, and the re-check is
//! the slow half: it is a full uncached [`scan::scan`] of the
//! repository, ~64ms per branch, so 562 selected branches spent
//! MINUTES in the gate before the first ref was touched. Reported as
//! one counter, that reads as broken -- the number would sit at 0/562
//! through the longest part of the wait, which is exactly the "ten
//! minutes with no progress" this module was reported for.
//!
//! So [`DeleteProgress`] has a call per phase. `checking` counts
//! branches classified by the re-check scan; `deleted` counts refs
//! actually removed, and carries the failures so far, because a batch
//! where thirty branches were refused must say so while it runs rather
//! than in a burst of toasts at the end.

use std::path::Path;

use super::model::{Branch, Deletable};
use super::scan;
use crate::worktrees::scan::git;

/// What a caller wants told while a deletion runs.
///
/// Three calls, in this order: `checking` repeatedly while the safety
/// re-check scans the repository, then `deleted` once per branch as
/// refs come off. `failed` is reported through `deleted`'s count
/// rather than separately so a consumer cannot render a total that
/// disagrees with itself.
///
/// A trait rather than closures for the same reason [`scan::Progress`]
/// is one: the re-check phase delegates to `scan_with_progress`, whose
/// sink is shared across eight classification threads and so must be
/// `Sync`. `checking` must therefore tolerate arriving from several
/// threads at once, out of order.
pub trait DeleteProgress: Sync {
    /// The re-check scan classified `done` of `total` branches.
    ///
    /// `total` is every branch in the REPOSITORY, not the batch: the
    /// gate scans the whole repository once, and saying "12 of 562
    /// selected" while scanning 900 would be a count of the wrong
    /// thing. The phase label is what tells the user which is which.
    fn checking(&self, done: usize, total: usize);
    /// `done` of `total` selected branches have been attempted, of
    /// which `failed` were refused or errored.
    ///
    /// Called AFTER each attempt, so the count means "done" and not
    /// "started" -- the same rule `remove_worktrees_with_progress`
    /// follows. `total` here IS the batch.
    fn deleted(&self, done: usize, total: usize, failed: usize);
}

/// The sink for callers that want only the answer.
pub struct SilentDelete;

impl DeleteProgress for SilentDelete {
    fn checking(&self, _: usize, _: usize) {}
    fn deleted(&self, _: usize, _: usize, _: usize) {}
}

/// Bridges the scan's own progress into the deletion's checking phase.
///
/// The re-check IS a scan, so this reuses `scan_with_progress` rather
/// than inventing a second way to count the same work (#723's
/// mechanism, one operation, one protocol). The scan reports a listing
/// then batches of verdicts; the deletion only needs how far through
/// it is, so that is all this forwards.
struct CheckingPhase<'a> {
    to: &'a dyn DeleteProgress,
    done: std::sync::atomic::AtomicUsize,
    /// Remembered from the listing so `classified` can repeat it: the
    /// scan tells a sink the total once and the verdicts thereafter,
    /// but every frame this forwards has to carry both numbers or the
    /// page has nothing to render a fraction against.
    total: std::sync::atomic::AtomicUsize,
}

impl scan::Progress for CheckingPhase<'_> {
    fn listed(&self, branches: &[Branch]) {
        self.total
            .store(branches.len(), std::sync::atomic::Ordering::Relaxed);
        // Emitted at 0/N rather than withheld: this is the frame that
        // lets the page say "Checking 562 branches" instead of showing
        // nothing for the minutes that follow.
        self.to.checking(0, branches.len());
    }

    fn classified(&self, verdicts: &[(String, Deletable)]) {
        // Relaxed: the eight classification threads need this counter
        // correct in total, and no other memory is published through
        // it.
        let done = self
            .done
            .fetch_add(verdicts.len(), std::sync::atomic::Ordering::Relaxed)
            + verdicts.len();
        self.to
            .checking(done, self.total.load(std::sync::atomic::Ordering::Relaxed));
    }
}

/// What happened to one branch.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteOutcome {
    pub name: String,
    /// `None` on success; the reason otherwise. A refusal and a git
    /// failure are both errors here because both leave the branch in
    /// place -- the caller does not have to tell them apart to be
    /// correct, and the message says which.
    pub error: Option<String>,
}

/// Re-check that this branch is still deletable, right now.
/// Re-check one branch against a scan taken for THIS batch.
///
/// The scan is passed in rather than taken here. It used to call
/// `scan::scan` per branch -- ~9.4s on a 507-branch repository -- so a
/// ten-branch deletion spent a minute and a half in git before saying
/// anything (#492).
///
/// Still a delete-time check: the scan is taken when the batch starts,
/// not when the list was rendered, so a branch that changed since the
/// user looked is still caught.
fn still_deletable(branches: &[Branch], name: &str) -> Result<(), String> {
    let Some(b) = branches.iter().find(|b| b.name == name) else {
        // ABSENT, not undeletable.
        //
        // A local delete in the same batch removes the branch from this
        // scan, and the remote half then could not find it and reported
        // "no longer exists" -- about a LOCAL ref, for a remote
        // deletion the user had asked for and which would have
        // succeeded. That was the whole #492 symptom.
        //
        // Nothing here established that the ref is undeletable, so the
        // caller proceeds and git gives the real answer.
        return Ok(());
    };
    match &b.deletable {
        Deletable::Merged { .. } => Ok(()),
        Deletable::DefaultBranch => Err(format!("{name} is the default branch")),
        Deletable::CheckedOut { path } => Err(format!("{name} is checked out in {path}")),
        // The refusal does not depend on the count -- `Unmerged` is the
        // refusal -- but the REASON must not fabricate one (#967). "0
        // commit(s) are not on the default branch" beside a refusal is a
        // self-contradiction the user reads as the app being broken, and
        // on a git older than 2.41 it was every unmerged branch.
        Deletable::Unmerged { ahead: Some(n) } => Err(format!(
            "{name} is not merged: {n} commit(s) are not on the default branch"
        )),
        Deletable::Unmerged { ahead: None } => Err(format!(
            "{name} is not merged, and the commit count could not be read \
             (git 2.41 or newer is needed to count)"
        )),
        Deletable::Pending => Err(format!("{name} has not been checked yet")),
        Deletable::Unknown { reason } => Err(format!("{name}: {reason}")),
    }
}

/// Delete local branches that are still provably merged.
///
/// Uses `-D`, and the gate above is the reason that is safe.
///
/// `-d` was the first choice, as a second opinion from git. It does not
/// work here: git's own check is ANCESTRY, and a squash merge is never
/// an ancestor. Measured on a real repository, 489 of 536 merged
/// branches were squashes -- `-d` refuses all of them, so the feature
/// would refuse to delete 91% of what it correctly identified as
/// deletable.
///
/// So the patch-id gate in `still_deletable` is not a convenience in
/// front of git's check; it IS the check, and it is strictly stronger
/// than `-d` on this workflow. It is re-run immediately before the
/// delete, against the repository as it stands, never against what the
/// UI last displayed.
pub fn delete_local(repo_path: &str, names: &[String]) -> Vec<DeleteOutcome> {
    delete_local_with_progress(repo_path, names, &SilentDelete)
}

/// The re-check every deletion opens with, reporting as it scans.
///
/// Factored out of both delete functions because the phase boundary
/// has to be identical in each: the local and remote halves of one
/// "delete in both places" run back to back, and a user watching two
/// differently-shaped progress reports for one action learns nothing
/// from either.
///
/// `Err` carries the already-refused outcomes, so the caller returns
/// them unchanged: a scan that failed is not permission to delete.
fn recheck(
    dir: &Path,
    names: &[String],
    progress: &dyn DeleteProgress,
) -> Result<Vec<Branch>, Vec<DeleteOutcome>> {
    let phase = CheckingPhase {
        to: progress,
        done: std::sync::atomic::AtomicUsize::new(0),
        total: std::sync::atomic::AtomicUsize::new(0),
    };
    // ONE scan for the whole batch, not one per branch, and `scan`
    // rather than `scan_cached`: the gate runs against the repository
    // as it stands, never against what the UI last displayed. The
    // progress sink observes that scan; it does not replace it.
    scan::scan_with_progress(dir, &phase).map_err(|e| {
        names
            .iter()
            .map(|name| DeleteOutcome {
                name: name.clone(),
                error: Some(format!("could not re-check branches before deleting: {e}")),
            })
            .collect()
    })
}

/// Delete one branch per call, counting attempts and failures.
///
/// The failure count goes out with every frame rather than only at the
/// end. Thirty refusals in a batch of 562 is something the user wants
/// to know while there is still a run to abandon, not after it.
fn delete_each(
    names: &[String],
    progress: &dyn DeleteProgress,
    mut delete_one: impl FnMut(&str) -> Option<String>,
) -> Vec<DeleteOutcome> {
    let total = names.len();
    let mut failed = 0;
    names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let error = delete_one(name);
            if error.is_some() {
                failed += 1;
            }
            // AFTER the attempt, so the count means "done".
            progress.deleted(i + 1, total, failed);
            DeleteOutcome {
                name: name.clone(),
                error,
            }
        })
        .collect()
}

/// `delete_local`, telling `progress` which phase it is in and how far
/// through that phase it has got (#724).
pub fn delete_local_with_progress(
    repo_path: &str,
    names: &[String],
    progress: &dyn DeleteProgress,
) -> Vec<DeleteOutcome> {
    let dir = Path::new(repo_path);
    let branches = match recheck(dir, names, progress) {
        Ok(b) => b,
        Err(refused) => return refused,
    };
    delete_each(names, progress, |name| {
        still_deletable(&branches, name)
            .err()
            .or_else(|| git(dir, &["branch", "-D", name]).err())
    })
}

/// Delete branches on the remote.
///
/// A push to shared state: everyone else loses the ref too, and there
/// is no reflog on the other side to recover it from. Separate from
/// `delete_local` so it cannot be reached by the same click, and it
/// still re-checks the merge gate first -- being remote does not make
/// the branch any more disposable.
pub fn delete_remote(repo_path: &str, names: &[String]) -> Vec<DeleteOutcome> {
    delete_remote_with_progress(repo_path, names, &SilentDelete)
}

/// `delete_remote`, reporting its phases the same way (#724).
///
/// The remote half is if anything the one that needs it more: each
/// deletion is a network round trip, so the second phase is slow here
/// too, not just the re-check.
pub fn delete_remote_with_progress(
    repo_path: &str,
    names: &[String],
    progress: &dyn DeleteProgress,
) -> Vec<DeleteOutcome> {
    let dir = Path::new(repo_path);
    let branches = match recheck(dir, names, progress) {
        Ok(b) => b,
        Err(refused) => return refused,
    };
    delete_each(names, progress, |name| {
        // `origin/feature` names a remote-tracking ref; the push needs
        // the remote and the branch separately.
        match name.split_once('/') {
            None => Some(format!(
                "{name} does not name a remote branch (expected <remote>/<branch>)"
            )),
            Some((remote, branch)) => still_deletable(&branches, name)
                .err()
                .or_else(|| git(dir, &["push", remote, "--delete", branch]).err()),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const IDENT: [(&str, &str); 4] = [
        ("GIT_AUTHOR_NAME", "octocat"),
        ("GIT_COMMITTER_NAME", "octocat"),
        ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
        ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
    ];

    /// Retries the SPAWN, not the command.
    ///
    /// macOS `posix_spawn` intermittently returns ENOENT under process
    /// load -- the same failure #447 fixed in `is_ignored`. These
    /// fixtures each run a dozen git commands and the suite runs them
    /// concurrently, so a single attempt made three tests fail in the
    /// full run while passing when filtered. A git that RAN is
    /// believed, whatever it said.
    fn run(dir: &Path, args: &[&str]) {
        let mut last = None;
        for _ in 0..3 {
            match Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(IDENT)
                .output()
            {
                Ok(out) => {
                    assert!(
                        out.status.success(),
                        "git {args:?}: {}",
                        String::from_utf8_lossy(&out.stderr)
                    );
                    return;
                }
                Err(e) => last = Some(e),
            }
        }
        panic!("could not spawn git {args:?}: {last:?}");
    }

    fn commit(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), name).unwrap();
        run(dir, &["add", "-A"]);
        run(dir, &["commit", "-q", "-m", name]);
    }

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let remote = tmp.path().join("remote.git");
        std::fs::create_dir_all(&remote).unwrap();
        run(&remote, &["init", "-q", "--bare", "-b", "main"]);
        let repo = tmp.path().join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        commit(&repo, "base");
        run(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run(&repo, &["push", "-q", "-u", "origin", "main"]);
        run(&repo, &["remote", "set-head", "origin", "main"]);
        (tmp, repo)
    }

    /// Squash-merge `branch` into main so it is genuinely deletable.
    fn squash_merge(repo: &Path, branch: &str) {
        run(repo, &["checkout", "-q", "main"]);
        run(repo, &["merge", "-q", "--squash", branch]);
        run(repo, &["commit", "-q", "-m", &format!("squashed {branch}")]);
        run(repo, &["push", "-q", "origin", "main"]);
    }

    /// A branch checked out in a worktree must stop reading as
    /// deletable, even though checking it out moves no ref.
    ///
    /// This is the case the cache key was originally wrong about. The
    /// first version keyed on `for-each-ref` alone, and `git worktree
    /// add` leaves that output byte-identical -- so the cache called
    /// the state unchanged and kept serving `Merged` for a branch git
    /// would now refuse to delete. The listing would offer an action
    /// that could only fail.
    ///
    /// The key now includes `worktree list --porcelain`, which is what
    /// `checked_out` itself parses.
    #[test]
    fn checking_a_branch_out_invalidates_its_cached_verdict() {
        let (tmp, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "feature"]);
        commit(&repo, "work");
        squash_merge(&repo, "feature");

        let before = crate::branches::scan_cached(&repo).unwrap();
        let f = before.iter().find(|b| b.name == "feature").unwrap();
        assert!(
            matches!(f.deletable, Deletable::Merged { .. }),
            "fixture wrong: feature should read as merged first, got {:?}",
            f.deletable
        );

        // Moves no ref, changes the answer.
        let wt = tmp.path().join("live-wt");
        run(
            &repo,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "feature"],
        );

        let after = crate::branches::scan_cached(&repo).unwrap();
        let f = after.iter().find(|b| b.name == "feature").unwrap();
        assert!(
            matches!(f.deletable, Deletable::CheckedOut { .. }),
            "the cache served a stale merged verdict for a branch that is now checked out; got {:?}",
            f.deletable
        );
    }

    /// A new commit moves the tip, so the cached verdict must go.
    ///
    /// The plainer half of the same property, and the one a refs-only
    /// key already got right -- kept so a future change to the key
    /// cannot quietly lose it.
    #[test]
    fn the_listing_cache_notices_a_new_commit() {
        let (_tmp, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "feature"]);
        commit(&repo, "work");
        squash_merge(&repo, "feature");

        let before = crate::branches::scan_cached(&repo).unwrap();
        assert!(before
            .iter()
            .any(|b| b.name == "feature" && matches!(b.deletable, Deletable::Merged { .. })));

        run(&repo, &["checkout", "-q", "feature"]);
        commit(&repo, "more work");
        run(&repo, &["checkout", "-q", "main"]);

        let after = crate::branches::scan_cached(&repo).unwrap();
        let feature = after.iter().find(|b| b.name == "feature").unwrap();
        assert!(
            !matches!(feature.deletable, Deletable::Merged { .. }),
            "the cache served a merged verdict after the branch tip moved"
        );
    }

    /// Deletion re-checks against an UNCACHED scan.
    ///
    /// `delete_local`'s doc comment states the property that makes its
    /// gate trustworthy: the check runs against the repository as it
    /// stands, never against what the UI last displayed. A cached
    /// answer is precisely what the UI last displayed, so this asserts
    /// the delete path stays on `scan` even as the listing moves to
    /// `scan_cached`.
    #[test]
    fn a_delete_refuses_work_that_landed_after_the_listing() {
        let (_tmp, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "feature"]);
        commit(&repo, "work");
        squash_merge(&repo, "feature");

        // The listing caches "deletable".
        let _ = crate::branches::scan_cached(&repo).unwrap();

        // New work lands.
        run(&repo, &["checkout", "-q", "feature"]);
        commit(&repo, "work that is not merged anywhere");
        run(&repo, &["checkout", "-q", "main"]);

        let out = delete_local(repo.to_str().unwrap(), &["feature".to_string()]);
        assert_eq!(out.len(), 1);
        assert!(
            out[0].error.is_some(),
            "the delete was allowed after new work landed; that work would be gone"
        );
        assert!(
            branch_exists(&repo, "feature"),
            "the branch was deleted despite carrying unmerged work"
        );
    }

    fn branch_exists(repo: &Path, name: &str) -> bool {
        Command::new(crate::auth::git_program())
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", "--verify", &format!("refs/heads/{name}")])
            .output()
            .unwrap()
            .status
            .success()
    }

    #[test]
    fn a_merged_branch_is_deleted() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "done"]);
        commit(&repo, "work");
        squash_merge(&repo, "done");

        let out = delete_local(repo.to_str().unwrap(), &["done".to_string()]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].error, None, "{:?}", out[0]);
        assert!(!branch_exists(&repo, "done"));
    }

    /// The gate that matters. Deleting an unmerged branch loses commits
    /// that exist nowhere else.
    #[test]
    fn an_unmerged_branch_is_refused_and_survives() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "wip"]);
        commit(&repo, "unshared-work");
        run(&repo, &["checkout", "-q", "main"]);

        let out = delete_local(repo.to_str().unwrap(), &["wip".to_string()]);
        assert!(out[0].error.is_some(), "an unmerged branch must be refused");
        assert!(
            branch_exists(&repo, "wip"),
            "a refused deletion must leave the branch in place"
        );
    }

    /// Asserts on OUR reason, not merely that it failed.
    ///
    /// Both this and the checked-out case are refused by git anyway --
    /// which is exactly the trap: a test that only checks `is_some()`
    /// passes with the gate deleted, because git's own refusal fills
    /// in. Mutation testing caught both. Since local deletion runs
    /// `-D`, git's refusal is not a backstop we may lean on, so the
    /// assertion names the gate's own message.
    #[test]
    fn the_default_branch_is_refused_by_our_gate() {
        let (_t, repo) = fixture();
        let out = delete_local(repo.to_str().unwrap(), &["main".to_string()]);
        let err = out[0].error.as_deref().unwrap_or("");
        assert!(
            err.contains("is the default branch"),
            "expected the gate's own refusal, got: {err}"
        );
        assert!(branch_exists(&repo, "main"));
    }

    /// THE contract from `remove_worktree`: the gate is re-evaluated
    /// now, not trusted from whatever the UI last rendered. Here the
    /// branch is merged when listed and has new work by delete time.
    #[test]
    fn the_gate_is_re_evaluated_at_delete_time_not_taken_from_the_ui() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "moving"]);
        commit(&repo, "first");
        squash_merge(&repo, "moving");

        // What the UI would have seen: deletable.
        let listed = scan::scan(&repo).unwrap();
        let seen = listed.iter().find(|b| b.name == "moving").unwrap();
        assert!(seen.deletable.is_deletable());

        // Now the branch advances, exactly as it would if the user had
        // committed to it in another window.
        run(&repo, &["checkout", "-q", "moving"]);
        commit(&repo, "second");
        run(&repo, &["checkout", "-q", "main"]);

        let out = delete_local(repo.to_str().unwrap(), &["moving".to_string()]);
        assert!(
            out[0].error.is_some(),
            "a branch that gained commits after listing must be refused"
        );
        assert!(branch_exists(&repo, "moving"));
    }

    #[test]
    fn a_branch_checked_out_in_a_worktree_is_refused() {
        let (tmp, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "live"]);
        commit(&repo, "live-work");
        squash_merge(&repo, "live");
        let wt = tmp.path().join("live-wt");
        run(
            &repo,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "live"],
        );

        let out = delete_local(repo.to_str().unwrap(), &["live".to_string()]);
        let err = out[0].error.as_deref().unwrap_or("");
        assert!(
            err.contains("is checked out in"),
            "expected the gate's own refusal, got: {err}"
        );
        assert!(branch_exists(&repo, "live"));
    }

    /// One bad name must not stop the rest: a bulk deletion that
    /// aborted halfway would leave the user guessing what landed.
    #[test]
    fn one_refusal_does_not_abort_the_others() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "ok-one"]);
        commit(&repo, "a");
        squash_merge(&repo, "ok-one");
        run(&repo, &["checkout", "-q", "-b", "nope"]);
        commit(&repo, "b");
        run(&repo, &["checkout", "-q", "-b", "ok-two", "main"]);
        commit(&repo, "c");
        squash_merge(&repo, "ok-two");

        let out = delete_local(
            repo.to_str().unwrap(),
            &["ok-one".into(), "nope".into(), "ok-two".into()],
        );
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].error, None);
        assert!(out[1].error.is_some());
        assert_eq!(out[2].error, None, "{:?}", out[2]);
        assert!(!branch_exists(&repo, "ok-one"));
        assert!(branch_exists(&repo, "nope"));
        assert!(!branch_exists(&repo, "ok-two"));
    }

    #[test]
    fn a_merged_remote_branch_is_deleted_from_the_remote() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "shipped"]);
        commit(&repo, "shipped-work");
        run(&repo, &["push", "-q", "origin", "shipped"]);
        squash_merge(&repo, "shipped");
        run(&repo, &["branch", "-q", "-D", "shipped"]);

        let out = delete_remote(repo.to_str().unwrap(), &["origin/shipped".to_string()]);
        assert_eq!(out[0].error, None, "{:?}", out[0]);

        let refs = Command::new(crate::auth::git_program())
            .arg("-C")
            .arg(&repo)
            .args(["ls-remote", "--heads", "origin", "shipped"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&refs.stdout).trim().is_empty(),
            "the branch must be gone from the remote"
        );
    }

    /// Being remote is not a reason to skip the gate: a push deletion
    /// cannot be undone from a local reflog.
    #[test]
    fn an_unmerged_remote_branch_is_refused() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "live-remote"]);
        commit(&repo, "remote-work");
        run(&repo, &["push", "-q", "origin", "live-remote"]);
        run(&repo, &["checkout", "-q", "main"]);
        run(&repo, &["branch", "-q", "-D", "live-remote"]);

        let out = delete_remote(repo.to_str().unwrap(), &["origin/live-remote".to_string()]);
        assert!(out[0].error.is_some(), "unmerged remote must be refused");

        let refs = Command::new(crate::auth::git_program())
            .arg("-C")
            .arg(&repo)
            .args(["ls-remote", "--heads", "origin", "live-remote"])
            .output()
            .unwrap();
        assert!(!String::from_utf8_lossy(&refs.stdout).trim().is_empty());
    }

    /// #492: a branch the scan cannot see is not thereby undeletable.
    ///
    /// Deleting a tracked branch in both places used to run the two
    /// halves concurrently. The local half removed the ref, the remote
    /// half's gate then failed to find the branch, and the user got
    /// "no longer exists" for a REMOTE deletion that would otherwise
    /// have succeeded -- which is exactly why doing them one at a time
    /// worked.
    ///
    /// The gate must not be the thing that refuses. Whatever git says
    /// about a ref that is genuinely gone is git's to say.
    #[test]
    fn a_branch_missing_from_the_scan_is_not_refused_by_the_gate() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "shipped"]);
        commit(&repo, "shipped-work");
        run(&repo, &["push", "-q", "-u", "origin", "shipped"]);
        squash_merge(&repo, "shipped");

        // Both refs gone: the state the racing halves could reach, and
        // the one where the scan has nothing to say about either name.
        run(&repo, &["branch", "-q", "-D", "shipped"]);
        run(&repo, &["push", "-q", "origin", "--delete", "shipped"]);
        run(&repo, &["fetch", "-q", "--prune", "origin"]);

        let out = delete_remote(repo.to_str().unwrap(), &["origin/shipped".to_string()]);
        let err = out[0].error.as_deref().unwrap_or("");
        assert!(
            !err.contains("no longer exists"),
            "the gate must not refuse an absent branch; got: {err}"
        );
    }

    /// The gate still refuses what it CAN see is unmergeable. Tolerating
    /// an absent branch must not become tolerating everything.
    #[test]
    fn a_branch_present_and_unmerged_is_still_refused() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "wip"]);
        commit(&repo, "unshared");
        run(&repo, &["checkout", "-q", "main"]);

        let out = delete_local(repo.to_str().unwrap(), &["wip".to_string()]);
        assert!(out[0].error.is_some());
        assert!(branch_exists(&repo, "wip"));
    }

    /// A scan that FAILED is not permission to delete. Every branch is
    /// refused with the reason rather than proceeding blind.
    #[test]
    fn a_failed_scan_refuses_the_whole_batch() {
        let tmp = tempfile::TempDir::new().unwrap();
        let notrepo = tmp.path().join("nope");
        std::fs::create_dir_all(&notrepo).unwrap();

        let out = delete_local(notrepo.to_str().unwrap(), &["anything".to_string()]);
        assert_eq!(out.len(), 1);
        let err = out[0].error.as_deref().unwrap_or("");
        assert!(
            err.contains("could not re-check"),
            "a failed scan must refuse, got: {err}"
        );
    }

    /// A tracked branch deleted in BOTH places.
    ///
    /// #473: the view filed tracked branches under "local" and never
    /// offered the remote half, so the local ref went and the remote
    /// branch stayed -- and, its local ref now gone, it came back in
    /// the list as remote-only. The user believed it was cleaned up.
    ///
    /// The backend already supports this; what it needs is a caller
    /// that passes the branch name to one function and the UPSTREAM
    /// name to the other. This proves that pairing works.
    #[test]
    fn a_tracked_branch_can_be_deleted_in_both_places() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "shipped"]);
        commit(&repo, "shipped-work");
        run(&repo, &["push", "-q", "-u", "origin", "shipped"]);
        squash_merge(&repo, "shipped");

        // As the view sees it: one branch, location Tracked.
        let listed = scan::scan(&repo).unwrap();
        let b = listed.iter().find(|b| b.name == "shipped").unwrap();
        assert_eq!(b.location, super::super::model::Location::Tracked);
        let upstream = b
            .upstream
            .clone()
            .expect("a tracked branch has an upstream");

        let local = delete_local(repo.to_str().unwrap(), &["shipped".to_string()]);
        assert_eq!(local[0].error, None, "{:?}", local[0]);
        let remote = delete_remote(repo.to_str().unwrap(), &[upstream]);
        assert_eq!(remote[0].error, None, "{:?}", remote[0]);

        assert!(!branch_exists(&repo, "shipped"));
        let refs = Command::new(crate::auth::git_program())
            .arg("-C")
            .arg(&repo)
            .args(["ls-remote", "--heads", "origin", "shipped"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&refs.stdout).trim().is_empty(),
            "the remote branch must be gone too -- this is the #473 failure"
        );
    }

    // ---- progress, two phases (#724) --------------------------------

    /// Records what a deletion reported, in order, from any thread.
    ///
    /// Order is the property under test as much as the numbers are: the
    /// bug was a counter that sat at zero through the checking phase,
    /// which is only visible if the phases are told apart and the
    /// sequence is kept.
    #[derive(Default)]
    struct Recorder {
        checking: std::sync::Mutex<Vec<(usize, usize)>>,
        deleting: std::sync::Mutex<Vec<(usize, usize, usize)>>,
        /// Every call in arrival order, tagged by phase, so a
        /// `deleted` that arrived before the checking finished would
        /// be caught rather than averaged away.
        order: std::sync::Mutex<Vec<&'static str>>,
    }

    impl DeleteProgress for Recorder {
        fn checking(&self, done: usize, total: usize) {
            self.checking.lock().unwrap().push((done, total));
            self.order.lock().unwrap().push("checking");
        }
        fn deleted(&self, done: usize, total: usize, failed: usize) {
            self.deleting.lock().unwrap().push((done, total, failed));
            self.order.lock().unwrap().push("deleting");
        }
    }

    /// Build `n` merged branches, all genuinely deletable.
    fn merged_branches(repo: &Path, n: usize) -> Vec<String> {
        (0..n)
            .map(|i| {
                let name = format!("done-{i}");
                run(repo, &["checkout", "-q", "-b", &name, "main"]);
                commit(repo, &format!("work-{i}"));
                squash_merge(repo, &name);
                name
            })
            .collect()
    }

    /// THE bug: the checking phase must report progress of its own.
    ///
    /// The re-check is a full uncached scan and it runs BEFORE any
    /// deletion, so a single counter reads 0/N for the whole of the
    /// slowest part -- on the reported 562-branch batch, minutes of it.
    /// This asserts the phase is reported at all, and that the count
    /// inside it actually moves rather than only announcing itself.
    #[test]
    fn the_checking_phase_reports_before_anything_is_deleted() {
        let (_t, repo) = fixture();
        let names = merged_branches(&repo, 3);

        let rec = Recorder::default();
        let out = delete_local_with_progress(repo.to_str().unwrap(), &names, &rec);
        assert!(out.iter().all(|o| o.error.is_none()), "{out:?}");

        let checking = rec.checking.lock().unwrap().clone();
        assert!(
            !checking.is_empty(),
            "the checking phase reported nothing; a counter that only starts at deletion \
             sits at zero for the whole of the slow half"
        );
        // The total is the REPOSITORY's branches, which includes main
        // and so exceeds the batch. Reporting the batch size here would
        // be a count of the wrong thing.
        let (_, total) = checking[0];
        assert!(
            total >= names.len(),
            "the checking phase must count the branches the gate scans, got {total}"
        );
        let deepest = checking.iter().map(|(d, _)| *d).max().unwrap();
        assert!(
            deepest > 0,
            "the checking count never moved off zero, which is the reported failure"
        );

        // And it is genuinely a PHASE: no deletion is reported until
        // the checking is over.
        let order = rec.order.lock().unwrap().clone();
        let first_delete = order.iter().position(|p| *p == "deleting").unwrap();
        assert!(
            order[..first_delete].iter().all(|p| *p == "checking"),
            "checking and deleting interleaved; the phases must be distinguishable"
        );
        assert!(
            order[first_delete..].iter().all(|p| *p == "deleting"),
            "the checking phase reported after deletion had started"
        );
    }

    /// The deleting phase counts the BATCH, one frame per branch,
    /// after each attempt.
    #[test]
    fn the_deleting_phase_counts_every_branch_in_the_batch() {
        let (_t, repo) = fixture();
        let names = merged_branches(&repo, 3);

        let rec = Recorder::default();
        delete_local_with_progress(repo.to_str().unwrap(), &names, &rec);

        let deleting = rec.deleting.lock().unwrap().clone();
        assert_eq!(
            deleting,
            vec![(1, 3, 0), (2, 3, 0), (3, 3, 0)],
            "expected one frame per branch, counting done out of the batch"
        );
    }

    /// Refusals are visible WHILE it runs, not only in the summary.
    ///
    /// A batch where thirty of 562 are refused should say so with five
    /// hundred still to go. The count rides on every frame from the
    /// failure onwards, so it cannot be missed by a page that rendered
    /// between two of them.
    #[test]
    fn a_refusal_mid_batch_is_reported_as_it_happens() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "ok-one"]);
        commit(&repo, "a");
        squash_merge(&repo, "ok-one");
        // Unmerged: the gate refuses it, and it sits in the MIDDLE so
        // the failure has to surface before the batch is over.
        run(&repo, &["checkout", "-q", "-b", "nope", "main"]);
        commit(&repo, "b");
        run(&repo, &["checkout", "-q", "-b", "ok-two", "main"]);
        commit(&repo, "c");
        squash_merge(&repo, "ok-two");

        let rec = Recorder::default();
        let names = vec![
            "ok-one".to_string(),
            "nope".to_string(),
            "ok-two".to_string(),
        ];
        let out = delete_local_with_progress(repo.to_str().unwrap(), &names, &rec);
        assert!(
            out[1].error.is_some(),
            "fixture wrong: nope must be refused"
        );

        let deleting = rec.deleting.lock().unwrap().clone();
        assert_eq!(deleting[0], (1, 3, 0));
        assert_eq!(
            deleting[1],
            (2, 3, 1),
            "the refusal must be reported on the frame it happened on"
        );
        assert_eq!(
            deleting[2],
            (3, 3, 1),
            "and must still be counted on the frames after it"
        );
    }

    /// A failed scan reports nothing about deleting, because nothing
    /// was deleted.
    ///
    /// The refusal path returns early, and a progress report claiming
    /// branches were attempted would contradict outcomes that say they
    /// were not.
    #[test]
    fn a_failed_recheck_reports_no_deletion_progress() {
        let tmp = tempfile::TempDir::new().unwrap();
        let notrepo = tmp.path().join("nope");
        std::fs::create_dir_all(&notrepo).unwrap();

        let rec = Recorder::default();
        let out = delete_local_with_progress(notrepo.to_str().unwrap(), &["anything".into()], &rec);
        assert!(out[0]
            .error
            .as_deref()
            .unwrap_or("")
            .contains("could not re-check"));
        assert!(
            rec.deleting.lock().unwrap().is_empty(),
            "nothing was deleted, so nothing may be reported as deleted"
        );
    }

    /// The remote half reports the same two phases.
    ///
    /// Local and remote run back to back for a "delete in both places",
    /// so a phase shape that differed between them would show the user
    /// two unrelated progress reports for one action.
    #[test]
    fn the_remote_half_reports_both_phases_too() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "shipped"]);
        commit(&repo, "shipped-work");
        run(&repo, &["push", "-q", "origin", "shipped"]);
        squash_merge(&repo, "shipped");
        run(&repo, &["branch", "-q", "-D", "shipped"]);

        let rec = Recorder::default();
        let out =
            delete_remote_with_progress(repo.to_str().unwrap(), &["origin/shipped".into()], &rec);
        assert_eq!(out[0].error, None, "{:?}", out[0]);
        assert!(
            !rec.checking.lock().unwrap().is_empty(),
            "the remote half skipped the checking phase"
        );
        assert_eq!(rec.deleting.lock().unwrap().clone(), vec![(1, 1, 0)]);
    }

    /// The silent path is byte-for-byte the old behaviour.
    ///
    /// `delete_local` still exists and still takes no sink, so every
    /// caller that does not want progress -- and every existing test
    /// above -- is unchanged by this.
    #[test]
    fn reporting_does_not_change_the_answer() {
        let (_t, repo) = fixture();
        let names = merged_branches(&repo, 2);

        let quiet = delete_local(repo.to_str().unwrap(), &names[..1]);
        let loud =
            delete_local_with_progress(repo.to_str().unwrap(), &names[1..], &Recorder::default());
        assert_eq!(quiet[0].error, None, "{:?}", quiet[0]);
        assert_eq!(loud[0].error, None, "{:?}", loud[0]);
    }
}

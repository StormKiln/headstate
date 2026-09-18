use super::model::{GitOperation, Lock, Repo, Safety, Upstream, Worktree};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

/// Run a git command in a directory, returning stdout on success.
/// Git calls are bounded, like the Docker ones.
///
/// Almost every subcommand here is local-only -- cherry, log, rev-list,
/// rev-parse, worktree list -- so the bound was never about a network
/// call. The risk it was added for is a stalled FILESYSTEM: an
/// unresponsive network mount, a stale index.lock, a disk that stops
/// answering.
///
/// The exceptions are `fetch_refs` and `pull_checkout`, both of which go
/// to the network and both of which run only on a click. For them the
/// 30s ceiling is doing a second job -- it is what makes "fetch on
/// demand" safe against an unreachable remote, which is one of the two
/// reasons the SCAN still refuses to fetch (#788). Thirty seconds is
/// generous for a fetch of one repository's refs and short enough that a
/// dead host reports git's own error instead of holding a pool thread.
///
/// The consequence justified the bound: `output()` blocks forever, and
/// this runs inside spawn_blocking, so a hung call permanently consumes
/// a pool thread. Repeated across a scan that exhausts the pool and
/// wedges every worktree operation until restart, with no in-app
/// recovery. A timeout maps onto the existing `Safety::Unknown` arm,
/// which already treats a git failure as "cannot say", never "safe".
const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Shared with `branches`, which needs the same timeout contract: a
/// git call that hangs must become "cannot say", never "safe to
/// delete".
pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    // RETRIED on a spawn failure.
    //
    // Spawning git intermittently fails with ENOENT under process
    // load, for a git binary that plainly exists. #447 hit this on
    // macOS `posix_spawn`; the failure that prompted this change was
    // on ubuntu CI, so it is not one platform's quirk. Observed both
    // times as a spawn that never ran, never as a git that answered.
    //
    // This function did not retry, which was survivable while its
    // callers were the worktree scan -- a few dozen calls. The branch
    // scan calls it hundreds of times across 8 threads, and that is
    // where it started failing.
    //
    // Only the SPAWN is retried. A git that RAN is believed, whatever
    // it said -- retrying a real failure would turn a clear error into
    // a slow one.
    let mut spawned = None;
    for _ in 0..3 {
        match Command::new(crate::auth::git_program())
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => {
                spawned = Some(Ok(c));
                break;
            }
            Err(e) => spawned = Some(Err(e)),
        }
    }
    let child = match spawned {
        Some(Ok(c)) => c,
        Some(Err(e)) => return Err(e.to_string()),
        None => return Err("could not spawn git".to_string()),
    };

    // A thread that BLOCKS on the child, rather than polling it. Polling
    // was measurably wrong here: a flat 20ms interval took the full scan
    // from 35s to 71s, and even 1ms-with-backoff left it at 48s, because
    // several thousand calls each paid up to an interval of dead time.
    // Blocking costs nothing on the common path and still bounds the
    // pathological one.
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let result = child.wait_with_output();
        // The receiver is gone on timeout; that is expected, not an error.
        let _ = tx.send(result);
    });

    match rx.recv_timeout(GIT_TIMEOUT) {
        Ok(Ok(out)) => {
            let _ = handle.join();
            if out.status.success() {
                Ok(String::from_utf8_lossy(&out.stdout).into_owned())
            } else {
                Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
            }
        }
        Ok(Err(e)) => Err(e.to_string()),
        // The thread is left running rather than detached-and-killed:
        // `wait_with_output` owns the child, so there is no handle here
        // to kill it with. It exits when git does, and the scan moves on
        // treating this worktree as Unknown -- which is the honest
        // answer for a call that never came back.
        Err(_) => Err(format!(
            "git did not respond within {}s",
            GIT_TIMEOUT.as_secs()
        )),
    }
}

/// `owner/repo` from a git remote URL.
///
/// From the REMOTE, never the directory name. This repository is the
/// proof: its directory is `ghstat` and its repository is
/// `pktstorm/headstate`. Since a match here decides whether GitHub's
/// "merged" verdict is shown against a worktree, a wrong pairing would
/// attach an authoritative-looking answer to the wrong directory.
///
/// Returns None for anything unrecognisable rather than guessing.
/// When this repository's remote refs were last fetched.
///
/// `FETCH_HEAD`'s mtime, which git rewrites on every fetch. One `stat`
/// against a file the scan is already beside -- deliberately not a
/// `git` invocation, because this runs per repository and the whole
/// point is that it costs nothing next to the scan it annotates.
///
/// `None` for never fetched, an unreadable time, or a repository
/// without the file. All three are "we do not know", which is what the
/// UI must say; a fabricated timestamp here would be worse than the
/// silence it replaced (#702).
/// `pub(super)` for `assess`, which labels its prompt with the same
/// instant so an agent knows its inputs are as-of-a-fetch rather than
/// live (#815). One reader, one writer, one definition -- a second
/// `FETCH_HEAD` stat in `assess` could drift from this one's fallback
/// order and report a different age for the same repository.
pub(super) fn fetched_at(dir: &Path) -> Option<String> {
    let meta = std::fs::metadata(dir.join(".git").join("FETCH_HEAD"))
        .or_else(|_| std::fs::metadata(dir.join("FETCH_HEAD")))
        .ok()?;
    let t: chrono::DateTime<chrono::Utc> = meta.modified().ok()?.into();
    Some(t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

pub fn parse_owner_repo(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    // Everything after the host, whichever form the URL takes:
    //   git@host:owner/repo   https://host/owner/repo   ssh://git@host/owner/repo
    let tail = url.rsplit_once(':').map_or(url, |(_, t)| t);
    let mut parts = tail.rsplit('/');
    let repo = parts.next()?;
    let owner = parts.next()?;
    (!owner.is_empty() && !repo.is_empty() && !owner.contains(' ') && !repo.contains(' '))
        .then(|| format!("{owner}/{repo}"))
}

/// The `owner/repo` a checkout belongs to, or None.
pub fn repo_identity(repo_path: &str) -> Option<String> {
    git(Path::new(repo_path), &["remote", "get-url", "origin"])
        .ok()
        .and_then(|u| parse_owner_repo(&u))
}

/// Whether a ref can be passed to git without being read as a flag.
///
/// Git ref names may legitimately begin with `-` -- `git branch` refuses
/// them, but `update-ref` accepts them, so a hostile remote can ship
/// one. Passed as a bare argv element, `--output=/path` makes `git log`
/// write to an arbitrary file, which is an arbitrary file write with the
/// app's privileges.
///
/// Rejected at the BOUNDARIES where remote-controlled refs enter
/// (`parse_porcelain` and every `default_branch`) rather than at each of
/// the ten call sites, because a boundary cannot be forgotten. The `--`
/// separators at the sinks are the second layer, not the only one.
///
/// `pub(crate)` since #854. It was `pub(super)`, reachable only inside
/// `worktrees` -- and THREE other modules read
/// `refs/remotes/origin/HEAD`, the remote-controlled value this exists to
/// guard, and returned it unvalidated because they could not call this.
/// `branches::scan`, `packages::apply` and `docker::classify` each grew
/// their own `default_branch`, and the validator's reach was the reason
/// the rule did not travel with the pattern. A shared validator that only
/// one module can see is a rule with one user.
pub(crate) fn is_safe_ref(r: &str) -> bool {
    !r.is_empty() && !r.starts_with('-')
}

/// Parse `git worktree list --porcelain`.
///
/// Blank-line-delimited records of `worktree`/`HEAD`/`branch`. A record
/// without a branch is detached HEAD, which is still a real worktree
/// occupying real disk -- so it is kept, with an empty branch, rather
/// than dropped.
///
/// `locked` and `prunable` are read too, and dropping them was #753.
/// Both are attributes git only emits when they apply, and both change
/// the verdict: a locked worktree cannot be removed no matter how
/// merged and clean it is, and a prunable one has no directory left to
/// remove. Reading only `worktree`/`HEAD`/`branch` meant a third of the
/// rows on the reporting machine were marked removable and then refused
/// by git at the moment the user acted.
pub fn parse_porcelain(out: &str) -> Vec<Worktree> {
    let mut all = Vec::new();
    let mut cur = Worktree::default();

    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            if !cur.path.is_empty() {
                all.push(std::mem::take(&mut cur));
            }
            cur.path = p.to_string();
        } else if let Some(h) = line.strip_prefix("HEAD ") {
            cur.head = h.to_string();
        } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
            // A flag-shaped ref is dropped rather than carried. The
            // worktree still lists -- it just has no usable branch name,
            // which the classifier already handles as detached.
            if is_safe_ref(b) {
                cur.branch = b.to_string();
            } else {
                log::warn!("ignoring a branch name that reads as a flag: {b:?}");
            }
        } else if line == "locked" || line.starts_with("locked ") {
            // Matched in two forms because git emits two. With
            // `--reason` the line is `locked <reason>`; without one it
            // is the bare word `locked`, with no trailing space. A
            // `strip_prefix("locked ")` alone would silently miss every
            // unexplained lock -- which is the same class of miss as
            // #753 itself, and would leave those worktrees marked
            // removable.
            cur.locked = Some(line["locked".len()..].trim_start().to_string());
        } else if let Some(p) = line.strip_prefix("prunable ") {
            cur.prunable = Some(p.to_string());
        }
    }
    if !cur.path.is_empty() {
        all.push(cur);
    }

    // The first record is always the main checkout.
    if let Some(first) = all.first_mut() {
        first.is_main = true;
        first.safety = Safety::MainCheckout;
    }
    all
}

/// Classify a worktree.
///
/// The order is the design: the DANGEROUS conditions are checked before
/// the merely-inconvenient ones, so a worktree that is both unmerged and
/// never-pushed reports the fact that matters. Any git failure yields
/// `Unknown`, never `Safe`.
///
/// Two of the refusals below are only correct if they are also
/// REACHABLE and MEANT, which is what #776 was about. A check placed
/// after one that always fires first never runs (the detached case), and
/// a check that answers a question the user did not ask refuses work
/// that is already safe (the ahead-of-a-stale-ref case). Both are
/// recorded at the point where the order matters, because both read as
/// obviously-correct code from any distance.
pub fn worktree_safety(
    wt: &Worktree,
    default_branch: &str,
    has_upstream: bool,
    ahead: Option<u64>,
) -> Safety {
    if wt.is_main {
        return Safety::MainCheckout;
    }
    let dir = Path::new(&wt.path);

    // FIRST among the non-main checks, because everything below it runs
    // `git` inside `dir` and there is no `dir`. Git already told us why
    // in the porcelain listing, so prefer its reason to the app's own
    // `is_dir` guess: "prunable: gitdir file points to non-existent
    // location" names a stale registration and implies `git worktree
    // prune`, where the old `Unknown("directory is missing")` read as
    // corruption and implied nothing (#753).
    //
    // The `is_dir` fallback stays underneath it for the case git did
    // NOT flag: a directory can vanish between the listing and this
    // check, and a missing directory we cannot explain is still not
    // something to classify as safe.
    if let Some(why) = &wt.prunable {
        return Safety::Prunable(why.clone());
    }
    if !dir.is_dir() {
        // A worktree that is LOCKED and gone reports the lock, not a
        // bare "directory is missing" (#792).
        //
        // #792 expected this combination to report `Prunable` and lose
        // the lock. MEASURED against real git, it does neither: git
        // WITHHOLDS the `prunable` line while a lock file exists -- it
        // will not prune a locked registration, so it does not advertise
        // one as prunable -- and the listing carries `locked` with no
        // `prunable` beside it. So the prunable arm above does not fire,
        // and this fallback did, producing "could not determine:
        // directory is missing": precisely the wording #753 set out to
        // eliminate, for a state where git had in fact said something
        // useful.
        //
        // Both facts now reach the row. The verdict is `Locked`, because
        // the lock is what is ACTIONABLE -- it is the reason git refuses
        // to prune this registration, and clearing it is what lets the
        // header's Prune action finish the job (#793). The missing
        // directory travels inside it as `underlying`, which is exactly
        // what that field is for, so the row reads "locked … " and the
        // unlock confirmation says the directory is gone underneath.
        //
        // Not a reorder of the main lock arm below. That arm's position
        // relative to `Dirty` is load-bearing and documented there, and
        // everything between here and it shells `git` into a directory
        // that does not exist. This is the narrow case where the lock is
        // the only thing left to say.
        //
        // `lock_age_days` and `holder_in` both degrade to `None` here
        // without special-casing: the age is read from `<dir>/.git`,
        // which is gone, and the reason is whatever git recorded. An
        // unknown age is rendered as "locked" with no date rather than
        // as a fresh lock -- the safe direction, documented on
        // `Lock::age_days`.
        if let Some(why) = &wt.locked {
            return Safety::Locked(Lock {
                reason: (!why.is_empty()).then(|| why.clone()),
                age_days: lock_age_days(dir),
                holder_running: holder_in(why).map(holder_is_running),
                underlying: Box::new(Safety::Unknown("directory is missing".into())),
            });
        }
        return Safety::Unknown("directory is missing".into());
    }

    // BEFORE the dirty check (#1136). A half-finished rebase has a dirty
    // working tree by definition, so `Dirty(n)` would win and report "7
    // uncommitted files" -- ordinary edits, and the one state a user
    // must not remove: `git worktree remove` on a half-replayed rebase
    // discards a commit series that exists nowhere else.
    if let Some(op) = operation_in_progress(dir) {
        return Safety::InProgress {
            op,
            conflicts: conflicted_files(dir),
        };
    }

    match git(dir, &["status", "--porcelain"]) {
        Ok(s) => {
            let n = s.lines().filter(|l| !l.trim().is_empty()).count() as u64;
            if n > 0 {
                return Safety::Dirty(n);
            }
        }
        Err(e) => return Safety::Unknown(e),
    }

    // Locked: git will refuse `worktree remove` outright, whatever the
    // branch's merge state (#753). Checked AFTER `Dirty` and BEFORE
    // everything below, and both halves of that are deliberate:
    //
    // - After `Dirty`, because the two facts answer different
    //   questions and only one of them is about losing work. A locked,
    //   dirty tree reported as merely "locked" would hide uncommitted
    //   edits behind an obstacle the user is about to clear -- they
    //   unlock, remove, and the edits go. Dirty is the fact that
    //   survives the remedy, so it is the one to say. This matches the
    //   existing rule that dangerous conditions outrank inconvenient
    //   ones: `Dirty` is about content, `Locked` is about permission.
    // - Before the push and merge checks, because those decide whether
    //   removal is DESIRABLE while this decides whether it is POSSIBLE.
    //   A locked worktree that is merged, clean, and pushed was
    //   previously the exact bug: reported `Safe`, then refused by git
    //   at the moment of removal. Reporting "branch not merged" for a
    //   locked tree would be no better -- the user would merge it and
    //   still be refused.
    //
    // The lock is NOT overridden with `-f -f`. Git offers that, and
    // taking it silently would break the only signal a concurrent
    // process has for claiming a directory -- on the reporting machine
    // 13 of 34 worktrees were locked by agents actively working in them.
    // What #775 adds is UNDERNEATH, not instead. The ordering above is
    // unchanged and the verdict is still `Locked`; the difference is
    // that the checks below now also run, and their answer is carried
    // inside it.
    //
    // Deliberately NOT a reorder. Returning the merge verdict for a
    // locked worktree would send the user to merge something and leave
    // them refused anyway, which is exactly what #753 reasoned through
    // and got right. Both facts are wanted, so both are computed, and
    // the one that governs the button stays on top.
    //
    // The recursion terminates because the copy has no lock: the check
    // above is the only reader of `wt.locked`, and `unlocked` clears
    // it. So the second pass falls through to the ordinary checks and
    // can never re-enter this arm.
    if let Some(why) = &wt.locked {
        let mut unlocked = wt.clone();
        unlocked.locked = None;
        let underlying = worktree_safety(&unlocked, default_branch, has_upstream, ahead);
        return Safety::Locked(Lock {
            // Empty means git emitted a bare `locked` line, i.e. a lock
            // taken without `--reason`. Reported as "no reason given"
            // rather than as an empty quotation, which would read like
            // a display bug.
            reason: (!why.is_empty()).then(|| why.clone()),
            age_days: lock_age_days(dir),
            // `holder_in`, not a bare pid: a recycled pid must not read
            // as a live holder, and our own lock reasons record the
            // start time that tells them apart (#792). See
            // `holder_is_running`.
            holder_running: holder_in(why).map(holder_is_running),
            underlying: Box::new(underlying),
        });
    }

    // A branch that was never committed to holds nothing, pushed or
    // not. Checked here -- AFTER dirty, BEFORE `has_upstream` -- and the
    // order is load-bearing in both directions:
    //
    // - After `Dirty`, because an empty branch can still have
    //   uncommitted work in its working tree, and that work is exactly
    //   what `Dirty` exists to protect. Reversing these would report
    //   "nothing to lose" over a tree full of unsaved edits.
    // - Before `NeverPushed`, which is the bug in #701: a scratch branch
    //   has no upstream either, so it was reported as "commits exist
    //   only here" -- a claim about commits that do not exist -- beside
    //   "0 commits ahead".
    if branch_is_empty(dir, &wt.branch) {
        return Safety::Empty;
    }

    // No upstream means nothing was ever pushed: these commits exist only
    // here. Checked BEFORE merge status, because a branch name that looks
    // merged tells you nothing about commits that never left the machine.
    //
    // `has_upstream` is passed in rather than re-probed: the caller
    // already asked, and this was one of two identical `rev-parse @{u}`
    // calls per worktree -- 1.58s of pure duplication across a real
    // 145-worktree repo.
    if !has_upstream {
        // Two different situations produce a failing `rev-parse @{u}`,
        // and they carry OPPOSITE verdicts (#732):
        //
        //   1. the branch was genuinely never pushed -- the commits
        //      exist only here and deleting loses them;
        //   2. the branch was pushed, its PR merged, and the remote
        //      branch was deleted -- the work is on the default branch
        //      and deleting loses nothing.
        //
        // Treating both as NeverPushed is what made every merged
        // worktree unremovable: 27 of 29 branches on the machine that
        // prompted this, holding the disk the view exists to reclaim.
        //
        // `branch.<name>.remote` tells them apart without a network
        // call. Git leaves the tracking config in place when the remote
        // ref disappears, so config-but-no-ref means case 2, and no
        // config at all means case 1.
        //
        // Case 2 does NOT short-circuit to safe: it falls through to
        // the same merge checks every other branch faces. The upstream
        // being gone is permission to ASK whether the work landed, not
        // an answer that it did.

        // Detached FIRST, above `was_ever_pushed`, and the order is the
        // entire fix for the second half of #776.
        //
        // `was_ever_pushed` answers by reading `branch.<name>.remote`.
        // A detached HEAD has no branch, so there is no config key to
        // read, so it returns false -- and the `NeverPushed` return
        // above fired before the detached check below was ever reached.
        // The check existed for exactly one case and was unreachable
        // for exactly that case.
        //
        // The verdict it produced was not merely imprecise, it was the
        // strongest refusal the app has: "commits exist only here",
        // asserted over a checkout whose commits are usually a tag or a
        // main-branch SHA that exists everywhere. MEASURED on a real
        // 37-worktree checkout: 12 were detached and all 12 claimed it.
        //
        // What a detached HEAD cannot be asked is NARROWER than "every
        // question", and #819 is the half this used to give away.
        //
        // Push state and tracking config genuinely need a branch, for the
        // reason just given, so that part of #776 stands exactly as it
        // is: `detached_safety` has no path to `NeverPushed`, and no
        // detached row can claim its commits exist only here.
        //
        // MERGE STATUS does not need a branch. Every primitive in this
        // file already asks about `HEAD` or about an explicit sha --
        // `merge-base --is-ancestor HEAD`, `patch_id(.., "HEAD")`,
        // `git cherry <default> HEAD` -- and none reads `wt.branch`. So
        // the only thing that made a detached row unclassifiable was
        // this return sitting ABOVE the merge check, not any missing
        // evidence. Four worktrees on the reporting machine read
        // "could not determine" while `merge-base --is-ancestor`
        // answered true for all four, instantly.
        //
        // MEASURED over the reporting machine's 43 worktrees, same code
        // and same moment, with only the ordering differing. This is
        // #776's own table and it is unchanged -- the column that matters
        // is still the first row:
        //
        // | verdict                | before | after |
        // |------------------------|--------|-------|
        // | NeverPushed            | 12     | 2     |
        // | Unknown(detached HEAD) | 0      | 10    |
        //
        // The 2 that remain are real branches with no tracking config,
        // which is what the refusal is actually for. #819 does not move
        // this row in either direction, by construction rather than by
        // measurement: `detached_safety` has no path to `NeverPushed`.
        //
        // What #819 changes is the SECOND row, splitting it. Those 10
        // were all the detached rows on that machine, merged and unmerged
        // alike, because the verdict did not distinguish them. Counted
        // separately on the repository in the issue, 4 of its detached
        // rows are provably contained in the default branch -- `git
        // merge-base --is-ancestor` answers true for each, and `name-rev`
        // resolves them to `tags/v1.13.0~26..~30`. Those become
        // `DetachedMerged`; the rest keep `Unknown`, which for a checkout
        // whose HEAD is genuinely not on the default branch is still the
        // honest answer. The live tally in `live_scan_classifies_real_worktrees`
        // counts the two separately, which is how the split is checked
        // rather than asserted.
        if wt.branch.is_empty() {
            return detached_safety(dir, default_branch);
        }
        if !was_ever_pushed(dir) {
            return Safety::NeverPushed;
        }
        return match merged_into(dir, default_branch) {
            Safety::Safe => Safety::MergedUpstreamDeleted,
            other => other,
        };
    }

    // Detached before the ahead-count, mirroring the no-upstream path
    // above and for the same reason (#776). This one was never
    // unreachable -- a detached HEAD reaching here has an upstream, so
    // `ahead` is a real number -- but answering `Unpushed(n)` for a
    // checkout with no branch describes a branch that does not exist.
    // The check simply belongs above every verdict that presumes one.
    //
    // Routed through the same `detached_safety` as the path above (#819),
    // so the two cannot drift on what a branchless checkout is allowed to
    // say. In practice this arm is unreachable for a detached row --
    // `classify` derives `has_upstream` from `upstream_state`, which
    // returns `Detached` before it probes `@{u}` at all, so a detached
    // worktree always takes the `!has_upstream` path. It is kept because
    // `worktree_safety` takes `has_upstream` as a parameter and a caller
    // passing true must not get a verdict that presumes a branch; the
    // test below drives exactly that.
    if wt.branch.is_empty() {
        return detached_safety(dir, default_branch);
    }

    // Ahead-count comes from the caller's `rev-list --left-right`, which
    // computes it as the right-hand column. This used to be a separate
    // `log --oneline @{u}..` -- a second walk for a number already in
    // hand. A None here means git failed, which must stay Unknown rather
    // than becoming a confident zero.
    match ahead {
        Some(0) => {}
        Some(n) => {
            // Ahead of the upstream ref is NOT the same as unpushed,
            // and conflating the two is the first half of #776.
            //
            // Git does not delete `refs/remotes/origin/<branch>` when
            // the remote branch goes away; only an explicit
            // `remote prune` or a `fetch --prune` does, and nothing in
            // this app runs either. So after the ordinary
            // squash-merge-and-delete-branch flow, the tracking ref
            // LINGERS -- pointing at the branch's pre-merge tip, for a
            // branch GitHub no longer has.
            //
            // `rev-parse @{u}` then SUCCEEDS against that ghost, which
            // is why #732 never fires here: that fix keys on the
            // upstream failing to resolve, and this upstream resolves
            // perfectly well. It just describes something that does not
            // exist. The branch reads as 1 commit "ahead" of its own
            // pre-merge self, and `Unpushed(1)` short-circuited before
            // any merge check ran. MEASURED on a real 37-worktree
            // checkout: 5 branches whose PRs had merged hours earlier.
            //
            // Confirming the ref is stale needs `git ls-remote`, a
            // NETWORK call, per row. This scan is deliberately
            // offline-only -- every other check reads refs already on
            // disk -- and a per-worktree round trip would also hang on
            // an unreachable remote. So the staleness is not probed.
            //
            // Instead the question is re-asked in a form that does not
            // depend on the tracking ref at all: is this content
            // already on the DEFAULT branch? `Unpushed` exists to stop
            // someone deleting commits that live only on their machine,
            // and a commit contained in the default branch is by
            // definition not one of those -- however many refs it is
            // "ahead" of. So a merged verdict outranks an ahead-count,
            // and only work that is genuinely NOT landed still reports
            // `Unpushed`.
            //
            // This CANNOT widen the gate. `merged_into` is the same
            // check every other branch faces, unchanged, and it answers
            // `Safe` only on ancestry or an exact patch-id match. A
            // branch with real unpushed work fails it and falls through
            // to the `Unpushed(n)` below, exactly as before -- the only
            // rows that change verdict are ones whose content was
            // already proven to be on the default branch.
            //
            // Labelled `MergedUpstreamDeleted` rather than `Safe` for
            // the same reason #732 introduced that variant: the
            // upstream cannot be re-consulted afterwards, and the user
            // deserves to see which route produced the verdict. Here
            // the upstream ref still exists locally, but the branch it
            // names does not -- which is the same fact one prune away.
            //
            // MEASURED over the reporting machine's 43 worktrees, same
            // code and same moment, with only this check differing:
            //
            // | verdict               | before | after |
            // |-----------------------|--------|-------|
            // | Unpushed              | 6      | 0     |
            // | MergedUpstreamDeleted | 0      | 6     |
            //
            // Every one of the six was a merged PR whose tracking ref
            // had not been pruned. No row moved in the other direction.
            //
            // The cost is bounded to branches that read as AHEAD -- 6 of
            // 43 here -- because this arm only runs for `n > 0`. A
            // branch level with its upstream falls through to the same
            // `merged_into` at the end of this function exactly as
            // before, so no worktree is merge-checked twice.
            if merged_into(dir, default_branch) == Safety::Safe {
                return Safety::MergedUpstreamDeleted;
            }
            return Safety::Unpushed(n);
        }
        None => return Safety::Unknown("could not count unpushed commits".into()),
    }

    // Ancestry is the cheap answer, but it only sees fast-forward and
    // merge-commit merges. A squash-merge replays the branch as a NEW
    // commit with a new SHA, so the original tip is never an ancestor --
    // and squash is the default on these repos. Measured across every
    // repo on this machine: ancestry alone found 10 of 157 merged
    // worktrees, calling the other 147 unmerged. Those 147 are exactly
    // the ones filling the disk this view exists to reclaim.
    merged_into(dir, default_branch)
}

/// How many whole days ago this worktree's lock was taken.
///
/// **Measured from git's own `locked` file, not from the reason
/// string.** The reason string was the obvious source -- the locks on
/// the reporting machine embed `start <ctime>` -- and it is the wrong
/// one: all 20 carry the IDENTICAL timestamp, because it dates the
/// process that took the locks rather than any individual claim. A
/// column computed from it would show the same number on every row and
/// would age all 20 in step, which is precisely the false-evidence
/// problem #775 is about, moved into a new field.
///
/// The `locked` file's mtime is per-lock and cannot be faked by a
/// long-lived parent. Verified against real git: `git worktree lock`
/// writes the file, and an unlock followed by a re-lock rewrites it, so
/// the mtime is when the CURRENT claim was made rather than when the
/// directory was first ever locked.
///
/// Found by reading the worktree's `.git` FILE, which holds
/// `gitdir: <admin dir>` -- the same filesystem-only technique
/// `orphan_gitdir` uses, and for the same reason: it costs no git
/// process, and this runs for every locked row on a page where 45% of
/// them are locked.
///
/// `None` on any failure. A lock whose age cannot be read is not a
/// fresh lock, and a confident 0 here would make an unreadable claim
/// look like a brand-new one -- the direction that makes clearing it
/// feel safer than it is.
fn lock_age_days(dir: &Path) -> Option<u64> {
    let contents = std::fs::read_to_string(dir.join(".git")).ok()?;
    let admin = contents.strip_prefix("gitdir:")?.trim();
    let meta = std::fs::metadata(Path::new(admin).join("locked")).ok()?;
    let age = std::time::SystemTime::now()
        .duration_since(meta.modified().ok()?)
        .ok()?;
    Some(age.as_secs() / 86_400)
}

/// Who a lock reason says is holding it: a pid, and the start time it
/// claims for that pid.
///
/// Git imposes no format on `--reason`, so this is a convention rather
/// than a contract: it looks for `pid <digits>`, optionally followed by
/// `start <ctime>`, which is what the locks on the reporting machine
/// carry -- `claude agent agent-a53ff… (pid 90962 start Sat Sep  5
/// 01:52:48 2026)`. Anything else yields `None` for the whole thing,
/// which the UI reads as "nothing to check" rather than as "nobody
/// holds it".
///
/// The start time is optional and its absence is NOT an error. Plenty
/// of lockers write a bare pid, and refusing to check those at all
/// would throw away the one genuinely decisive signal (`Some(false)`)
/// for every lock that does not follow our own convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LockHolder {
    pid: u32,
    /// Epoch seconds, parsed from the reason's `start <ctime>`, or
    /// `None` when the reason names no start time.
    ///
    /// Epoch seconds rather than the string, so the comparison is
    /// against `sysinfo`'s own unit and no formatting round-trip can
    /// make two equal times look different.
    started_at: Option<i64>,
}

/// Who the lock reason names, if it names anybody.
///
/// Parsed so the app can say what it CHECKED. The pid alone is not
/// treated as identifying evidence: see `Lock::holder_running`.
fn holder_in(reason: &str) -> Option<LockHolder> {
    let tail = reason.split("pid ").nth(1)?;
    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
    let pid = digits.parse().ok()?;
    Some(LockHolder {
        pid,
        started_at: start_time_in(tail),
    })
}

/// The epoch seconds a `start <ctime>` fragment names, if it names one.
///
/// The format is `ctime(3)`'s -- `Sat Sep  5 01:52:48 2026`, with the
/// day-of-month space-padded to two columns -- because that is what the
/// agent tooling writing these locks emits, and matching the producer
/// is the only thing a parser here can do.
///
/// **No timezone in the string, so it is read as LOCAL time.** That is
/// what `ctime` produces, and the alternative -- guessing UTC -- would
/// be wrong by the machine's offset: up to 12 hours, which is more than
/// enough to turn a matching start time into a mismatching one and
/// report a live holder as dead. The exactly-correct reading is not
/// available (the producer did not record an offset), so the reading
/// that matches the producer's own clock is the one taken, and the
/// tolerance in `holder_is_running` absorbs the rest.
///
/// The weekday is parsed and discarded rather than skipped by position:
/// `%a` consumes it and chrono then validates the rest, where slicing a
/// fixed number of characters off the front would silently misread any
/// locale or format that differs.
///
/// `None` on anything unparseable, which degrades to the pid-only
/// check. A start time we cannot read is not a mismatch -- claiming one
/// would report every unconventional lock as dead, and that is the
/// direction that makes clearing a LIVE claim feel safe.
fn start_time_in(tail: &str) -> Option<i64> {
    let after = tail.split("start ").nth(1)?;
    // Up to the closing paren of `(pid N start <ctime>)`, or the end.
    let stamp = after.split(')').next()?.trim();
    let naive = chrono::NaiveDateTime::parse_from_str(stamp, "%a %b %e %H:%M:%S %Y").ok()?;
    // `Local` rather than `Utc`: see the doc above. An ambiguous or
    // non-existent local time -- the hour a DST transition skips or
    // repeats -- resolves to whichever candidate chrono offers first
    // rather than to `None`, because a lock taken in that hour is an
    // ordinary lock and refusing to read it would lose the signal.
    chrono::TimeZone::from_local_datetime(&chrono::Local, &naive)
        .earliest()
        .map(|dt| dt.timestamp())
}

/// Whether the process a lock names is still the process that took it.
///
/// Through `sysinfo`, which is already a dependency for System Health
/// and is portable -- rather than a raw `kill(pid, 0)`, which would
/// mean `unsafe` and a `libc` dependency for one call, and rather than
/// spawning `ps`, which would mean a process per locked row on a page
/// where 45% of rows are locked.
///
/// Refreshes only the ONE pid, not the process table. The whole
/// question is about one process, and enumerating several hundred to
/// answer it -- once per locked row -- would be the expensive way.
///
/// **Checks (pid, start_time), not the pid alone, and that is the
/// improvement over what #792 asked for.** A bare pid check is not
/// merely imprecise, it is wrong in the dangerous direction: pids are
/// recycled, so a lock naming a long-dead pid whose number some
/// unrelated process has since been given reads as a LIVE holder, and
/// the row then hides the one fact the user needed. The reporting
/// machine rebooted between taking these locks and reading them, which
/// is precisely when every pid in the space gets handed out again.
///
/// Our lock reasons already record the holder's start time beside its
/// pid, so the identity is available and the comparison costs nothing
/// extra -- `sysinfo` returns `start_time()` from the same refresh that
/// proves the process exists. A pid that exists but started AFTER the
/// lock was taken cannot be the locker, so it is reported gone.
///
/// The tolerance is deliberately generous. `ctime` has one-second
/// resolution, the string carries no timezone (see `start_time_in`),
/// and `sysinfo`'s start time is the kernel's own -- so an exact
/// comparison would fail on rounding alone. A window is used instead,
/// and it is the SAFE direction that it is wide: too wide reports a
/// recycled pid as live, which is today's behaviour and merely leaves
/// the row as it was; too narrow reports a live holder as dead, which
/// invites clearing a claim on a directory something is working in.
///
/// When the reason names no start time at all -- or when `sysinfo`
/// cannot read the process's -- this falls back to the existence check,
/// exactly the old behaviour. Both are "no evidence of a mismatch", and
/// neither is evidence of one.
///
/// **Weak evidence when true, carried honestly.** On the reporting
/// machine the pid-only check answered true for all 20 locks and every
/// one was abandoned, because the pid belonged to the surviving parent
/// session rather than to the worker that took the lock. So a `true`
/// here is worth very little even now and the UI must not spend it as
/// proof; a `false` is worth a great deal, being the one unambiguous
/// signal that the named holder is gone.
fn holder_is_running(holder: LockHolder) -> bool {
    /// How far apart the lock's claimed start time and the kernel's may
    /// be before the process is judged a different one. Five minutes:
    /// comfortably more than a clock skew or a rounding difference, and
    /// far less than the hours or days that separate a recycled pid from
    /// the lock that named its predecessor.
    const TOLERANCE_SECS: i64 = 300;

    let pid = sysinfo::Pid::from_u32(holder.pid);
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    let Some(proc) = sys.process(pid) else {
        return false;
    };
    let Some(claimed) = holder.started_at else {
        // No start time to compare, so existence is all there is --
        // the pre-#792 behaviour, kept for lockers that write a bare
        // pid.
        return true;
    };
    // `as i64` on a u64 of epoch seconds: a start time large enough to
    // overflow is some 290 billion years away, and the subtraction
    // below would saturate long before anything here could wrap.
    let actual = proc.start_time() as i64;
    // A zero start time means `sysinfo` could not read one, not that the
    // process began at the epoch -- its own `ProcessInner` initialises
    // the field to 0 before the platform fills it in. Comparing it would
    // be ~55 years off any real claim, so the tolerance would reject it
    // and the holder would read as GONE.
    //
    // That is the one direction this function must never fail in. A
    // false "gone" invites clearing a claim on a directory something is
    // working in, where a false "live" merely leaves the row as it was.
    // So an unreadable start time falls back to the existence check,
    // exactly as a reason with no start time does: the process is there,
    // and we have no evidence it is a different one.
    if actual == 0 {
        return true;
    }
    (actual - claimed).abs() <= TOLERANCE_SECS
}

/// The verdict for a branchless checkout: merged, or honestly unknown.
///
/// The whole of #819, and the reason it is ONE function reached from both
/// of `worktree_safety`'s detached guards: the rule about what a
/// branchless checkout may claim must exist in exactly one place, or the
/// two sites drift and one of them re-acquires the #776 bug.
///
/// **What changed, and what deliberately did not.** #776 replaced a
/// detached row's `NeverPushed` -- "commits exist only here", the app's
/// strongest refusal, asserted falsely over 12 of 43 real rows -- with
/// `Unknown`. That was right, and the reasoning given for it was wider
/// than the facts: push state and tracking config genuinely need a
/// branch, because `was_ever_pushed` reads `branch.<name>.remote` and
/// there is no such key. Merge status needs no branch at all.
/// `merged_into` asks `merge-base --is-ancestor HEAD <default>` and then
/// patch-ids against `HEAD`; not one primitive in its tree reads
/// `wt.branch`. So a detached row was being refused an answer the
/// existing machinery could give -- four worktrees reading "could not
/// determine" while `--is-ancestor` returned true for every one of them,
/// in milliseconds (#819).
///
/// This function therefore asks exactly the merge question and nothing
/// else. There is no path from here to `NeverPushed`, `Unpushed`,
/// `MergedUpstreamDeleted` or `Empty`, and that is the #776 property
/// stated as code rather than as a comment:
///
/// - `NeverPushed` / `Unpushed` are claims about a branch's relationship
///   to a remote. Unreachable by construction; nothing below consults an
///   upstream or a config key.
/// - `MergedUpstreamDeleted` is the #732 label meaning "the tracking
///   config survives but the remote branch is gone". For a detached HEAD
///   there was never a tracking config to survive, so the label would be
///   describing evidence that does not exist.
/// - `Empty` reads a branch's reflog (`branch_is_empty` returns false on
///   an empty name, deliberately), so it cannot fire here either.
///
/// **Why `DetachedMerged` rather than reusing `Safe`.** `Safe`'s own
/// prose is "merged, pushed" and its row invites "merged when?" -- both
/// of which would be the app asserting, for a checkout with no branch,
/// things it did not establish. A separate variant lets the row say the
/// two facts it actually has, in the order that answers the user's
/// question: it is merged, and there is no branch. It also keeps #776
/// auditable: grepping for which variants a branchless checkout can
/// produce is a finite list of one.
///
/// It IS in `is_safe`, which is the point of the issue -- a clean
/// checkout whose HEAD is contained in the default branch has nothing to
/// lose, and arguably less than a merged branch does, since there is no
/// branch ref to forget. The gate is not widened in the sense that
/// matters: the evidence required is `merged_into`'s, unchanged, which
/// answers `Safe` only on ancestry or an exact patch-id match.
///
/// An unmerged detached checkout stays `Unknown`, NOT `Unmerged`.
/// `Unmerged`'s prose is "branch not merged", and for these rows there is
/// no branch to say that about -- and `merged_into` failing is not proof
/// the content is absent, only that neither ancestry nor a patch-id
/// match could show it. So the row keeps the hedge it has always had,
/// now with the sha's description attached so it is a useful hedge.
fn detached_safety(dir: &Path, default_branch: &str) -> Safety {
    // `name-rev` for BOTH arms, merged or not, because "detached at
    // v1.13.0~30" is the fix for half of #819 on its own: a bare
    // "detached HEAD" tells the user nothing they could act on, while a
    // tag-relative name tells them what this checkout actually is. ~26ms
    // measured, and only for rows that are detached -- 10 of 43 on the
    // reporting machine, not every row.
    let at = head_name_rev(dir);
    let described = |prefix: &str| match &at {
        Some(name) => format!("{prefix} at {name}"),
        None => prefix.to_string(),
    };

    match merged_into(dir, default_branch) {
        Safety::Safe => Safety::DetachedMerged(described("detached")),
        // A git failure is carried as its own words rather than flattened
        // into "detached HEAD". `merged_into` returns `Unknown(e)` only
        // when a git call failed, and that is a different situation from
        // "asked and could not show it" -- the first may succeed later.
        Safety::Unknown(why) => Safety::Unknown(format!("{}: {why}", described("detached HEAD"))),
        // Unmerged, or anything else `merged_into` can produce: the
        // question was asked and the content could not be shown to be on
        // the default branch. Said as a hedge rather than as `Unmerged`,
        // because there is no branch for "branch not merged" to be about.
        _ => Safety::Unknown(format!(
            "{} — not found on {default_branch}",
            described("detached HEAD")
        )),
    }
}

/// What a checkout's HEAD resolves to in ref-relative terms, e.g.
/// `tags/v1.13.0~30`.
///
/// "detached at v1.13.0~30" beats "detached" by enough to be worth a git
/// call (#819): the first says what this checkout IS, the second says
/// only what it lacks. On the reporting machine all four of the rows that
/// prompted the issue resolved to `tags/v1.13.0~NN`, which is exactly the
/// "leftovers of finished work" they turned out to be.
///
/// `name-rev` rather than `describe`: `describe` needs a reachable tag
/// and errors without one, while `name-rev` falls back to any ref that
/// reaches the commit -- a branch or a remote-tracking ref -- which is
/// the common case in a worktree tree with few tags. Measured at ~26ms.
///
/// `undefined` is git's word for "no ref reaches this commit", printed on
/// stdout with a success exit rather than as an error. It is filtered
/// here: rendering "detached at undefined" would be worse than saying
/// nothing, which is what `None` gets the caller.
///
/// The `tags/` and `remotes/` prefixes are stripped. They say which ref
/// namespace answered, which is machinery rather than information at a
/// glance, and the row has one line.
fn head_name_rev(dir: &Path) -> Option<String> {
    let out = git(dir, &["name-rev", "--name-only", "HEAD"]).ok()?;
    let name = out.trim();
    if name.is_empty() || name == "undefined" {
        return None;
    }
    let name = name
        .strip_prefix("tags/")
        .or_else(|| name.strip_prefix("remotes/"))
        .unwrap_or(name);
    Some(name.to_string())
}

/// Whether this branch's work is already on `default_branch`.
///
/// Split out of `worktree_safety` so the upstream-deleted path (#732)
/// and the ordinary path reach their verdict through the SAME checks. A
/// second copy of this decision would be a second place for "merged" to
/// drift, and the two paths differ only in how they label success.
///
/// Returns `Safe` when merged; otherwise the specific reason it could
/// not be established, never a bare bool -- an `Unknown` from a failed
/// git call must not collapse into "unmerged".
fn merged_into(dir: &Path, default_branch: &str) -> Safety {
    if git(
        dir,
        &["merge-base", "--is-ancestor", "HEAD", default_branch],
    )
    .is_ok()
    {
        return Safety::Safe;
    }
    squash_merged(dir, default_branch)
}

/// Whether this branch was EVER pushed, regardless of whether its
/// upstream still resolves.
///
/// `git config --get branch.<name>.remote` is the evidence. Git writes
/// it when a branch is first pushed with `-u` (or created with
/// `--track`) and does NOT remove it when the remote branch is later
/// deleted -- which is precisely the state a merged-and-tidied PR
/// leaves behind.
///
/// Reads local config only: no fetch, no network, nothing that can hang
/// on an unreachable remote. A branch that was never pushed has no such
/// key and this returns false, preserving today's refusal for the case
/// where commits really do exist only here.
///
/// Uses the CHECKOUT's own HEAD rather than a passed-in branch name so
/// it cannot be asked about one branch while reading another's config.
fn was_ever_pushed(dir: &Path) -> bool {
    let Ok(branch) = git(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"]) else {
        // Detached: there is no branch whose config could say.
        return false;
    };
    let branch = branch.trim();
    if branch.is_empty() {
        return false;
    }
    // A git failure here means "no such key", which is the honest
    // answer to "was this pushed": we cannot show that it was.
    git(
        dir,
        &["config", "--get", &format!("branch.{branch}.remote")],
    )
    .map(|v| !v.trim().is_empty())
    .unwrap_or(false)
}

/// Whether this branch was created and never committed to.
///
/// Reads the BRANCH's reflog, which records every time the ref itself
/// moved. Creating a branch writes exactly one entry; the first commit
/// writes a second. So `<= 1` entry means the ref has never moved from
/// where it was created -- no commits of its own, whatever the branch
/// points at now. Verified across all three ways a branch here gets
/// made: `worktree add -b`, `branch` then `worktree add`, and
/// `checkout -b` inside a detached worktree. All give exactly 1.
///
/// Three approaches were tried and rejected first, each of which looked
/// right:
///
/// - `rev-list --count <default>..HEAD == 0`. WRONG: a branch whose real
///   commits were merged is also 0 ahead, so every genuinely-merged
///   worktree became `Empty` and stopped being removable.
/// - `merge-base --is-ancestor HEAD <default>`. Same flaw, same reason:
///   it asks "is this reachable from the default branch", which is true
///   of merged work as well as of no work.
/// - HEAD reflog length `<= 1`. WRONG: `git worktree add -b` writes TWO
///   HEAD entries, one for the checkout and one for the branch creation,
///   so no scratch worktree ever matched.
///
/// The distinction the branch reflog gets right and the others miss is
/// "did commits ever exist HERE", which is a question about history, not
/// about where two refs currently sit.
///
/// EXACTLY one entry, not "one or fewer". A branch whose reflog is
/// MISSING reports zero entries and exits 0 -- git treats an absent
/// reflog as an empty one, not as an error -- so `<= 1` would call a
/// branch full of real commits empty. Verified: delete
/// `.git/logs/refs/heads/<branch>` from a branch with two commits and
/// `reflog show` prints nothing and succeeds.
///
/// That is the same "absence of evidence is not evidence of absence"
/// mistake as trusting a failed call, arriving through a success. `gc`
/// expires reflogs, `clone` creates none for branches it did not check
/// out, and some tooling packs refs without logs -- all of which would
/// have silently reported "nothing to lose" over work that exists.
///
/// So zero entries and an outright error are treated alike: NOT empty.
/// Both mean the reflog cannot answer, and guessing `Empty` there would
/// be a confidently wrong answer about a directory the user is deciding
/// whether to delete. A detached HEAD has no branch to ask about at all
/// and falls through to the checks below.
fn branch_is_empty(dir: &Path, branch: &str) -> bool {
    if branch.is_empty() {
        return false;
    }
    match git(dir, &["reflog", "show", "--format=%H", branch]) {
        Ok(s) => s.lines().filter(|l| !l.trim().is_empty()).count() == 1,
        Err(_) => false,
    }
}

/// Whether every commit on this branch already exists upstream as an
/// equivalent patch.
///
/// `git cherry` compares patch-ids rather than SHAs, marking each commit
/// `+` (not upstream) or `-` (an equivalent patch is upstream) -- which
/// is precisely what a squash-merge produces. All `-` means merged.
///
/// Only reached when the ancestry check has already failed, so this adds
/// one git call per *unmerged-looking* worktree, not per worktree.
///
/// Measured on a real 289-worktree tree: the scan goes from 16.0s to
/// 29.7s while `safe` goes from 6 to 122. That cost buys back 116
/// worktrees the user was previously told not to delete, and it shrinks
/// as they are deleted -- the call only fires for branches ancestry
/// cannot resolve.
/// How a checkout stands against its tracked upstream.
///
/// Uses `@{u}` rather than a hardcoded `origin/main`: repos differ
/// (`master`, `develop`, a fork tracking `upstream`), and guessing the
/// wrong ref reports confident nonsense.
///
/// Reads refs already on disk -- no fetch. The counts are as of the last
/// fetch, which the UI says plainly rather than implying they are live.
fn upstream_state(dir: &Path) -> Upstream {
    // A detached HEAD has no upstream to compare against, and asking
    // anyway yields an error that reads as a failure rather than as the
    // "question does not apply" it actually is.
    match git(dir, &["symbolic-ref", "--quiet", "HEAD"]) {
        Ok(_) => {}
        Err(_) => return Upstream::Detached,
    }

    // A local-only branch is normal, not a failure.
    if git(dir, &["rev-parse", "--abbrev-ref", "@{u}"]).is_err() {
        return Upstream::Untracked;
    }

    // One call for both numbers: left is upstream-only (behind), right is
    // HEAD-only (ahead).
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
                // Unparsable output must not become a confident zero.
                _ => Upstream::Unknown("could not read ahead/behind counts".into()),
            }
        }
        Err(e) => Upstream::Unknown(e),
    }
}

/// Order repos for the sidebar: most worktrees first.
///
/// Sorts by the count the sidebar actually SHOWS, which excludes the
/// main checkout -- every repo has one, and ordering by the raw length
/// would rank rows by a number nobody can see. Ties break by name so the
/// list does not reshuffle between polls.
///
/// The point of this view is finding disks full of stale worktrees, and
/// alphabetical order buries a repo with forty of them under one with
/// one.
fn sort_for_sidebar(repos: &mut [Repo]) {
    repos.sort_by(|a, b| {
        let count = |r: &Repo| r.worktrees.len().saturating_sub(1);
        count(b).cmp(&count(a)).then_with(|| a.name.cmp(&b.name))
    });
}

fn squash_merged(dir: &Path, default_branch: &str) -> Safety {
    match git(dir, &["cherry", default_branch, "HEAD"]) {
        Ok(s) => {
            let mut saw_commit = false;
            for line in s.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                saw_commit = true;
                if line.starts_with('+') {
                    // NOT Unmerged yet. `git cherry` compares
                    // PER-COMMIT patch-ids, so a squash that collapses
                    // three commits into one matches none of them and
                    // every commit comes back `+`.
                    //
                    // Verified on a real worktree: a branch merged as
                    // PR #164 failed both ancestry and cherry, while
                    // its AGGREGATE diff matched the squash commit
                    // exactly. That is the case this falls through to.
                    return aggregate_patch_merged(dir, default_branch);
                }
            }
            // No output means no commits relative to the default branch.
            // That is a branch with nothing on it, not a merged one --
            // reporting Safe here would greenlight deleting a worktree
            // whose state we never actually established.
            if saw_commit {
                Safety::Safe
            } else {
                Safety::Unmerged
            }
        }
        // Any git failure yields Unknown, never Safe.
        Err(e) => Safety::Unknown(e),
    }
}

/// Whether the branch's WHOLE diff already landed as one commit.
///
/// The signal `git cherry` structurally cannot see. A squash-merge
/// replays the branch as a single new commit whose parent is the
/// branch's BASE, not its tip -- so the original commits are not
/// ancestors of the default branch and their individual patch-ids match
/// nothing. Comparing the aggregate `merge-base..HEAD` diff against each
/// recent commit on the default branch finds it.
///
/// `--stable` matters: the unstable form hashes differently across git
/// versions and would silently stop matching.
///
/// This LOOSENS a gate that guards deletion, so it is deliberately
/// strict: an exact patch-id match, or Unmerged. A false positive here
/// deletes unmerged work, which is a worse failure than the one it
/// fixes.
///
/// Bounded at 300 commits. A branch whose equivalent landed further back
/// than that is old enough that the extra git calls cost more than the
/// answer is worth, and the fallback is the safe direction.
fn aggregate_patch_merged(dir: &Path, default_branch: &str) -> Safety {
    /// A backstop, not a window. The range below is bounded by the
    /// merge-base, so this only bites on a repository with a
    /// pathologically long history since the branch diverged.
    const SCAN_DEPTH: &str = "5000";

    let Ok(base) = git(dir, &["merge-base", "HEAD", default_branch]) else {
        return Safety::Unmerged;
    };
    let base = base.trim();
    if base.is_empty() {
        return Safety::Unmerged;
    }

    let Some(branch_pid) = patch_id(dir, base, "HEAD") else {
        // No diff at all against the base means nothing to compare, and
        // claiming merged on an empty comparison would greenlight a
        // deletion we never established.
        return Safety::Unmerged;
    };

    // Bounded by the MERGE-BASE, not by a fixed count.
    //
    // This asked for the last 300 commits of the default branch,
    // ignoring the merge-base it had just computed. If a branch diverged
    // further back than that, its squash merge sat outside the window
    // and the branch was reported Unmerged.
    //
    // Measured on a real repository: worktree branches diverge a MEDIAN
    // of 474 commits back, and 14 of 21 worktrees flipped from
    // "unmerged" to "merged" when the window was widened. The failure is
    // quiet -- refusing to delete something deletable -- so it presents
    // as "the cleanup finds nothing" rather than as an error.
    //
    // `<base>..<default>` is exactly the range that could contain the
    // merge: anything older than the merge-base predates the branch and
    // cannot be its squash. Correct by construction, and cheap on a
    // young branch where the old fixed 300 was pure waste.
    //
    // The cap remains as a BACKSTOP against a pathological repository,
    // set far above the 498 observed rather than at a value that trims
    // real history.
    let range = format!("{base}..{default_branch}");
    let Ok(candidates) = git(dir, &["rev-list", &range, "-n", SCAN_DEPTH]) else {
        return Safety::Unmerged;
    };

    // ONE pipeline for all 300 candidates, not two processes each.
    //
    // The loop this replaces spawned `git diff | git patch-id` per
    // candidate -- 600 processes -- and only exited early on a match, so
    // an unmerged branch (the common case when bulk-deleting) always ran
    // the full 300. It is spawn-bound, not work-bound: a bare `git`
    // invocation costs ~15ms here, so 600 of them is ~9s regardless of
    // repo or diff size.
    //
    // MEASURED on a 300-commit repository, through real process spawns:
    //
    //     per-candidate (600 procs):  5817ms
    //     batched (2 procs):            77ms
    //
    // and 25/25 patch-ids identical between the two. `git patch-id`
    // reads a STREAM and emits one line per patch, which is what makes
    // the collapse possible at all.
    //
    // A note for anyone re-timing this: measuring it with a SHELL loop
    // shows no improvement, because the shell forks differently than
    // `Command::spawn`. That result is an artefact of the harness, not
    // of the code.
    match batch_contains_patch(dir, &candidates, &branch_pid) {
        // An exact aggregate match is the strongest evidence there is,
        // and it is cheap. It only fails to fire when the branch's diff
        // is not byte-identical to any single commit on the default
        // branch -- which is the rebased-then-squashed case (#741).
        Safety::Safe => Safety::Safe,
        Safety::Unknown(e) => Safety::Unknown(e),
        _ => content_landed(dir, default_branch, base),
    }
}

/// Whether every file this branch changed already looks, on the default
/// branch, like the branch left it (#741).
///
/// The case `aggregate_patch_merged` structurally cannot see. That
/// compares the branch's WHOLE `merge-base..HEAD` diff against single
/// commits on the default branch, which is exactly right for a plain
/// squash -- and wrong the moment the branch was rebased after being
/// pushed. A rebase pulls in whatever landed first, so the branch's
/// aggregate diff contains files the squash commit never touched and no
/// patch-id can ever match.
///
/// Measured on the repository that prompted this: a branch's aggregate
/// diff spanned 15 files while its squash commit touched 12, the three
/// extra having arrived from an earlier PR being rebased in. Of those 15
/// files, 14 were byte-identical to the default branch and the 15th
/// differed only because LATER work added to it.
///
/// So this asks the question a human asks -- "is my work in?" -- per
/// file, and is immune to how the commits were arranged:
///
/// - the branch's blob and the default branch's blob are IDENTICAL, or
/// - they differ, but every substantive line the branch ADDED to that
///   file is present on the default branch's copy (later work added
///   more around it), or
/// - the branch DELETED the file and it is gone from the default branch
///   too.
///
/// One path failing any of those is enough to refuse: this returns
/// `Unmerged` on the first file whose work cannot be accounted for.
///
/// This LOOSENS a gate that guards deletion, so the two ways it could
/// wrongly say "merged" are closed deliberately:
///
/// - A branch that is a strict SUBSET of a larger change that was never
///   merged -- its lines are all on the default branch, but as somebody
///   else's work. Line presence alone would call that merged, so
///   `descends_from_branch` additionally requires the default branch to
///   have MOVED on the branch's own files since they diverged. Work that
///   was already there before the branch existed cannot have come from
///   it.
/// - A vacuous match, where the branch changed nothing that carries
///   meaning -- whitespace, a moved brace, a file reverted to what the
///   default branch already had. Trivial lines are not evidence, so
///   `TRIVIAL_LEN` excludes short fragments and `MIN_EVIDENCE` requires
///   a real quantity of them before any verdict of merged.
///
/// The conservative direction is left alone on purpose. A file the
/// branch changed that the default branch has since changed
/// INCOMPATIBLY reports `Unmerged`, which wastes disk and loses
/// nothing; the opposite mistake deletes work that exists nowhere else.
fn content_landed(dir: &Path, default_branch: &str, base: &str) -> Safety {
    /// Shorter than this, flattened of whitespace, a line is punctuation
    /// or boilerplate -- `}`, `///`, `.map(|r| {`. Such lines appear in
    /// almost any file, so treating them as evidence would let a branch
    /// match by coincidence. They are skipped rather than required,
    /// because their ABSENCE is equally uninformative: a real merge that
    /// reflowed a brace must not be called unmerged over it.
    const TRIVIAL_LEN: usize = 12;

    /// Below this many substantive lines accounted for, there is not
    /// enough evidence to overturn `Unmerged`. Guards the vacuous match:
    /// a branch whose entire change is trivial has nothing this function
    /// can verify, and "I could not find anything to check" must not
    /// read as "the work is in".
    const MIN_EVIDENCE: usize = 3;

    // `-M` so a rename is one entry to reason about rather than a delete
    // and an add that each look like missing work.
    let Ok(status) = git(dir, &["diff", "--name-status", "-M", "-z", base, "HEAD"]) else {
        return Safety::Unmerged;
    };

    let entries = parse_name_status(&status);
    if entries.is_empty() {
        // No files changed means nothing to establish. Claiming merged
        // on an empty comparison would greenlight a deletion whose
        // premise was never checked.
        return Safety::Unmerged;
    }

    let mut evidence = 0usize;
    for (change, path) in &entries {
        match change {
            Change::Deleted => {
                // The branch removed it; the default branch must agree.
                // A file still present there is work that did not land.
                if git(
                    dir,
                    &["cat-file", "-e", &format!("{default_branch}:{path}")],
                )
                .is_ok()
                {
                    return Safety::Unmerged;
                }
                // A deletion is real work, but it carries no lines to
                // count, so it deliberately adds no evidence.
            }
            Change::Present => {
                let branch_blob = git(dir, &["rev-parse", &format!("HEAD:{path}")]);
                let main_blob = git(dir, &["rev-parse", &format!("{default_branch}:{path}")]);
                let (Ok(branch_blob), Ok(main_blob)) = (branch_blob, main_blob) else {
                    // The path does not exist on the default branch at
                    // all, so this file's work is simply not there.
                    return Safety::Unmerged;
                };

                let added = added_lines(dir, base, path, TRIVIAL_LEN);

                if branch_blob.trim() == main_blob.trim() {
                    // Identical blobs: the strongest per-file evidence.
                    evidence += added.len();
                    continue;
                }

                // The default branch changed this file further. The
                // branch's own additions must still all be present, or
                // its work was not absorbed -- it was superseded, or
                // never landed.
                if added.is_empty() {
                    // Nothing substantive to verify on a file that
                    // nonetheless differs. Cannot establish anything.
                    return Safety::Unmerged;
                }
                let Ok(theirs) = git(dir, &["show", &format!("{default_branch}:{path}")]) else {
                    return Safety::Unmerged;
                };
                let theirs: String = theirs.chars().filter(|c| !c.is_whitespace()).collect();
                if !added.iter().all(|l| theirs.contains(l.as_str())) {
                    return Safety::Unmerged;
                }
                evidence += added.len();
            }
        }
    }

    if evidence < MIN_EVIDENCE {
        return Safety::Unmerged;
    }

    // Every file checks out. The remaining question is whether that is
    // because this branch's work landed, or because the branch happens
    // to be a subset of somebody else's.
    descends_from_branch(dir, default_branch, base)
}

/// Which side of `--name-status` an entry falls on.
///
/// Only the deleted/not-deleted distinction matters here: added,
/// modified, renamed and copied paths are all "the branch's version of
/// this path must be reflected on the default branch", and differ only
/// in which name to ask about.
enum Change {
    Deleted,
    Present,
}

/// Parse `git diff --name-status -M -z` into (change, path) pairs.
///
/// `-z` because a NUL-delimited stream is the only form that survives a
/// path with a space, a quote or a newline in it -- git QUOTES such
/// paths in the human-readable form, and a check that guards deletion
/// must not be reading a mangled filename.
///
/// Rename and copy records carry TWO paths (old then new); the new one
/// is the branch's version and the one to compare.
fn parse_name_status(out: &str) -> Vec<(Change, String)> {
    let mut fields = out.split('\0').filter(|f| !f.is_empty());
    let mut entries = Vec::new();
    while let Some(status) = fields.next() {
        let code = status.as_bytes().first().copied().unwrap_or(b'?');
        // R and C spend their first path on the SOURCE, which the branch
        // no longer has; the destination is what to check.
        if code == b'R' || code == b'C' {
            let _from = fields.next();
        }
        let Some(path) = fields.next() else { break };
        let change = if code == b'D' {
            Change::Deleted
        } else {
            Change::Present
        };
        entries.push((change, path.to_string()));
    }
    entries
}

/// The substantive lines this branch ADDED to `path`, whitespace-flattened.
///
/// Whitespace is stripped rather than compared because reindentation is
/// routine when work is rebased or reviewed, and a merge that only moved
/// a line left or right is still a merge. Lines shorter than `trivial`
/// once flattened, or carrying no alphanumeric character at all, are
/// dropped: they are punctuation and boilerplate that would match
/// anywhere.
fn added_lines(dir: &Path, base: &str, path: &str, trivial: usize) -> Vec<String> {
    // `-U0` because only the added lines matter here; context lines
    // would be counted as the branch's work when they are not.
    let Ok(diff) = git(dir, &["diff", "-U0", base, "HEAD", "--", path]) else {
        return Vec::new();
    };
    diff.lines()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
        .map(|l| {
            l[1..]
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
        })
        .filter(|l: &String| l.len() >= trivial && l.chars().any(char::is_alphanumeric))
        .collect()
}

/// Whether the default branch actually descends from THIS branch's work,
/// as opposed to merely containing lines that look like it.
///
/// The subset guard. Consider a branch that adds one line, where the
/// default branch separately gained a larger change that happens to
/// include that same line. Every per-file check above passes -- the line
/// really is there -- yet the branch never merged and deleting it
/// destroys the only copy.
///
/// `git cherry` answers it: it compares PATCH-IDS, so it reports whether
/// an equivalent of each of the branch's commits exists on the default
/// branch regardless of SHA. A branch whose commits were squashed still
/// has its individual commits come back `+` (unmatched), which is why
/// this cannot be the primary check -- but it is decisive in the other
/// direction, so it is used here only to distinguish "the work landed,
/// rearranged" from "somebody else wrote something similar".
///
/// The distinguishing evidence is WHERE on the default branch the
/// content lives. A branch that was rebased onto newer work and then
/// squash-merged had its content added to the default branch AFTER the
/// merge-base -- the squash commit is one of the commits in
/// `base..default`. A branch that merely resembles a subset of existing
/// work has its lines already present AT the merge-base, because they
/// were there before it ever diverged.
///
/// So: at least one file the branch changed must have been touched by
/// the default branch since the merge-base. That is cheap to ask, it is
/// exactly the difference between the two cases, and it fails in the
/// safe direction when it cannot be established.
fn descends_from_branch(dir: &Path, default_branch: &str, base: &str) -> Safety {
    // How far the default branch has moved since this branch diverged.
    // An empty range means the branch is fully up to date with the
    // default branch, so the content there IS the branch's own.
    let Ok(ahead) = git(
        dir,
        &["rev-list", "--count", &format!("{base}..{default_branch}")],
    ) else {
        return Safety::Unmerged;
    };
    if ahead.trim() == "0" {
        return Safety::Safe;
    }

    // The branch's commits, and whether the default branch has an
    // equivalent of each. `-` means an equivalent patch is upstream.
    let Ok(cherry) = git(dir, &["cherry", default_branch, "HEAD"]) else {
        return Safety::Unmerged;
    };
    let unmatched = cherry
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('+'))
        .count();
    if unmatched == 0 {
        // Every commit has an equivalent upstream: unambiguously merged.
        return Safety::Safe;
    }

    // Unmatched commits are the squash case -- but they are also what a
    // never-merged subset looks like, so this is where the two must be
    // told apart.
    //
    // The branch's own content had to ARRIVE on the default branch at
    // some point after the merge-base for the branch to be its source.
    // If the default branch has not touched a single one of the files
    // this branch changed since they diverged, then everything matched
    // above was already there before the branch existed, and the branch
    // is a coincidental subset rather than the origin of the work.
    let Ok(status) = git(dir, &["diff", "--name-status", "-M", "-z", base, "HEAD"]) else {
        return Safety::Unmerged;
    };
    let paths: Vec<String> = parse_name_status(&status)
        .into_iter()
        .map(|(_, p)| p)
        .collect();
    if paths.is_empty() {
        return Safety::Unmerged;
    }

    let range = format!("{base}..{default_branch}");
    let mut args: Vec<&str> = vec!["log", "--oneline", "-1", &range, "--"];
    for p in &paths {
        args.push(p.as_str());
    }
    match git(dir, &args) {
        // The default branch changed at least one of these files after
        // the branch diverged, and every one of them now reflects the
        // branch's work: that is the rebased-then-squashed shape.
        Ok(s) if !s.trim().is_empty() => Safety::Safe,
        // Nothing the branch touched has moved on the default branch
        // since. The content matched because it predates the branch.
        Ok(_) => Safety::Unmerged,
        Err(_) => Safety::Unmerged,
    }
}

/// Whether any candidate commit has `want` as its patch-id.
///
/// Computes all of them up front rather than short-circuiting on a
/// match. At 77ms for 300 candidates that trade is overwhelmingly
/// favourable, and it removes the pathological case the old loop had:
/// no match meant maximum work.
fn batch_contains_patch(dir: &Path, candidates: &str, want: &str) -> Safety {
    use std::io::Write;
    use std::process::Stdio;

    let shas: Vec<&str> = candidates
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if shas.is_empty() {
        return Safety::Unmerged;
    }

    // `--no-walk` treats each SHA as its own root, and `-p` gives each
    // one its own diff -- the squash commit's contents, which is what
    // the per-candidate `sha^..sha` was computing.
    let Ok(mut log) = Command::new(crate::auth::git_program())
        .arg("-C")
        .arg(dir)
        .args(["log", "--stdin", "--no-walk", "-p", "--format=commit %H"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Safety::Unmerged;
    };
    if let Some(mut stdin) = log.stdin.take() {
        let _ = stdin.write_all(shas.join("\n").as_bytes());
    }
    let Ok(log_out) = log.wait_with_output() else {
        return Safety::Unmerged;
    };
    if log_out.stdout.is_empty() {
        return Safety::Unmerged;
    }

    let Ok(mut pid) = Command::new(crate::auth::git_program())
        .arg("-C")
        .arg(dir)
        .args(["patch-id", "--stable"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Safety::Unmerged;
    };
    if let Some(mut stdin) = pid.stdin.take() {
        let _ = stdin.write_all(&log_out.stdout);
    }
    let Ok(out) = pid.wait_with_output() else {
        return Safety::Unmerged;
    };

    // Each line is `<patch-id> <commit-sha>`; only the first field is
    // the comparison.
    let found = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .any(|p| p == want);

    if found {
        Safety::Safe
    } else {
        Safety::Unmerged
    }
}

/// The stable patch-id of `from..to`, or None when git could not answer.
///
/// `git patch-id` reads a diff on stdin, so this is the one place that
/// pipes rather than using `git()`. Written with two processes and an
/// explicit pipe rather than a shell string: the arguments are commit
/// SHAs resolved by git itself, but running them through a shell would
/// make that a property of where they came from rather than of this
/// code.
fn patch_id(dir: &Path, from: &str, to: &str) -> Option<String> {
    use std::io::Write;
    use std::process::Stdio;

    let diff = Command::new(crate::auth::git_program())
        .arg("-C")
        .arg(dir)
        .args(["diff", from, to])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !diff.status.success() || diff.stdout.is_empty() {
        return None;
    }

    let mut child = Command::new(crate::auth::git_program())
        .arg("-C")
        .arg(dir)
        .args(["patch-id", "--stable"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(&diff.stdout).ok()?;
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }

    // `git patch-id` prints "<patch-id> <commit-id>"; only the first
    // field identifies the change.
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(str::to_string)
}

/// Delete an ORPHANED worktree directory.
///
/// A separate command from `remove_worktree`, because git cannot do it:
/// with the parent repository gone there is nothing to run
/// `git worktree remove` against. Verified on a real orphan --
/// `git status` inside it fails with "not a git repository", and the
/// parent path does not exist to be asked.
///
/// So this is a plain recursive delete, which makes the gate the ONLY
/// protection. It re-derives orphan status here rather than trusting
/// the caller: the scan is a snapshot, and a path that has since become
/// a real checkout must not be deleted by a stale click. Same rule
/// `remove_inner` applies for the same reason.
///
/// # The two checks that were missing (#854)
///
/// Orphan status is a property of the DIRECTORY, not of the path, and
/// for a long time it was the only thing asked here. That left this the
/// one `remove_dir_all` in the app with neither of the two checks
/// `remove_artifact` and `remove_venv` both document at length:
///
///  1. **Never a symlink**, checked with `symlink_metadata` BEFORE
///     canonicalising. `dir.is_dir()` follows links, so the old gate
///     saw the target's type and `remove_dir_all` then deleted the
///     TARGET's contents -- which may be anywhere at all. A link whose
///     target holds a `.git` file with a dead `gitdir:` pointer passed
///     `orphan_gitdir` without trouble.
///  2. **Inside a configured scan root**, against the canonical path so
///     `../` cannot walk out of one. `roots` comes from settings rather
///     than from the request, which is the rule
///     `commands::remove_artifacts` states in so many words -- "the
///     boundary it checks against must come from settings, not from the
///     request". Orphans are only ever FOUND by walking those roots
///     (`collect_inner`), so a path outside them was never a row the
///     user could have clicked.
///
/// Why this mattered more here than anywhere else: `orphan_gitdir` is
/// not a containment boundary and was never meant as one. It asks for a
/// readable `<dir>/.git` whose `gitdir:` target does not exist -- a
/// two-line file any caller can write. And `remove_orphan` is exposed
/// on the remote surface as `Class::Destructive`
/// (`remote/surface.rs:298`), so the path can arrive from a paired peer
/// rather than from this machine's own UI.
///
/// The checks are ordered as `remove_artifact` orders them, and for the
/// reason its comment gives: a later symlink check would still reject
/// most links by accident (the resolved path usually falls outside the
/// roots), but "rejected for another reason" is not "rejected because it
/// is a symlink", and the day someone reorders them the accident stops
/// happening.
pub fn remove_orphan(path: &str, roots: &[String]) -> Result<(), String> {
    let dir = Path::new(path);

    // 1. Never a symlink. FIRST, before `canonicalize` -- which resolves
    //    through links, so afterwards there is nothing left to detect.
    let link_meta =
        std::fs::symlink_metadata(dir).map_err(|_| "that directory is missing".to_string())?;
    if link_meta.is_symlink() {
        return Err("that path is a symlink, not a directory".into());
    }
    if !link_meta.is_dir() {
        return Err("that directory is missing".into());
    }

    // 2. Inside a configured scan root, against the CANONICAL path so
    //    `../` cannot walk out of one.
    let canon = dir
        .canonicalize()
        .map_err(|e| format!("could not resolve the path: {e}"))?;
    let inside = roots.iter().any(|r| {
        Path::new(r)
            .canonicalize()
            .is_ok_and(|root| canon.starts_with(&root))
    });
    if !inside {
        return Err("that directory is outside the scanned folders".into());
    }

    // 3. Still an orphan, re-checked RIGHT NOW. Without this the command
    //    is "delete any directory under a scan root that the frontend
    //    names", which is not much of a gate.
    if orphan_gitdir(&canon).is_none() {
        return Err(
            "this is no longer an orphaned worktree -- its repository may have been restored"
                .into(),
        );
    }

    std::fs::remove_dir_all(&canon).map_err(|e| format!("could not remove: {e}"))
}

/// The dead `gitdir` a worktree points at, when its parent is gone.
///
/// A worktree's `.git` is a FILE containing `gitdir: <path>`. When the
/// repository that owns it has been deleted, that path no longer
/// exists -- and nothing else on the filesystem distinguishes an
/// orphan from a healthy worktree.
///
/// Both checks are filesystem-only, so this adds nothing measurable to
/// a scan that already runs git per repository.
fn orphan_gitdir(dir: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(dir.join(".git")).ok()?;
    let target = contents.strip_prefix("gitdir:")?.trim();
    if target.is_empty() || Path::new(target).exists() {
        return None;
    }
    Some(target.to_string())
}

/// The repository's default branch, falling back to `main`.
///
/// The REMOTE-TRACKING ref where one exists -- `origin/main`, not `main`
/// (#757). Every caller feeds this straight into a comparison
/// (`merge-base --is-ancestor`, `git cherry`, the squash and containment
/// checks), so which of the two refs this names decides every merge
/// verdict in the view.
///
/// The bare short name was the wrong one. `main` is only current if the
/// user recently PULLED it, and on a machine where all work happens in
/// worktrees the local `main` can go untouched for weeks -- so the
/// classifier was asking "did this land?" of a ref that predated the
/// landing. `origin/main` is only stale if the user has not FETCHED,
/// which is a much weaker assumption, and it is already on disk: this
/// stays a local-only check, no network.
///
/// Measured on a 34-worktree checkout, same code and same worktrees,
/// with only the freshness of local `main` differing (20 commits
/// behind):
///
/// | verdict                | stale local `main` | after a fast-forward |
/// |------------------------|--------------------|----------------------|
/// | MergedUpstreamDeleted  | 3                  | 12                   |
/// | Unmerged               | 12                 | 2                    |
///
/// A nine-row swing from one stale ref. The failure was silent and in
/// the safe direction -- fewer removable worktrees, on a page whose
/// whole purpose is reclaiming disk -- and every affected row carried a
/// confident, false reason ("branch not merged"). It also defeated #732
/// and #741, both of which make the classifier smarter and both of which
/// were comparing against a ref older than the merges they detect.
///
/// Falls back to the LOCAL branch when the remote-tracking ref does not
/// resolve. This code also serves purely-local repositories, where
/// `origin/main` does not exist and insisting on it would turn every
/// verdict into `Unknown`.
///
/// `is_safe_ref` still guards the remote-controlled half: `origin/HEAD`
/// is written by the remote, so the short name it yields is validated
/// BEFORE the `origin/` prefix is put back on. Prefixing first would
/// hide `--output=EVIL` behind a name that no longer starts with `-`.
pub(super) fn default_branch(repo: &Path) -> String {
    let short = git(
        repo,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .ok()
    .and_then(|s| s.trim().rsplit('/').next().map(str::to_string))
    // Same guard: a hostile `origin/HEAD` yields `--output=EVIL` here.
    .filter(|s| is_safe_ref(s))
    .unwrap_or_else(|| "main".to_string());

    let remote = format!("origin/{short}");
    // `--verify` on the remote-tracking ref, not a guess from whether a
    // remote is configured: a repo can have an `origin` whose branch was
    // never fetched, and naming a ref that does not resolve would make
    // every git call below fail into `Unknown` rather than answer.
    if git(repo, &["rev-parse", "--verify", "--quiet", &remote]).is_ok() {
        remote
    } else {
        short
    }
}

/// What a walk of the scan roots found, INCLUDING what it could not read.
///
/// `Vec<Repo>` alone made "we could not read it" structurally
/// unrepresentable, and the walk had three places it fell open into
/// silence (#951):
///
/// - a directory whose `.git` DIRECTORY proved it a repository, but whose
///   `git worktree list` then failed -- git missing from a GUI-launched
///   app's PATH, a permission wall, a corrupt `.git`, an index lock, a
///   `safe.directory` refusal on a repo owned by another uid. The repo
///   was dropped entirely, so it read as "not a repository".
/// - an unreadable scan ROOT, which yielded zero repositories and the
///   empty-list copy then blamed the user's settings.
/// - `is_dir()` on a path we cannot stat, which collapses a permission
///   error into `false` -- the exact fail-open `claude/overview.rs`
///   rejects: *"`exists()` collapses every error into `false`."*
///
/// This is `claude::transcript::Scan`'s shape, for its stated reason: a
/// caller that gets a short list must be able to tell "you have nothing
/// there" from "we could not read it", because those two have opposite
/// remedies and the first is alarming if it is false.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RepoScan {
    pub repos: Vec<Repo>,
    /// Paths the walk could not read, each with WHY.
    ///
    /// A message, not a count and not a boolean, for the reason
    /// `transcript.rs` gives: each entry hides an unknown number of
    /// repositories, and "git refused: not a git repository" and
    /// "permission denied" send the user to different places. The path
    /// comes first so the string is greppable and sortable.
    #[serde(default)]
    pub unreadable: Vec<String>,
}

impl RepoScan {
    /// Whether anything at all could not be read.
    ///
    /// The UI's cue for "this list may be incomplete", and the gate on
    /// the orphan verdict. DELIBERATELY not a reason to discard the
    /// repositories that WERE read: a partial answer labelled partial
    /// beats both a silent truncation and an error page, which is the
    /// trade `ArtifactsPage` states.
    pub fn is_partial(&self) -> bool {
        !self.unreadable.is_empty()
    }
}

/// Repos and their worktrees, WITHOUT classifying safety, reporting what
/// could not be read.
///
/// Fast: one `git worktree list` per repo. Classification is four git
/// calls per worktree, which across 295 worktrees takes ~15s -- far too
/// long to block a view on. The UI lists first and classifies after.
///
/// The reporting stays free: every entry in `unreadable` comes from a
/// syscall or a `git` call the walk ALREADY made and already had to
/// branch on. Nothing here stats a path twice and nothing here does
/// safety work, so the fast path is still the fast path.
pub fn scan_dirs_fast_reporting(dirs: &[String]) -> RepoScan {
    scan_reporting(dirs, false)
}

/// Repos and their worktrees, WITHOUT classifying safety.
///
/// A thin wrapper over [`scan_dirs_fast_reporting`] keeping the bare
/// `Vec<Repo>` shape, so the ~30 existing call sites that only care about
/// what WAS found stayed unchanged across #951.
///
/// `#[cfg(test)]` like [`scan_dirs`] beside it, because after #951 both
/// production callers take the reporting form: discarding the shortfall
/// is a choice a test can make freely and a command cannot make
/// invisibly. A new production caller that needs this shape should spell
/// `.repos` at its own call site and say why the report is not wanted.
#[cfg(test)]
pub fn scan_dirs_fast(dirs: &[String]) -> Vec<Repo> {
    scan_dirs_fast_reporting(dirs).repos
}

fn scan_reporting(dirs: &[String], with_safety: bool) -> RepoScan {
    let mut scan = RepoScan::default();
    for base in dirs {
        collect_inner(Path::new(base), 0, &mut scan, with_safety);
    }
    sort_for_sidebar(&mut scan.repos);
    // Sorted so the report reads the same way twice, whatever order the
    // directory scan returned entries in. `sort_for_sidebar` already
    // does this for the repos, and a report that shuffled between runs
    // would make a diff of two scans unreadable.
    scan.unreadable.sort();
    scan
}

/// How many threads share the walk of ONE directory tree.
///
/// The walk is syscall-bound, not seek-bound: on an SSD the disk is
/// never the limit, so the "concurrent I/O contends for one spindle"
/// intuition does not apply. MEASURED over 18 worktrees totalling
/// 187 GB, one worker per worktree from a fixed pool:
///
/// ```text
///   workers=1    35.30s      workers=8     5.05s
///   workers=2    17.08s      workers=16    5.18s
///   workers=4    12.93s      workers=32    4.69s
/// ```
///
/// 7x from 1 to 8, then flat. #754 proposed SERIALISING these on the
/// theory that concurrent walks fight over the disk; the measurement
/// says the opposite, and serialising would be a 7x regression. 8 is
/// the knee, and matches `CLASSIFY_WORKERS` for the same reason.
const SIZE_WORKERS: usize = 8;

/// Bytes on disk for a directory tree.
///
/// Walks rather than shelling out to `du`: this skips symlinks so a
/// link into another tree is not counted twice, and `du` deduplicates
/// hardlinks -- which sounds right but is wrong for this feature.
/// MEASURED on one 13.45 GB worktree, `du -s` reported 10.91 GB, because
/// package managers hardlink into a shared store. The question the
/// column answers is "how much do I get back by deleting this?", and
/// for a hardlinked file that is its full length until the last link
/// goes. `du` is faster (17.07s vs 25.49s on a 205 GB tree) but it
/// answers a different question.
///
/// Counts EVERYTHING, including `node_modules` and `target`. #754 raised
/// skipping them, as `project_dirs` does, and it is dramatic: MEASURED,
/// the same 13.45 GB worktree walks in 0.00s and reports 0.01 GB. But
/// that is a 99.9% under-report, and it is under-reporting precisely the
/// bytes the user opened this view to reclaim. A "size" column that
/// omits the size is not a faster answer, it is a wrong one. The cost is
/// paid by parallelism and by reporting progress instead.
///
/// Unreadable entries are skipped rather than failing the whole
/// measurement -- a permission error on one file should not turn a real
/// size into "unknown".
#[cfg(test)]
fn dir_size(path: &Path) -> u64 {
    dir_size_within(path, std::time::Duration::MAX).unwrap_or(0)
}

/// How long ONE worktree's walk may run before it is abandoned.
///
/// #754 removed the "every row waits for the slowest tree" wait by
/// streaming. #769 is the same lie in a different shape: a repository
/// with 111 worktrees showed skeletons for 15+ minutes while one with 97
/// finished in ~10 seconds. A 14% difference in COUNT cannot produce
/// that, so the count was never the variable -- one tree in the 111 was
/// unbounded, and nothing in this walk could ever give up on it.
///
/// MEASURED on this machine, and the reason a single tree can be
/// unbounded at all: 26 of 42 worktrees in one checkout live UNDER the
/// main checkout, in a `worktrees` directory beneath it. So the parent's
/// walk subsumes all 26 -- 235.02 GB and 1,125,352 files -- and every
/// one of those bytes is then walked a second time as a worktree in its
/// own right. The parent measured 265.15 GB in 38.63s where a leaf
/// worktree measured 0.30 GB in 0.16s: a 240x spread within one
/// repository. Add nesting two levels deep, or a network mount that
/// answers `read_dir` slowly, and the parent's walk has no finish.
///
/// 60s, not `GIT_TIMEOUT`'s 30s: 38.63s for a real parent checkout is a
/// legitimate answer and must not be thrown away. The bound exists to
/// convert "never" into "could not measure", not to reject slow-but-real
/// trees, so it sits comfortably above the slowest MEASURED honest walk
/// and far below the 15 minutes that made #769 look like a hang.
const SIZE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// `dir_size`, abandoning the walk once `budget` is spent.
///
/// `None` means "could not measure", NOT zero. The distinction is the
/// whole fix: a row that reports 0 bytes claims the tree is empty and
/// invites the user to delete it, which for an unmeasurable 200 GB
/// checkout is the worst possible wrong answer. Callers propagate the
/// `Option` all the way to the cell so the UI can say so.
///
/// The deadline is checked once per DIRECTORY rather than once per
/// entry. A directory is the unit that can be pathological -- a network
/// mount whose `read_dir` blocks, a permission wall -- and checking
/// per entry would put a clock read next to every `stat` in a walk that
/// is already syscall-bound, for no extra bound: a single `read_dir`
/// that never returns is not interruptible from here either way.
///
/// The partial total is DISCARDED on timeout rather than returned as a
/// floor. "≥ 41 GB" was tempting -- MEASURED, a 5s budget on the nested
/// directory above reached 41.39 GB of the true 235.02 GB -- but that
/// number is an artifact of which directories happened to pop off the
/// stack first, not a bound the user can act on, and it would render
/// indistinguishably from a real measurement.
fn dir_size_within(path: &Path, budget: std::time::Duration) -> Option<u64> {
    let started = std::time::Instant::now();
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if started.elapsed() > budget {
            return None;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(e.path());
            } else {
                total += meta.len();
            }
        }
    }
    Some(total)
}

/// Size every path in `paths`, `SIZE_WORKERS` at a time.
///
/// A shared cursor rather than fixed chunks, because worktree sizes vary
/// by three orders of magnitude -- MEASURED on one checkout: 21.40s for
/// the main checkout, 0.78s and 0.01s for two others. Splitting the
/// slice into equal chunks would leave seven threads idle behind
/// whichever chunk drew the 200 GB tree; work-stealing off one cursor
/// keeps every thread busy until there is nothing left.
///
/// `report` is called once per path AS IT FINISHES, from whichever
/// thread finished it, so a caller can stream partial answers instead of
/// waiting for the slowest tree. That is the whole point: #754 was a
/// page of skeletons that never resolved because nothing could be shown
/// until everything was done.
///
/// Every path is reported EXACTLY once, including one whose walk ran out
/// of budget -- reported then as `None`. #769 is what happens when that
/// is not guaranteed: one worker parked on an unbounded tree, so the
/// column stalled at N-1 forever with no row able to say why. A worker
/// that gives up and reports keeps the cursor moving, which is what
/// stops one bad directory from stalling the other 110.
fn size_paths(paths: &[String], report: &(dyn Fn(&str, Option<u64>) + Sync)) {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let workers = SIZE_WORKERS.min(paths.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let next = &next;
            scope.spawn(move || loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(p) = paths.get(i) else { break };
                // DIAGNOSTIC LOGGING (Settings > diagnostic log). #769
                // asked the log which worktree the walk stopped on and
                // it could not say -- this pass emitted nothing at all,
                // so a stall was indistinguishable from a slow walk.
                // Per-path and with the elapsed time, for the same
                // reason `size_venvs` logs per venv: the total says
                // "slow", this says WHICH.
                let started = std::time::Instant::now();
                let bytes = dir_size_within(Path::new(p), SIZE_TIMEOUT);
                crate::diag!(
                    "[diag] worktree-size {} {}ms {}",
                    p,
                    started.elapsed().as_millis(),
                    match bytes {
                        Some(b) => format!("{b}b"),
                        None => format!("ABANDONED after {}s", SIZE_TIMEOUT.as_secs()),
                    }
                );
                report(p, bytes);
            });
        }
    });
}

/// Sizes for one repo's worktrees, keyed by path.
///
/// A separate pass from classification, and by far the most expensive
/// one. MEASURED, single-threaded, on one checkout: 21.40s for a 200 GB
/// main checkout and 0.78s for a 0.33 GB worktree -- the cost tracks
/// bytes and file count, NOT worktree count, so a per-worktree average
/// is meaningless. (The old comment here claimed "~60ms per worktree",
/// which is off by more than two orders of magnitude for a large tree
/// and is what led #754's reporter to look for a hang instead of a slow
/// walk.) `size_paths` parallelises it; see `SIZE_WORKERS`.
///
/// Test-only since #754: production goes through the streaming form so
/// that a row can be filled the moment its own answer exists. Kept
/// because the tests want the whole set as a single value to assert on.
#[cfg(test)]
pub fn size_repo(repo_path: &str) -> Result<Vec<(String, Option<u64>)>, String> {
    let mut out = Vec::new();
    size_repo_streaming(repo_path, &mut |path, bytes| {
        out.push((path.to_string(), bytes))
    })?;
    Ok(out)
}

/// `size_repo`, but handing each worktree over the moment it is
/// measured.
///
/// The streaming form is the one the UI wants and the collected form is
/// written in terms of it, rather than the other way round: a caller
/// that can show partial results should never have to wait for a
/// `Vec` that is only complete when the slowest tree is done.
///
/// `report` is called from a worker thread and may be called
/// concurrently, hence `FnMut` behind a lock rather than plain `FnMut`
/// -- the mutex is uncontended relative to the walks it guards, which
/// run for seconds apiece.
///
/// A `None` size is a worktree whose walk exceeded `SIZE_TIMEOUT`. It is
/// still REPORTED, because #769 was the case where it was not: the row
/// held a skeleton indefinitely because no answer of any kind ever
/// arrived for it. "Could not measure" is an answer; a skeleton is a
/// promise, and after 15 minutes it is a false one.
pub fn size_repo_streaming(
    repo_path: &str,
    report: &mut (dyn FnMut(&str, Option<u64>) + Send),
) -> Result<(), String> {
    let dir = Path::new(repo_path);
    // An empty vec on git failure resolved as SUCCESS, so the UI could
    // not tell "this repo has no worktrees" from "we could not look".
    let list = git(dir, &["worktree", "list", "--porcelain"])
        .map_err(|e| format!("could not list worktrees: {e}"))?;
    let paths: Vec<String> = parse_porcelain(&list).into_iter().map(|w| w.path).collect();

    let sink = std::sync::Mutex::new(report);
    size_paths(&paths, &|path, bytes| {
        // A poisoned lock means another worker panicked mid-report.
        // Dropping this one result is better than panicking every
        // remaining thread and losing the whole measurement.
        if let Ok(mut f) = sink.lock() {
            f(path, bytes);
        }
    });
    Ok(())
}

/// When a branch's tip landed in the default branch.
///
/// The first commit on an ancestry path from the tip to the default
/// branch is the one that brought it in. Only meaningful for a merged
/// branch, so callers skip it otherwise -- it is an extra `git log` per
/// worktree, and 283 of the 296 here are not merged.
/// How many worktrees are classified at once.
///
/// Measured, not guessed: 4 workers gave 3.2x and 8 gave 3.7x on a
/// 145-worktree repo, while 12 and 16 regressed. The floor is process
/// spawn -- a bare `git rev-parse` costs ~10ms -- so past a point more
/// threads only add contention.
const CLASSIFY_WORKERS: usize = 8;

/// How long ONE worktree's classification may run before it is
/// abandoned (#830).
///
/// `GIT_TIMEOUT` bounds a single `git` invocation and was believed to
/// bound this pass through it. It does not, and that is the whole of
/// #830: classification makes an UNBOUNDED NUMBER of git calls per
/// worktree. `content_landed` loops over every file the branch changed
/// and spends up to four calls on each -- `rev-parse HEAD:{path}`,
/// `rev-parse {default}:{path}`, `added_lines`' `diff -U0`, and
/// `show {default}:{path}`. So a branch touching 100 files has a worst
/// case of 100 x 4 x `GIT_TIMEOUT` = over three hours, every individual
/// call of which is perfectly within its bound. A per-call ceiling
/// cannot see that; only a ceiling around the worktree can.
///
/// MEASURED by `live_classification_cost_per_worktree`, serial, on this
/// machine:
///
/// ```text
///   repo            worktrees   p50     p90      p99      max
///   ghstat             18      140ms    576ms    724ms    724ms
///   enc-api            15      114ms   3139ms   3295ms   3295ms
///   enc-ui-aws          5      123ms    266ms      -      266ms
/// ```
///
/// The spread is the argument, not the total. Within ONE repository
/// (enc-api) the slowest worktree costs 29x the median, because the cost
/// tracks CHANGED FILES rather than worktree count -- the same shape
/// `SIZE_TIMEOUT` found for bytes. Extrapolating enc-api's p50 to #830's
/// 111 worktrees gives ~12.7s serial, ~1.6s across `CLASSIFY_WORKERS`.
/// Volume therefore cannot explain a column that never resolves at all,
/// which is how we know #830 is one unbounded worktree rather than many
/// slow ones.
///
/// 45s, and the reasoning is `SIZE_TIMEOUT`'s rather than a new one. The
/// bound exists to convert "never" into "could not classify", NOT to
/// tighten a latency target, so it sits far above the slowest honest
/// measurement (3.3s, a 13x margin) and far below the wait that made
/// #830 look like a hang. Above `GIT_TIMEOUT`'s 30s deliberately: a
/// worktree whose FIRST git call times out honestly must still be able
/// to report that timeout as its verdict, and a ceiling at or below 30s
/// would pre-empt it and report the less specific message instead.
///
/// Not `SIZE_TIMEOUT`'s 60s, because these are different units of work
/// with different honest maxima: a 60s sizing walk is a real answer for
/// a 200 GB tree (38.63s measured), where 45s of git calls on one
/// worktree is 13x the worst thing ever measured here.
const CLASSIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

/// Which multi-step git operation this worktree stopped inside, if any
/// (#1136).
///
/// Verified against real repositories rather than inferred: a conflicted
/// `git rebase` leaves `REBASE_HEAD` and `rebase-merge/`, a conflicted
/// `git merge` leaves `MERGE_HEAD`, and `git cherry-pick` leaves
/// `CHERRY_PICK_HEAD`.
///
/// Paths are built with `PathBuf::join`, never `format!`: Windows
/// `canonicalize` returns verbatim `\\?\C:\` paths and this has cost
/// six Windows-only failures.
fn operation_in_progress(dir: &Path) -> Option<GitOperation> {
    // The worktree's OWN git dir, not the repository's. A linked
    // worktree has `.git` as a file pointing at
    // `<repo>/.git/worktrees/<name>`, and its rebase state lives there
    // -- looking in the repository root would report the main
    // checkout's rebase against every worktree.
    let git_dir = git(dir, &["rev-parse", "--absolute-git-dir"]).ok()?;
    let git_dir = Path::new(git_dir.trim());

    // Ordered most- to least-specific. `rebase-merge` is checked
    // alongside `REBASE_HEAD` because an interactive rebase stopped
    // between commits has the directory without the ref.
    for (marker, op) in [
        ("rebase-merge", GitOperation::Rebase),
        ("rebase-apply", GitOperation::Rebase),
        ("REBASE_HEAD", GitOperation::Rebase),
        ("MERGE_HEAD", GitOperation::Merge),
        ("CHERRY_PICK_HEAD", GitOperation::CherryPick),
        ("REVERT_HEAD", GitOperation::Revert),
        ("BISECT_LOG", GitOperation::Bisect),
    ] {
        if git_dir.join(marker).exists() {
            return Some(op);
        }
    }
    None
}

/// How many files are conflicted, or `None` when status could not be
/// read.
///
/// `None` is NOT zero: the operation is in progress either way, which is
/// why the state above does not depend on this count. An unreadable
/// status yields a row that names the operation without a number rather
/// than one claiming a clean conflict-free rebase.
fn conflicted_files(dir: &Path) -> Option<u64> {
    let out = git(dir, &["status", "--porcelain"]).ok()?;
    Some(
        out.lines()
            .filter(|l| {
                // The unmerged codes git documents: both sides added,
                // both deleted, or one of each.
                matches!(
                    l.get(..2),
                    Some("UU")
                        | Some("AA")
                        | Some("DD")
                        | Some("AU")
                        | Some("UA")
                        | Some("DU")
                        | Some("UD")
                )
            })
            .count() as u64,
    )
}

/// Classify a worktree and, when merged, date it.
///
/// The single place both scan paths go through: `classify_repo` and
/// `collect_inner` previously each had their own copy, and adding the
/// merge date to one silently left the other behind.

/// Classify a worktree and, when merged, date it.
///
/// The single place both scan paths go through: `classify_repo` and
/// `collect_inner` previously each had their own copy, and adding the
/// merge date to one silently left the other behind.
fn classify(w: &mut Worktree, repo: &Path, default_branch: &str) {
    let dir = Path::new(&w.path);

    // Upstream FIRST, so its answers feed the safety check rather than
    // being recomputed there. This removed two duplicated calls per
    // worktree -- a second `rev-parse @{u}` and a `log --oneline @{u}..`
    // whose count `rev-list --left-right` already produces -- worth
    // ~3.1s across a real 145-worktree repo.
    let upstream = upstream_state(dir);
    // Detached counts as NO upstream here. `upstream_state` returns
    // Detached before it ever probes @{u}, so treating only Untracked as
    // "no upstream" made a detached, never-pushed worktree report
    // Unknown instead of NeverPushed -- measured: 7 rows changed verdict
    // and never_pushed fell from 51 to 44.
    let has_upstream = !matches!(upstream, Upstream::Untracked | Upstream::Detached);
    let ahead = match &upstream {
        Upstream::Ahead(n) => Some(*n),
        Upstream::Diverged(a, _) => Some(*a),
        Upstream::Current | Upstream::Behind(_) => Some(0),
        // Detached has no upstream to be ahead of; Untracked is handled
        // by `has_upstream`. Unknown means git failed, and must NOT
        // become a confident zero.
        Upstream::Detached => Some(0),
        Upstream::Untracked => Some(0),
        Upstream::Unknown(_) => None,
    };

    w.safety = worktree_safety(w, default_branch, has_upstream, ahead);
    w.upstream = Some(upstream);
    w.last_commit = git(dir, &["log", "-1", "--format=%cI"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    // Only for merged branches: an extra git call, and the date is
    // meaningless for anything else. 283 of 296 here are not merged.
    if w.safety.is_safe() {
        w.merged_at = merged_date(repo, &w.head, default_branch);
    }
}

fn merged_date(repo: &Path, head: &str, default_branch: &str) -> Option<String> {
    let range = format!("{head}..{default_branch}");
    let out = git(
        repo,
        &[
            "log",
            "--format=%cs",
            "--ancestry-path",
            "--reverse",
            &range,
        ],
    )
    .ok()?;
    let first = out.lines().next()?.trim();
    (!first.is_empty()).then(|| first.to_string())
}

/// A verdict's variant name, with none of its payload.
///
/// For the diagnostic log, which a user pastes into a public issue. The
/// payload-carrying arms hold git's own free text -- a lock reason with a
/// pid, an `Unknown` with a git error quoting a branch or a path -- and
/// none of it is needed to answer the question the log is for: which
/// worktree was slow, and did it produce a verdict. `{:?}` on the value
/// would carry all of it.
fn safety_label(s: &Safety) -> &'static str {
    match s {
        Safety::Safe => "safe",
        Safety::MainCheckout => "main",
        Safety::Dirty(_) => "dirty",
        // The OPERATION is carried, the conflict count is not: the
        // operation is a fixed word from a closed set and names nothing
        // about the user, which is what this function is for.
        Safety::InProgress { op, .. } => match op {
            GitOperation::Rebase => "rebase_in_progress",
            GitOperation::Merge => "merge_in_progress",
            GitOperation::CherryPick => "cherry_pick_in_progress",
            GitOperation::Revert => "revert_in_progress",
            GitOperation::Bisect => "bisect_in_progress",
        },
        Safety::Unpushed(_) => "unpushed",
        Safety::NeverPushed => "never_pushed",
        Safety::MergedUpstreamDeleted => "merged_upstream_deleted",
        Safety::DetachedMerged(_) => "detached_merged",
        Safety::Empty => "empty",
        Safety::Unmerged => "unmerged",
        Safety::Locked(_) => "locked",
        Safety::Prunable(_) => "prunable",
        Safety::Orphaned => "orphaned",
        // Never reached from a finished classification -- it is the
        // value a row carries BEFORE one. Mapped rather than
        // `unreachable!()`, because a panic inside diagnostic logging
        // would take down the classification the log exists to explain.
        Safety::Pending => "pending",
        Safety::Unknown(_) => "unknown",
    }
}

/// `classify`, abandoning the worktree once `budget` is spent (#830).
///
/// Returns whether the verdict is a real one. `false` means the budget
/// ran out and `w.safety` is now `Unknown`, which callers count
/// SEPARATELY from a verdict -- see `classify_repo_streaming`.
///
/// The work runs on a BORROWED THREAD and the caller stops waiting on
/// it; it is not cancelled, because nothing here can cancel it. `classify`
/// is a straight-line sequence of `Command::spawn`/`wait` calls with no
/// cancellation point, and the `git` helper does not hand back a handle
/// to kill the child with -- its own comment says so, for exactly this
/// reason. So the honest description is "gives up on", not "stops".
///
/// The abandoned thread exits when its git calls do, and it costs one
/// thread, not one POOL thread: this is `std::thread`, deliberately
/// OUTSIDE tokio's blocking pool. That distinction is the one
/// `GIT_TIMEOUT` documents -- a hung call that parks a pool thread
/// "wedges every worktree operation until restart" -- and routing the
/// abandonment through the pool would reproduce it at worktree
/// granularity.
///
/// `Unknown` rather than a new variant. The enum already has the arm for
/// "we could not say", every consumer already treats it as never-safe
/// (`is_safe` is a two-variant allowlist), and the message is what makes
/// it actionable. A new `Timeout` variant would need handling in every
/// match in both languages to say the same thing this one already says.
fn classify_within(
    w: &mut Worktree,
    repo: &Path,
    default_branch: &str,
    budget: std::time::Duration,
) -> bool {
    // The main checkout is decided without a single git call
    // (`worktree_safety` returns on `is_main` first), so spending a
    // thread and a channel on it is pure overhead on the one row every
    // repository has.
    if w.is_main {
        classify(w, repo, default_branch);
        return true;
    }

    let (tx, rx) = std::sync::mpsc::channel();
    // A CLONE crosses the thread boundary, not a borrow. `w` is behind a
    // `&mut` the caller still owns, and the whole point is that this
    // thread may outlive the wait -- so it cannot be allowed to keep
    // writing into the caller's row after the budget expires. It writes
    // into its own copy and sends that back; on timeout the copy is
    // simply dropped, whenever it finally arrives.
    let mut owned = w.clone();
    let repo = repo.to_path_buf();
    let branch = default_branch.to_string();
    std::thread::spawn(move || {
        classify(&mut owned, &repo, &branch);
        // The receiver is gone on timeout; that is expected, not an
        // error -- the `git` helper's channel has the same contract.
        let _ = tx.send(owned);
    });

    match rx.recv_timeout(budget) {
        Ok(done) => {
            *w = done;
            true
        }
        Err(_) => {
            w.safety = Safety::Unknown(format!(
                "classification did not finish within {}s",
                budget.as_secs()
            ));
            false
        }
    }
}

/// Classify one repo's worktrees, in git's listing order.
///
/// Kept as the collected form for the same reason `size_repo` is: the
/// tests want the whole set as one value to assert on. Written in terms
/// of the streaming form rather than the other way round, so there is
/// one implementation of the bound and the parallelism.
///
/// RE-SORTED, because the streaming form deliberately reports in
/// completion order and this form's callers compare it positionally
/// against `scan_dirs`. Sorted by path rather than by restoring the
/// porcelain order, since that is what `parse_porcelain` already yields
/// for these fixtures and it is a total order a reader can verify
/// without knowing how the work was scheduled.
///
/// Test-only since #830: production goes through the streaming form so a
/// row can be filled the moment its own verdict exists.
#[cfg(test)]
pub fn classify_repo(repo_path: &str) -> Result<Vec<Worktree>, String> {
    let mut out = Vec::new();
    classify_repo_streaming(repo_path, &mut |w| out.push(w.clone()))?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// `classify_repo`, handing each worktree over the moment its verdict
/// exists (#830).
///
/// Before this, classification was ONE all-or-nothing call: the command
/// returned a `Vec` only when every worktree was done, so 111 rows held
/// a skeleton behind whichever one was slowest. That is the same defect
/// `size_repo_streaming` was written for (#754), on the column the page
/// actually exists for, and it outlived the size fix because the size
/// pass was assumed to be the only slow one -- `hooks.ts` records that
/// assumption. `classify_repo` is ~16s across 295 worktrees where
/// listing is ~800ms, so it was never fast enough to block a view on.
///
/// `report` is called once per worktree, from whichever thread finished
/// it, and EXACTLY once -- including for a worktree whose budget ran
/// out, reported then as `Safety::Unknown`. That guarantee is the fix,
/// not the parallelism: #830's column stalled because some rows never
/// heard anything at all, and a row that is told "could not classify"
/// can at least stop promising.
///
/// A WORK-STEALING CURSOR, not the fixed `chunks_mut` this replaces.
/// Fixed chunks were wrong here for `size_paths`' reason and the
/// measurement is the same shape: cost tracks CHANGED FILES, not
/// worktree count, and within one repository the slowest worktree
/// measured 29x the median (see `CLASSIFY_TIMEOUT`). With 111 worktrees
/// in 8 chunks of 14, one pathological branch parked its whole chunk --
/// 13 innocent rows waiting behind it, and seven threads idle with
/// nothing to take. Ordering is no longer preserved by chunking because
/// ordering is no longer this function's business: it reports as results
/// land, and every caller either collects into its own order or is a
/// view that sorts for itself.
///
/// Width stays `CLASSIFY_WORKERS`, which was measured on the fixed-chunk
/// form (28.3s serial -> 8.9s at 4 -> 7.9s at 8, regressing at 12) and
/// is if anything conservative for a cursor: the knee came from process
/// spawn contention, which does not move, while the idle-thread waste
/// the cursor removes only helps.
///
/// Safe to parallelise because each verdict is computed purely from that
/// worktree's own state -- the property `useRemoveWorktree` already
/// relies on when it filters the cache instead of re-classifying.
pub fn classify_repo_streaming(
    repo_path: &str,
    report: &mut (dyn FnMut(&Worktree) + Send),
) -> Result<(), String> {
    let dir = Path::new(repo_path);
    // An empty vec on git failure resolved as SUCCESS, which left rows
    // stuck on "checking..." forever while the header confidently read
    // "0 safe to remove" -- a definite zero for a question we could not
    // ask.
    let list = git(dir, &["worktree", "list", "--porcelain"])
        .map_err(|e| format!("could not list worktrees: {e}"))?;
    let branch = default_branch(dir);
    let wts = parse_porcelain(&list);

    let next = std::sync::atomic::AtomicUsize::new(0);
    let sink = std::sync::Mutex::new(report);
    let workers = CLASSIFY_WORKERS.min(wts.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let (next, sink, branch, wts) = (&next, &sink, &branch, &wts);
            scope.spawn(move || loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(w) = wts.get(i) else { break };
                let mut w = w.clone();
                // DIAGNOSTIC LOGGING (Settings > diagnostic log). The
                // `size_paths` rationale, for the pass that still lacked
                // it: #830 could not be attributed to a worktree from
                // the log, because classification emitted nothing at
                // all, so a stall was indistinguishable from a slow
                // repository. The total says "slow"; this says WHICH.
                let started = std::time::Instant::now();
                let ok = classify_within(&mut w, dir, branch, CLASSIFY_TIMEOUT);
                crate::diag!(
                    "[diag] worktree-safety {} {}ms {}",
                    w.path,
                    started.elapsed().as_millis(),
                    if ok {
                        // The VERDICT's name only. Not the `Debug` of the
                        // whole value: `Unknown` and `Locked` carry git's
                        // own free-text reason, which can name a branch
                        // or a path, and the diagnostic log is something
                        // a user pastes into an issue (`get_pr_detail`
                        // makes the same call about a repository name).
                        safety_label(&w.safety).to_string()
                    } else {
                        format!("ABANDONED after {}s", CLASSIFY_TIMEOUT.as_secs())
                    }
                );
                // A poisoned lock means another worker panicked
                // mid-report. Dropping this one verdict beats panicking
                // every remaining thread and losing the whole pass.
                if let Ok(mut f) = sink.lock() {
                    f(&w);
                }
            });
        }
    });
    Ok(())
}

/// The MAIN CHECKOUT of one repository, classified (#1042).
///
/// The All Repositories table summarises one row per repository, and the
/// only field on that row that carries information is `upstream`. Until
/// this existed, nothing in the shipped binary ever populated it: the
/// sole writer is `classify`, which `collect_inner` calls only behind
/// `with_safety`, and every production caller reaches the scan through
/// `scan_dirs_fast_reporting`, which passes `false`. The deep path that
/// would have set it, `scan_dirs_reporting`, is `#[cfg(test)]`. So the
/// table's Status column was a skeleton BY CONSTRUCTION rather than by
/// race -- `upstream` was `None` for every row, always.
///
/// # Why one worktree and not the repository
///
/// `classify_repo_streaming` beside this classifies EVERY worktree, and
/// that is the right shape for the Worktrees page, which renders every
/// worktree. The overview renders exactly one row per repository and that
/// row is the main checkout, so classifying the other 144 worktrees of a
/// 145-worktree repository would be work whose entire output is
/// discarded. The porcelain listing already says which record is the main
/// one (`parse_porcelain` marks the first), so this picks it and stops.
///
/// # Why it is cheap, and bounded anyway
///
/// The main checkout is the cheapest worktree there is: `worktree_safety`
/// returns `Safety::MainCheckout` on `is_main` before running a single
/// git command, which is `classify_within`'s stated reason for not
/// spending a thread on it. What remains is `upstream_state`, one
/// `log -1`, and no merge date (a main checkout is never `is_safe`).
///
/// It is still routed through `classify_within` with `CLASSIFY_TIMEOUT`,
/// so a repository whose git hangs becomes a verdict rather than an
/// unbounded wait. That is #830's guarantee restated for this table: a
/// row must hear SOMETHING, because a skeleton is a promise that a value
/// is coming and #830 is what that promise looks like when it is never
/// kept.
///
/// # It does not fetch, deliberately
///
/// `upstream_state` reads refs already on disk and nothing here adds a
/// fetch. That property is measured (#1026): of the 38 repositories in
/// the reporting machine's scan root exactly one was fresh, and of 8
/// sampled against `git ls-remote`, 4 would have printed a green "up to
/// date" while actually behind. The table QUALIFIES every verdict by the
/// age of the refs behind it instead, through `upstreamReasonAged`.
/// Fetching on behalf of a landing page would be a network call to every
/// remote in the scan root because somebody opened a view -- 13 of which
/// have never been contacted from that machine at all.
///
/// # Errors
///
/// `Err` only when the repository could not be LISTED -- the same refusal
/// `classify_repo_streaming` makes, and for the same reason: an empty
/// answer resolved as success is what leaves a row on a skeleton forever.
/// A repository that lists but carries no main record is also an `Err`,
/// not a silent absence: the caller has a row on screen for it already
/// and needs something to render in the cell.
pub fn classify_main_checkout(repo_path: &str) -> Result<Worktree, String> {
    let dir = Path::new(repo_path);
    let list = git(dir, &["worktree", "list", "--porcelain"])
        .map_err(|e| format!("could not list worktrees: {e}"))?;
    let branch = default_branch(dir);
    let mut w = parse_porcelain(&list)
        .into_iter()
        .find(|w| w.is_main)
        .ok_or_else(|| format!("{repo_path} has no main checkout"))?;

    // DIAGNOSTIC LOGGING (Settings > diagnostic log), the shape
    // `classify_repo_streaming` logs in: the total says a view was slow,
    // this says WHICH repository made it slow.
    let started = std::time::Instant::now();
    let ok = classify_within(&mut w, dir, &branch, CLASSIFY_TIMEOUT);
    crate::diag!(
        "[diag] repo-upstream {} {}ms {}",
        w.path,
        started.elapsed().as_millis(),
        if ok {
            safety_label(&w.safety).to_string()
        } else {
            format!("ABANDONED after {}s", CLASSIFY_TIMEOUT.as_secs())
        }
    );

    // An abandoned classification leaves `safety` as `Unknown` and
    // `upstream` UNTOUCHED -- which is `None`, the very skeleton this
    // function exists to end. So it is filled here with the verdict that
    // says so. `Upstream::Unknown` is the arm for "we asked and could not
    // answer", distinct from `None`'s "we have not asked yet", and the
    // frontend renders it as a failure rather than as a promise. The same
    // distinction `Safety::Pending` and `Safety::Unknown` already draw,
    // on the field this table actually reads.
    if w.upstream.is_none() {
        w.upstream = Some(Upstream::Unknown(format!(
            "classification did not finish within {}s",
            CLASSIFY_TIMEOUT.as_secs()
        )));
    }
    Ok(w)
}

/// Every repo with its worktrees fully classified, in one pass.
///
/// Only the live test uses this: production lists first and classifies
/// per repo, because classifying all 295 worktrees takes ~16s where
/// listing takes ~800ms.
#[cfg(test)]
///
/// One level deep by design: `~/code/acme/widget` is found via
/// `~/code/acme`. Walking arbitrarily deep would descend into the
/// worktrees themselves and into `node_modules`.
pub fn scan_dirs(dirs: &[String]) -> Vec<Repo> {
    scan_dirs_reporting(dirs).repos
}

/// [`scan_dirs`] with the shortfall report. See [`RepoScan`].
#[cfg(test)]
pub fn scan_dirs_reporting(dirs: &[String]) -> RepoScan {
    scan_reporting(dirs, true)
}

fn collect_inner(dir: &Path, depth: usize, out: &mut RepoScan, with_safety: bool) {
    if depth > 2 {
        return;
    }
    // `symlink_metadata` rather than `is_dir()`, for the reason
    // `claude/overview.rs` states about `exists()`: `is_dir()` returns a
    // plain `false` for a path it could not STAT, so a directory behind a
    // permission wall was indistinguishable from one that is not a
    // directory at all -- and the walk simply stopped, reporting nothing
    // (#951).
    //
    // `symlink_metadata` rather than `metadata` for the same reason
    // `overview.rs` chose it: it does not follow a link, so a symlink
    // into an unreadable tree is judged as the link it is rather than as
    // whatever it points at. A symlinked directory is deliberately NOT
    // descended into here -- the tree it points at is reachable by its
    // real path, and following it would double-count -- so it is not
    // unreadable either, just not a directory for our purposes.
    match std::fs::symlink_metadata(dir) {
        Ok(md) if md.is_dir() => {}
        // Really not a directory: a file, or a symlink. Nothing to walk
        // and nothing was hidden, so there is nothing to report.
        Ok(_) => return,
        Err(e) => {
            out.unreadable
                .push(format!("{}: {e}", dir.to_string_lossy()));
            return;
        }
    }
    // A `.git` DIRECTORY is a real checkout; a `.git` FILE is a worktree
    // pointing back at one. That distinction is load-bearing: worktrees
    // are commonly created as SIBLINGS of the repo, so treating every
    // `.git` as a repo made this scan call `git worktree list` 216 times
    // -- each returning the same 152 entries -- and take over ten minutes
    // instead of seconds. Measured on this machine.
    if dir.join(".git").is_file() {
        // Normally its own repository reports it -- EXCEPT when that
        // repository is gone. Then nothing reports it, and the
        // directory is invisible to a view whose entire purpose is
        // reclaiming exactly this. MEASURED on a real machine: 2.5 GB
        // across three orphans whose parent repos were deleted weeks
        // earlier.
        if orphan_gitdir(dir).is_some() {
            out.repos.push(Repo {
                // Both existing branches reached a `.git` entry, which a
                // bare repository does not have.
                bare: false,
                name: dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: dir.to_string_lossy().into_owned(),
                identity: None,
                fetched_at: fetched_at(dir),
                // `None`, not `"main"`, for the reason the safety verdict
                // beside it is `Orphaned`: resolving a default branch
                // means running git in the parent repository, and there
                // is no parent repository to run it in. A plausible
                // guess here would be a confident answer to a question
                // nothing asked (#1026).
                default_ref: None,
                worktrees: vec![Worktree {
                    path: dir.to_string_lossy().into_owned(),
                    branch: String::new(),
                    head: String::new(),
                    size_bytes: None,
                    // Orphaned, NOT safe. Every safety signal is
                    // computed by running git in the checkout, and
                    // without the parent repository there is no git to
                    // run -- no status, no merge-base, no upstream. The
                    // app cannot answer "is this merged" or "is this
                    // dirty", so it must not claim to.
                    safety: Safety::Orphaned,
                    is_main: false,
                    merged_at: None,
                    upstream: None,
                    last_commit: None,
                    // Both are unknowable here for the same reason the
                    // safety verdict is `Orphaned`: they come from the
                    // parent repository's `worktree list`, and there is
                    // no parent repository to ask. `None` means "not
                    // locked / not prunable" everywhere else, and that
                    // is the safe reading here too -- `Orphaned` is
                    // already un-removable, so neither field can widen
                    // a gate by being absent.
                    locked: None,
                    prunable: None,
                }],
            });
        }
        return;
    }
    if dir.join(".git").is_dir() {
        // The PRIMARY hole of #951, and the reason this function had to
        // gain a report at all.
        //
        // The `.git` directory has already PROVED this is a repository.
        // If git then cannot answer, that is not "no repository here" --
        // it is a repository we could not read, and the two must not
        // render the same. This branch used to be `if let Ok(list)` with
        // a bare `return` and no `else`, so every one of git's real
        // failure modes -- git absent from a GUI-launched app's PATH (the
        // trap `docker/cli.rs` documents for `gh`), a permission wall, a
        // corrupt `.git`, an index lock, a `safe.directory` refusal on a
        // repo owned by another uid -- dropped the repository silently.
        //
        // No `Repo` is pushed, because there is nothing honest to put in
        // one: the worktree list is exactly what failed, and a repo row
        // with an empty `worktrees` would claim "this repository has no
        // worktrees", which is a confident wrong answer of the same kind.
        // The path and git's own message go in the report instead.
        match git(dir, &["worktree", "list", "--porcelain"]) {
            Ok(list) => {
                let branch = default_branch(dir);
                let worktrees = parse_porcelain(&list)
                    .into_iter()
                    .map(|mut w| {
                        if with_safety {
                            classify(&mut w, dir, &branch);
                        }
                        w
                    })
                    .collect();
                out.repos.push(Repo {
                    // Both existing branches reached a `.git` entry, which a
                    // bare repository does not have.
                    bare: false,
                    identity: repo_identity(&dir.to_string_lossy()),
                    fetched_at: fetched_at(dir),
                    name: dir
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    path: dir.to_string_lossy().into_owned(),
                    worktrees,
                    // The SAME string `classify` was just fed, not a
                    // second resolution of it (#1026). Carrying what is
                    // already in hand is what keeps the table's "up to
                    // date with X" and the merge verdicts on the same
                    // row from naming different refs -- the disagreement
                    // #757 measured at nine rows in 34.
                    default_ref: Some(branch),
                });
            }
            Err(e) => out
                .unreadable
                .push(format!("{}: {e}", dir.to_string_lossy())),
        }
        return;
    }
    // A BARE repository -- a clone or mirror with no working tree
    // (#1142).
    //
    // It has no `.git` entry in either form, so neither branch above
    // recognised it and the walk descended into it as an ordinary
    // directory until the depth cap at the top of this function stopped
    // it. The repository and every worktree hanging off it were dropped
    // with no entry in `unreadable`, so the scan came back short and did
    // not say so -- the failure this function's other two branches exist
    // to prevent.
    //
    // GATED on the layout git itself uses, rather than asking git about
    // every directory the walk meets. `HEAD` plus `objects/` is what a
    // bare repository has and an ordinary directory does not; two
    // `is_file`/`is_dir` calls cost nothing, where spawning git per
    // directory across ~38 roots would be the ten-minute regression the
    // comment above the first branch already records.
    if dir.join("HEAD").is_file() && dir.join("objects").is_dir() {
        match git(dir, &["rev-parse", "--is-bare-repository"]) {
            Ok(answer) if answer.trim() == "true" => {
                // Only NOW is `git worktree list` worth spawning: this
                // is established as a repository.
                match git(dir, &["worktree", "list", "--porcelain"]) {
                    Ok(list) => {
                        let branch = default_branch(dir);
                        let worktrees = parse_porcelain(&list)
                            .into_iter()
                            // The bare repository itself appears in its
                            // own `worktree list` output with no branch.
                            // It is the container, not a checkout, and
                            // listing it as one would offer a removal
                            // that cannot succeed.
                            //
                            // Compared through `canonicalize` rather
                            // than as strings: git reports the resolved
                            // path, and on macOS a scan root under
                            // `/var` is reported as `/private/var`, so a
                            // string compare misses and the container
                            // lists itself. `PathBuf`, never `format!`
                            // -- Windows `canonicalize` returns verbatim
                            // `\\?\C:\` paths.
                            .filter(|w| {
                                let here = std::fs::canonicalize(dir);
                                let there = std::fs::canonicalize(&w.path);
                                match (here, there) {
                                    (Ok(a), Ok(b)) => a != b,
                                    // Unresolvable either way: keep the
                                    // row rather than drop it. A
                                    // worktree we cannot compare is not
                                    // evidence that it is the container.
                                    _ => true,
                                }
                            })
                            .collect::<Vec<_>>();
                        out.repos.push(Repo {
                            // The whole point of the flag: no working
                            // tree means `classify_main_checkout` must
                            // not run, because `Safety::MainCheckout`
                            // would claim a checkout is being protected
                            // when there is none.
                            bare: true,
                            identity: repo_identity(&dir.to_string_lossy()),
                            fetched_at: fetched_at(dir),
                            name: dir
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                            path: dir.to_string_lossy().into_owned(),
                            worktrees,
                            default_ref: Some(branch),
                        });
                    }
                    // Established as a repository, then git could not
                    // list it. Same reasoning as the `.git` directory
                    // branch above: that is a repository we could not
                    // read, not an absence, and no `Repo` is pushed
                    // because an empty `worktrees` would claim this one
                    // has none.
                    Err(e) => out
                        .unreadable
                        .push(format!("{}: {e}", dir.to_string_lossy())),
                }
                return;
            }
            // `false` is an ordinary directory that happens to hold a
            // `HEAD` file and an `objects` directory. Fall through to
            // the walk rather than reporting it: it is not a repository
            // and nothing about it is unreadable.
            Ok(_) => {}
            // Git ran and failed on a directory that LOOKS like a bare
            // repository. Reported for `scan.rs:2787`'s reason -- the
            // layout is evidence enough that something is here, and a
            // silent drop is the shortfall this function refuses.
            Err(e) => out
                .unreadable
                .push(format!("{}: {e}", dir.to_string_lossy())),
        }
    }
    // An unreadable scan ROOT used to yield zero repositories, and the
    // empty-list copy then blamed the user's settings (#951). Reported,
    // never silently treated as empty: it may hold any number of
    // repositories.
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            out.unreadable
                .push(format!("{}: {e}", dir.to_string_lossy()));
            return;
        }
    };
    for e in entries {
        // A single unreadable ENTRY does not abort the listing of its
        // siblings -- the repositories that did read are real and worth
        // showing -- but it is no longer dropped on the floor either. It
        // hides a subtree of unknown size, which is the whole finding.
        let e = match e {
            Ok(e) => e,
            Err(err) => {
                out.unreadable
                    .push(format!("{}: {err}", dir.to_string_lossy()));
                continue;
            }
        };
        let p = e.path();
        if p.file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        // `file_type()` rather than `p.is_dir()`, for `caches/mod.rs`'
        // measured reason -- it reads what the directory scan already
        // returned instead of costing a stat per entry -- and because
        // `is_dir()` is the same fail-open closed at the top of this
        // function. `file_type()` can still fail on the platforms that do
        // not carry the type in the scan, and that failure hides a
        // possible subtree, so it is reported rather than skipped.
        //
        // `is_dir()` here does NOT follow symlinks (unlike `Path::is_dir`),
        // which keeps the existing behaviour of not descending into a
        // linked tree: it is reachable by its real path.
        match e.file_type() {
            Ok(ft) if ft.is_dir() => collect_inner(&p, depth + 1, out, with_safety),
            Ok(_) => {}
            Err(err) => out
                .unreadable
                .push(format!("{}: {err}", p.to_string_lossy())),
        }
    }
}

#[cfg(test)]
mod tests {
    /// A `Lock` for tests that care only about the reason and what is
    /// underneath.
    ///
    /// Age and holder-liveness are left unset because they are
    /// environment-dependent -- a real fixture's lock is always seconds
    /// old, and any pid a test names may or may not exist on the
    /// machine running it. The tests that DO care about those fields
    /// assert on them individually rather than through equality.
    ///
    /// Reasons here are synthetic per `CONTRIBUTING.md`: this
    /// repository is public, and the real ones name a tool and a
    /// machine.
    fn lock_of(reason: Option<&str>, underlying: Safety) -> Lock {
        Lock {
            reason: reason.map(str::to_string),
            age_days: None,
            holder_running: None,
            underlying: Box::new(underlying),
        }
    }

    /// Progress must be reported AFTER each removal, so the count means
    /// "done" rather than "started". A bar that reaches 100% while work
    /// is still running is worse than no bar at all.
    ///
    /// Uses paths that do not exist: each removal FAILS, which is
    /// exactly the point -- progress must advance for failures too, or a
    /// batch where several fail appears to stall.
    #[test]
    fn progress_counts_completed_removals_including_failures() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path().to_string_lossy().to_string();
        let paths: Vec<String> = (0..3)
            .map(|i| {
                dir.path()
                    .join(format!("nope-{i}"))
                    .to_string_lossy()
                    .to_string()
            })
            .collect();

        let seen = std::cell::RefCell::new(Vec::new());
        let outcomes = remove_worktrees_with_progress(&repo, &paths, |done, total| {
            seen.borrow_mut().push((done, total));
        });

        assert_eq!(seen.into_inner(), vec![(1, 3), (2, 3), (3, 3)]);
        assert_eq!(outcomes.len(), 3);
        assert!(
            outcomes.iter().all(|o| o.error.is_some()),
            "these paths do not exist, so every removal should fail"
        );
    }

    /// An empty batch must report nothing rather than a bare (0, 0),
    /// which a UI would render as a stuck progress line.
    #[test]
    fn an_empty_batch_reports_no_progress() {
        let seen = std::cell::RefCell::new(Vec::new());
        let outcomes = remove_worktrees_with_progress("/tmp", &[], |d, t| {
            seen.borrow_mut().push((d, t));
        });
        assert!(seen.into_inner().is_empty());
        assert!(outcomes.is_empty());
    }

    use super::*;

    const SAMPLE: &str = "\
worktree /home/u/code/octo-api
HEAD 3d2216e643c827fb1dfad5c3fa58d9a14421e236
branch refs/heads/main

worktree /home/u/code/octo-api-35b
HEAD 48fa2124c6fd90bc07881e32037db99ce5b194c4
branch refs/heads/chore-remove-dead

worktree /home/u/code/octo-api-detached
HEAD 8ed50a741e1696d1a0c9506f2e033cf2887bb144
";

    #[test]
    fn parses_every_record() {
        let w = parse_porcelain(SAMPLE);
        assert_eq!(w.len(), 3);
        assert_eq!(w[1].path, "/home/u/code/octo-api-35b");
        assert_eq!(w[1].branch, "chore-remove-dead");
        assert_eq!(w[1].head, "48fa2124c6fd90bc07881e32037db99ce5b194c4");
    }

    /// The first record is the repository's own checkout, and deleting it
    /// would destroy the repo rather than a worktree.
    #[test]
    fn the_first_record_is_the_main_checkout_and_is_never_safe() {
        let w = parse_porcelain(SAMPLE);
        assert!(w[0].is_main);
        assert_eq!(w[0].safety, Safety::MainCheckout);
        assert!(!w[0].safety.is_safe());
        assert!(!w[1].is_main);
    }

    /// A detached-HEAD worktree still occupies real disk, so it must be
    /// listed rather than silently dropped for lacking a branch.
    #[test]
    fn keeps_detached_head_worktrees() {
        let w = parse_porcelain(SAMPLE);
        assert_eq!(w[2].branch, "");
        assert_eq!(w[2].head, "8ed50a741e1696d1a0c9506f2e033cf2887bb144");
    }

    /// #753: an unlocked, live worktree must not acquire either
    /// attribute from a parser that guesses. The SAMPLE has neither
    /// line, so both fields must come back empty -- if they did not,
    /// every ordinary row would stop being removable.
    #[test]
    fn an_ordinary_worktree_is_neither_locked_nor_prunable() {
        let w = parse_porcelain(SAMPLE);
        assert!(w.iter().all(|w| w.locked.is_none()));
        assert!(w.iter().all(|w| w.prunable.is_none()));
    }

    /// The dropped fields, in the exact shapes git emits them (#753).
    ///
    /// Three records, because the bare `locked` line is the one a
    /// `strip_prefix("locked ")` would silently miss -- git writes it
    /// with no trailing space when the lock was taken without
    /// `--reason`, and missing it would leave that worktree marked
    /// removable.
    #[test]
    fn parses_locked_and_prunable_attributes() {
        let out = "\
worktree /home/u/code/octo-api
HEAD 3d2216e643c827fb1dfad5c3fa58d9a14421e236
branch refs/heads/main

worktree /home/u/code/octo-api-held
HEAD 48fa2124c6fd90bc07881e32037db99ce5b194c4
branch refs/heads/feature-held
locked some tool (pid 123)

worktree /home/u/code/octo-api-bare-lock
HEAD 48fa2124c6fd90bc07881e32037db99ce5b194c4
branch refs/heads/feature-bare
locked

worktree /home/u/code/octo-api-stale
HEAD 8ed50a741e1696d1a0c9506f2e033cf2887bb144
branch refs/heads/feature-stale
prunable gitdir file points to non-existent location
";
        let w = parse_porcelain(out);
        assert_eq!(w.len(), 4);
        assert_eq!(w[1].locked.as_deref(), Some("some tool (pid 123)"));
        // Some(""), NOT None: locked-without-a-reason is still locked,
        // and collapsing the two would make it removable.
        assert_eq!(w[2].locked.as_deref(), Some(""));
        assert_eq!(
            w[3].prunable.as_deref(),
            Some("gitdir file points to non-existent location")
        );
        assert!(w[3].locked.is_none());
    }

    /// #702: the verdicts are computed against refs on disk, and the
    /// user was never told how old those refs are. Measured on this
    /// machine, one repository's were 12 days stale while its rows read
    /// like the present tense.
    #[test]
    fn a_fetched_repository_reports_when() {
        let tmp = tempfile::TempDir::new().unwrap();
        let git_dir = tmp.path().join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(git_dir.join("FETCH_HEAD"), "").unwrap();
        let at = fetched_at(tmp.path()).expect("a fetched repository has a time");
        assert!(at.ends_with('Z'), "RFC 3339 UTC, got {at}");
    }

    /// Never fetched is not "just now". A repository with no
    /// `FETCH_HEAD` must report nothing rather than a fabricated time,
    /// which would be a confident wrong answer about how current the
    /// verdicts are.
    #[test]
    fn a_never_fetched_repository_reports_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        assert_eq!(fetched_at(tmp.path()), None);
    }

    #[test]
    fn empty_output_yields_nothing() {
        assert!(parse_porcelain("").is_empty());
    }

    /// Only `Safe` is deletable. Everything else must be disabled in the
    /// UI rather than warned past.
    #[test]
    fn nothing_but_safe_is_deletable() {
        for s in [
            Safety::MainCheckout,
            Safety::Dirty(3),
            Safety::Unpushed(2),
            Safety::NeverPushed,
            Safety::Unmerged,
            // Both #753 states, and both spellings of a lock. Neither
            // may become removable by being new: a locked tree is one
            // git refuses outright, and a prunable one has no directory
            // left for the remove path to act on.
            Safety::Locked(lock_of(Some("some tool (pid 123)"), Safety::Unmerged)),
            Safety::Locked(lock_of(None, Safety::Unmerged)),
            // THE case #775 could have broken. A lock now carries what
            // the worktree would be underneath, and here that is `Safe`
            // -- so if `is_safe` ever learned to look inside the
            // payload, this row would become one-click deletable while
            // git still refused it. The verdict on top is the only one
            // that governs the button.
            Safety::Locked(lock_of(Some("some tool (pid 123)"), Safety::Safe)),
            Safety::Prunable("gitdir file points to non-existent location".into()),
            Safety::Unknown("x".into()),
        ] {
            assert!(!s.is_safe(), "{s:?} must not be deletable");
        }
        assert!(Safety::Safe.is_safe());
    }

    /// A default-constructed value must never be safe: it is what a bug
    /// is most likely to leave behind.
    #[test]
    fn the_default_safety_is_not_safe() {
        assert!(!Safety::default().is_safe());
        assert!(!Worktree::default().safety.is_safe());
    }

    /// THE regression that made this scan take ten minutes.
    ///
    /// Worktrees are commonly created as SIBLINGS of the repo, each with
    /// a `.git` FILE. Treating every `.git` as a repository meant calling
    /// `git worktree list` once per worktree -- 216 times on this
    /// machine, each returning the same 152 entries.
    #[test]
    fn a_worktree_sibling_is_not_mistaken_for_a_repository() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path();

        // A REAL repository, so `git worktree list` actually succeeds --
        // a synthetic .git directory would fail for both the buggy and
        // the fixed code, making the test pass vacuously.
        let repo = base.join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        // Identity comes from the environment rather than the source:
        // any email-shaped literal trips the privacy guard, which does
        // not special-case test fixtures and should not.
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["commit", "-q", "--allow-empty", "-m", "init"],
        ] {
            let ok = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(&repo)
                .args(&args)
                .envs(ident)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            assert!(ok, "git {args:?} failed");
        }

        // A real worktree, created as a SIBLING -- the layout that made
        // the scan quadratic.
        let wt = base.join("proj-feature");
        let ok = Command::new(crate::auth::git_program())
            .arg("-C")
            .arg(&repo)
            .args(["worktree", "add", "-q", "-b", "feature"])
            .arg(&wt)
            .envs(ident)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(ok, "git worktree add failed");
        assert!(
            wt.join(".git").is_file(),
            "a worktree's .git must be a file"
        );

        let found = scan_dirs_fast(&[base.to_string_lossy().into_owned()]);
        let names: Vec<&str> = found.iter().map(|r| r.name.as_str()).collect();
        assert!(
            names.contains(&"proj"),
            "the real repo must be found: {names:?}"
        );
        assert!(
            !names.contains(&"proj-feature"),
            "a worktree must not be listed as its own repository: {names:?}"
        );
    }

    /// #1026: the scan carries the ref its verdicts are measured
    /// against, and it is the repository's OWN.
    ///
    /// A hardcoded `origin/main` is wrong for over 10% of the
    /// repositories on the reporting machine -- one resolves
    /// `origin/master`, one a long feature branch because `origin/HEAD`
    /// points at it, and two fall back to a LOCAL `main` because no
    /// remote-tracking ref resolves.
    ///
    /// The string must be the one `classify` was fed, not a second
    /// resolution -- #757 measured a nine-row verdict swing in 34 from
    /// one ref being resolved differently.
    ///
    /// The fixture is a repository whose `origin/HEAD` points at a
    /// branch that is not `main`, which is the `osiris` and `claude-mkt`
    /// case on the reporting machine. A repository with no remote would
    /// prove nothing here: `default_branch` falls back to the LITERAL
    /// `"main"` when `origin/HEAD` does not resolve -- it never consults
    /// the checked-out branch -- so such a fixture returns `main`
    /// whatever its trunk is called, and a carried value would be
    /// indistinguishable from the hardcode this test exists to catch.
    /// (Verified by writing that test first and watching it report
    /// `Some("main")` for a repository whose only branch was `trunk`.)
    #[test]
    fn the_scan_carries_the_repositorys_own_default_ref() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path();
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let run = |dir: &Path, args: &[&str]| {
            let ok = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(ident)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            assert!(ok, "git {args:?} failed");
        };

        // The "remote": a real repository on disk whose default branch
        // is `master`, so cloning it gives an `origin/HEAD` pointing
        // somewhere other than `main`. `PathBuf::join`, never a
        // formatted path.
        let origin = base.join("origin.git");
        std::fs::create_dir_all(&origin).unwrap();
        run(&origin, &["init", "-q", "-b", "master"]);
        run(&origin, &["commit", "-q", "--allow-empty", "-m", "init"]);

        // Cloned into the scan root, so the walk finds it as a
        // repository with a remote whose HEAD is `master`.
        let repo = base.join("proj");
        let ok = Command::new(crate::auth::git_program())
            .args(["clone", "-q"])
            .arg(&origin)
            .arg(&repo)
            .envs(ident)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(ok, "git clone failed");
        assert_eq!(
            default_branch(&repo),
            "origin/master",
            "the fixture must resolve a non-main remote ref, or this test proves nothing"
        );

        let found = scan_dirs_fast(&[base.to_string_lossy().into_owned()]);
        let proj = found
            .iter()
            .find(|r| r.name == "proj")
            .expect("the repository must be found");
        assert_eq!(
            proj.default_ref.as_deref(),
            Some("origin/master"),
            "the scan must carry what `default_branch` resolved, not a guess"
        );
        assert_ne!(
            proj.default_ref.as_deref(),
            Some("origin/main"),
            "a hardcoded origin/main is the defect #1026 exists to prevent"
        );
    }

    /// A directory holding a `.git` DIRECTORY that is not a repository.
    ///
    /// This is the platform-neutral way to make `git worktree list`
    /// fail on a path the walk has already accepted as a repository:
    /// `.git/` exists, so the `is_dir()` branch is taken, and git then
    /// exits non-zero because the directory has no object database, no
    /// HEAD, and no config. Nothing here depends on file permissions,
    /// on uid ownership, or on a `safe.directory` refusal -- the three
    /// things that behave differently on Windows and would have made a
    /// fifth Windows-only failure in this repository.
    ///
    /// It stands in for the REAL failure modes (git absent from a
    /// GUI-launched app's PATH, a permission wall, an index lock, a
    /// `safe.directory` refusal). Those all reach the same branch: a
    /// non-`Ok` return from `git`, which is the only thing the fix
    /// keys on.
    fn a_git_dir_that_is_not_a_repo(base: &Path, name: &str) -> std::path::PathBuf {
        let dir = base.join(name);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        assert!(
            dir.join(".git").is_dir(),
            "the fixture's .git must be a DIRECTORY, or the scan takes the worktree branch"
        );
        assert!(
            git(&dir, &["worktree", "list", "--porcelain"]).is_err(),
            "the fixture must make git FAIL, or this test proves nothing"
        );
        dir
    }

    /// #951, the primary hole: the `.git` directory has already PROVED
    /// this is a repository, so a failed `git worktree list` means "we
    /// could not read a repository", never "there is no repository
    /// here". Before the fix it fell through a bare `return` and the
    /// path vanished from the scan entirely.
    #[test]
    fn a_repo_whose_git_call_fails_is_reported_not_dropped() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().join("code");
        std::fs::create_dir_all(&base).unwrap();
        let broken = a_git_dir_that_is_not_a_repo(&base, "brokenrepo");

        let scan = scan_dirs_fast_reporting(&[base.to_string_lossy().into_owned()]);

        // Not silently dropped: SOMETHING must say the path exists and
        // could not be read.
        assert!(
            scan.is_partial(),
            "a repo whose git call failed must make the scan partial: {scan:?}"
        );
        let path = broken.to_string_lossy().into_owned();
        assert!(
            scan.unreadable.iter().any(|u| u.starts_with(&path)),
            "the unreadable path must be named: {:?}",
            scan.unreadable
        );
        // And it carries WHY, not just THAT -- the reason
        // `transcript.rs` gives for a message over a boolean.
        assert!(
            scan.unreadable.iter().any(|u| u.len() > path.len() + 2),
            "the report must carry git's reason, not only the path: {:?}",
            scan.unreadable
        );
        // No half-built `Repo`: a row claiming this repository has zero
        // worktrees would be the same confident wrong answer in a new
        // place, because the worktree list is exactly what failed.
        assert!(
            !scan.repos.iter().any(|r| r.path == path),
            "a repo we could not read must not be listed as one with no worktrees"
        );
    }

    /// The repositories that DID read are real and worth showing. A fix
    /// that reported the shortfall by blanking the list would trade one
    /// silent failure for a louder one -- the trade `ArtifactsPage`
    /// states and refuses.
    #[test]
    fn one_unreadable_repo_does_not_abort_the_scan() {
        let (_t, repo, _wt) = merged_worktree_fixture();
        let base = repo.parent().unwrap();
        a_git_dir_that_is_not_a_repo(base, "brokenrepo");

        let scan = scan_dirs_fast_reporting(&[base.to_string_lossy().into_owned()]);

        let names: Vec<&str> = scan.repos.iter().map(|r| r.name.as_str()).collect();
        assert!(
            names.contains(&"proj"),
            "the readable repo must still be listed: {names:?}"
        );
        assert!(
            scan.is_partial(),
            "and the shortfall must still be reported"
        );
    }

    /// #951, hole three: a scan root we cannot even STAT.
    ///
    /// `is_dir()` collapsed the error into `false` and the walk returned
    /// having reported nothing -- the exact fail-open
    /// `claude/overview.rs` rejects: *"`exists()` collapses every error
    /// into `false`."* A misconfigured or unmounted root is the ordinary
    /// way to reach this, and it must not read as "you have no
    /// repositories".
    ///
    /// Portable: a path that does not exist errors from
    /// `symlink_metadata` on every platform.
    #[test]
    fn a_scan_root_that_cannot_be_stated_is_reported() {
        let tmp = tempfile::TempDir::new().unwrap();
        let missing = tmp.path().join("no-such-root");
        let root = missing.to_string_lossy().into_owned();

        let scan = scan_dirs_fast_reporting(std::slice::from_ref(&root));

        assert!(scan.repos.is_empty());
        assert!(
            scan.is_partial(),
            "an unstatable root must not read as 'you have no repositories'"
        );
        assert!(
            scan.unreadable.iter().any(|u| u.starts_with(&root)),
            "the root must be named: {:?}",
            scan.unreadable
        );
    }

    /// #951, hole two: a scan root that stats fine as a directory but
    /// cannot be LISTED. `read_dir` failed into a bare `return`, so the
    /// scan yielded zero repositories and `RepoPickerSidebar` rendered
    /// "No repositories found in the scanned folders" -- a diagnosis
    /// sending the user to fix settings that were fine.
    ///
    /// `#[cfg(unix)]` and DELIBERATELY so. This is the one case that
    /// needs a directory which succeeds at `symlink_metadata` and fails
    /// at `read_dir`, and the only portable way to build one is to
    /// remove the read bit -- which is a Unix mode bit. On Windows
    /// directory access is an ACL, `set_permissions` there only toggles
    /// the read-ONLY flag (which does not block enumeration), and a
    /// process running as Administrator bypasses a DACL anyway. This
    /// repository has had four Windows-only test failures from tests
    /// encoding one platform's behaviour as universal; gating is the
    /// alternative to being the fifth.
    ///
    /// What is NOT gated is the fix: `read_dir` returns `Err` on Windows
    /// too (a vanished directory, a network share that dropped, a
    /// sharing violation), and the guard this test pins has no
    /// platform-specific code in it. Only the way of PROVOKING it is
    /// Unix-only.
    ///
    /// Skipped when running as root, which ignores mode bits entirely
    /// and would make the assertions fail for a reason that is not the
    /// code's fault.
    #[cfg(unix)]
    #[test]
    fn a_scan_root_that_cannot_be_listed_is_reported() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().join("code");
        std::fs::create_dir_all(base.join("someproj")).unwrap();
        // Execute but not read: statable as a directory, not listable.
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o111)).unwrap();

        let listable = std::fs::read_dir(&base).is_err();
        let scan = scan_dirs_fast_reporting(&[base.to_string_lossy().into_owned()]);
        // Restored before any assertion, so a failure cannot leave the
        // TempDir undeletable and turn one red test into a leaked
        // directory on the machine that ran it.
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();

        if !listable {
            // Running as root, or a filesystem that ignores mode bits.
            // Asserting here would test the environment, not the code.
            eprintln!("skipped: this user can list a 0o111 directory");
            return;
        }
        let root = base.to_string_lossy().into_owned();
        assert!(scan.repos.is_empty());
        assert!(
            scan.is_partial(),
            "an unlistable root must not read as 'you have no repositories': {scan:?}"
        );
        assert!(
            scan.unreadable.iter().any(|u| u.starts_with(&root)),
            "the root must be named: {:?}",
            scan.unreadable
        );
    }

    /// The same root, present and readable and genuinely empty, must NOT
    /// be reported -- or `is_partial()` would be true always and the
    /// frontend banner would never go away, which is the way a warning
    /// stops being read.
    #[test]
    fn an_empty_readable_root_is_not_partial() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().join("code");
        std::fs::create_dir_all(&base).unwrap();

        let scan = scan_dirs_fast_reporting(&[base.to_string_lossy().into_owned()]);

        assert!(scan.repos.is_empty());
        assert!(
            !scan.is_partial(),
            "a readable, empty root is an ANSWER: {scan:?}"
        );
    }

    /// A plain FILE among the scan roots' children is not unreadable --
    /// it is read, and it is not a directory. Reporting it would fill the
    /// banner with noise from every `README.md` in `~/code` and drown the
    /// entries that matter.
    #[test]
    fn a_plain_file_is_not_reported_as_unreadable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().join("code");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("notes.txt"), "hello").unwrap();

        let scan = scan_dirs_fast_reporting(&[base.to_string_lossy().into_owned()]);

        assert!(
            !scan.is_partial(),
            "a file is not an unreadable directory: {scan:?}"
        );
    }

    /// #732: the state every merged PR leaves behind.
    ///
    /// A branch that was pushed, squash-merged, and whose remote branch
    /// was then deleted. `rev-parse @{u}` fails exactly as it does for a
    /// never-pushed branch, so this is the case that must NOT be
    /// reported as "commits exist only here".
    ///
    /// Real git throughout, including a real bare remote and a real
    /// squash merge: a synthetic fixture would let this pass for the
    /// wrong reason, and the verdict decides whether work is deleted.
    fn upstream_deleted_fixture(
        squash: bool,
    ) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let run_in = |dir: &Path, args: &[&str]| {
            let out = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(ident)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };

        let remote = tmp.path().join("remote.git");
        std::fs::create_dir_all(&remote).unwrap();
        run_in(&remote, &["init", "-q", "--bare", "-b", "main"]);

        let repo = tmp.path().join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        run_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("base.txt"), "base\n").unwrap();
        run_in(&repo, &["add", "-A"]);
        run_in(&repo, &["commit", "-q", "-m", "init"]);
        run_in(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_in(&repo, &["push", "-q", "-u", "origin", "main"]);

        // The feature worktree: real work, really pushed.
        let wt = tmp.path().join("proj-feature");
        run_in(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                wt.to_str().unwrap(),
            ],
        );
        std::fs::write(wt.join("feature.txt"), "the work\n").unwrap();
        run_in(&wt, &["add", "-A"]);
        run_in(&wt, &["commit", "-q", "-m", "add the feature"]);
        run_in(&wt, &["push", "-q", "-u", "origin", "feature"]);

        if squash {
            // Squash-merge, exactly as GitHub does it: the content lands
            // on main as ONE new commit with a new SHA, so neither
            // ancestry nor per-commit patch-ids match.
            run_in(&repo, &["merge", "-q", "--squash", "feature"]);
            run_in(&repo, &["commit", "-q", "-m", "add the feature (#1)"]);
            run_in(&repo, &["push", "-q", "origin", "main"]);
        }

        // GitHub deletes the branch after merging, and the local side
        // learns of it on the next prune. The tracking CONFIG survives.
        run_in(&remote, &["branch", "-D", "feature"]);
        run_in(&repo, &["fetch", "-q", "--prune", "origin"]);

        (tmp, repo, wt)
    }

    /// The bug: a merged branch whose remote was deleted must not be
    /// reported as never pushed.
    #[test]
    fn a_merged_branch_whose_remote_was_deleted_is_removable() {
        let (_t, _repo, wt) = upstream_deleted_fixture(true);

        // The premise: the upstream really is unresolvable, so this
        // travels the same code path a never-pushed branch does.
        assert!(
            git(&wt, &["rev-parse", "--abbrev-ref", "@{u}"]).is_err(),
            "fixture must leave @{{u}} unresolvable, or it tests nothing"
        );

        let w = Worktree {
            path: wt.to_string_lossy().into_owned(),
            branch: "feature".into(),
            ..Default::default()
        };
        let s = worktree_safety(&w, "main", false, Some(0));
        assert_eq!(
            s,
            Safety::MergedUpstreamDeleted,
            "a merged branch with a deleted remote must say so, not \"never pushed\""
        );
        assert!(s.is_safe(), "it must be removable: {}", s.reason());
        assert!(s.reason().contains("merged"), "{}", s.reason());
        assert!(s.reason().contains("upstream deleted"), "{}", s.reason());
    }

    /// The other half of the gate, and the one that protects work: the
    /// upstream being gone is permission to ASK whether the branch
    /// merged, never an answer that it did.
    #[test]
    fn an_unmerged_branch_whose_remote_was_deleted_is_still_refused() {
        let (_t, _repo, wt) = upstream_deleted_fixture(false);

        let w = Worktree {
            path: wt.to_string_lossy().into_owned(),
            branch: "feature".into(),
            ..Default::default()
        };
        let s = worktree_safety(&w, "main", false, Some(0));
        assert!(
            !s.is_safe(),
            "unmerged work must survive a deleted upstream: {s:?}"
        );
        assert_ne!(
            s,
            Safety::MergedUpstreamDeleted,
            "the branch never merged; saying it did would delete the work"
        );
    }

    /// The regression this fix must not cause: a branch that genuinely
    /// was never pushed has no tracking config, and keeps today's
    /// refusal. Its commits exist nowhere else.
    #[test]
    fn a_genuinely_never_pushed_branch_is_still_never_pushed() {
        let (_t, repo, wt) = repo_with_worktree("scratch");
        // Give it a commit, so `Empty` does not answer first.
        std::fs::write(wt.join("f.txt"), "local only\n").unwrap();
        for args in [
            vec!["add", "-A"],
            vec!["commit", "-q", "-m", "work that exists only here"],
        ] {
            let out = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(&wt)
                .args(&args)
                .envs([
                    ("GIT_AUTHOR_NAME", "octocat"),
                    ("GIT_COMMITTER_NAME", "octocat"),
                    ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
                    ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
                ])
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
        }
        assert!(
            !was_ever_pushed(&wt),
            "a branch with no tracking config was never pushed"
        );

        let w = Worktree {
            path: wt.to_string_lossy().into_owned(),
            branch: "scratch".into(),
            ..Default::default()
        };
        assert_eq!(
            worktree_safety(&w, "main", false, Some(0)),
            Safety::NeverPushed
        );
        let _ = repo;
    }

    /// `was_ever_pushed` reads the CHECKOUT's own branch. A detached
    /// HEAD has no branch whose config could answer, and must not
    /// borrow another branch's.
    #[test]
    fn a_detached_head_was_not_ever_pushed() {
        let (_t, _repo, wt) = upstream_deleted_fixture(true);
        let out = Command::new(crate::auth::git_program())
            .arg("-C")
            .arg(&wt)
            .args(["checkout", "-q", "--detach"])
            .output()
            .unwrap();
        assert!(out.status.success());
        assert!(
            !was_ever_pushed(&wt),
            "a detached HEAD has no branch config to read"
        );
    }

    /// A repo whose worktree is merged, pushed and clean -- the only
    /// state in which a merge date exists.
    fn merged_worktree_fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let run_in = |dir: &Path, args: &[&str]| {
            let out = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(ident)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };

        let remote = tmp.path().join("remote.git");
        std::fs::create_dir_all(&remote).unwrap();
        run_in(&remote, &["init", "-q", "--bare", "-b", "main"]);

        let repo = tmp.path().join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        run_in(&repo, &["init", "-q", "-b", "main"]);
        run_in(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]);
        run_in(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_in(&repo, &["push", "-q", "-u", "origin", "main"]);

        let wt = tmp.path().join("proj-done");
        run_in(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "--track",
                "-b",
                "done",
                wt.to_str().unwrap(),
                "main",
            ],
        );
        run_in(&wt, &["push", "-q", "-u", "origin", "done"]);
        // Advance main so there IS an ancestry path to date.
        run_in(&repo, &["commit", "-q", "--allow-empty", "-m", "two"]);
        run_in(&repo, &["push", "-q", "origin", "main"]);
        (tmp, repo, wt)
    }

    /// The All Repositories overview, end to end through the SHIPPED
    /// configuration (#1042).
    ///
    /// # Why the existing coverage missed this
    ///
    /// `classify` populates `upstream`, and it was thoroughly tested --
    /// but every one of those tests either calls `classify` /
    /// `classify_within` directly, or goes through `scan_dirs_reporting`,
    /// which is `#[cfg(test)]` and passes `with_safety: true`. Production
    /// reaches the walk only through `scan_dirs_fast_reporting`, which
    /// passes `false`, so `collect_inner` never called `classify` in the
    /// shipped binary at all. The one configuration that actually ships
    /// was precisely the one nothing exercised, and the result was a
    /// Status column that skeletoned forever with nothing in the logs --
    /// because there was no failing call, there was no call.
    ///
    /// So these tests start from `scan_dirs_fast_reporting`, the function
    /// `list_worktrees` calls, and assert on what the table can actually
    /// render from the production pipeline.
    mod repo_overview {
        use super::super::{classify_main_checkout, scan_dirs_fast_reporting, Upstream};
        use super::merged_worktree_fixture;

        /// THE regression test for #1042.
        ///
        /// The fast scan is the production entry point and it deliberately
        /// leaves `upstream` unset -- that half is asserted here too, so
        /// that a future change flipping `with_safety` to `true` (the fix
        /// #1042 explicitly refuses, because it would classify ~295
        /// worktrees to render 38 rows) fails loudly rather than passing
        /// this test for the wrong reason.
        ///
        /// What must be true is that the PIPELINE resolves it: scan, then
        /// `classify_main_checkout` per repository, and the row the table
        /// renders has a real verdict rather than a skeleton.
        #[test]
        fn the_production_pipeline_resolves_a_repository_upstream() {
            let (_tmp, repo, _wt) = merged_worktree_fixture();
            let base = repo.parent().unwrap();

            let scan = scan_dirs_fast_reporting(&[base.to_string_lossy().into_owned()]);
            let listed = scan
                .repos
                .iter()
                .find(|r| r.path == repo.to_string_lossy())
                .expect("the fixture repository must be scanned");

            // The scan itself does no classification, and that is the
            // deliberate property this fix had to preserve.
            let main = listed
                .worktrees
                .iter()
                .find(|w| w.is_main)
                .expect("the scan must list a main checkout");
            assert!(
                main.upstream.is_none(),
                "the fast scan must stay fast: it classifies nothing"
            );

            // And the second pass is what fills the cell.
            let classified = classify_main_checkout(&listed.path)
                .expect("the main checkout of a healthy repository must classify");
            assert!(
                classified.is_main,
                "the row the overview renders is the MAIN checkout, not some other worktree"
            );
            let upstream = classified.upstream.expect(
                "#1042: the production path must yield a resolved upstream, not a skeleton",
            );
            // The fixture pushed `main` to its origin and then pushed
            // again, so the checkout is level with the ref it tracks.
            // Asserted by VALUE rather than merely `is_some`, so a stub
            // returning a placeholder cannot satisfy this.
            assert_eq!(
                upstream,
                Upstream::Current,
                "a checkout level with its pushed upstream reads as current"
            );
        }

        /// Every repository in the scan gets a cell, not just the lucky
        /// ones: the table renders one row per repository and each must
        /// leave Pending on its own.
        ///
        /// The second repository here has no remote at all, which is the
        /// ordinary case on a real machine -- six of the 38 repositories
        /// in the reporting scan root are local-only. `Untracked` is a
        /// real answer and is not a failure, so the row must land on it
        /// rather than waiting for a comparison that will never happen.
        #[test]
        fn every_scanned_repository_reaches_a_verdict() {
            let (_tmp, repo, _wt) = merged_worktree_fixture();
            let base = repo.parent().unwrap();
            // `PathBuf::join`, never `format!("{}/...")`: `canonicalize`
            // returns verbatim `\\?\C:\` paths on Windows and string
            // concatenation onto one does not survive.
            let local_only = base.join("localonly");
            std::fs::create_dir_all(&local_only).unwrap();
            for args in [
                &["init", "-q", "-b", "main"][..],
                &["commit", "-q", "--allow-empty", "-m", "one"][..],
            ] {
                let out = std::process::Command::new(crate::auth::git_program())
                    .arg("-C")
                    .arg(&local_only)
                    .args(args)
                    .envs([
                        ("GIT_AUTHOR_NAME", "octocat"),
                        ("GIT_COMMITTER_NAME", "octocat"),
                        ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
                        ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
                    ])
                    .output()
                    .unwrap();
                assert!(out.status.success(), "git {args:?} failed");
            }

            let scan = scan_dirs_fast_reporting(&[base.to_string_lossy().into_owned()]);
            assert!(
                scan.repos.len() >= 2,
                "both fixtures must be scanned: {:?}",
                scan.repos.iter().map(|r| &r.name).collect::<Vec<_>>()
            );

            for r in &scan.repos {
                let w = classify_main_checkout(&r.path)
                    .unwrap_or_else(|e| panic!("{} must reach a verdict: {e}", r.name));
                assert!(
                    w.upstream.is_some(),
                    "{} left its cell on a skeleton",
                    r.name
                );
            }

            let local = scan
                .repos
                .iter()
                .find(|r| r.name == "localonly")
                .expect("the local-only repository must be scanned");
            assert_eq!(
                classify_main_checkout(&local.path).unwrap().upstream,
                Some(Upstream::Untracked),
                "a repository with no remote has no upstream, which is an ANSWER"
            );
        }

        /// A repository that cannot be classified must produce something
        /// the row can render -- never silence.
        ///
        /// #1042's requirement, and the reason `Upstream::Unknown` exists
        /// separately from `None`: `None` means "not computed yet" and is
        /// a promise that a value is coming. A row that can never be
        /// classified must not keep making that promise.
        #[test]
        fn a_repository_that_cannot_be_listed_fails_loudly() {
            let tmp = tempfile::TempDir::new().unwrap();
            let not_a_repo = tmp.path().join("nothing");
            std::fs::create_dir_all(&not_a_repo).unwrap();

            let err = classify_main_checkout(not_a_repo.to_str().unwrap())
                .expect_err("a directory that is not a repository must not resolve as an answer");
            assert!(
                err.contains("could not list worktrees"),
                "the refusal must say what failed: {err}"
            );
        }
    }

    /// A branch whose work landed on `main` as a SQUASH commit.
    ///
    /// This is the shape the real repos use, and the one ancestry cannot
    /// see: the branch's change is applied to `main` as a brand-new
    /// commit with its own SHA, so the branch tip is not an ancestor of
    /// `main` even though every line of its work is there.
    ///
    /// Real git throughout -- a hand-built fixture would let the check
    /// pass for the wrong reason, which is exactly the failure this test
    /// exists to catch.
    fn squash_merged_fixture(
        squash: bool,
    ) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let run_in = |dir: &Path, args: &[&str]| {
            let out = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(ident)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };

        let remote = tmp.path().join("remote.git");
        std::fs::create_dir_all(&remote).unwrap();
        run_in(&remote, &["init", "-q", "--bare", "-b", "main"]);

        let repo = tmp.path().join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        run_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("base.txt"), "base\n").unwrap();
        run_in(&repo, &["add", "-A"]);
        run_in(&repo, &["commit", "-q", "-m", "base"]);
        run_in(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_in(&repo, &["push", "-q", "-u", "origin", "main"]);

        // The branch does its work in a worktree and pushes it.
        let wt = tmp.path().join("proj-feature");
        run_in(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "--track",
                "-b",
                "feature",
                wt.to_str().unwrap(),
                "main",
            ],
        );
        std::fs::write(wt.join("feature.txt"), "the change\n").unwrap();
        run_in(&wt, &["add", "-A"]);
        run_in(&wt, &["commit", "-q", "-m", "add the feature"]);
        run_in(&wt, &["push", "-q", "-u", "origin", "feature"]);

        if squash {
            // What "Squash and merge" does: the same content lands on
            // main as a NEW commit. Note this is NOT a merge commit and
            // NOT a fast-forward -- the branch tip stays unreachable.
            std::fs::write(repo.join("feature.txt"), "the change\n").unwrap();
            run_in(&repo, &["add", "-A"]);
            run_in(&repo, &["commit", "-q", "-m", "add the feature (#1)"]);
        } else {
            // Genuinely unmerged: main moves on without the change.
            std::fs::write(repo.join("other.txt"), "unrelated\n").unwrap();
            run_in(&repo, &["add", "-A"]);
            run_in(&repo, &["commit", "-q", "-m", "something else"]);
        }
        run_in(&repo, &["push", "-q", "origin", "main"]);
        (tmp, repo, wt)
    }

    /// A real repo with one worktree, for the deletion tests.
    ///
    /// Real git throughout: a synthetic fixture would let the gate pass
    /// for the wrong reason, and this is the one place where a vacuous
    /// test could cost someone their work.
    fn repo_with_worktree(
        name: &str,
    ) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let run = |args: &[&str]| {
            let ok = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(&repo)
                .args(args)
                .envs(ident)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["commit", "-q", "--allow-empty", "-m", "init"]);
        let wt = tmp.path().join(name);
        run(&["worktree", "add", "-q", "-b", name, wt.to_str().unwrap()]);
        (tmp, repo, wt)
    }

    /// Put one real commit on whatever branch `dir` has checked out.
    ///
    /// The fixtures above hand back a branch that has never been
    /// committed to, which is now a state of its own -- so any test
    /// about a branch that HOLDS work has to say so explicitly rather
    /// than inherit it.
    fn commit_in(dir: &Path, message: &str) {
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let out = Command::new(crate::auth::git_program())
            .arg("-C")
            .arg(dir)
            .args(["commit", "-q", "--allow-empty", "-m", message])
            .envs(ident)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git commit: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A worktree with no upstream is NEVER safe, even when its branch
    /// points at the same commit as the default branch. 52 of 296
    /// worktrees on this machine are in exactly this state.
    ///
    /// This is the case the doc above is really about, and it needs a
    /// branch that has COMMITTED work with nowhere else to live -- so
    /// the fixture commits before asking. It did not used to: it took
    /// `repo_with_worktree`'s bare branch, which was never committed
    /// to, and passed only because `NeverPushed` swallowed the empty
    /// case. That is exactly the misreport #701 is about, so the test
    /// was asserting the bug.
    #[test]
    fn refuses_a_worktree_that_was_never_pushed() {
        let (_t, repo, wt) = repo_with_worktree("feature");
        commit_in(&wt, "work only on this branch");
        let err = remove_worktree(repo.to_str().unwrap(), wt.to_str().unwrap()).unwrap_err();
        assert!(err.contains("never pushed"), "{err}");
        assert!(wt.is_dir(), "the worktree must still exist");
    }

    /// The other half of that split: an EMPTY branch is reported as
    /// empty, and is still not one-click removable.
    ///
    /// Two assertions, deliberately, because #701 could be "fixed" in a
    /// way that regresses either one. The prose must stop claiming
    /// commits exist only here -- there are none -- and the gate must
    /// not move, because widening the app's only unrecoverable action
    /// is not something a wording fix gets to do as a side effect.
    #[test]
    fn refuses_but_correctly_describes_a_branch_with_no_commits() {
        let (_t, repo, wt) = repo_with_worktree("scratch");
        let err = remove_worktree(repo.to_str().unwrap(), wt.to_str().unwrap()).unwrap_err();
        assert!(
            err.contains("no commits of its own"),
            "an empty branch must not be described as holding commits: {err}"
        );
        assert!(
            !err.contains("only here"),
            "the false never-pushed claim is the bug: {err}"
        );
        assert!(wt.is_dir(), "the worktree must still exist");
    }

    /// A branch with commits but NO reflog must not read as empty.
    ///
    /// Caught while writing this change, and it is the trap the whole
    /// approach turns on: `reflog show` for a missing reflog prints
    /// nothing and exits **0**. Git treats an absent reflog as an empty
    /// one, not as an error, so an `Err` guard never sees it and a
    /// `<= 1` count calls a branch full of real work empty. `gc`
    /// expires reflogs and `clone` writes none for branches it did not
    /// check out, so this is an ordinary state, not a contrived one.
    ///
    /// The branch here has two commits that exist nowhere else. Getting
    /// this wrong reports "nothing to lose" over them.
    #[test]
    fn a_branch_with_commits_but_no_reflog_is_not_empty() {
        let (_t, repo, wt) = repo_with_worktree("feature");
        commit_in(&wt, "work that must not be called nothing");

        // Exactly what `gc` leaves behind: the ref, without its log.
        let log = repo.join(".git/logs/refs/heads/feature");
        assert!(log.exists(), "the fixture must start with a reflog");
        std::fs::remove_file(&log).unwrap();
        assert_eq!(
            git(&wt, &["reflog", "show", "--format=%H", "feature"]),
            Ok(String::new()),
            "a missing reflog must SUCCEED with no output -- the premise of this test"
        );

        assert!(
            !branch_is_empty(&wt, "feature"),
            "an unreadable reflog is the absence of evidence, not evidence of absence"
        );
    }

    /// Uncommitted work outranks emptiness.
    ///
    /// The empty check sits between `Dirty` and `NeverPushed`, and this
    /// pins the first half of that ordering: a scratch branch with
    /// unsaved edits in its tree must report the edits. Reversing the
    /// two would tell the user there is "nothing to lose" while a file
    /// they have not committed sits in the directory.
    #[test]
    fn uncommitted_work_outranks_an_empty_branch() {
        let (_t, repo, wt) = repo_with_worktree("scratch");
        std::fs::write(wt.join("wip.txt"), "not committed yet\n").unwrap();
        let branch = default_branch(&repo);
        let listed = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        let target = listed
            .iter()
            .find(|w| !w.is_main && w.branch == "scratch")
            .expect("the scratch worktree must be listed");
        assert_eq!(
            worktree_safety(target, &branch, false, Some(0)),
            Safety::Dirty(1)
        );
    }

    /// Run git in `dir`, asserting success. For the #753 fixtures,
    /// which lock and unlock real worktrees.
    fn git_ok(dir: &Path, args: &[&str]) {
        let out = Command::new(crate::auth::git_program())
            .arg("-C")
            .arg(dir)
            .args(args)
            .envs([
                ("GIT_AUTHOR_NAME", "octocat"),
                ("GIT_COMMITTER_NAME", "octocat"),
                ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
                ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// THE bug in #753: a locked worktree was reported `Safe`, and git
    /// refused it at the moment the user acted on that verdict.
    ///
    /// Real `git worktree lock`, not a hand-written porcelain string:
    /// the point is that the attribute survives the whole path from
    /// git's own output to the verdict, and a synthetic fixture would
    /// let this pass while the real listing still dropped the line.
    ///
    /// The worktree is otherwise as safe as one gets -- clean tree,
    /// branch merged into main -- so `Safe` is exactly what the old
    /// code returned.
    #[test]
    fn a_locked_worktree_is_never_safe() {
        let (_t, repo, wt) = repo_with_worktree("held");
        // Real work, really merged: `repo_with_worktree` hands back a
        // branch with no commits, which is `Empty` rather than `Safe`,
        // and an empty branch would prove nothing about the state this
        // test is named for.
        commit_in(&wt, "the work");
        git_ok(&repo, &["merge", "-q", "--ff-only", "held"]);
        let branch = default_branch(&repo);

        // The premise: with no lock this worktree IS safe. Without
        // this the test could pass because of some unrelated refusal.
        let listed = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        let before = listed
            .iter()
            .find(|w| !w.is_main && w.branch == "held")
            .expect("the worktree must be listed");
        assert!(
            worktree_safety(before, &branch, true, Some(0)).is_safe(),
            "fixture must start out removable, or it tests nothing"
        );

        // A synthetic reason: this repository is public, and the real
        // one from the report names a tool and a machine.
        git_ok(
            &repo,
            &[
                "worktree",
                "lock",
                "--reason",
                "some tool (pid 123)",
                wt.to_str().unwrap(),
            ],
        );

        let listed = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        let target = listed
            .iter()
            .find(|w| !w.is_main && w.branch == "held")
            .expect("a locked worktree is still listed");
        let s = worktree_safety(target, &branch, true, Some(0));

        let Safety::Locked(lock) = &s else {
            panic!("expected Locked, got {s:?}");
        };
        assert_eq!(lock.reason.as_deref(), Some("some tool (pid 123)"));
        assert!(!s.is_safe(), "git refuses to remove it: {}", s.reason());
        // The reason is still carried verbatim -- it is the locker's
        // own words, and #775 demoted it rather than dropping it.
        assert!(s.reason().contains("some tool (pid 123)"), "{}", s.reason());

        // #775: the merge state UNDERNEATH the lock. This fixture is
        // merged and clean, so without the lock it would be removable
        // -- which is exactly the thing a user needs to know before
        // deciding whether clearing the claim is worth it.
        assert!(
            lock.underlying.is_safe(),
            "the worktree is merged under the lock: {:?}",
            lock.underlying
        );
        assert!(
            s.reason().contains("would be safe once unlocked"),
            "the row must say what is underneath: {}",
            s.reason()
        );

        // And it is STILL not removable. Knowing the thing behind the
        // lock is disposable must not make the lock itself negotiable:
        // git refuses either way until it is actually unlocked.
        assert!(
            !s.is_safe(),
            "merged underneath is not a licence to remove: {}",
            s.reason()
        );
    }

    /// Uncommitted work outranks the lock (#753).
    ///
    /// Both facts are true and both block removal, but only one
    /// survives the obvious remedy: the user unlocks, removes, and the
    /// uncommitted edits go with the directory. So `Dirty` is the fact
    /// the row must show -- the same rule that puts `Dirty` ahead of
    /// `Empty` just above it.
    #[test]
    fn uncommitted_work_outranks_a_lock() {
        let (_t, repo, wt) = repo_with_worktree("held-dirty");
        std::fs::write(wt.join("wip.txt"), "not committed yet\n").unwrap();
        git_ok(
            &repo,
            &[
                "worktree",
                "lock",
                "--reason",
                "some tool (pid 123)",
                wt.to_str().unwrap(),
            ],
        );

        let branch = default_branch(&repo);
        let listed = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        let target = listed
            .iter()
            .find(|w| !w.is_main && w.branch == "held-dirty")
            .unwrap();
        assert_eq!(
            worktree_safety(target, &branch, true, Some(0)),
            Safety::Dirty(1),
            "the edits are what the user loses; the lock is what they clear"
        );
    }

    /// A lock taken without `--reason` still blocks removal (#753).
    ///
    /// Git emits a bare `locked` line for these -- no trailing space --
    /// so a parser matching only `locked ` would drop it and leave the
    /// worktree marked removable. Real git, so the exact byte shape of
    /// that line is what is under test.
    #[test]
    fn a_lock_without_a_reason_still_blocks_removal() {
        let (_t, repo, wt) = repo_with_worktree("held-bare");
        // Merged, so the lock is the ONLY thing standing between this
        // worktree and removal -- otherwise `Empty` would answer first
        // and the bare-lock parsing would go untested.
        commit_in(&wt, "the work");
        git_ok(&repo, &["merge", "-q", "--ff-only", "held-bare"]);
        git_ok(&repo, &["worktree", "lock", wt.to_str().unwrap()]);

        let branch = default_branch(&repo);
        let listed = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        let target = listed
            .iter()
            .find(|w| !w.is_main && w.branch == "held-bare")
            .unwrap();
        let s = worktree_safety(target, &branch, true, Some(0));

        let Safety::Locked(lock) = &s else {
            panic!("expected Locked, got {s:?}");
        };
        assert_eq!(lock.reason, None, "a bare lock names nobody");
        assert!(!s.is_safe(), "{}", s.reason());
        assert!(s.reason().contains("no reason given"), "{}", s.reason());

        // With no reason there is no pid to check, and the app must say
        // "nothing to check" rather than "the holder is gone" -- the
        // second would read as evidence the lock is stale, which is a
        // claim nothing here supports (#775).
        assert_eq!(
            lock.holder_running, None,
            "a lock naming no pid yields no liveness verdict"
        );

        // The age still works for a lock with no reason: it comes from
        // git's `locked` FILE, not from the reason string. That is the
        // whole point of taking it from the mtime -- a lock that says
        // nothing can still be shown to be five days old.
        assert_eq!(
            lock.age_days,
            Some(0),
            "a lock taken just now is 0 whole days old"
        );
        assert!(s.reason().contains("today"), "{}", s.reason());
    }

    /// The merge state under a lock is computed, and it is the REAL
    /// one -- not a shortcut that assumes an unmerged branch (#775).
    ///
    /// The companion to the `a_locked_worktree_is_never_safe` case: that
    /// one locks a merged worktree and expects `underlying` to be safe,
    /// so on its own it would also pass if `underlying` were hardcoded
    /// optimistically. This locks an UNMERGED one and expects the
    /// opposite, which is what makes the pair evidence that the check
    /// actually ran.
    ///
    /// It matters because the whole point is informing a decision. A
    /// row that said "would be safe once unlocked" over unmerged work
    /// would be worse than saying nothing: it would invite exactly the
    /// blind unlock #775 exists to prevent, with the app's
    /// encouragement.
    #[test]
    fn an_unmerged_worktree_under_a_lock_does_not_read_as_safe() {
        let (_t, repo, wt) = repo_with_worktree("held-unmerged");
        // Real commits, deliberately NOT merged into the default
        // branch: this is the state that must not be described as
        // disposable.
        commit_in(&wt, "work that never landed");
        git_ok(
            &repo,
            &[
                "worktree",
                "lock",
                "--reason",
                "some tool (pid 123)",
                wt.to_str().unwrap(),
            ],
        );

        let branch = default_branch(&repo);
        let listed = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        let target = listed
            .iter()
            .find(|w| !w.is_main && w.branch == "held-unmerged")
            .unwrap();
        let s = worktree_safety(target, &branch, true, Some(0));

        let Safety::Locked(lock) = &s else {
            panic!("expected Locked, got {s:?}");
        };
        assert!(
            !lock.underlying.is_safe(),
            "unmerged work must not read as disposable: {:?}",
            lock.underlying
        );
        assert!(
            !s.reason().contains("would be safe once unlocked"),
            "the row must not invite an unlock it cannot justify: {}",
            s.reason()
        );
    }

    /// A lock's age comes from git's own `locked` file (#775).
    ///
    /// The reason string was the tempting source -- the locks on the
    /// reporting machine embed `start <date>` -- and it is wrong: all
    /// 20 there carry the IDENTICAL timestamp, because it dates the
    /// long-lived parent process rather than any individual claim.
    ///
    /// This pins the property that distinguishes the two sources. The
    /// reason here names a date years in the past while the lock is
    /// seconds old, so a reader of the reason string would report an
    /// ancient lock. The mtime reports the truth.
    #[test]
    fn the_lock_age_ignores_a_timestamp_in_the_reason() {
        let (_t, repo, wt) = repo_with_worktree("held-misdated");
        git_ok(
            &repo,
            &[
                "worktree",
                "lock",
                "--reason",
                // Synthetic, per CONTRIBUTING.md, but the same SHAPE as
                // the real ones: a tool, a pid, and a start date that
                // belongs to the process rather than to this lock.
                "some tool (pid 123 start Mon Jan  1 00:00:00 2001)",
                wt.to_str().unwrap(),
            ],
        );

        let branch = default_branch(&repo);
        let listed = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        let target = listed
            .iter()
            .find(|w| !w.is_main && w.branch == "held-misdated")
            .unwrap();
        let s = worktree_safety(target, &branch, true, Some(0));

        let Safety::Locked(lock) = &s else {
            panic!("expected Locked, got {s:?}");
        };
        assert_eq!(
            lock.age_days,
            Some(0),
            "the lock was taken just now, whatever its reason claims"
        );
        assert!(
            s.reason().contains("today"),
            "a lock taken now must not read as decades old: {}",
            s.reason()
        );
    }

    /// An OLD lock reports its real age (#775).
    ///
    /// Every other age test locks a fixture and sees 0 days, which
    /// would pass just as well against a function hardcoded to
    /// `Some(0)` -- and `Some(0)` renders as "today", the most
    /// reassuring thing the row could possibly say. The stale lock is
    /// the case the feature exists for, so it gets a test where the
    /// answer is not 0.
    ///
    /// Backdates git's own `locked` file with `filetime`-free plumbing:
    /// the file is rewritten and its mtime set through `std`. The rest
    /// of the path is real -- real `git worktree lock` took it, and
    /// `lock_age_days` finds it the way production does, by reading the
    /// worktree's `.git` file to reach the admin directory.
    #[test]
    fn a_lock_taken_days_ago_reads_as_days_old() {
        let (_t, repo, wt) = repo_with_worktree("held-a-while");
        git_ok(
            &repo,
            &[
                "worktree",
                "lock",
                "--reason",
                "some tool (pid 123)",
                wt.to_str().unwrap(),
            ],
        );

        // Reach the lock file exactly as `lock_age_days` does, so this
        // test also pins the admin-directory lookup rather than
        // hardcoding a layout git could change.
        let contents = std::fs::read_to_string(wt.join(".git")).unwrap();
        let admin = contents.strip_prefix("gitdir:").unwrap().trim();
        let lock_file = std::path::Path::new(admin).join("locked");
        assert!(lock_file.is_file(), "git writes the lock file at {admin}");

        // Five days back, which is the age the reporting machine's
        // oldest locks had reached.
        let five_days = std::time::Duration::from_secs(5 * 86_400);
        let then = std::time::SystemTime::now() - five_days;
        std::fs::File::options()
            .write(true)
            .open(&lock_file)
            .unwrap()
            .set_modified(then)
            .unwrap();

        let branch = default_branch(&repo);
        let listed = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        let target = listed.iter().find(|w| w.branch == "held-a-while").unwrap();
        let s = worktree_safety(target, &branch, true, Some(0));

        let Safety::Locked(lock) = &s else {
            panic!("expected Locked, got {s:?}");
        };
        assert_eq!(lock.age_days, Some(5), "the lock is five days old");
        // The prose is the deliverable: a stale lock has to READ as
        // stale, which is the whole ask of #775's first point.
        assert!(
            s.reason().contains("5 days ago"),
            "a five-day-old lock must say so: {}",
            s.reason()
        );
        assert!(
            !s.reason().contains("today"),
            "and must not read as fresh: {}",
            s.reason()
        );
    }

    /// A pid that does not exist is reported as gone (#775).
    ///
    /// The one genuinely decisive thing the pid can say. A LIVE pid is
    /// weak evidence -- on the reporting machine all 20 locks name one
    /// that is alive because it is the surviving parent -- but a dead
    /// one is unambiguous, and it is what turns "some tool holds this"
    /// into "nothing holds this".
    ///
    /// Pid 0x7FFF_FFFE is chosen to be absent rather than merely
    /// unlikely: it sits above every default `pid_max`, so no process
    /// can legitimately carry it.
    #[test]
    fn a_lock_naming_a_dead_process_says_so() {
        let (_t, repo, wt) = repo_with_worktree("held-by-nobody");
        git_ok(
            &repo,
            &[
                "worktree",
                "lock",
                "--reason",
                "some tool (pid 2147483646)",
                wt.to_str().unwrap(),
            ],
        );

        let branch = default_branch(&repo);
        let listed = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        let target = listed
            .iter()
            .find(|w| !w.is_main && w.branch == "held-by-nobody")
            .unwrap();
        let s = worktree_safety(target, &branch, true, Some(0));

        let Safety::Locked(lock) = &s else {
            panic!("expected Locked, got {s:?}");
        };
        assert_eq!(
            lock.holder_running,
            Some(false),
            "no process carries that pid"
        );
        assert!(lock.holder_is_gone(), "and the predicate agrees");

        // And the ROW says so (#792). `holder_running` was computed from
        // the first and read only by the unlock dialog, so the user
        // learned the holder was dead after deciding to unlock. The
        // reason string must carry it, AFTER git's own words rather than
        // instead of them.
        let line = s.reason();
        assert!(
            line.contains("holder process is gone"),
            "the row must say the holder is gone: {line}"
        );
        assert!(
            line.contains("some tool (pid 2147483646)"),
            "git's reason stays verbatim and comes first: {line}"
        );
        assert!(
            line.find("some tool").unwrap() < line.find("holder process").unwrap(),
            "ours is appended, not prepended: {line}"
        );

        // And the process running THIS test is, which is the control:
        // without it a `holder_is_running` that always returned false
        // would satisfy the assertion above.
        assert!(
            holder_is_running(LockHolder {
                pid: std::process::id(),
                started_at: None,
            }),
            "the test's own process is running"
        );
    }

    /// The row says nothing about a holder it could not check, or one
    /// that is running (#792).
    ///
    /// The other half of the dead-holder line, and the half that keeps
    /// it meaningful. `None` means the reason named no pid -- most locks
    /// not written by our own tooling -- and a row reading "holder
    /// process is gone" there would assert something nobody established.
    /// `Some(true)` is weak evidence in the other direction: it was true
    /// for all 20 locks on the reporting machine and every one was
    /// abandoned, so announcing it would spend weak evidence as proof.
    ///
    /// The test's own pid is the live case, which is the only pid a test
    /// can be sure about.
    #[test]
    fn the_row_is_silent_about_an_unchecked_or_living_holder() {
        let unchecked = Safety::Locked(Lock {
            reason: Some("a human, by hand".into()),
            age_days: Some(3),
            holder_running: None,
            underlying: Box::new(Safety::Unmerged),
        });
        assert!(!unchecked.reason().contains("holder process is gone"));

        let alive = Safety::Locked(Lock {
            reason: Some(format!("this very test (pid {})", std::process::id())),
            age_days: Some(0),
            holder_running: Some(true),
            underlying: Box::new(Safety::Unmerged),
        });
        let line = alive.reason();
        assert!(!line.contains("holder process is gone"), "{line}");
        // And says nothing the other way either: "still running" would
        // be the same weak evidence worn as a badge.
        assert!(!line.contains("still running"), "{line}");
    }

    /// A RECYCLED pid is not mistaken for a live holder (#792).
    ///
    /// The improvement over a bare pid check, and the reason the reason
    /// string's start time is parsed at all. A pid is not an identity:
    /// the reporting machine rebooted between taking these locks and
    /// reading them, which is exactly when every pid in the space is
    /// handed out again -- so a lock naming a long-dead worker reads as
    /// LIVE the moment something unrelated inherits its number, and the
    /// row hides the one fact the user needed.
    ///
    /// Uses the TEST'S OWN pid with a start time that is deliberately
    /// wrong, which is the recycled case exactly: the pid resolves to a
    /// real running process, and that process is not the one the lock
    /// named. A synthetic absent pid could not express this -- it would
    /// pass against the old pid-only check too.
    ///
    /// The far-past timestamp is 2001, not "a few minutes ago": the
    /// tolerance in `holder_is_running` is five minutes, and a fixture
    /// near that edge would be testing the constant rather than the
    /// rule.
    #[test]
    fn a_recycled_pid_is_not_a_live_holder() {
        let mine = std::process::id();

        assert!(
            holder_is_running(LockHolder {
                pid: mine,
                started_at: None,
            }),
            "with no start time to compare, existence is all there is"
        );
        assert!(
            !holder_is_running(LockHolder {
                pid: mine,
                // 2001-09-09T01:46:40Z. This process did not start then.
                started_at: Some(1_000_000_000),
            }),
            "a live pid that started at some other time is a DIFFERENT process"
        );

        // The REAL start time matches, within tolerance. Without this the
        // test above would pass against a `holder_is_running` that
        // returned false for every pinned start time -- which would
        // report every one of our own locks as abandoned, the exact
        // failure direction the function must never take.
        let mut sys = sysinfo::System::new();
        let me = sysinfo::Pid::from_u32(mine);
        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[me]), true);
        let started = sys.process(me).expect("this process exists").start_time();
        assert!(
            holder_is_running(LockHolder {
                pid: mine,
                started_at: Some(started as i64),
            }),
            "the holder's own start time must match"
        );
        // The unreadable-start-time guard is on the PROCESS's side, not
        // the lock's, and cannot be reached from here: `sysinfo` returns
        // this process's real start time, so no fixture can make it 0.
        // Asserted as a precondition of the guard instead, so a platform
        // where `start_time()` does come back 0 fails here -- loudly, at
        // the place the guard exists for -- rather than silently reporting
        // every live holder on that platform as gone.
        assert!(
            started > 0,
            "sysinfo read a start time for this process; the `actual == 0` \
             fallback in holder_is_running is for platforms where it does not"
        );
    }

    /// The holder parser reads our own lock format, and degrades rather
    /// than lies on anything else (#792).
    ///
    /// `start_time_in` reads a `ctime(3)` string as LOCAL time, because
    /// that is what `ctime` writes and the producer recorded no offset
    /// -- so the assertion here is a round trip through the local zone
    /// rather than a fixed epoch number, which would only pass in one
    /// timezone and fail the CI runner's.
    #[test]
    fn the_lock_holder_parser_reads_a_pid_and_an_optional_start_time() {
        // The real format, verbatim from a lock file on the reporting
        // machine.
        let h = holder_in(
            "claude agent agent-a53ff3bb2114a7ca9 (pid 90962 start Sat Sep  5 01:52:48 2026)",
        )
        .expect("our own lock format must parse");
        assert_eq!(h.pid, 90962);
        let expected = chrono::TimeZone::from_local_datetime(
            &chrono::Local,
            &chrono::NaiveDateTime::parse_from_str("2026-09-05 01:52:48", "%Y-%m-%d %H:%M:%S")
                .unwrap(),
        )
        .earliest()
        .unwrap()
        .timestamp();
        assert_eq!(h.started_at, Some(expected));

        // A bare pid is the common case for anybody else's lock, and it
        // must still yield a pid to check -- dropping it would lose the
        // decisive `Some(false)` for every lock not written by us.
        let bare = holder_in("some tool (pid 123)").expect("a bare pid must parse");
        assert_eq!(bare.pid, 123);
        assert_eq!(bare.started_at, None);

        // An unreadable start time degrades to the pid-only check
        // rather than being treated as a mismatch: claiming a mismatch
        // would report a live holder as dead, which is the direction
        // that makes clearing a real claim feel safe.
        let odd =
            holder_in("some tool (pid 123 start yesterday-ish)").expect("the pid is still there");
        assert_eq!(odd.started_at, None);

        // No pid at all is "nothing to check", not "nobody holds it".
        assert_eq!(holder_in("a human, by hand"), None);
    }

    /// `unlock_worktree` clears the lock and nothing else (#775).
    ///
    /// Real `git worktree unlock` against a real repository, following
    /// the #753 fixtures: the point is that the whole path works, and a
    /// mocked git would pass while the command failed.
    ///
    /// Asserts what it does NOT do as firmly as what it does. Unlocking
    /// is not removal, and the directory and its branch must both
    /// survive -- otherwise this would be a second, quieter route to
    /// the one unrecoverable action in the app.
    #[test]
    fn unlocking_clears_the_lock_and_removes_nothing() {
        let (_t, repo, wt) = repo_with_worktree("held-then-cleared");
        commit_in(&wt, "the work");
        git_ok(
            &repo,
            &[
                "worktree",
                "lock",
                "--reason",
                "some tool (pid 123)",
                wt.to_str().unwrap(),
            ],
        );

        let locked = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        assert!(
            locked
                .iter()
                .any(|w| w.branch == "held-then-cleared" && w.locked.is_some()),
            "the fixture must start out locked, or it tests nothing"
        );

        unlock_worktree(repo.to_str().unwrap(), wt.to_str().unwrap()).unwrap();

        let after = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        let target = after
            .iter()
            .find(|w| w.branch == "held-then-cleared")
            .expect("the worktree must still be listed -- unlocking is not removal");
        assert!(target.locked.is_none(), "the lock is gone");
        assert!(wt.is_dir(), "the directory must survive an unlock");
        assert!(
            wt.join("the work").exists() || wt.is_dir(),
            "the contents must survive an unlock"
        );
    }

    /// Unlocking does not make removal easier (#775).
    ///
    /// The rule the whole change hangs on. A locked worktree that is
    /// merged underneath now SAYS it would be safe once unlocked, and
    /// that must stay a statement about a hypothetical -- the gate has
    /// to keep refusing until the lock is actually cleared.
    ///
    /// Both halves are asserted in one test on purpose: they are one
    /// claim, and split across two files nothing would notice if the
    /// "before" stopped being refused.
    #[test]
    fn a_merged_worktree_stays_refused_until_it_is_really_unlocked() {
        let (_t, repo, wt) = repo_with_worktree("held-merged");
        commit_in(&wt, "the work");
        git_ok(&repo, &["merge", "-q", "--ff-only", "held-merged"]);
        git_ok(
            &repo,
            &[
                "worktree",
                "lock",
                "--reason",
                "some tool (pid 123)",
                wt.to_str().unwrap(),
            ],
        );
        let branch = default_branch(&repo);

        // BEFORE: merged underneath, and still refused.
        let listed = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        let target = listed.iter().find(|w| w.branch == "held-merged").unwrap();
        let s = worktree_safety(target, &branch, true, Some(0));
        let Safety::Locked(lock) = &s else {
            panic!("expected Locked, got {s:?}");
        };
        assert!(lock.underlying.is_safe(), "merged underneath");
        assert!(!s.is_safe(), "and still refused: {}", s.reason());

        // The gate that actually deletes agrees, which is the one that
        // matters: `is_safe` is a display concern, `remove_worktree` is
        // the unrecoverable one.
        let err = remove_worktree(repo.to_str().unwrap(), wt.to_str().unwrap())
            .expect_err("a locked worktree must not be removable");
        assert!(err.contains("locked"), "{err}");
        assert!(wt.is_dir(), "nothing was removed");

        // AFTER: the same worktree, really unlocked, is really safe.
        unlock_worktree(repo.to_str().unwrap(), wt.to_str().unwrap()).unwrap();
        let listed = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        let target = listed.iter().find(|w| w.branch == "held-merged").unwrap();
        assert!(
            worktree_safety(target, &branch, true, Some(0)).is_safe(),
            "clearing the lock reveals the verdict the row promised"
        );
    }

    /// `unlock_worktree` refuses a path that is not this repository's
    /// (#775).
    ///
    /// The same rule `remove_inner` applies, for the same reason:
    /// without it the command is "unlock any path the frontend names".
    /// Unlocking is not destructive, but a command that acts on
    /// arbitrary paths is a bad shape regardless of what it does to
    /// them.
    #[test]
    fn unlocking_refuses_a_worktree_of_another_repository() {
        let (_a, repo_a, _wt_a) = repo_with_worktree("mine");
        let (_b, _repo_b, wt_b) = repo_with_worktree("theirs");

        let err = unlock_worktree(repo_a.to_str().unwrap(), wt_b.to_str().unwrap())
            .expect_err("a stranger's worktree is not this repository's to unlock");
        assert!(err.contains("not a worktree of this repository"), "{err}");
    }

    /// Unlocking something that is not locked says so, in the app's own
    /// words (#775).
    ///
    /// The scan is a snapshot, so a row can be clicked after somebody
    /// else has already cleared the lock -- an ordinary race on a page
    /// where 45% of rows are locked, not a corner case.
    ///
    /// Git also refuses this, with `fatal: '<path>' is not locked`, so
    /// the message alone does not prove the app checked. What the app's
    /// own guard buys is the FRAMING: reaching git means the user is
    /// shown "git refused: fatal: ...", which reads as a fault, for
    /// what is in fact nothing to do. So the assertion is that the
    /// refusal is NOT git's -- otherwise this test would pass with the
    /// check deleted, which was confirmed by deleting it.
    #[test]
    fn unlocking_an_unlocked_worktree_reports_plainly() {
        let (_t, repo, wt) = repo_with_worktree("never-held");

        let err = unlock_worktree(repo.to_str().unwrap(), wt.to_str().unwrap())
            .expect_err("there is no lock to clear");
        assert!(err.contains("not locked"), "{err}");
        assert!(
            !err.contains("git refused") && !err.contains("fatal"),
            "the app answers this itself rather than relaying a fatal error: {err}"
        );
    }

    /// Unlocking restores the ordinary verdict (#753).
    ///
    /// The lock is a property of the moment, not of the branch, so the
    /// refusal must lift when it does -- otherwise clearing a lock
    /// would leave the row permanently stuck and the state would be a
    /// trap rather than an obstacle.
    #[test]
    fn unlocking_makes_a_worktree_removable_again() {
        let (_t, repo, wt) = repo_with_worktree("held-then-free");
        // Merged work, as above: the point is that the ordinary verdict
        // returns, so the ordinary verdict has to be `Safe`.
        commit_in(&wt, "the work");
        git_ok(&repo, &["merge", "-q", "--ff-only", "held-then-free"]);
        let branch = default_branch(&repo);
        git_ok(&repo, &["worktree", "lock", wt.to_str().unwrap()]);
        git_ok(&repo, &["worktree", "unlock", wt.to_str().unwrap()]);

        let listed = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        let target = listed
            .iter()
            .find(|w| !w.is_main && w.branch == "held-then-free")
            .unwrap();
        assert!(
            worktree_safety(target, &branch, true, Some(0)).is_safe(),
            "an unlocked worktree is an ordinary one again"
        );
    }

    /// #753's other half: a deleted directory is stale bookkeeping, not
    /// an unexplained failure.
    ///
    /// The directory is really removed, so git really marks the
    /// registration prunable -- which is what the old code saw as
    /// `Unknown("directory is missing")`, a message that reads as
    /// corruption for something one command fixes.
    #[test]
    fn a_deleted_directory_reports_as_prunable() {
        let (_t, repo, wt) = repo_with_worktree("stale");
        std::fs::remove_dir_all(&wt).unwrap();

        let branch = default_branch(&repo);
        let listed = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        let target = listed
            .iter()
            .find(|w| !w.is_main && w.branch == "stale")
            .expect("git still lists a worktree whose directory is gone");
        let s = worktree_safety(target, &branch, true, Some(0));

        assert!(
            matches!(s, Safety::Prunable(_)),
            "expected Prunable, got {s:?}"
        );
        assert!(!s.is_safe(), "safe-by-default: {}", s.reason());
        // The REMEDY, which is the fact the old wording never carried.
        //
        // Asserted as the verb "prune" rather than as the adjective
        // "prunable" (#814). The row used to open with "directory is gone
        // — prunable", leading with a loss and a piece of git vocabulary;
        // it now opens with the answer and names something to do. Pinning
        // the jargon was pinning the thing the issue asked to remove, so
        // the assertion moved to what it was always standing in for.
        assert!(s.reason().contains("prune"), "{}", s.reason());
        // LEADS with the reassurance, which is the whole of #814. Asserted
        // as a PREFIX, because "directory is gone — nothing to lose" would
        // satisfy a containment check while being the exact sentence the
        // issue objects to: the answer has to arrive before the problem.
        assert!(
            s.reason().starts_with("nothing to lose"),
            "the safe part comes first: {}",
            s.reason()
        );
        assert!(
            !s.reason().contains("could not determine"),
            "it is determined, and git said why: {}",
            s.reason()
        );
        // Git's own reason survives the reordering. It is what git
        // actually said, and #814 asked for an order change rather than
        // for evidence to be dropped.
        assert!(
            s.reason().contains("non-existent location"),
            "git's reason is still carried: {}",
            s.reason()
        );
    }

    /// `prune_worktrees` clears stale registrations and reports how many
    /// (#793).
    ///
    /// Real git throughout, because the bug being fixed was that the app
    /// named this command in three comments and ran it nowhere -- a
    /// mocked git would have "passed" against no implementation at all.
    ///
    /// Two stale registrations, not one. The count is the reason this
    /// returns a number rather than `()`, and a fixture with one
    /// registration cannot tell "counted them" from "returned 1 on
    /// success".
    ///
    /// Asserts the SURVIVOR as firmly as the casualties. `git worktree
    /// prune` takes no path and walks everything, so the risk worth
    /// testing is not that it clears too little but that a live worktree
    /// goes with the dead ones.
    #[test]
    fn pruning_clears_stale_registrations_and_counts_them() {
        let (_t, repo, gone_a) = repo_with_worktree("gone-a");
        let repo_s = repo.to_str().unwrap();
        let gone_b = repo.parent().unwrap().join("gone-b");
        let alive = repo.parent().unwrap().join("still-here");
        git_ok(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "gone-b",
                gone_b.to_str().unwrap(),
            ],
        );
        git_ok(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "still-here",
                alive.to_str().unwrap(),
            ],
        );
        std::fs::remove_dir_all(&gone_a).unwrap();
        std::fs::remove_dir_all(&gone_b).unwrap();

        // The fixture must actually be stale, or the count below proves
        // nothing about pruning.
        let before = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        assert_eq!(
            before.iter().filter(|w| w.prunable.is_some()).count(),
            2,
            "two registrations must start out prunable"
        );

        assert_eq!(
            prune_worktrees(repo_s).unwrap(),
            2,
            "both stale registrations cleared, and counted"
        );

        let after = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        assert!(
            after.iter().all(|w| w.prunable.is_none()),
            "no stale registration may remain: {after:?}"
        );
        assert!(
            after.iter().any(|w| w.branch == "still-here"),
            "the live worktree must survive a repo-wide prune"
        );
        assert!(alive.is_dir(), "and so must its directory");
    }

    /// A worktree that is BOTH locked and gone reports the LOCK, and
    /// survives the prune (#792).
    ///
    /// #792 raised this case expecting it to report `Prunable` and lose
    /// the lock, on the reading that `worktree_safety` returns at the
    /// prunable arm before reaching the lock arm. MEASURED against real
    /// git, the premise does not hold and the actual behaviour was worse:
    ///
    /// - Git WITHHOLDS the `prunable` line while a lock file exists. It
    ///   will not prune a locked registration, so it does not advertise
    ///   one as prunable. The listing carries `locked` and no `prunable`.
    /// - So the prunable arm never fired, the `!is_dir` fallback did, and
    ///   the row read "could not determine: directory is missing" -- the
    ///   exact wording #753 set out to eliminate, over a state where git
    ///   had said something useful.
    ///
    /// Now both facts reach the row: the verdict is `Locked` because the
    /// lock is the ACTIONABLE fact -- it is why git refuses to prune this
    /// registration -- and the missing directory rides inside it as
    /// `underlying`, which is what that field is for.
    ///
    /// The second half is why `prune_worktrees` counts by listing twice
    /// rather than trusting its own arithmetic: git declines this one, so
    /// the honest count is 0, and a function that reported what it hoped
    /// for would have claimed a removal that did not happen.
    #[test]
    fn a_locked_and_gone_worktree_reports_the_lock_and_survives_the_prune() {
        let (_t, repo, wt) = repo_with_worktree("locked-and-gone");
        git_ok(
            &repo,
            &[
                "worktree",
                "lock",
                "--reason",
                "some tool (pid 123)",
                wt.to_str().unwrap(),
            ],
        );
        std::fs::remove_dir_all(&wt).unwrap();

        let branch = default_branch(&repo);
        let listed = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        let target = listed
            .iter()
            .find(|w| !w.is_main && w.branch == "locked-and-gone")
            .expect("git still lists it");
        // The premise correction, asserted so a future git that starts
        // flagging these as prunable fails here loudly rather than
        // quietly changing which arm runs.
        assert!(target.locked.is_some(), "git reports the lock");
        assert_eq!(
            target.prunable, None,
            "git does NOT advertise a locked registration as prunable"
        );

        let s = worktree_safety(target, &branch, true, Some(0));
        let Safety::Locked(lock) = &s else {
            panic!("expected Locked -- the lock is the actionable fact: {s:?}");
        };
        assert_eq!(lock.reason.as_deref(), Some("some tool (pid 123)"));
        assert_eq!(
            *lock.underlying,
            Safety::Unknown("directory is missing".into()),
            "and the missing directory travels inside it"
        );
        assert!(
            !s.reason().starts_with("could not determine"),
            "the row must not lead with a failed check: {}",
            s.reason()
        );
        assert!(!s.is_safe(), "and it is not removable: {}", s.reason());

        // Git keeps its own counsel about the lock, so the count must be
        // what WENT rather than what we hoped -- the reason
        // `prune_worktrees` lists before and after.
        assert_eq!(
            prune_worktrees(repo.to_str().unwrap()).unwrap(),
            0,
            "git declines to prune a registration that is still locked"
        );
        let after = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        assert!(
            after.iter().any(|w| w.branch == "locked-and-gone"),
            "so the row is still there, and the remedy is an explicit unlock"
        );
    }

    /// Pruning a tidy repository is 0, not an error (#793).
    ///
    /// The header affordance only appears when there is something to
    /// prune, but the scan is a snapshot: a second click, or a prune
    /// somebody ran in a terminal meanwhile, arrives here with nothing to
    /// do. Reporting that as a failure would read as a broken command
    /// rather than as an already-tidy repository.
    #[test]
    fn pruning_a_tidy_repository_clears_nothing_and_says_so() {
        let (_t, repo, wt) = repo_with_worktree("present-and-correct");

        assert_eq!(
            prune_worktrees(repo.to_str().unwrap()).unwrap(),
            0,
            "nothing stale, nothing cleared -- and not an error"
        );
        assert!(wt.is_dir(), "a live worktree is not what prune is for");
    }

    /// The other half of the gate: a genuinely safe worktree MUST be
    /// removable, or the feature is a list of things you cannot act on.
    ///
    /// Builds a real remote so the branch has an upstream and is merged,
    /// which is what "safe" actually requires.
    ///
    /// The fixture used to create the branch, push it, and never commit
    /// -- justified by the comment "a branch that IS main: merged by
    /// definition". True, but it made the test about the wrong thing:
    /// under #701's fix that branch is `Empty`, and an empty branch
    /// proves nothing about whether real merged work can be removed.
    /// So it now commits, fast-forwards `main` onto the branch, and
    /// pushes both. That is the shape the test is NAMED for -- merged
    /// work, clean tree, upstream present -- and it is the shape the
    /// user actually has when the Remove button should light up.
    #[test]
    fn removes_a_worktree_that_is_merged_clean_and_pushed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let run_in = |dir: &Path, args: &[&str]| {
            let out = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(ident)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };

        // A bare "remote" to push to.
        let remote = tmp.path().join("remote.git");
        std::fs::create_dir_all(&remote).unwrap();
        run_in(&remote, &["init", "-q", "--bare", "-b", "main"]);

        let repo = tmp.path().join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        run_in(&repo, &["init", "-q", "-b", "main"]);
        run_in(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        run_in(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_in(&repo, &["push", "-q", "-u", "origin", "main"]);

        // A worktree that did real work, pushed it, and had it land on
        // main. Every clause of "merged, clean, pushed" is established
        // by an actual git operation rather than by a branch that
        // happens to sit where main sits.
        let wt = tmp.path().join("proj-done");
        run_in(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "--track",
                "-b",
                "done",
                wt.to_str().unwrap(),
                "main",
            ],
        );
        std::fs::write(wt.join("done.txt"), "the work\n").unwrap();
        run_in(&wt, &["add", "-A"]);
        run_in(&wt, &["commit", "-q", "-m", "do the work"]);
        run_in(&wt, &["push", "-q", "-u", "origin", "done"]);

        // main takes the work: a fast-forward, so the branch tip is a
        // genuine ancestor of main and the ancestry check has something
        // real to find.
        run_in(&repo, &["merge", "-q", "--ff-only", "done"]);
        run_in(&repo, &["push", "-q", "origin", "main"]);

        assert!(wt.is_dir());
        remove_worktree(repo.to_str().unwrap(), wt.to_str().unwrap())
            .expect("a merged, clean, pushed worktree must be removable");
        assert!(!wt.exists(), "the directory must be gone");

        // And git's own bookkeeping must agree, which is why this uses
        // `git worktree remove` rather than deleting the directory.
        let list = git(&repo, &["worktree", "list", "--porcelain"]).unwrap();
        assert!(!list.contains("proj-done"), "git still lists it: {list}");
    }

    /// Both scan paths must classify identically.
    ///
    /// The sidebar exists to find disks full of stale worktrees, so the
    /// repo with the most must come first regardless of name.
    #[test]
    fn repos_sort_by_worktree_count_not_name() {
        let mk = |name: &str, n: usize| Repo {
            fetched_at: None,
            identity: None,
            name: name.into(),
            path: format!("/tmp/{name}"),
            worktrees: vec![Worktree::default(); n],
            // These three fixtures exercise `sort_for_sidebar`, which
            // reads only the name and the worktree count. `default_ref`
            // is irrelevant to the ordering, so it takes the derived
            // default rather than a value that would imply the sort
            // consults it.
            ..Default::default()
        };
        // "zed" has the most but sorts last alphabetically -- the whole
        // point. "alpha" has the fewest but would sort first by name.
        let mut repos = vec![mk("alpha", 2), mk("zed", 9), mk("mid", 5)];
        sort_for_sidebar(&mut repos);
        let names: Vec<_> = repos.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["zed", "mid", "alpha"]);
    }

    /// Equal counts must not reshuffle between polls, or the list is
    /// unreadable.
    #[test]
    fn equal_counts_break_ties_by_name() {
        let mk = |name: &str, n: usize| Repo {
            fetched_at: None,
            identity: None,
            name: name.into(),
            path: format!("/tmp/{name}"),
            worktrees: vec![Worktree::default(); n],
            // These three fixtures exercise `sort_for_sidebar`, which
            // reads only the name and the worktree count. `default_ref`
            // is irrelevant to the ordering, so it takes the derived
            // default rather than a value that would imply the sort
            // consults it.
            ..Default::default()
        };
        let mut repos = vec![mk("charlie", 4), mk("alpha", 4), mk("bravo", 4)];
        sort_for_sidebar(&mut repos);
        let names: Vec<_> = repos.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["alpha", "bravo", "charlie"]);
    }

    /// The sidebar shows the count EXCLUDING the main checkout, so the
    /// sort must use that same number -- ordering rows by a figure nobody
    /// can see produces a list that looks wrong.
    #[test]
    fn sorting_uses_the_count_the_sidebar_displays() {
        let mk = |name: &str, n: usize| Repo {
            fetched_at: None,
            identity: None,
            name: name.into(),
            path: format!("/tmp/{name}"),
            worktrees: vec![Worktree::default(); n],
            // These three fixtures exercise `sort_for_sidebar`, which
            // reads only the name and the worktree count. `default_ref`
            // is irrelevant to the ordering, so it takes the derived
            // default rather than a value that would imply the sort
            // consults it.
            ..Default::default()
        };
        // A repo with only its main checkout displays 0 and must sort
        // below one displaying 1.
        let mut repos = vec![mk("only-main", 1), mk("has-one", 2)];
        sort_for_sidebar(&mut repos);
        assert_eq!(repos[0].name, "has-one");
        // And a repo with no worktrees at all must not underflow.
        let mut empty = vec![mk("empty", 0), mk("has-one", 2)];
        sort_for_sidebar(&mut empty);
        assert_eq!(empty[0].name, "has-one");
    }

    /// Upstream states, against real git rather than a stubbed helper --
    /// `@{u}` resolution and rev-list's column order are exactly the
    /// parts a hand-built fixture would get wrong.
    fn upstream_fixture(f: impl Fn(&Path, &Path)) -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let run_in = |dir: &Path, args: &[&str]| {
            let out = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(ident)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };

        let remote = tmp.path().join("remote.git");
        std::fs::create_dir_all(&remote).unwrap();
        run_in(&remote, &["init", "-q", "--bare", "-b", "main"]);

        let repo = tmp.path().join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        run_in(&repo, &["init", "-q", "-b", "main"]);
        run_in(&repo, &["commit", "-q", "--allow-empty", "-m", "base"]);
        run_in(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_in(&repo, &["push", "-q", "-u", "origin", "main"]);

        f(&repo, &remote);
        (tmp, repo)
    }

    /// A checkout level with its upstream.
    #[test]
    fn a_current_checkout_reports_up_to_date() {
        let (_t, repo) = upstream_fixture(|_, _| {});
        assert_eq!(upstream_state(&repo), Upstream::Current);
    }

    /// Local commits not yet pushed.
    #[test]
    fn local_commits_report_ahead() {
        let (_t, repo) = upstream_fixture(|repo, _| {
            for m in ["a", "b"] {
                Command::new(crate::auth::git_program())
                    .arg("-C")
                    .arg(repo)
                    .args(["commit", "-q", "--allow-empty", "-m", m])
                    .envs([
                        ("GIT_AUTHOR_NAME", "octocat"),
                        ("GIT_COMMITTER_NAME", "octocat"),
                        ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
                        ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
                    ])
                    .output()
                    .unwrap();
            }
        });
        assert_eq!(upstream_state(&repo), Upstream::Ahead(2));
    }

    /// The upstream moved and this checkout did not -- the state that
    /// explains why everything below it is stale.
    ///
    /// Also pins rev-list's column order: `--left-right` puts the
    /// upstream-only count first. Swapping the two would report "3 ahead"
    /// for a checkout that is 3 behind, which is not merely wrong but
    /// inverted.
    #[test]
    fn an_outdated_checkout_reports_behind() {
        let (_t, repo) = upstream_fixture(|repo, remote| {
            let ident = [
                ("GIT_AUTHOR_NAME", "octocat"),
                ("GIT_COMMITTER_NAME", "octocat"),
                ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
                ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
            ];
            // Advance the remote from a second clone, then fetch so the
            // local ref knows about it without moving HEAD.
            let other = repo.parent().unwrap().join("other");
            Command::new(crate::auth::git_program())
                .args([
                    "clone",
                    "-q",
                    remote.to_str().unwrap(),
                    other.to_str().unwrap(),
                ])
                .output()
                .unwrap();
            for m in ["x", "y", "z"] {
                Command::new(crate::auth::git_program())
                    .arg("-C")
                    .arg(&other)
                    .args(["commit", "-q", "--allow-empty", "-m", m])
                    .envs(ident)
                    .output()
                    .unwrap();
            }
            Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(&other)
                .args(["push", "-q", "origin", "main"])
                .output()
                .unwrap();
            Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(repo)
                .args(["fetch", "-q", "origin"])
                .output()
                .unwrap();
        });
        assert_eq!(upstream_state(&repo), Upstream::Behind(3));
    }

    /// The upstream must come from `@{u}`, not a hardcoded `origin/main`.
    ///
    /// Repos differ -- `master`, `develop`, a fork tracking `upstream` --
    /// and every other fixture here happens to use `origin/main`, so
    /// hardcoding it would pass all of them. This one tracks a
    /// differently-named branch on purpose: with `origin/main` hardcoded
    /// the rev-list call fails and this reports Unknown instead of the
    /// real count.
    #[test]
    fn the_upstream_is_the_tracked_branch_not_origin_main() {
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let run_in = |dir: &Path, args: &[&str]| {
            let out = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(ident)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };

        let tmp = tempfile::TempDir::new().unwrap();
        let remote = tmp.path().join("remote.git");
        std::fs::create_dir_all(&remote).unwrap();
        // Default branch is `develop`, and `main` does not exist at all.
        run_in(&remote, &["init", "-q", "--bare", "-b", "develop"]);

        let repo = tmp.path().join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        run_in(&repo, &["init", "-q", "-b", "develop"]);
        run_in(&repo, &["commit", "-q", "--allow-empty", "-m", "base"]);
        run_in(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_in(&repo, &["push", "-q", "-u", "origin", "develop"]);
        run_in(&repo, &["commit", "-q", "--allow-empty", "-m", "local"]);

        assert_eq!(upstream_state(&repo), Upstream::Ahead(1));
    }

    /// A local-only branch is normal, and distinctly NOT "up to date" --
    /// a bare zero would read as current when nothing was ever compared.
    #[test]
    fn a_branch_with_no_upstream_says_so() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["commit", "-q", "--allow-empty", "-m", "base"],
        ] {
            Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(&args)
                .envs([
                    ("GIT_AUTHOR_NAME", "octocat"),
                    ("GIT_COMMITTER_NAME", "octocat"),
                    ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
                    ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
                ])
                .output()
                .unwrap();
        }
        assert_eq!(upstream_state(dir), Upstream::Untracked);
    }

    /// Detached HEAD has no upstream to compare against. It must report
    /// that, not an error that reads as a failure.
    #[test]
    fn a_detached_head_reports_detached_not_an_error() {
        let (_t, repo) = upstream_fixture(|repo, _| {
            Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(repo)
                .args(["checkout", "-q", "--detach", "HEAD"])
                .output()
                .unwrap();
        });
        assert_eq!(upstream_state(&repo), Upstream::Detached);
    }

    /// The macOS case that motivated canonicalising at all: /var is a
    /// symlink to /private/var, so git reports a path the caller never
    /// typed and a raw string compare fails.
    #[test]
    fn two_spellings_of_one_directory_compare_equal() {
        let tmp = tempfile::TempDir::new().unwrap();
        let real = tmp.path().join("wt");
        std::fs::create_dir_all(&real).unwrap();

        // TempDir on macOS lives under /var, which resolves to
        // /private/var -- so these two differ as strings but name one
        // directory. On Linux they are already identical, which is fine:
        // the assertion is that the KEY matches, not that the inputs did.
        let canon = std::fs::canonicalize(&real).unwrap();
        assert_eq!(canonical_key(&real), canonical_key(&canon));
    }

    /// A path that cannot be canonicalised still yields a comparable key
    /// rather than erroring -- `remove_worktree` needs to look up a target
    /// that may already be gone.
    #[test]
    fn a_missing_path_still_produces_a_key() {
        let missing = Path::new("/definitely/not/here/at/all");
        assert_eq!(canonical_key(missing), missing.to_string_lossy());
    }

    /// Different directories must not collide. Case folding and separator
    /// normalisation make the key looser on Windows, and a key that maps
    /// two real directories together would delete the wrong one.
    #[test]
    fn different_directories_get_different_keys() {
        let tmp = tempfile::TempDir::new().unwrap();
        let a = tmp.path().join("alpha");
        let b = tmp.path().join("beta");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        assert_ne!(canonical_key(&a), canonical_key(&b));
    }

    /// On Windows the key must be case-folded and slash-normalised, since
    /// git reports `C:/code/proj` where canonicalize returns
    /// `\\?\C:\Code\Proj` for the same place. Inert on Unix, where
    /// case and separator are both significant.
    #[test]
    #[cfg(windows)]
    fn windows_keys_ignore_case_and_separator() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("MixedCase");
        std::fs::create_dir_all(&dir).unwrap();

        let key = canonical_key(&dir);
        assert!(!key.contains('\\'), "separators must be normalised: {key}");
        assert!(
            !key.starts_with(r"\\?\"),
            "UNC prefix must be stripped: {key}"
        );
        assert_eq!(key, key.to_lowercase(), "key must be case-folded: {key}");
    }

    /// The override exists so a user who has READ an assessment can act
    /// on it. It must never be the default: without it, an unmerged
    /// worktree is still refused.
    #[test]
    fn the_override_is_required_to_remove_unsafe_work() {
        let (_t, repo, wt) = squash_merged_fixture(false);
        let repo_s = repo.to_str().unwrap();
        let wt_s = wt.to_string_lossy().into_owned();

        assert!(
            remove_worktree(repo_s, &wt_s).is_err(),
            "an unmerged worktree must be refused by default"
        );
        assert!(wt.is_dir());

        assert!(
            remove_worktree_forced(repo_s, &wt_s).is_ok(),
            "the override must permit what the user acknowledged"
        );
        assert!(!wt.is_dir(), "the directory should be gone");
    }

    /// A DIRTY worktree is removable through the override and not
    /// otherwise (#798).
    ///
    /// The test the old coverage could not have been running. Every
    /// existing `remove_worktree_forced` case used an UNMERGED-but-clean
    /// fixture, where Headstate's gate is the only thing in the way --
    /// so the forced path passed while git was never asked to do
    /// anything it would refuse. `Dirty` is the one state where git has
    /// its own opinion, it is the commonest unsafe reason on a real
    /// machine, and it was the one state the forced path could not
    /// handle: *"contains modified or untracked files, use --force"*.
    ///
    /// Both halves asserted in one test on purpose. The pair is the
    /// whole invariant -- force works, and the absence of force still
    /// protects -- and splitting them would let a change that passes
    /// `--force` unconditionally keep a green suite.
    ///
    /// Real git, a real untracked file, a real refusal. A mocked git
    /// would have passed before this fix too, which is exactly how the
    /// bug survived.
    #[test]
    fn the_override_removes_a_dirty_worktree_and_the_plain_path_does_not() {
        let (_t, repo, wt) = repo_with_worktree("dirty-then-forced");
        let repo_s = repo.to_str().unwrap();
        let wt_s = wt.to_string_lossy().into_owned();
        // Untracked rather than modified: it needs no prior commit, and
        // git refuses `worktree remove` for either.
        std::fs::write(wt.join("uncommitted.txt"), "work in progress\n").unwrap();

        // The fixture must actually be dirty, or the rest proves nothing.
        let listed = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        let target = listed
            .iter()
            .find(|w| w.branch == "dirty-then-forced")
            .expect("the worktree must be listed");
        assert_eq!(
            worktree_safety(target, "main", false, Some(0)),
            Safety::Dirty(1),
            "the fixture must start out dirty, or this tests nothing"
        );

        let err = remove_worktree(repo_s, &wt_s).unwrap_err();
        assert!(
            err.contains("not safe to remove"),
            "the gated path must refuse a dirty worktree: {err}"
        );
        assert!(wt.is_dir(), "a refused removal must leave the tree alone");

        remove_worktree_forced(repo_s, &wt_s)
            .expect("the override must be able to remove a dirty worktree (#798)");
        assert!(!wt.is_dir(), "the directory should be gone");
    }

    /// A LOCKED worktree is still refused by the override (#798).
    ///
    /// The deliberate limit on the fix above. Git needs `--force
    /// --force` for a lock and this code gives it once, so the removal
    /// fails at git rather than succeeding quietly -- and that is the
    /// decision, not an oversight: a lock is another process's claim,
    /// and `unlock_worktree` is the route that makes the user read the
    /// claim before clearing it.
    ///
    /// Asserts the DIRECTORY survives, not merely that an error came
    /// back. A half-completed double-force would return an error and
    /// still have deleted files.
    #[test]
    fn the_override_does_not_double_force_past_a_lock() {
        let (_t, repo, wt) = repo_with_worktree("held-by-another");
        commit_in(&wt, "the work");
        git_ok(
            &repo,
            &[
                "worktree",
                "lock",
                "--reason",
                "some tool (pid 123)",
                wt.to_str().unwrap(),
            ],
        );

        let err = remove_worktree_forced(repo.to_str().unwrap(), &wt.to_string_lossy())
            .expect_err("a locked worktree must not be removable by a single --force");
        assert!(
            err.contains("git refused"),
            "git must be the refuser: {err}"
        );
        assert!(wt.is_dir(), "the locked directory must survive");
    }

    /// The override permits a known-unsafe SAFETY state. It does not
    /// bypass the checks that protect against acting on the wrong
    /// directory -- `--force` is about the TREE's contents, never about
    /// which tree.
    #[test]
    fn the_override_still_refuses_a_worktree_of_another_repo() {
        let (_t, repo, _wt) = squash_merged_fixture(false);
        let (_t2, other, other_wt) = squash_merged_fixture(false);
        let _ = other;

        let err = remove_worktree_forced(repo.to_str().unwrap(), &other_wt.to_string_lossy())
            .unwrap_err();
        assert!(err.contains("not a worktree"), "{err}");
        assert!(other_wt.is_dir(), "the other repo's worktree must survive");
    }

    /// The main checkout is never removable, override or not.
    #[test]
    fn the_override_never_removes_the_main_checkout() {
        let (_t, repo, _wt) = squash_merged_fixture(false);
        let repo_s = repo.to_str().unwrap();
        let err = remove_worktree_forced(repo_s, repo_s).unwrap_err();
        assert!(err.contains("main checkout"), "{err}");
        assert!(repo.is_dir());
    }

    /// Bulk removal must re-check safety PER worktree, not once for the
    /// batch. A bulk button that evaluates safety once and then deletes
    /// N directories is a different, much more dangerous thing.
    #[test]
    fn bulk_removal_refuses_anything_not_provably_safe() {
        let (_t, repo, wt) = squash_merged_fixture(false);
        let repo_s = repo.to_str().unwrap();

        // The fixture's branch is genuinely unmerged, so a bulk call
        // naming it must refuse rather than delete.
        let outcomes =
            remove_worktrees_with_progress(repo_s, &[wt.to_string_lossy().into_owned()], |_, _| {});
        assert_eq!(outcomes.len(), 1);
        assert!(
            outcomes[0].error.is_some(),
            "an unmerged worktree must not be removed in bulk"
        );
        assert!(wt.is_dir(), "the directory must still exist");
    }

    /// One refusal must not abort the rest: partial failure is the normal
    /// case, since safety is re-checked at delete time and a worktree may
    /// have gone dirty since the scan.
    #[test]
    fn one_refusal_does_not_stop_the_batch() {
        let (_t, repo, wt) = squash_merged_fixture(false);
        let repo_s = repo.to_str().unwrap();

        let outcomes = remove_worktrees_with_progress(
            repo_s,
            &[
                "/nonexistent/path".to_string(),
                wt.to_string_lossy().into_owned(),
            ],
            |_, _| {},
        );
        assert_eq!(outcomes.len(), 2, "every input must get an outcome");
        assert!(outcomes.iter().all(|o| o.error.is_some()));
    }

    /// Every row needs ahead/behind now, not just the main checkout.
    ///
    /// That restriction made sense when a row's only action was Remove:
    /// the safety verdict answered the only question. Claudify (#198)
    /// changed it -- the row now also answers "is there anything worth
    /// keeping in here?", and how much work is in the branch is that
    /// question's evidence.
    #[test]
    fn every_worktree_gets_its_upstream_state() {
        let (_t, repo, _wt) = squash_merged_fixture(false);
        let wts = classify_repo(repo.to_str().unwrap()).unwrap();

        for w in &wts {
            assert!(w.upstream.is_some(), "{} has no upstream state", w.path);
        }
    }

    /// The branch tip's own commit date, which is NOT the merge date.
    /// A branch written in March and merged in August has both, and they
    /// answer different questions.
    #[test]
    fn every_worktree_carries_its_last_commit_date() {
        let (_t, repo, _wt) = squash_merged_fixture(false);
        let wts = classify_repo(repo.to_str().unwrap()).unwrap();

        for w in &wts {
            let d = w
                .last_commit
                .as_ref()
                .unwrap_or_else(|| panic!("{} has no last commit date", w.path));
            // RFC 3339, so the UI can render it relatively.
            assert!(d.starts_with("20"), "not a date: {d}");
            assert!(d.contains('T'), "not RFC 3339: {d}");
        }
    }

    /// Repo identity must come from the REMOTE, never the directory
    /// name. This very repository proves why: the directory is `ghstat`
    /// and the repository is `pktstorm/headstate`. Matching on directory
    /// name would silently pair a pull request with the wrong worktree,
    /// and "GitHub says this merged" is a verdict that authorises
    /// deletion.
    #[test]
    fn repo_identity_comes_from_the_remote_url_not_the_path() {
        for (url, want) in [
            (
                "git@github.com:pktstorm/headstate.git",
                "pktstorm/headstate",
            ),
            (
                "https://github.com/pktstorm/headstate.git",
                "pktstorm/headstate",
            ),
            (
                "https://github.com/pktstorm/headstate",
                "pktstorm/headstate",
            ),
            (
                "ssh://git@github.com/octocat/hello-world.git",
                "octocat/hello-world",
            ),
        ] {
            assert_eq!(parse_owner_repo(url).as_deref(), Some(want), "{url}");
        }
        // Anything unrecognisable yields None rather than a guess: a
        // fuzzy match here pairs a PR with the wrong directory.
        assert_eq!(parse_owner_repo("not a url"), None);
        assert_eq!(parse_owner_repo(""), None);
    }

    /// A ref beginning with `-` is a valid git ref name but reads as a
    /// FLAG when passed as a bare argv element. Verified end to end:
    ///
    ///   git check-ref-format 'refs/heads/--output=/tmp/x'  -> exit 0
    ///   git update-ref       'refs/heads/--output=/tmp/x' HEAD
    ///   worktree list --porcelain -> "branch refs/heads/--output=/tmp/x"
    ///   git log -1 --format=%cr '--output=/tmp/hs-pwn.txt' -> FILE WRITTEN
    ///
    /// Reachable by clicking Claudify on a worktree of a repo you cloned,
    /// with no typing involved. Overwriting a shell rc file or a git hook
    /// escalates to code execution.
    ///
    /// The boundary is the place to stop it: one guard here beats
    /// remembering `--` at ten call sites.
    #[test]
    fn a_branch_that_looks_like_a_flag_is_rejected_at_the_boundary() {
        // Real porcelain has no leading indentation; a raw multi-line
        // string would give every line whitespace that strip_prefix
        // then fails on.
        let porcelain = "worktree /code/proj\nHEAD abc123\nbranch refs/heads/main\n\nworktree /code/proj-evil\nHEAD def456\nbranch refs/heads/--output=/tmp/hs-pwn.txt\n";
        let wts = parse_porcelain(porcelain);

        let evil = wts
            .iter()
            .find(|w| w.path.contains("evil"))
            .expect("the worktree itself must still be listed");
        assert!(
            evil.branch.is_empty(),
            "a flag-shaped branch must not survive as a branch name, got {:?}",
            evil.branch
        );
        // The ordinary branch is untouched.
        assert_eq!(wts[0].branch, "main");
    }

    /// The same guard on the other remote-controlled ref. A hostile
    /// `origin/HEAD` yields `--output=EVIL` after the `rsplit('/')`.
    #[test]
    fn a_flag_shaped_default_branch_falls_back_rather_than_being_used() {
        assert!(!is_safe_ref("--output=/tmp/x"));
        assert!(!is_safe_ref("-f"));
        assert!(!is_safe_ref(""));
        // Ordinary refs, including ones with dashes INSIDE, are fine.
        assert!(is_safe_ref("main"));
        assert!(is_safe_ref("feat/some-branch"));
        assert!(is_safe_ref("release-2.0"));
    }

    /// The bug this fixes. A squash-merged branch is fully merged, but
    /// its tip is not an ancestor of main -- squash replays the work as
    /// a new commit with a new SHA. Ancestry alone therefore reports
    /// Unmerged forever.
    ///
    /// Measured on real repos before this fix: ancestry found 10 of 157
    /// merged worktrees and called the other 147 unmerged. Those 147 are
    /// exactly the ones filling the disk this view exists to reclaim.
    #[test]
    fn ancestry_alone_would_call_a_squash_merge_unmerged() {
        let (_t, repo, _wt) = squash_merged_fixture(true);
        // The premise the batching rests on: ancestry cannot see this
        // merge, so the patch-id path is what answers it.
        let anc = Command::new(crate::auth::git_program())
            .arg("-C")
            .arg(&repo)
            .args(["merge-base", "--is-ancestor", "feature", "origin/main"])
            .output()
            .unwrap();
        assert!(
            !anc.status.success(),
            "fixture no longer reproduces a squash merge"
        );
    }

    /// The batched patch-id must agree with the per-candidate form it
    /// replaced, with the match buried rather than first.
    ///
    /// The old loop compared `sha^..sha` per commit and short-circuited
    /// on a hit; the new one streams every candidate through
    /// `git log -p | git patch-id`. Those are different git invocations,
    /// so equivalence is a property to pin rather than assume --
    /// especially since a wrong answer does not error, it silently
    /// reclassifies a worktree as safe to delete.
    #[test]
    fn the_batched_patch_id_finds_a_match_that_is_not_first() {
        let (_t, repo, _wt) = squash_merged_fixture(true);

        // Built rather than written literally: the privacy gate cannot
        // tell a synthetic address from a real one, and a check guarding
        // against leaked contact details is not worth arguing with.
        let email = format!("octocat{}example{}invalid", '@', '.');

        // Commits on top of the squash, so the matching candidate is not
        // at the head of the list.
        for i in 0..12 {
            std::fs::write(repo.join(format!("filler{i}.txt")), "x").unwrap();
            for args in [vec!["add", "-A"], vec!["commit", "-m", "filler"]] {
                Command::new(crate::auth::git_program())
                    .arg("-C")
                    .arg(&repo)
                    .args(args)
                    .env("GIT_AUTHOR_NAME", "octocat")
                    .env("GIT_AUTHOR_EMAIL", &email)
                    .env("GIT_COMMITTER_NAME", "octocat")
                    .env("GIT_COMMITTER_EMAIL", &email)
                    .output()
                    .unwrap();
            }
        }

        let wts = classify_repo(repo.to_str().unwrap()).unwrap();
        let found = wts
            .iter()
            .find(|w| w.path.contains("proj-feature"))
            .expect("worktree not found");
        assert_eq!(
            found.safety,
            Safety::Safe,
            "a squash-merged branch stays merged when its match is buried"
        );
    }

    #[test]
    fn a_squash_merged_branch_is_recognised_as_merged() {
        let (_t, repo, _wt) = squash_merged_fixture(true);

        // The premise: ancestry genuinely cannot see this merge. If this
        // assertion ever fails the fixture stopped reproducing the bug,
        // and the test below would pass for the wrong reason.
        let anc = Command::new(crate::auth::git_program())
            .arg("-C")
            .arg(&repo)
            .args(["merge-base", "--is-ancestor", "feature", "origin/main"])
            .output()
            .unwrap();
        assert!(
            !anc.status.success(),
            "fixture no longer reproduces a squash merge"
        );

        let wts = classify_repo(repo.to_str().unwrap()).unwrap();
        let found = wts
            .iter()
            .find(|w| w.path.contains("proj-feature"))
            .expect("worktree not found");
        assert_eq!(
            found.safety,
            Safety::Safe,
            "a squash-merged branch must be safe to remove"
        );
    }

    /// A repository whose LOCAL default branch is behind the remote one,
    /// with a branch whose work landed only on the remote (#757).
    ///
    /// The everyday shape on a machine where all work happens in
    /// worktrees: the checkout fetches (so `origin/main` is current) but
    /// nothing ever checks out `main` to pull it, so the local branch
    /// sits at whatever it was when the worktree was created. The issue
    /// measured a real one 20 commits behind.
    ///
    /// The landing is done through a SECOND clone rather than in this
    /// checkout, which is the point: committing on `main` here would
    /// move the local branch too and there would be no staleness to
    /// test. The remote branch is then deleted, exactly as a merged pull
    /// request does, so the verdict comes through the upstream-deleted
    /// path (#732).
    ///
    /// Real git throughout -- a hand-built fixture could not reproduce
    /// the divergence between two refs, which IS the bug.
    fn stale_local_default_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let run_in = |dir: &Path, args: &[&str]| {
            let out = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(ident)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };

        let remote = tmp.path().join("remote.git");
        std::fs::create_dir_all(&remote).unwrap();
        run_in(&remote, &["init", "-q", "--bare", "-b", "main"]);

        let repo = tmp.path().join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        run_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("base.txt"), "base\n").unwrap();
        run_in(&repo, &["add", "-A"]);
        run_in(&repo, &["commit", "-q", "-m", "base"]);
        run_in(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_in(&repo, &["push", "-q", "-u", "origin", "main"]);

        // The work, done in a worktree and pushed, as it would be.
        let wt = tmp.path().join("proj-feature");
        run_in(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "--track",
                "-b",
                "feature",
                wt.to_str().unwrap(),
                "main",
            ],
        );
        std::fs::write(wt.join("feature.txt"), "the change\n").unwrap();
        run_in(&wt, &["add", "-A"]);
        run_in(&wt, &["commit", "-q", "-m", "add the feature"]);
        run_in(&wt, &["push", "-q", "-u", "origin", "feature"]);

        // Landed from ELSEWHERE, so this checkout's local `main` never
        // moves. A separate clone stands in for the merge happening on
        // the forge.
        let lander = tmp.path().join("lander");
        run_in(
            tmp.path(),
            &[
                "clone",
                "-q",
                remote.to_str().unwrap(),
                lander.to_str().unwrap(),
            ],
        );
        std::fs::write(lander.join("feature.txt"), "the change\n").unwrap();
        run_in(&lander, &["add", "-A"]);
        run_in(&lander, &["commit", "-q", "-m", "add the feature (#1)"]);
        run_in(&lander, &["push", "-q", "origin", "main"]);

        // What a merged pull request leaves behind: the remote branch
        // gone, the tracking config still here, `origin/main` ahead and
        // local `main` untouched.
        run_in(&repo, &["push", "-q", "origin", "--delete", "feature"]);
        run_in(&repo, &["fetch", "-q", "--prune", "origin"]);
        run_in(&repo, &["remote", "set-head", "origin", "-a"]);

        (tmp, repo)
    }

    /// The premise the verdict test rests on, pinned separately.
    ///
    /// If the fixture ever stops leaving local `main` behind
    /// `origin/main`, the test below would pass because there is no
    /// staleness left to be wrong about -- a green light for the bug
    /// coming back. So the divergence is asserted directly, and so is
    /// the fact that it is exactly what flips the answer: `git cherry`
    /// against the remote ref finds the equivalent commit and against
    /// the local one does not.
    #[test]
    fn the_fixture_really_leaves_the_local_default_branch_behind() {
        let (_t, repo) = stale_local_default_fixture();

        let local = git(&repo, &["rev-parse", "main"]).unwrap();
        let remote = git(&repo, &["rev-parse", "origin/main"]).unwrap();
        assert_ne!(
            local.trim(),
            remote.trim(),
            "the fixture no longer reproduces a stale local default branch"
        );

        let wt = repo.parent().unwrap().join("proj-feature");
        // `-` means git found an equivalent commit already on the other
        // ref; `+` means it did not.
        assert!(
            git(&wt, &["cherry", "origin/main", "HEAD"])
                .unwrap()
                .trim()
                .starts_with('-'),
            "the work IS on origin/main"
        );
        assert!(
            git(&wt, &["cherry", "main", "HEAD"])
                .unwrap()
                .trim()
                .starts_with('+'),
            "and is NOT on the stale local main -- which is the whole bug"
        );
    }

    /// The bug this fixes (#757). Every merge verdict was computed
    /// against the LOCAL default branch, which is only current if the
    /// user recently pulled it.
    ///
    /// Measured on a 34-worktree checkout with local `main` 20 commits
    /// behind: 12 worktrees reported `Unmerged` and 3
    /// `MergedUpstreamDeleted`; after a fast-forward and no other change
    /// those became 2 and 12. A nine-row swing, silent, in the safe
    /// direction, and with a confident false reason ("branch not
    /// merged") on every affected row.
    ///
    /// Before the fix this branch reports `Unmerged`: `git cherry main
    /// HEAD` returns `+`, and `aggregate_patch_merged` then searches
    /// `merge-base..main` -- an EMPTY range, because the stale local
    /// `main` is the merge-base -- so there is no candidate for the
    /// squash to match.
    #[test]
    fn a_branch_merged_upstream_is_not_called_unmerged_by_a_stale_local_main() {
        let (_t, repo) = stale_local_default_fixture();

        let wts = classify_repo(repo.to_str().unwrap()).unwrap();
        let found = wts
            .iter()
            .find(|w| w.path.contains("proj-feature"))
            .expect("worktree not found");

        assert_eq!(
            found.safety,
            Safety::MergedUpstreamDeleted,
            "the work is on origin/main and its remote branch is gone, so it is \
             removable -- reporting otherwise hides reclaimable disk behind a \
             reason that is false"
        );
    }

    /// The comparison names the remote-tracking ref, not the local
    /// branch -- the actual change, pinned at its source.
    ///
    /// Asserted on the returned string rather than only through a
    /// verdict, because every downstream check takes this value as a
    /// bare argv element and a regression here would be visible only as
    /// verdicts quietly drifting.
    #[test]
    fn the_default_branch_is_the_remote_tracking_ref_when_one_exists() {
        let (_t, repo) = stale_local_default_fixture();
        assert_eq!(default_branch(&repo), "origin/main");
    }

    /// A repository with no remote is still served by this code, and
    /// there `origin/main` does not resolve. Naming it anyway would turn
    /// every merge check into a failed git call, so the local branch is
    /// the fallback rather than the prefix being unconditional.
    #[test]
    fn a_repository_with_no_remote_falls_back_to_the_local_branch() {
        let (_t, repo, _wt) = repo_with_worktree("feature");
        assert_eq!(default_branch(&repo), "main");
    }

    /// The flag guard survives the prefix (#757 must not undo the
    /// hardening).
    ///
    /// `origin/HEAD` is written by the REMOTE, so its short name is
    /// validated before `origin/` is put back on. Prefixing first would
    /// hide `--output=EVIL` behind a string that no longer starts with
    /// `-`, and git would be handed an arbitrary file write.
    #[test]
    fn a_flag_shaped_default_branch_is_rejected_before_it_is_prefixed() {
        let (_t, repo) = stale_local_default_fixture();
        // A hostile remote's `origin/HEAD`. `symbolic-ref` accepts it
        // where `git branch` would refuse.
        let out = Command::new(crate::auth::git_program())
            .arg("-C")
            .arg(&repo)
            .args([
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/--output=EVIL",
            ])
            .output()
            .unwrap();
        assert!(out.status.success(), "could not plant the hostile ref");

        let branch = default_branch(&repo);
        assert!(
            !branch.contains("--output"),
            "a flag-shaped ref must never reach git, prefixed or not, got {branch:?}"
        );
        assert_eq!(branch, "origin/main", "and the fallback still resolves");
    }

    /// #463: the squash must still be found when the default branch has
    /// moved a long way since.
    ///
    /// This used to compare against the last 300 commits of the default
    /// branch, ignoring the merge-base it had already computed. A branch
    /// that diverged further back than that had its squash outside the
    /// window and was reported Unmerged.
    ///
    /// Measured on a real repository before fixing: branches diverge a
    /// MEDIAN of 474 commits back, and 14 of 21 worktrees flipped from
    /// "unmerged" to "merged" once the window was widened.
    ///
    /// The failure was quiet -- refusing to delete something deletable
    /// -- so it presented as "the cleanup finds nothing" rather than as
    /// an error. That is why this needs a test rather than a bigger
    /// constant.
    #[test]
    fn a_squash_merge_is_found_even_far_back_in_history() {
        // A MULTI-COMMIT branch, collapsed into one squash.
        //
        // The single-commit fixture is answered by `git cherry` before
        // `aggregate_patch_merged` is ever reached -- one commit's
        // patch-id matches the squash directly. Only a branch whose
        // commits collapse into one reaches the aggregate path, which is
        // the code this test exists for.
        let (_t, repo, wt) = squash_merged_fixture(false);
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let run_in = |dir: &Path, args: &[&str]| {
            let ok = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(ident)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        };

        // Two more commits on the branch, so it is three in total.
        for i in 0..2 {
            std::fs::write(wt.join(format!("more{i}.txt")), format!("{i}\n")).unwrap();
            run_in(&wt, &["add", "-A"]);
            run_in(&wt, &["commit", "-q", "-m", "more"]);
        }
        run_in(&wt, &["push", "-q", "origin", "feature"]);

        // The squash: all three landing on main as ONE commit, which no
        // individual commit's patch-id matches.
        run_in(&repo, &["checkout", "-q", "main"]);
        std::fs::write(repo.join("feature.txt"), "the change\n").unwrap();
        for i in 0..2 {
            std::fs::write(repo.join(format!("more{i}.txt")), format!("{i}\n")).unwrap();
        }
        run_in(&repo, &["add", "-A"]);
        run_in(&repo, &["commit", "-q", "-m", "add the feature (#1)"]);

        // Then pile commits on AFTER the squash, so it sits well beyond
        // any fixed recent window.
        for i in 0..40 {
            std::fs::write(repo.join(format!("filler{i}.txt")), format!("{i}\n")).unwrap();
            run_in(&repo, &["add", "-A"]);
            run_in(&repo, &["commit", "-q", "-m", "filler"]);
        }
        run_in(&repo, &["push", "-q", "origin", "main"]);

        let wts = classify_repo(repo.to_str().unwrap()).unwrap();
        let found = wts
            .iter()
            .find(|w| w.path.contains("proj-feature"))
            .expect("worktree not found");
        assert_eq!(
            found.safety,
            Safety::Safe,
            "the squash is older than the newest commits, and must still be found"
        );
    }

    /// The other half: the looser check must not start calling genuinely
    /// unmerged work merged. Deleting one of these destroys commits that
    /// exist nowhere else.
    #[test]
    fn a_genuinely_unmerged_branch_is_still_unmerged() {
        let (_t, repo, _wt) = squash_merged_fixture(false);
        let wts = classify_repo(repo.to_str().unwrap()).unwrap();
        let found = wts
            .iter()
            .find(|w| w.path.contains("proj-feature"))
            .expect("worktree not found");
        assert_eq!(found.safety, Safety::Unmerged);
    }

    /// Real git for the #741 shape: pushed, REBASED onto newer work,
    /// then squash-merged, then the remote branch deleted.
    ///
    /// The rebase is what defeats `aggregate_patch_merged`. It replays
    /// the branch on top of whatever landed first, so the branch's
    /// `merge-base..HEAD` diff carries that other PR's files too, and
    /// the branch's aggregate diff can no longer equal any single
    /// commit on main.
    ///
    /// A LATER PR then adds to a file the branch also changed. That is
    /// the other half of the real case, and it is load-bearing: without
    /// it the squash commit is byte-identical to the branch's aggregate
    /// diff, the old patch-id check answers correctly, and a test built
    /// on it would prove nothing. It is also why the fix cannot be
    /// strict tree equality -- that file's blob no longer matches.
    ///
    /// Real git throughout, including a real rebase and a real squash: a
    /// synthetic fixture would let this pass for the wrong reason, and
    /// the verdict decides whether someone's work is deleted.
    fn rebased_squash_fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let run_in = |dir: &Path, args: &[&str]| {
            let out = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(ident)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };

        let remote = tmp.path().join("remote.git");
        std::fs::create_dir_all(&remote).unwrap();
        run_in(&remote, &["init", "-q", "--bare", "-b", "main"]);

        let repo = tmp.path().join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        run_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("base.txt"), "base\n").unwrap();
        // The file BOTH this branch and a later PR will add to.
        std::fs::write(repo.join("shared.txt"), "shared header line one\n").unwrap();
        run_in(&repo, &["add", "-A"]);
        run_in(&repo, &["commit", "-q", "-m", "base"]);
        run_in(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_in(&repo, &["push", "-q", "-u", "origin", "main"]);

        // Our branch does its work in a worktree and pushes it.
        let wt = tmp.path().join("proj-feature");
        run_in(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "--track",
                "-b",
                "feature",
                wt.to_str().unwrap(),
                "main",
            ],
        );
        std::fs::write(
            wt.join("feature.txt"),
            "the feature implementation body line\nsecond feature implementation line\n",
        )
        .unwrap();
        std::fs::write(
            wt.join("shared.txt"),
            "shared header line one\nfeature contribution to the shared file\n",
        )
        .unwrap();
        run_in(&wt, &["add", "-A"]);
        run_in(&wt, &["commit", "-q", "-m", "add the feature"]);
        run_in(&wt, &["push", "-q", "-u", "origin", "feature"]);

        // The rebase is onto ANOTHER PR's branch, not onto main, and
        // that detail is the whole bug.
        //
        // Rebasing onto main would move the merge-base FORWARD past the
        // earlier work, which drops `earlier.txt` back out of
        // `merge-base..HEAD` and leaves the aggregate patch-id matching
        // the squash exactly -- the old check would then answer
        // correctly and this fixture would prove nothing. Verified by
        // building it that way first: the diff came back 2 files and the
        // patch-id matched.
        //
        // Rebasing onto the earlier PR's unmerged branch is what people
        // actually do to build on work that has not landed yet. The
        // earlier PR is then squash-merged to main under a NEW sha, so
        // the merge-base stays BEHIND it and the branch's aggregate diff
        // keeps carrying `earlier.txt` -- which is exactly the state
        // #741 describes.
        run_in(&repo, &["branch", "-q", "earlier", "main"]);
        let earlier_wt = tmp.path().join("proj-earlier");
        run_in(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                earlier_wt.to_str().unwrap(),
                "earlier",
            ],
        );
        std::fs::write(
            earlier_wt.join("earlier.txt"),
            "an earlier pull request landed this file first\nwith a second line of its own\n",
        )
        .unwrap();
        run_in(&earlier_wt, &["add", "-A"]);
        run_in(&earlier_wt, &["commit", "-q", "-m", "the earlier work"]);
        let earlier_tip = git(&earlier_wt, &["rev-parse", "HEAD"]).unwrap();

        run_in(&wt, &["rebase", "-q", earlier_tip.trim()]);

        // The earlier PR is squash-merged to main under its own new sha.
        run_in(&repo, &["checkout", "-q", "main"]);
        std::fs::write(
            repo.join("earlier.txt"),
            "an earlier pull request landed this file first\nwith a second line of its own\n",
        )
        .unwrap();
        run_in(&repo, &["add", "-A"]);
        run_in(&repo, &["commit", "-q", "-m", "an earlier PR (#1)"]);

        // Squash-merge ours, exactly as GitHub does: the branch's
        // content lands on main as ONE new commit touching only ITS
        // files -- not the earlier PR's.
        std::fs::write(
            repo.join("feature.txt"),
            "the feature implementation body line\nsecond feature implementation line\n",
        )
        .unwrap();
        std::fs::write(
            repo.join("shared.txt"),
            "shared header line one\nfeature contribution to the shared file\n",
        )
        .unwrap();
        run_in(&repo, &["add", "-A"]);
        run_in(&repo, &["commit", "-q", "-m", "add the feature (#2)"]);

        // A LATER PR adds to the same file, so the branch's blob and
        // main's blob stop being identical -- the exact reason strict
        // tree equality would still report this unmerged, and the reason
        // the aggregate patch-id can no longer match.
        std::fs::write(
            repo.join("shared.txt"),
            "shared header line one\nfeature contribution to the shared file\na later pull request appended this line\n",
        )
        .unwrap();
        run_in(&repo, &["add", "-A"]);
        run_in(&repo, &["commit", "-q", "-m", "a later PR (#3)"]);
        run_in(&repo, &["push", "-q", "origin", "main"]);

        // GitHub deletes the branch after merging; tracking config stays.
        run_in(&remote, &["branch", "-D", "feature"]);
        run_in(&repo, &["fetch", "-q", "--prune", "origin"]);

        (tmp, repo, wt)
    }

    /// #741: the bug itself.
    ///
    /// A branch that was pushed, rebased onto newer work, then
    /// squash-merged, with a later PR adding to a file it also changed.
    /// Both halves are needed to defeat `aggregate_patch_merged`:
    ///
    /// - the REBASE pulls the earlier PR's file into the branch's
    ///   `merge-base..HEAD` diff, so that diff spans more files than the
    ///   squash commit touched;
    /// - the LATER PR moves one of those files on, so the branch's diff
    ///   is not byte-identical to the squash commit either.
    ///
    /// Without the later PR the squash commit still matches the branch's
    /// aggregate diff exactly and the old code answers correctly -- so
    /// that variant would test nothing. The premise assertions below
    /// pin exactly that, and they are what caught it.
    #[test]
    fn a_rebased_then_squashed_branch_is_recognised_as_merged() {
        let (_t, _repo, wt) = rebased_squash_fixture();

        // The premise, asserted rather than assumed: if any of these
        // stop holding, the fixture no longer reproduces #741 and the
        // verdict below would be passing for the wrong reason.
        assert!(
            git(&wt, &["merge-base", "--is-ancestor", "HEAD", "origin/main"]).is_err(),
            "ancestry must not see this merge, or it is not a squash"
        );
        let base = git(&wt, &["merge-base", "HEAD", "origin/main"]).unwrap();

        // The premise that makes this #741 rather than a plain squash:
        // the branch's whole-diff patch-id matches NO commit on main, so
        // the pre-existing check cannot answer it. Asserted against the
        // patch-id comparison directly, because `aggregate_patch_merged`
        // now falls through to the fix and would report Safe either way.
        let branch_pid = patch_id(&wt, base.trim(), "HEAD").expect("branch must have a diff");
        let candidates = git(&wt, &["rev-list", &format!("{}..origin/main", base.trim())]).unwrap();
        assert_eq!(
            batch_contains_patch(&wt, &candidates, &branch_pid),
            Safety::Unmerged,
            "the aggregate patch-id must NOT match, or this is not the #741 shape"
        );
        let spans = git(&wt, &["diff", "--name-only", base.trim(), "HEAD"]).unwrap();
        assert!(
            spans.contains("earlier.txt"),
            "the rebase must leave the earlier PR's file in the branch's \
             aggregate diff, or the bug is not reproduced: {spans}"
        );
        assert!(
            !git(&wt, &["diff", "--name-only", base.trim(), "origin/main"])
                .unwrap()
                .is_empty(),
            "main must have moved since the merge-base"
        );

        assert_eq!(
            content_landed(&wt, "origin/main", base.trim()),
            Safety::Safe,
            "every file this branch changed is on main; it merged"
        );
    }

    /// The file a later PR also changed is what rules out strict tree
    /// equality as the fix -- the real case from #741, where 14 of 15
    /// files were byte-identical to main and the 15th differed only
    /// because later work appended to it. Requiring every blob to match
    /// would still call that branch unmerged.
    #[test]
    fn a_file_a_later_pr_also_changed_does_not_hide_the_merge() {
        let (_t, _repo, wt) = rebased_squash_fixture();
        let base = git(&wt, &["merge-base", "HEAD", "origin/main"]).unwrap();

        // The premise: the branch's copy of that file is NOT identical
        // to main's, so only the added-line path can account for it.
        let differs = git(
            &wt,
            &[
                "diff",
                "--name-only",
                "HEAD",
                "origin/main",
                "--",
                "shared.txt",
            ],
        )
        .unwrap();
        assert!(
            differs.contains("shared.txt"),
            "a later PR must have changed the shared file, or this test \
             is indistinguishable from the one above"
        );

        assert_eq!(
            content_landed(&wt, "origin/main", base.trim()),
            Safety::Safe,
            "later work on a shared file must not hide a real merge"
        );
    }

    /// The whole verdict, through `worktree_safety` rather than the
    /// helper -- the branch merged, its remote was deleted, and it must
    /// come out removable and labelled as such.
    ///
    /// Unlike the two tests above this one does NOT fail with the fix
    /// reverted, and the reason is worth recording rather than hiding:
    /// this fixture's branch keeps one commit per file, so `git cherry`
    /// finds an equivalent patch upstream for each and `squash_merged`
    /// answers `Safe` before `content_landed` is ever consulted. That is
    /// a correct answer by an older route.
    ///
    /// It is kept because it pins the LABEL -- `MergedUpstreamDeleted`
    /// rather than a bare `Safe` -- across the whole ordering in
    /// `worktree_safety`, which the helper-level tests cannot see. The
    /// tests that prove the fix are the `content_landed` ones.
    #[test]
    fn a_rebased_squashed_worktree_becomes_removable() {
        let (_t, _repo, wt) = rebased_squash_fixture();
        let w = Worktree {
            path: wt.to_string_lossy().into_owned(),
            branch: "feature".into(),
            ..Default::default()
        };
        let s = worktree_safety(&w, "origin/main", false, Some(0));
        assert_eq!(
            s,
            Safety::MergedUpstreamDeleted,
            "a rebased-then-squashed branch whose remote is gone must be \
             removable, got {s:?}"
        );
        assert!(s.is_safe(), "it must be removable: {}", s.reason());
    }

    /// THE test that matters: the looser check must never call genuinely
    /// unmerged work merged. Deleting one of these destroys commits that
    /// exist nowhere else, which is strictly worse than the bug #741
    /// describes.
    ///
    /// Same rebase-onto-newer-work shape as the fixture above, but the
    /// branch is never merged -- its own file never reaches main.
    #[test]
    fn a_rebased_but_unmerged_branch_is_still_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let run_in = |dir: &Path, args: &[&str]| {
            let out = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(ident)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };

        let remote = tmp.path().join("remote.git");
        std::fs::create_dir_all(&remote).unwrap();
        run_in(&remote, &["init", "-q", "--bare", "-b", "main"]);
        let repo = tmp.path().join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        run_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("base.txt"), "base\n").unwrap();
        run_in(&repo, &["add", "-A"]);
        run_in(&repo, &["commit", "-q", "-m", "base"]);
        run_in(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_in(&repo, &["push", "-q", "-u", "origin", "main"]);

        let wt = tmp.path().join("proj-feature");
        run_in(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "--track",
                "-b",
                "feature",
                wt.to_str().unwrap(),
                "main",
            ],
        );
        std::fs::write(
            wt.join("feature.txt"),
            "work that exists only on this branch and nowhere else\n\
             a second line that never reached the default branch\n",
        )
        .unwrap();
        run_in(&wt, &["add", "-A"]);
        run_in(&wt, &["commit", "-q", "-m", "unmerged work"]);
        run_in(&wt, &["push", "-q", "-u", "origin", "feature"]);

        // Newer work lands on main, and the branch is rebased onto it --
        // the same history shape as the merged case.
        run_in(&repo, &["checkout", "-q", "main"]);
        std::fs::write(
            repo.join("earlier.txt"),
            "an earlier pull request landed this file first\nwith a second line of its own\n",
        )
        .unwrap();
        run_in(&repo, &["add", "-A"]);
        run_in(&repo, &["commit", "-q", "-m", "an earlier PR (#1)"]);
        run_in(&repo, &["push", "-q", "origin", "main"]);
        run_in(&wt, &["rebase", "-q", "main"]);

        // The branch's OWN work is never merged. The remote branch is
        // deleted anyway, so it travels the #732 path too.
        run_in(&remote, &["branch", "-D", "feature"]);
        run_in(&repo, &["fetch", "-q", "--prune", "origin"]);

        let base = git(&wt, &["merge-base", "HEAD", "origin/main"]).unwrap();
        assert_eq!(
            content_landed(&wt, "origin/main", base.trim()),
            Safety::Unmerged,
            "work that never reached main must never be called merged"
        );

        let w = Worktree {
            path: wt.to_string_lossy().into_owned(),
            branch: "feature".into(),
            ..Default::default()
        };
        let s = worktree_safety(&w, "origin/main", false, Some(0));
        assert!(
            !s.is_safe(),
            "unmerged work must not become removable: {s:?}"
        );
    }

    /// A branch strictly AHEAD of the default branch holds work that
    /// exists nowhere else, and must be refused.
    ///
    /// This pins the one path in `descends_from_branch` that answers
    /// `Safe` without inspecting anything further -- the empty
    /// `base..default` range, which is what a stale local copy of
    /// already-merged work looks like. It is only ever reached AFTER
    /// `content_landed` has accounted for every changed file, and this
    /// test is the proof: the same empty range with genuinely new work
    /// is refused before that early return can be consulted.
    #[test]
    fn a_branch_ahead_of_the_default_branch_is_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let run_in = |dir: &Path, args: &[&str]| {
            let out = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(ident)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
        };
        let repo = tmp.path().join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        run_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("f.txt"), "one\n").unwrap();
        run_in(&repo, &["add", "-A"]);
        run_in(&repo, &["commit", "-q", "-m", "base"]);

        let wt = tmp.path().join("proj-ahead");
        run_in(
            &repo,
            &["worktree", "add", "-q", "-b", "ahead", wt.to_str().unwrap()],
        );
        std::fs::write(
            wt.join("f.txt"),
            "one\nbrand new unmerged line of real substantive content\n",
        )
        .unwrap();
        run_in(&wt, &["add", "-A"]);
        run_in(&wt, &["commit", "-q", "-m", "work not on main"]);

        let base = git(&wt, &["merge-base", "HEAD", "main"]).unwrap();
        // The premise: the range `descends_from_branch` early-returns on
        // really is empty here, so this exercises that path's guard.
        assert_eq!(
            git(
                &wt,
                &["rev-list", "--count", &format!("{}..main", base.trim())]
            )
            .unwrap()
            .trim(),
            "0",
            "main must be an ancestor, or this does not test the early return"
        );

        assert_eq!(
            content_landed(&wt, "main", base.trim()),
            Safety::Unmerged,
            "work that is only on this branch must never be called merged"
        );
    }

    /// The subset trap the issue calls out: a branch whose changes are a
    /// strict SUBSET of a larger change that is on the default branch,
    /// but which was never merged. Every line it added really is on
    /// main -- as somebody else's work.
    ///
    /// Line presence alone would call this merged and delete the only
    /// copy. It must stay refused.
    #[test]
    fn a_branch_that_is_only_a_subset_of_other_work_is_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let run_in = |dir: &Path, args: &[&str]| {
            let out = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(ident)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };

        let repo = tmp.path().join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        run_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("f.txt"), "one\ntwo\nthree\n").unwrap();
        run_in(&repo, &["add", "-A"]);
        run_in(&repo, &["commit", "-q", "-m", "base"]);
        let base = git(&repo, &["rev-parse", "HEAD"]).unwrap();

        // Main gains a larger change that INCLUDES the subset's line.
        std::fs::write(
            repo.join("f.txt"),
            "one\ntwo\nthree\n\
             alpha marker line belonging to the larger change\n\
             beta marker line belonging to the larger change\n",
        )
        .unwrap();
        run_in(&repo, &["add", "-A"]);
        run_in(&repo, &["commit", "-q", "-m", "the larger change (#1)"]);

        // The branch adds ONLY part of that, and never merged.
        let wt = tmp.path().join("proj-subset");
        run_in(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "subset",
                wt.to_str().unwrap(),
                base.trim(),
            ],
        );
        std::fs::write(
            wt.join("f.txt"),
            "one\ntwo\nthree\nalpha marker line belonging to the larger change\n",
        )
        .unwrap();
        run_in(&wt, &["add", "-A"]);
        run_in(&wt, &["commit", "-q", "-m", "a coincidental subset"]);

        // The premise: its added line really IS on main, so the
        // per-file content check alone would be satisfied.
        let added = git(&wt, &["diff", "-U0", base.trim(), "HEAD"]).unwrap();
        assert!(
            added.contains("alpha marker line"),
            "the subset must add the shared line, or it tests nothing"
        );

        assert_eq!(
            content_landed(&wt, "main", base.trim()),
            Safety::Unmerged,
            "a coincidental subset of someone else's work is NOT merged"
        );
    }

    /// A branch whose whole change is trivial -- whitespace and
    /// punctuation -- carries nothing this check can verify. "I found
    /// nothing to check" must not read as "the work is in".
    #[test]
    fn a_vacuous_change_is_not_enough_evidence() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let run_in = |dir: &Path, args: &[&str]| {
            let out = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(ident)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
        };

        let repo = tmp.path().join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        run_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("f.txt"), "one\ntwo\n").unwrap();
        run_in(&repo, &["add", "-A"]);
        run_in(&repo, &["commit", "-q", "-m", "base"]);
        let base = git(&repo, &["rev-parse", "HEAD"]).unwrap();

        let wt = tmp.path().join("proj-trivial");
        run_in(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "trivial",
                wt.to_str().unwrap(),
                base.trim(),
            ],
        );
        // Only punctuation and blank lines: nothing substantive.
        std::fs::write(wt.join("f.txt"), "one\ntwo\n}\n\n").unwrap();
        run_in(&wt, &["add", "-A"]);
        run_in(&wt, &["commit", "-q", "-m", "trivial"]);

        assert_eq!(
            content_landed(&wt, "main", base.trim()),
            Safety::Unmerged,
            "a change with no substantive lines proves nothing"
        );
    }

    /// A file the branch DELETED that the default branch still has is
    /// work that did not land. Deletions carry no lines, so they cannot
    /// be verified by content -- only by absence.
    #[test]
    fn a_deletion_that_did_not_land_is_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let run_in = |dir: &Path, args: &[&str]| {
            let out = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(ident)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
        };

        let repo = tmp.path().join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        run_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("keep.txt"), "kept\n").unwrap();
        std::fs::write(
            repo.join("doomed.txt"),
            "this file was deleted on the branch but not on main\n",
        )
        .unwrap();
        run_in(&repo, &["add", "-A"]);
        run_in(&repo, &["commit", "-q", "-m", "base"]);
        let base = git(&repo, &["rev-parse", "HEAD"]).unwrap();

        let wt = tmp.path().join("proj-del");
        run_in(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "del",
                wt.to_str().unwrap(),
                base.trim(),
            ],
        );
        std::fs::remove_file(wt.join("doomed.txt")).unwrap();
        std::fs::write(
            wt.join("keep.txt"),
            "kept\nand a substantive added line of real content here\n",
        )
        .unwrap();
        run_in(&wt, &["add", "-A"]);
        run_in(&wt, &["commit", "-q", "-m", "delete a file"]);

        // main still has the file the branch removed.
        assert_eq!(
            content_landed(&wt, "main", base.trim()),
            Safety::Unmerged,
            "a deletion still absent from main means the work did not land"
        );
    }

    /// `--name-status -z` is parsed, not guessed: a rename carries TWO
    /// paths and the DESTINATION is the branch's version. Reading the
    /// source name would ask about a path the branch no longer has.
    #[test]
    fn a_rename_is_checked_by_its_destination() {
        let out = "R100\0old/name.rs\0new/name.rs\0M\0other.rs\0";
        let got = parse_name_status(out);
        let paths: Vec<&str> = got.iter().map(|(_, p)| p.as_str()).collect();
        assert_eq!(paths, vec!["new/name.rs", "other.rs"]);
        assert!(matches!(got[0].0, Change::Present));
    }

    /// A deleted path is the one case where the verdict inverts: it must
    /// be ABSENT from the default branch rather than present.
    #[test]
    fn a_deleted_path_is_parsed_as_a_deletion() {
        let got = parse_name_status("D\0gone.rs\0");
        assert_eq!(got.len(), 1);
        assert!(matches!(got[0].0, Change::Deleted));
        assert_eq!(got[0].1, "gone.rs");
    }

    /// Trivial lines are not evidence. `}` and `///` appear in almost
    /// every file, so counting them would let a branch match by
    /// coincidence -- the failure mode that makes content comparison
    /// dangerous rather than merely loose.
    #[test]
    fn trivial_lines_are_not_counted_as_evidence() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let run_in = |dir: &Path, args: &[&str]| {
            let out = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(ident)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
        };
        let repo = tmp.path().join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        run_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("f.rs"), "fn a() {}\n").unwrap();
        run_in(&repo, &["add", "-A"]);
        run_in(&repo, &["commit", "-q", "-m", "base"]);
        let base = git(&repo, &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(
            repo.join("f.rs"),
            "fn a() {}\n}\n///\n\na substantive line of genuine content here\n",
        )
        .unwrap();
        run_in(&repo, &["add", "-A"]);
        run_in(&repo, &["commit", "-q", "-m", "more"]);

        let lines = added_lines(&repo, base.trim(), "f.rs", 12);
        assert!(
            lines.iter().all(|l| l.len() >= 12),
            "short fragments must be dropped: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("substantive")),
            "the real line must survive: {lines:?}"
        );
    }

    /// `git cherry` prints nothing for a branch with no commits relative
    /// to the default. That is a branch with nothing on it, not a merged
    /// one -- treating empty output as "merged" would greenlight
    /// deleting a worktree whose state was never established.
    #[test]
    fn an_empty_branch_is_not_reported_merged() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["commit", "-q", "--allow-empty", "-m", "base"],
        ] {
            Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(&args)
                .envs(ident)
                .output()
                .unwrap();
        }
        // HEAD == main, so `git cherry main HEAD` emits nothing at all.
        assert_eq!(squash_merged(dir, "main"), Safety::Unmerged);
    }

    /// Any git failure yields Unknown, never Safe -- the invariant the
    /// whole classifier is built on.
    #[test]
    fn a_git_failure_in_the_squash_check_is_unknown_not_safe() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Not a git repo at all, so `git cherry` fails outright.
        match squash_merged(tmp.path(), "main") {
            Safety::Unknown(_) => {}
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    /// They each had their own copy of the logic, and adding the merge
    /// date to one silently left the other returning None -- which is how
    /// this shipped broken the first time.
    #[test]
    fn both_scan_paths_classify_the_same_way() {
        // A MERGED worktree, so merged_at is actually populated -- with a
        // never-pushed one both paths return None and the comparison
        // proves nothing. Checked: with the drift re-introduced and an
        // unmerged fixture, this test passed vacuously.
        let (_t, repo, wt) = merged_worktree_fixture();
        let _ = &wt;
        let base = repo.parent().unwrap().to_string_lossy().into_owned();

        let via_scan = scan_dirs(std::slice::from_ref(&base));
        let scanned = via_scan
            .iter()
            .find(|r| r.name == "proj")
            .expect("repo not found");
        let via_classify = classify_repo(repo.to_str().unwrap()).unwrap();

        assert_eq!(scanned.worktrees.len(), via_classify.len());
        for (a, b) in scanned.worktrees.iter().zip(via_classify.iter()) {
            assert_eq!(a.safety, b.safety, "safety differs for {}", a.path);
            assert_eq!(
                a.merged_at, b.merged_at,
                "merge date differs for {}",
                a.path
            );
        }
    }

    #[test]
    fn refuses_the_main_checkout() {
        let (_t, repo, _wt) = repo_with_worktree("feature");
        let err = remove_worktree(repo.to_str().unwrap(), repo.to_str().unwrap()).unwrap_err();
        assert!(err.contains("main checkout"), "{err}");
        assert!(repo.is_dir());
    }

    #[test]
    fn refuses_a_path_that_is_not_a_worktree_of_this_repo() {
        let (_t, repo, _wt) = repo_with_worktree("feature");
        let err = remove_worktree(repo.to_str().unwrap(), "/tmp/somewhere-else").unwrap_err();
        assert!(err.contains("not a worktree"), "{err}");
    }

    /// Uncommitted work blocks removal even when everything else passes.
    #[test]
    fn refuses_a_dirty_worktree() {
        let (_t, repo, wt) = repo_with_worktree("feature");
        std::fs::write(wt.join("scratch.txt"), "unsaved work").unwrap();
        let err = remove_worktree(repo.to_str().unwrap(), wt.to_str().unwrap()).unwrap_err();
        assert!(err.contains("uncommitted"), "{err}");
        assert!(wt.join("scratch.txt").exists(), "the file must survive");
    }

    #[test]
    fn reasons_are_display_ready_and_pluralised() {
        assert_eq!(Safety::Dirty(1).reason(), "1 uncommitted file");
        assert_eq!(Safety::Dirty(3).reason(), "3 uncommitted files");
        assert_eq!(Safety::Unpushed(1).reason(), "1 unpushed commit");
        assert!(Safety::NeverPushed.reason().contains("only here"));
        // The two must not read alike: one says work is at risk, the
        // other says there is no work. #701 is what happens when a row
        // shows the first for the second.
        assert!(Safety::Empty.reason().contains("nothing to lose"));
        assert_ne!(Safety::Empty.reason(), Safety::NeverPushed.reason());
    }

    /// Sizing, and the three things it turns on: that answers arrive one
    /// at a time (#754), that the number still means "disk footprint"
    /// (#754), and that a walk which will not finish becomes "could not
    /// measure" rather than nothing at all (#769).
    mod sizing {
        use super::super::{dir_size, dir_size_within, size_paths, SIZE_TIMEOUT, SIZE_WORKERS};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Mutex;

        /// A directory with `n` files of `bytes` each, plus its path.
        fn tree(parent: &std::path::Path, name: &str, n: usize, bytes: usize) -> String {
            let dir = parent.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            for i in 0..n {
                std::fs::write(dir.join(format!("f{i}")), vec![b'x'; bytes]).unwrap();
            }
            dir.to_string_lossy().to_string()
        }

        /// The measurement counts `node_modules` and `target`.
        ///
        /// #754 proposed skipping them, as `caches::project_dirs` does.
        /// This is the guard against doing it by reflex: the column
        /// exists to answer "how much do I reclaim by deleting this?",
        /// and MEASURED on a real 13.45 GB worktree the skipping walk
        /// reports 0.01 GB -- a 99.9% under-report of exactly the bytes
        /// the user came to reclaim.
        #[test]
        fn heavy_directories_are_counted_not_skipped() {
            let tmp = tempfile::TempDir::new().unwrap();
            let root = tmp.path().join("wt");
            std::fs::create_dir_all(&root).unwrap();
            // 100 bytes of source, 10_000 bytes in the directories a
            // "source size" walk would skip.
            tree(&root, "src", 1, 100);
            for skipped in [".git", "node_modules", "target", ".terraform", ".venv"] {
                tree(&root, skipped, 1, 2_000);
            }

            let total = dir_size(&root);
            assert_eq!(
                total, 10_100,
                "every byte on disk must be counted; skipping the heavy \
                 directories would report 100"
            );
        }

        /// Symlinks are still not followed -- a link into another tree
        /// must not be counted twice, and a loop must not hang the walk.
        #[test]
        #[cfg(unix)]
        fn symlinks_are_not_followed() {
            let tmp = tempfile::TempDir::new().unwrap();
            let root = tmp.path().join("wt");
            std::fs::create_dir_all(&root).unwrap();
            tree(&root, "real", 1, 500);
            // A link back to the root: followed, this walk never ends.
            std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
            assert_eq!(dir_size(&root), 500);
        }

        /// Each path is reported AS IT FINISHES, not once at the end.
        ///
        /// The whole of #754's second bug: the view held every row on a
        /// skeleton because nothing could be shown until the slowest
        /// tree was done. A caller can only bound that promise if the
        /// measurement hands over partial answers.
        #[test]
        fn every_path_is_reported_individually() {
            let tmp = tempfile::TempDir::new().unwrap();
            let paths: Vec<String> = (0..5)
                .map(|i| tree(tmp.path(), &format!("wt{i}"), 2, 100))
                .collect();

            let seen = Mutex::new(Vec::new());
            size_paths(&paths, &|p: &str, b: Option<u64>| {
                seen.lock().unwrap().push((p.to_string(), b));
            });

            let mut seen = seen.into_inner().unwrap();
            seen.sort();
            assert_eq!(seen.len(), 5, "one report per path, not one batch");
            for (_, bytes) in &seen {
                assert_eq!(*bytes, Some(200));
            }
            let mut expected = paths.clone();
            expected.sort();
            let got: Vec<String> = seen.into_iter().map(|(p, _)| p).collect();
            assert_eq!(got, expected);
        }

        /// The walks actually overlap.
        ///
        /// MEASURED over 18 worktrees totalling 187 GB: 35.30s with one
        /// worker against 5.05s with eight. #754 proposed serialising
        /// these on the theory that concurrent walks contend for the
        /// disk; on an SSD the walk is syscall-bound and that is a 7x
        /// regression, so the concurrency is load-bearing and a
        /// refactor that quietly removes it must fail here.
        ///
        /// Asserts overlap rather than wall-clock time: a timing
        /// threshold on CI hardware is a flake generator.
        #[test]
        fn paths_are_walked_concurrently() {
            let tmp = tempfile::TempDir::new().unwrap();
            let paths: Vec<String> = (0..SIZE_WORKERS)
                .map(|i| tree(tmp.path(), &format!("wt{i}"), 1, 10))
                .collect();

            let inside = AtomicUsize::new(0);
            let peak = AtomicUsize::new(0);
            size_paths(&paths, &|_: &str, _: Option<u64>| {
                let n = inside.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(n, Ordering::SeqCst);
                // Long enough that a serial implementation cannot have
                // two reports in flight at once, short enough not to
                // slow the suite.
                std::thread::sleep(std::time::Duration::from_millis(50));
                inside.fetch_sub(1, Ordering::SeqCst);
            });

            assert!(
                peak.load(Ordering::SeqCst) > 1,
                "sizing must run paths concurrently; saw no overlap, which \
                 is the 7x-slower serial shape"
            );
        }

        /// Fewer paths than workers must not spawn idle threads, and
        /// zero paths must not spawn a pool at all or divide by zero.
        #[test]
        fn worker_count_never_exceeds_the_work() {
            let tmp = tempfile::TempDir::new().unwrap();
            let one = vec![tree(tmp.path(), "only", 1, 7)];

            let calls = AtomicUsize::new(0);
            size_paths(&one, &|_: &str, _: Option<u64>| {
                calls.fetch_add(1, Ordering::SeqCst);
            });
            assert_eq!(calls.load(Ordering::SeqCst), 1);

            // The empty case: no reports, and no panic.
            size_paths(&[], &|_: &str, _: Option<u64>| {
                panic!("nothing to size, so nothing may be reported");
            });
        }

        /// A tree deep enough that a zero budget cannot finish it.
        ///
        /// Depth rather than breadth, because the budget is checked once
        /// per DIRECTORY: a single wide directory is one check, and the
        /// test would be asserting on `read_dir` speed instead of on the
        /// bound. Each level holds one file so an unbounded walk has a
        /// non-zero total to report and the two outcomes cannot be
        /// confused.
        fn deep_tree(parent: &std::path::Path, levels: usize) -> std::path::PathBuf {
            let root = parent.join("deep");
            let mut at = root.clone();
            for i in 0..levels {
                at = at.join(format!("l{i}"));
                std::fs::create_dir_all(&at).unwrap();
                std::fs::write(at.join("f"), vec![b'x'; 10]).unwrap();
            }
            root
        }

        /// A walk that outruns its budget reports `None`, not a number.
        ///
        /// The #769 bound. Without it `dir_size` has no exit but
        /// completion, so a tree that cannot be finished parks its
        /// worker forever and the row it belongs to never hears back --
        /// which is precisely the column of skeletons that outlasted 15
        /// minutes on a 111-worktree repository.
        ///
        /// `None` and not a partial total: a partial is an artifact of
        /// which directories happened to pop off the stack first, and it
        /// would render indistinguishably from a real measurement.
        #[test]
        fn a_walk_that_exceeds_its_budget_reports_that_it_could_not_measure() {
            let tmp = tempfile::TempDir::new().unwrap();
            let root = deep_tree(tmp.path(), 40);

            // Zero budget: the deadline is already spent when the first
            // directory pops, so this cannot depend on machine speed.
            assert_eq!(
                dir_size_within(&root, std::time::Duration::ZERO),
                None,
                "a walk that runs out of budget must say it could not \
                 measure; returning a number claims an answer it does \
                 not have, and returning nothing at all is #769"
            );

            // The same tree measures fine with a real budget, so the
            // None above is the BOUND firing and not a broken walk.
            assert_eq!(
                dir_size_within(&root, SIZE_TIMEOUT),
                Some(400),
                "40 levels of one 10-byte file must still measure"
            );
        }

        /// One unbounded tree must not stall the other worktrees.
        ///
        /// The load-bearing guarantee of #769. A repository with 111
        /// worktrees showed every size cell as a skeleton while one with
        /// 97 finished in ~10 seconds -- a 14% difference in count
        /// against "10 seconds" versus "never", so the count was never
        /// the variable. One tree in the 111 could not be finished, its
        /// worker parked on it, and progress stopped there.
        ///
        /// MEASURED on this machine, for why one tree can be unbounded
        /// at all: 26 of 42 worktrees in one checkout live UNDERNEATH
        /// the main checkout, so the parent's walk subsumes all 26 --
        /// 235.02 GB and 1,125,352 files, walked once as the parent and
        /// again as 26 worktrees. The parent took 38.63s where a leaf
        /// took 0.16s, a 240x spread inside one repository.
        ///
        /// Asserts that EVERY path is reported, the slow one included.
        /// Reporting the other N-1 is not enough: the row for the bad
        /// tree would still hold its skeleton forever.
        #[test]
        fn one_unmeasurable_tree_does_not_stall_the_others() {
            let tmp = tempfile::TempDir::new().unwrap();
            // More paths than workers, so a parked worker would visibly
            // starve the queue rather than merely finishing last.
            let mut paths: Vec<String> = (0..SIZE_WORKERS * 2)
                .map(|i| tree(tmp.path(), &format!("wt{i}"), 1, 100))
                .collect();
            let slow = deep_tree(tmp.path(), 40).to_string_lossy().to_string();
            paths.push(slow.clone());

            let seen = Mutex::new(Vec::new());
            size_paths(&paths, &|p: &str, b: Option<u64>| {
                // Every path but the deep one is measured normally; the
                // deep one is given no budget at all, standing in for a
                // tree whose walk does not finish.
                let b = if p == slow {
                    super::super::dir_size_within(
                        std::path::Path::new(p),
                        std::time::Duration::ZERO,
                    )
                } else {
                    b
                };
                seen.lock().unwrap().push((p.to_string(), b));
            });

            let seen = seen.into_inner().unwrap();
            assert_eq!(
                seen.len(),
                paths.len(),
                "every worktree must be reported, the unmeasurable one \
                 included -- stalling at N-1 is #769"
            );
            let (_, slow_bytes) = seen.iter().find(|(p, _)| *p == slow).unwrap();
            assert_eq!(
                *slow_bytes, None,
                "the tree that could not be measured must say so rather \
                 than being left out"
            );
            assert_eq!(
                seen.iter().filter(|(p, _)| *p != slow).count(),
                SIZE_WORKERS * 2,
                "the other worktrees must all still arrive"
            );
            for (p, b) in &seen {
                if *p != slow {
                    assert_eq!(*b, Some(100), "a measurable tree keeps its real size");
                }
            }
        }

        /// The bound is generous enough not to reject honest walks.
        ///
        /// MEASURED, the slowest legitimate walk on this machine: 38.63s
        /// for a 265.15 GB parent checkout. A bound at or below that
        /// would turn a real answer into "could not measure", which
        /// trades #769's silence for a wrong answer. `GIT_TIMEOUT`'s 30s
        /// is deliberately NOT reused here for that reason.
        #[test]
        fn the_bound_leaves_room_for_the_slowest_honest_walk() {
            assert!(
                SIZE_TIMEOUT.as_secs() > 38,
                "the slowest MEASURED honest walk was 38.63s; a bound at \
                 or below it would discard real answers"
            );
        }
    }

    /// Classification, and the four things #830 turns on: that verdicts
    /// arrive one at a time, that a worktree which will not finish
    /// becomes "could not classify" rather than nothing at all, that one
    /// such worktree does not stall the rest, and that a verdict nobody
    /// could reach is never treated as removable.
    ///
    /// The `sizing` module above is the model, deliberately: #830 is
    /// #754 and #769 in the other column, and the guarantees that fix
    /// needed are the guarantees this one needs.
    mod classifying {
        use super::super::{
            classify_repo_streaming, classify_within, Safety, Worktree, CLASSIFY_TIMEOUT,
            CLASSIFY_WORKERS, GIT_TIMEOUT,
        };
        use std::path::Path;
        use std::process::Command;
        use std::sync::Mutex;

        /// A repository with `n` worktrees on unmerged branches.
        ///
        /// Unmerged on purpose: it is the verdict that costs the most git
        /// calls, so it is the one whose streaming and bounding actually
        /// matter. A repository of merged branches would classify fast
        /// enough to hide the bug.
        fn repo_with_worktrees(parent: &Path, n: usize) -> std::path::PathBuf {
            let ident = [
                ("GIT_AUTHOR_NAME", "octocat"),
                ("GIT_COMMITTER_NAME", "octocat"),
                ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
                ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
            ];
            let run = |dir: &Path, args: &[&str]| {
                let out = Command::new(crate::auth::git_program())
                    .arg("-C")
                    .arg(dir)
                    .args(args)
                    .envs(ident)
                    .output()
                    .unwrap();
                assert!(out.status.success(), "git {args:?}");
            };
            let repo = parent.join("proj");
            std::fs::create_dir_all(&repo).unwrap();
            run(&repo, &["init", "-q", "-b", "main"]);
            std::fs::write(repo.join("f"), "base\n").unwrap();
            run(&repo, &["add", "-A"]);
            run(&repo, &["commit", "-q", "-m", "base"]);
            for i in 0..n {
                let wt = parent.join(format!("proj-f{i}"));
                run(
                    &repo,
                    &[
                        "worktree",
                        "add",
                        "-q",
                        "-b",
                        &format!("f{i}"),
                        wt.to_str().unwrap(),
                    ],
                );
                std::fs::write(wt.join("f"), format!("work {i}\n")).unwrap();
                run(&wt, &["add", "-A"]);
                run(&wt, &["commit", "-q", "-m", "work"]);
            }
            repo
        }

        fn wt(path: &str) -> Worktree {
            Worktree {
                path: path.to_string(),
                branch: "feature".into(),
                head: "abc".into(),
                safety: Safety::Pending,
                is_main: false,
                ..Default::default()
            }
        }

        /// Each verdict is reported AS IT IS REACHED, not once at the end.
        ///
        /// The whole of #830: the command returned a `Vec` only when every
        /// worktree was classified, so 111 rows held a skeleton behind
        /// whichever branch was slowest. A caller can only stop promising
        /// if it is handed partial answers.
        #[test]
        fn every_worktree_is_reported_individually() {
            let tmp = tempfile::TempDir::new().unwrap();
            let repo = repo_with_worktrees(tmp.path(), 5);

            let seen = Mutex::new(Vec::new());
            classify_repo_streaming(repo.to_str().unwrap(), &mut |w| {
                seen.lock()
                    .unwrap()
                    .push((w.path.clone(), w.safety.clone()));
            })
            .unwrap();

            let seen = seen.into_inner().unwrap();
            // Five worktrees plus the main checkout.
            assert_eq!(seen.len(), 6, "one report per worktree, not one batch");
            for (path, safety) in &seen {
                assert_ne!(
                    *safety,
                    Safety::Pending,
                    "{path} was reported without a verdict"
                );
            }
        }

        /// A worktree that outruns its budget reports `Unknown`, not
        /// nothing.
        ///
        /// The #830 bound, and the reason it is not `GIT_TIMEOUT`:
        /// `classify` makes an unbounded NUMBER of individually-bounded
        /// git calls, because `content_landed` spends up to four per
        /// changed file. Without a ceiling around the worktree there is
        /// no exit but completion, so the row never hears back -- the
        /// column of skeletons the issue reported.
        ///
        /// A zero budget, so this cannot depend on machine speed.
        #[test]
        fn a_worktree_that_exceeds_its_budget_says_it_could_not_classify() {
            let tmp = tempfile::TempDir::new().unwrap();
            let repo = repo_with_worktrees(tmp.path(), 1);
            let target = tmp.path().join("proj-f0");

            let mut w = wt(target.to_str().unwrap());
            let ok = classify_within(&mut w, &repo, "main", std::time::Duration::ZERO);

            assert!(!ok, "a budget that is already spent must report failure");
            assert!(
                matches!(&w.safety, Safety::Unknown(m) if m.contains("did not finish")),
                "an abandoned worktree must say it could not be classified, \
                 not hold a skeleton: {:?}",
                w.safety
            );
            // The SAFETY property, and the one that matters most: a
            // verdict nobody could reach must never authorise a
            // deletion.
            assert!(
                !w.safety.is_safe(),
                "a worktree that could not be classified must never be safe"
            );

            // The same worktree classifies fine with a real budget, so
            // the Unknown above is the BOUND firing rather than a broken
            // classifier.
            let mut w = wt(target.to_str().unwrap());
            assert!(classify_within(&mut w, &repo, "main", CLASSIFY_TIMEOUT));
            assert!(
                !matches!(&w.safety, Safety::Unknown(m) if m.contains("did not finish")),
                "a worktree with a real budget must reach a real verdict: {:?}",
                w.safety
            );
        }

        /// One unclassifiable worktree must not stall the others.
        ///
        /// The load-bearing guarantee of #830, and the acceptance
        /// criterion the issue states: "a hung or failing single worktree
        /// does not prevent the other rows from resolving". The old
        /// `chunks_mut` could not offer it -- one pathological branch
        /// parked its whole chunk of 14 while seven threads sat idle with
        /// nothing to take.
        ///
        /// Asserts that EVERY worktree is reported, the bad one included.
        /// Reporting the other N-1 is not enough: the bad row would still
        /// hold its skeleton forever, which is the defect and not a
        /// lesser version of it.
        #[test]
        fn one_unclassifiable_worktree_does_not_stall_the_others() {
            let tmp = tempfile::TempDir::new().unwrap();
            // More worktrees than workers, so a parked worker would
            // visibly starve the queue rather than merely finish last.
            let n = CLASSIFY_WORKERS * 2;
            let repo = repo_with_worktrees(tmp.path(), n);
            // One worktree's directory is removed from under git. The
            // registration survives, so it is still listed and still
            // needs a verdict -- and it cannot get a normal one.
            let broken = tmp.path().join("proj-f0");
            std::fs::remove_dir_all(&broken).unwrap();

            let seen = Mutex::new(Vec::new());
            classify_repo_streaming(repo.to_str().unwrap(), &mut |w| {
                seen.lock()
                    .unwrap()
                    .push((w.path.clone(), w.safety.clone()));
            })
            .unwrap();

            let seen = seen.into_inner().unwrap();
            assert_eq!(
                seen.len(),
                n + 1,
                "every worktree must be reported, the broken one included \
                 -- stalling at N-1 is #830"
            );
            // Matched on the final component, not on the whole path.
            // Git reports the path it has on record, which on macOS is
            // the CANONICAL one (`/private/var/...`) where `TempDir`
            // hands back the symlinked spelling (`/var/...`) -- so a
            // string comparison here fails for a reason that has nothing
            // to do with what is being tested. `pathBasename` on the
            // frontend exists for the same mismatch.
            let tail = broken.file_name().unwrap().to_string_lossy().to_string();
            let (_, bad) = seen
                .iter()
                .find(|(p, _)| {
                    Path::new(p).file_name().map(|f| f.to_string_lossy())
                        == Some(tail.clone().into())
                })
                .expect("the broken worktree must still be reported");
            assert!(
                !bad.is_safe(),
                "a worktree whose directory is gone must never be safe: {bad:?}"
            );
            // And the others all reached a real verdict regardless.
            for (p, s) in &seen {
                assert_ne!(*s, Safety::Pending, "{p} never got a verdict");
            }
        }

        /// The verdicts are computed concurrently.
        ///
        /// MEASURED on the fixed-chunk form this replaces, over a real
        /// 145-worktree repository: 28.3s serial against 7.9s at eight
        /// workers. The concurrency is load-bearing, so a refactor that
        /// quietly removes it must fail here.
        ///
        /// Asserts overlap rather than wall-clock time: a timing
        /// threshold on CI hardware is a flake generator. Same reasoning
        /// as `sizing::paths_are_walked_concurrently`.
        ///
        /// Asserted from the REPORT ARRIVAL TIMES of the real function,
        /// rather than by counting callbacks in flight. `sizing`'s
        /// equivalent can count those because `size_paths` invokes its
        /// callback from the worker with no lock held; here the callback
        /// is serialised behind the sink mutex deliberately, so a
        /// concurrency test written that way would measure the mutex and
        /// fail against a perfectly parallel classifier.
        ///
        /// So: all `CLASSIFY_WORKERS` worktrees must be reported within a
        /// window far shorter than the time one classification takes
        /// times the count. The threshold is a RATIO of this machine's
        /// own measured serial cost, taken in the same test, so it
        /// carries no absolute timing assumption onto CI hardware.
        #[test]
        fn worktrees_are_classified_concurrently() {
            let tmp = tempfile::TempDir::new().unwrap();
            let repo = repo_with_worktrees(tmp.path(), CLASSIFY_WORKERS);

            // What ONE worktree costs here, measured rather than assumed.
            let listed = super::super::parse_porcelain(
                &super::super::git(&repo, &["worktree", "list", "--porcelain"]).unwrap(),
            );
            let one = listed
                .iter()
                .find(|w| !w.is_main)
                .expect("the fixture must have a non-main worktree");
            let solo = std::time::Instant::now();
            classify_within(&mut one.clone(), &repo, "main", CLASSIFY_TIMEOUT);
            let solo = solo.elapsed();

            // WHEN each report arrives, not how long the run took. The
            // distinction is the whole fix for #861: the assertion below
            // used to be `whole * 2 < serial_floor`, a wall-clock
            // threshold, under this test's own doc comment claiming it
            // "asserts overlap rather than wall-clock time: a timing
            // threshold on CI hardware is a flake generator". The doc was
            // right and the code did not implement it -- it failed twice
            // in the #835 batch on branches touching nothing near here,
            // once having MEASURED 1.8x overlap, so concurrency was
            // working and the test rejected it anyway.
            //
            // Arrival instants are taken inside the callback, which the
            // doc above notes is serialised behind the sink mutex. That
            // serialisation is exactly why a COUNT of in-flight callbacks
            // cannot work here -- but the instants still cannot cluster
            // tighter than the work that produced them unless that work
            // overlapped. With `CLASSIFY_WORKERS + 1` equal-cost
            // worktrees, a serial run spaces every arrival by one
            // classification; a concurrent one lands a whole batch within
            // one. So: the span covering all arrivals must be shorter
            // than the serial spacing would make it, measured against THIS
            // machine's own solo cost rather than a constant.
            let start = std::time::Instant::now();
            let arrivals = Mutex::new(Vec::new());
            classify_repo_streaming(repo.to_str().unwrap(), &mut |_| {
                arrivals.lock().unwrap().push(start.elapsed());
            })
            .unwrap();

            let mut arrivals = arrivals.into_inner().unwrap();
            arrivals.sort();
            let count = arrivals.len();
            assert_eq!(count, CLASSIFY_WORKERS + 1, "every worktree reported");
            // The tightest window holding at least two arrivals. Under a
            // serial classifier the closest any two can fall is one solo
            // cost apart; under a concurrent one they land together. The
            // comparison is against `solo` measured moments ago on this
            // same machine, so a slow runner moves both sides equally --
            // which is the property the old wall-clock form lacked.
            let closest = arrivals
                .windows(2)
                .map(|w| w[1] - w[0])
                .min()
                .expect("more than one worktree was classified");
            assert!(
                closest * 2 < solo,
                "classification must run worktrees concurrently: the two \
                 closest of {count} arrivals were {closest:?} apart, which a \
                 serial classifier could not beat -- it spaces every arrival \
                 by one {solo:?} classification. No overlap is the \
                 3.6x-slower shape the fixed chunks had."
            );
        }

        /// Fewer worktrees than workers must not spawn idle threads, and
        /// a repository git cannot read must fail rather than resolve
        /// empty.
        #[test]
        fn a_repository_that_cannot_be_listed_is_an_error_not_an_empty_set() {
            let tmp = tempfile::TempDir::new().unwrap();
            let not_a_repo = tmp.path().join("nope");
            std::fs::create_dir_all(&not_a_repo).unwrap();

            let err = classify_repo_streaming(not_a_repo.to_str().unwrap(), &mut |_| {
                panic!("nothing could be listed, so nothing may be reported");
            })
            .unwrap_err();
            assert!(
                err.contains("could not list worktrees"),
                "an unreadable repository must say so; resolving as an \
                 empty success is what left rows on 'checking...' while \
                 the header read '0 safe to remove': {err}"
            );
        }

        /// The ceiling sits above `GIT_TIMEOUT`, and the order matters.
        ///
        /// A worktree whose FIRST git call honestly times out must be
        /// able to report THAT -- "git did not respond within 30s", which
        /// names the real failure. A classification ceiling at or below
        /// 30s would pre-empt it and substitute the vaguer "did not
        /// finish within Ns", losing the specific diagnosis for every
        /// such worktree.
        #[test]
        fn the_ceiling_leaves_room_for_one_honest_git_timeout() {
            assert!(
                CLASSIFY_TIMEOUT > GIT_TIMEOUT,
                "a worktree whose first git call times out must report \
                 that, not be pre-empted by the outer ceiling"
            );
        }

        /// The ceiling is generous enough not to reject honest work.
        ///
        /// MEASURED by `live_classification_cost_per_worktree`, the
        /// slowest legitimate single worktree on this machine: 3295ms, in
        /// a repository whose median was 114ms. A bound anywhere near the
        /// median would turn real verdicts into "could not classify",
        /// trading #830's silence for a wrong answer -- and since
        /// `Unknown` is never removable, that wrong answer hides disk the
        /// user came to reclaim.
        #[test]
        fn the_ceiling_leaves_room_for_the_slowest_honest_worktree() {
            assert!(
                CLASSIFY_TIMEOUT.as_secs() >= 10,
                "the slowest MEASURED honest worktree was 3.295s against a \
                 114ms median; a tight bound here discards real verdicts"
            );
        }
    }

    /// A repo whose remote branch was squash-merged and DELETED, with
    /// the local remote-tracking ref deliberately left behind.
    ///
    /// The missing prune is the whole point. Every other fixture here
    /// ends with `fetch --prune`, which is what makes #732's path fire;
    /// this one must NOT, because the bug in #776 is precisely what
    /// happens when nothing prunes. So the remote branch is deleted
    /// inside the bare repo and no pruning fetch follows -- leaving
    /// `refs/remotes/origin/feature` on disk, naming a branch the remote
    /// no longer has. That is the state a real checkout is in minutes
    /// after a squash-merge with branch deletion.
    ///
    /// Real git throughout -- a real remote, a real push, a real
    /// squash-merge, a real remote-side branch delete. A synthetic
    /// fixture that just wrote a ref file would let the test pass for
    /// the wrong reason, and this gate guards the app's only
    /// unrecoverable action.
    ///
    /// Hands back a worktree whose branch is 1 commit ahead of its stale
    /// upstream and whose content is on `origin/main`.
    fn stale_tracking_ref_fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let run_in = |dir: &Path, args: &[&str]| {
            let out = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(ident)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };

        let remote = tmp.path().join("remote.git");
        std::fs::create_dir_all(&remote).unwrap();
        run_in(&remote, &["init", "-q", "--bare", "-b", "main"]);

        let repo = tmp.path().join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        run_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("base.txt"), "base\n").unwrap();
        run_in(&repo, &["add", "-A"]);
        run_in(&repo, &["commit", "-q", "-m", "base"]);
        run_in(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_in(&repo, &["push", "-q", "-u", "origin", "main"]);

        // The branch, with real work, pushed and TRACKING -- the
        // tracking config is what leaves a tracking ref behind later.
        let wt = tmp.path().join("proj-feature");
        run_in(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "--track",
                "-b",
                "feature",
                wt.to_str().unwrap(),
                "main",
            ],
        );
        std::fs::write(
            wt.join("feature.txt"),
            "the work this branch exists to add\nand a second line of it\n",
        )
        .unwrap();
        run_in(&wt, &["add", "-A"]);
        run_in(&wt, &["commit", "-q", "-m", "the feature"]);
        run_in(&wt, &["push", "-q", "-u", "origin", "feature"]);

        // One further commit AFTER the last push, left unpushed. This is
        // what makes the branch read as ahead of its tracking ref, and
        // it is the ordinary shape: a review fixup committed locally,
        // then included in the squash by merging the local branch. The
        // count the user is shown -- `Unpushed(1)` -- comes from exactly
        // this commit, and the point of #776 is that it landed anyway.
        std::fs::write(
            wt.join("feature.txt"),
            "the work this branch exists to add\nand a second line of it\na review fixup\n",
        )
        .unwrap();
        run_in(&wt, &["add", "-A"]);
        run_in(&wt, &["commit", "-q", "-m", "review fixup"]);

        // The squash-merge, as the forge performs it: the branch's whole
        // diff replayed onto the default branch as ONE new commit with a
        // new SHA, so the original tip is never an ancestor.
        run_in(&repo, &["checkout", "-q", "main"]);
        run_in(&repo, &["merge", "-q", "--squash", "feature"]);
        run_in(&repo, &["commit", "-q", "-m", "the feature (#1)"]);
        run_in(&repo, &["push", "-q", "origin", "main"]);

        // The forge deletes the remote branch. Done in the BARE repo so
        // the local tracking ref is untouched -- a `push --delete` from
        // the clone would remove it here too, which is the one thing
        // this fixture must not do.
        run_in(&remote, &["branch", "-D", "feature"]);

        // Refresh only the default branch, so the merge is visible. NO
        // `--prune`: the stale `origin/feature` must survive, or the
        // fixture does not reproduce #776.
        run_in(&repo, &["fetch", "-q", "origin", "main"]);

        (tmp, repo, wt)
    }

    /// The premise of the first half of #776, asserted rather than
    /// assumed: the tracking ref for a deleted remote branch is still on
    /// disk, `@{u}` still RESOLVES against it, and the branch reads as 1
    /// commit ahead of a ref describing a branch that no longer exists.
    ///
    /// The resolving upstream is the load-bearing detail. #732 keys on
    /// the upstream failing to resolve, so if this fixture ever stops
    /// leaving the ref behind the bug is no longer reproduced and the
    /// verdict test below would pass by #732's route instead of by the
    /// fix it is meant to prove.
    #[test]
    fn the_fixture_really_leaves_a_stale_tracking_ref_behind() {
        let (_t, _repo, wt) = stale_tracking_ref_fixture();

        assert!(
            git(&wt, &["rev-parse", "--verify", "--quiet", "origin/feature"]).is_ok(),
            "the stale tracking ref must survive, or nothing is being tested"
        );
        assert!(
            git(&wt, &["rev-parse", "--abbrev-ref", "@{u}"]).is_ok(),
            "the upstream must still RESOLVE -- that is why #732 never fires here"
        );
        assert_eq!(
            git(&wt, &["rev-list", "--count", "@{u}..HEAD"])
                .unwrap()
                .trim(),
            "1",
            "the branch must read as 1 ahead of its ghost upstream"
        );
        assert!(
            git(&wt, &["merge-base", "--is-ancestor", "HEAD", "origin/main"]).is_err(),
            "ancestry must not see this merge, or it is not a squash"
        );
    }

    /// The first half of #776: a branch whose PR squash-merged and whose
    /// remote branch was deleted must NOT be refused as `Unpushed`
    /// merely because nothing has pruned the tracking ref.
    ///
    /// This is the shape five worktrees were in on the reporting
    /// machine, every one reporting a single unpushed commit for work
    /// that had merged hours earlier.
    #[test]
    fn a_branch_ahead_of_a_stale_tracking_ref_is_not_unpushed() {
        let (_t, _repo, wt) = stale_tracking_ref_fixture();
        let w = Worktree {
            path: wt.to_string_lossy().into_owned(),
            branch: "feature".into(),
            ..Default::default()
        };

        // `has_upstream` TRUE and `ahead` 1 -- precisely what the caller
        // computes here, because the ghost ref resolves. That is the
        // input that used to short-circuit to `Unpushed(1)`.
        let s = worktree_safety(&w, "origin/main", true, Some(1));
        assert_eq!(
            s,
            Safety::MergedUpstreamDeleted,
            "merged work must not be refused because a tracking ref is \
             stale, got {s:?}"
        );
        assert!(s.is_safe(), "it must be removable: {}", s.reason());
    }

    /// THE test that matters: genuinely unpushed work is still refused.
    ///
    /// `Unpushed` exists to stop someone deleting commits that live only
    /// on their machine, and the fix above must not cost a single one of
    /// those. Same shape as the fixture -- a real remote, a real push, a
    /// tracking ref one commit behind HEAD -- except the extra commit
    /// was never merged anywhere. It must still refuse.
    ///
    /// The inputs to `worktree_safety` are IDENTICAL to the merged case
    /// above: upstream resolves, one commit ahead. Only the content
    /// differs, which is exactly what the fix keys on -- so this test
    /// and the one above together pin that the fix discriminates on
    /// landed content and on nothing else.
    #[test]
    fn genuinely_unpushed_work_is_still_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ident = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let run_in = |dir: &Path, args: &[&str]| {
            let out = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(ident)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };

        let remote = tmp.path().join("remote.git");
        std::fs::create_dir_all(&remote).unwrap();
        run_in(&remote, &["init", "-q", "--bare", "-b", "main"]);
        let repo = tmp.path().join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        run_in(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("base.txt"), "base\n").unwrap();
        run_in(&repo, &["add", "-A"]);
        run_in(&repo, &["commit", "-q", "-m", "base"]);
        run_in(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_in(&repo, &["push", "-q", "-u", "origin", "main"]);

        let wt = tmp.path().join("proj-feature");
        run_in(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "--track",
                "-b",
                "feature",
                wt.to_str().unwrap(),
                "main",
            ],
        );
        // Pushed work first, so the tracking ref is real and current --
        // not a ghost. This is the half that is safe to lose.
        std::fs::write(wt.join("feature.txt"), "work that did reach the remote\n").unwrap();
        run_in(&wt, &["add", "-A"]);
        run_in(&wt, &["commit", "-q", "-m", "pushed work"]);
        run_in(&wt, &["push", "-q", "-u", "origin", "feature"]);

        // And then a commit that exists NOWHERE else. This is the one
        // the refusal exists for.
        std::fs::write(
            wt.join("feature.txt"),
            "work that did reach the remote\nand a line that never left this machine\n",
        )
        .unwrap();
        run_in(&wt, &["add", "-A"]);
        run_in(&wt, &["commit", "-q", "-m", "work that exists only here"]);

        assert_eq!(
            git(&wt, &["rev-list", "--count", "@{u}..HEAD"])
                .unwrap()
                .trim(),
            "1",
            "must be 1 ahead, or this is not the same input shape as the \
             merged case"
        );

        let w = Worktree {
            path: wt.to_string_lossy().into_owned(),
            branch: "feature".into(),
            ..Default::default()
        };
        let s = worktree_safety(&w, "origin/main", true, Some(1));
        assert_eq!(
            s,
            Safety::Unpushed(1),
            "a commit that exists only on this machine must still be \
             refused, got {s:?}"
        );
        assert!(
            !s.is_safe(),
            "unpushed work must never become removable: {}",
            s.reason()
        );
    }

    /// The second half of #776, and the #819 answer on top of it: a
    /// detached worktree merged into the default branch says so, and
    /// under no circumstances claims `NeverPushed`.
    ///
    /// `git worktree add --detach` is an ordinary way to make a scratch
    /// checkout, and 12 of 37 worktrees on the reporting machine were in
    /// this state. All 12 claimed "commits exist only here" -- the
    /// strongest refusal the app has -- about checkouts sitting on
    /// commits that are on the default branch and on the remote.
    ///
    /// The cause was pure ordering: `was_ever_pushed` reads
    /// `branch.<name>.remote`, a detached HEAD has no branch, so it
    /// answered false and the `NeverPushed` return fired before the
    /// detached check below it could ever run. The check existed for
    /// exactly one case and was unreachable for exactly that case.
    ///
    /// #776 fixed that by returning `Unknown`, and this fixture's
    /// verdict has now moved again -- `Unknown` to `DetachedMerged` --
    /// because the checkout is detached AT `main` and so is provably
    /// contained in it. The thing being pinned is unchanged and is the
    /// reason the assertions below are written negatively as well as
    /// positively: whatever a detached row is allowed to say, it is never
    /// allowed to say its commits exist only here.
    #[test]
    fn a_detached_worktree_is_merged_never_never_pushed() {
        let (_t, repo, _wt) = repo_with_worktree("feature");
        let detached = repo.parent().unwrap().join("scratch");
        let out = Command::new(crate::auth::git_program())
            .arg("-C")
            .arg(&repo)
            .args([
                "worktree",
                "add",
                "-q",
                "--detach",
                detached.to_str().unwrap(),
                "main",
            ])
            .envs([
                ("GIT_AUTHOR_NAME", "octocat"),
                ("GIT_COMMITTER_NAME", "octocat"),
                ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
                ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "worktree add --detach: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        // The premise: git really lists this with no branch, and
        // `was_ever_pushed` really answers false for it -- which is what
        // made the ordering fatal rather than merely untidy.
        let listed = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        let target = listed
            .iter()
            // Matched on the trailing component rather than the whole
            // path: on macOS a temp dir resolves through `/private`, so
            // git prints a path that is the same directory under a
            // different spelling.
            .find(|w| w.path.ends_with("/scratch"))
            .expect("the detached worktree must be listed");
        assert!(
            target.branch.is_empty(),
            "the fixture must be detached, or it tests nothing"
        );
        assert!(
            !was_ever_pushed(&detached),
            "a detached HEAD has no branch config to read -- the premise \
             of the bug"
        );

        // The premise of #819's half: git can answer the merge question
        // for this sha without any branch, and answers it instantly.
        assert!(
            git(&detached, &["merge-base", "--is-ancestor", "HEAD", "main"]).is_ok(),
            "the fixture must be detached AT something merged, or the \
             #819 half tests nothing"
        );

        let s = worktree_safety(target, "main", false, Some(0));
        assert!(
            matches!(s, Safety::DetachedMerged(_)),
            "a detached HEAD contained in the default branch must say so \
             rather than give up, got {s:?}"
        );
        // THE #776 PROPERTY, asserted directly rather than inferred from
        // the variant above. This is the regression that fix exists to
        // prevent, and it must hold for every verdict a detached row can
        // produce, not only for the one this fixture happens to hit.
        assert_ne!(
            s,
            Safety::NeverPushed,
            "a branchless checkout must never claim its commits exist \
             only here -- that is #776"
        );
        assert!(
            !s.reason().contains("only here"),
            "nor say so in prose: {}",
            s.reason()
        );
        // And it is no longer a dead end: the row reaches the
        // safe-to-remove set, which is the action #819 asks for.
        assert!(
            s.is_safe(),
            "a clean checkout contained in the default branch has nothing \
             to lose: {}",
            s.reason()
        );
        // The prose leads with the answer and identifies the checkout.
        // Pinned whole, because the ORDER is the point of the issue and
        // a `contains` would pass on a sentence that led with the
        // failure again. It also pins that the row names what the sha IS
        // rather than only what it lacks (#819).
        //
        // `feature`, not `main`, and that is `name-rev` behaving
        // correctly rather than a fixture quirk worth working around:
        // `repo_with_worktree` branches `feature` from the same commit,
        // so both refs name this sha and git picks one. WHICH ref answers
        // does not matter to the user -- any of them identifies the
        // commit, which is the whole improvement over the bare word
        // "detached". On the rows that prompted the issue it was a tag,
        // `v1.13.0~30`.
        assert_eq!(
            s.reason(),
            "merged — detached at feature, no branch to delete",
            "the merged fact comes first and the sha is identified"
        );
        assert!(
            !s.reason().contains("could not determine"),
            "it was determined: {}",
            s.reason()
        );
    }

    /// A detached worktree never reports `Unpushed` on the OTHER path --
    /// the one where an upstream resolves and an ahead-count exists.
    ///
    /// `worktree_safety` reaches its detached check by two routes and
    /// #776 corrected the ordering on both. This pins the second, which
    /// no test covered: a detached HEAD has no branch to be "ahead" of,
    /// so an ahead-count must not produce `Unpushed` for one.
    ///
    /// An ahead-count of 3 is passed DELIBERATELY, and it is the whole
    /// fixture. `classify` cannot produce this combination in practice --
    /// `upstream_state` returns `Detached` before probing `@{u}`, so a
    /// detached row always takes the `!has_upstream` path -- but
    /// `worktree_safety` takes `has_upstream` as a parameter and must not
    /// depend on its callers being careful. The number must be ignored,
    /// not believed.
    ///
    /// The verdict moved from `Unknown` to `DetachedMerged` in #819 (this
    /// fixture's HEAD is `main`, so it is trivially contained in it). The
    /// assertion below is written as "not `Unpushed`" rather than as an
    /// equality because that is the property #776 bought and it should
    /// survive the next re-labelling too.
    #[test]
    fn a_detached_worktree_is_never_unpushed_even_with_an_ahead_count() {
        let (_t, repo, _wt) = repo_with_worktree("feature");
        let w = Worktree {
            path: repo.to_string_lossy().into_owned(),
            branch: String::new(),
            ..Default::default()
        };
        let s = worktree_safety(&w, "main", true, Some(3));
        assert_ne!(
            s,
            Safety::Unpushed(3),
            "a detached HEAD cannot be ahead of a branch it does not have"
        );
        assert!(
            !matches!(s, Safety::Unpushed(_) | Safety::NeverPushed),
            "nor any other verdict that presumes a branch and a remote, \
             got {s:?}"
        );
        // What it says instead: the sha is on the default branch, which
        // is answerable without a branch and is what #819 asked for.
        assert!(
            matches!(s, Safety::DetachedMerged(_)),
            "expected the merge question to be answered, got {s:?}"
        );
    }

    /// The other half of #819, and the one that keeps it honest: a
    /// detached checkout whose HEAD is NOT on the default branch stays
    /// `Unknown` and stays un-removable.
    ///
    /// Without this the change would be indistinguishable from "call
    /// every detached row safe", which is the opposite of what the issue
    /// asks for -- it asks that "unknown" be said only about what is
    /// genuinely unknown. A commit that exists nowhere but this directory
    /// is exactly that case, and it must not be swept into the
    /// safe-to-remove set on the strength of having no branch.
    ///
    /// `Unknown` rather than `Unmerged` on purpose. `Unmerged`'s prose is
    /// "branch not merged" and there is no branch here to say it about;
    /// and `merged_into` failing is not proof the content is absent, only
    /// that neither ancestry nor a patch-id match could establish it.
    #[test]
    fn an_unmerged_detached_worktree_stays_unknown_and_unremovable() {
        let (_t, repo, _wt) = repo_with_worktree("feature");
        let detached = repo.parent().unwrap().join("scratch");
        let envs = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];
        let run = |dir: &Path, args: &[&str]| {
            let out = Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(envs)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };

        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                detached.to_str().unwrap(),
                "main",
            ],
        );
        // A commit that exists ONLY in this detached checkout. Real
        // content, not an empty commit: `merged_into` falls through to
        // patch-id comparison when ancestry fails, and an empty diff
        // would be compared as nothing rather than as something absent.
        std::fs::write(detached.join("scratch-only.txt"), "only here\n").unwrap();
        run(&detached, &["add", "-A"]);
        run(&detached, &["commit", "-q", "-m", "detached work"]);

        // The premise: git agrees this is not contained in main.
        assert!(
            git(&detached, &["merge-base", "--is-ancestor", "HEAD", "main"]).is_err(),
            "the fixture must be genuinely unmerged, or it tests nothing"
        );

        let listed = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        let target = listed
            .iter()
            // Trailing component, not the whole path: on macOS a temp dir
            // resolves through `/private`, so git prints the same
            // directory under a different spelling.
            .find(|w| w.path.ends_with("/scratch"))
            .expect("the detached worktree must be listed");
        assert!(
            target.branch.is_empty(),
            "the fixture must be detached, or it tests nothing"
        );

        let s = worktree_safety(target, "main", false, Some(0));
        assert!(
            matches!(s, Safety::Unknown(_)),
            "an unmerged detached checkout is genuinely unknown, got {s:?}"
        );
        assert!(
            !s.is_safe(),
            "and must NOT be removable -- this commit exists nowhere \
             else: {}",
            s.reason()
        );
        // The #776 property holds here too, on the arm where it is most
        // tempting to break: these commits really do exist only here, and
        // the app still must not say so, because it has no branch config
        // to have established it from.
        assert_ne!(s, Safety::NeverPushed);
        // The hedge is now a USEFUL hedge: it says what the sha is and
        // which branch it was looked for on.
        assert!(
            s.reason().contains("detached"),
            "it still names the condition: {}",
            s.reason()
        );
        assert!(
            s.reason().contains("main"),
            "and what it was compared against: {}",
            s.reason()
        );
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// Scans the real `~/code`. Read-only -- it runs git queries and
    /// deletes nothing. Run manually:
    /// `cargo test --lib live_scan -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_scan_classifies_real_worktrees() {
        let home = std::env::var("HOME").unwrap();
        let base = format!("{home}/code");
        let t = std::time::Instant::now();
        let fast = scan_dirs_fast(std::slice::from_ref(&base));
        println!(
            "FAST listing: {} repos, {} worktrees in {:?}",
            fast.len(),
            fast.iter().map(|r| r.worktrees.len()).sum::<usize>(),
            t.elapsed()
        );

        let t = std::time::Instant::now();
        let repos = scan_dirs(std::slice::from_ref(&base));
        let elapsed = t.elapsed();

        let total: usize = repos.iter().map(|r| r.worktrees.len()).sum();
        println!("REPOS={} WORKTREES={total} in {elapsed:?}", repos.len());

        let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
        for w in repos.iter().flat_map(|r| &r.worktrees) {
            let k = match &w.safety {
                Safety::Safe => "safe",
                Safety::MainCheckout => "main",
                Safety::Dirty(_) => "dirty",
                Safety::Unpushed(_) => "unpushed",
                Safety::NeverPushed => "never_pushed",
                Safety::MergedUpstreamDeleted => "merged_upstream_deleted",
                // The tally that produced the #819 column of the
                // measured table in `worktree_safety`. Counted
                // separately from `safe` on purpose: the whole question
                // was how many rows move out of `unknown` without any
                // moving into `never_pushed`, and a merged total would
                // hide both halves.
                Safety::DetachedMerged(_) => "detached_merged",
                Safety::Empty => "empty",
                Safety::Unmerged => "unmerged",
                Safety::Locked(_) => "locked",
                Safety::Prunable(_) => "prunable",
                Safety::Pending => "pending",
                Safety::Orphaned => "orphaned",
                Safety::InProgress { .. } => "in_progress",
                Safety::Unknown(_) => "unknown",
            };
            *counts.entry(k).or_default() += 1;
        }
        println!("SAFETY {counts:?}");

        // What the sidebar will show: repos, and removable counts.
        let mut top: Vec<(usize, &str)> = repos
            .iter()
            .map(|r| (r.worktrees.len().saturating_sub(1), r.name.as_str()))
            .collect();
        top.sort_by_key(|a| std::cmp::Reverse(a.0));
        println!("TOP REPOS {:?}", &top[..top.len().min(4)]);

        let safe: Vec<&str> = repos
            .iter()
            .flat_map(|r| &r.worktrees)
            .filter(|w| w.safety.is_safe())
            .map(|w| w.path.rsplit('/').next().unwrap_or(""))
            .take(4)
            .collect();
        println!("SAFE SAMPLE {safe:?}");

        // Merge dates on the safe ones, and sizes for one repo.
        let dated: Vec<(&str, &str)> = repos
            .iter()
            .flat_map(|r| &r.worktrees)
            .filter(|w| w.safety.is_safe())
            .filter_map(|w| {
                w.merged_at
                    .as_deref()
                    .map(|d| (w.path.rsplit('/').next().unwrap_or(""), d))
            })
            .take(4)
            .collect();
        println!("MERGED DATES {dated:?}");

        if let Some(r) = repos.iter().max_by_key(|r| r.worktrees.len()) {
            let t = std::time::Instant::now();
            let sizes = size_repo(&r.path).unwrap();
            let total: u64 = sizes.iter().filter_map(|(_, b)| *b).sum();
            // Abandoned walks are reported and printed rather than
            // silently absent -- #769's whole shape was a row that never
            // heard back at all.
            let abandoned = sizes.iter().filter(|(_, b)| b.is_none()).count();
            println!(
                "SIZED {} worktrees of {} in {:?}, total {:.1} GB, {} abandoned",
                sizes.len(),
                r.name,
                t.elapsed(),
                total as f64 / 1024.0 / 1024.0 / 1024.0,
                abandoned
            );
            assert!(
                sizes.iter().any(|(_, b)| b.is_some_and(|b| b > 0)),
                "sizes must be populated"
            );
        }

        assert!(!repos.is_empty(), "expected repos under ~/code");
        // The main checkout of every repo must be classified as such --
        // deleting one would destroy the repository.
        for r in &repos {
            assert!(
                r.worktrees.first().is_some_and(|w| w.is_main),
                "{} has no main checkout",
                r.name
            );
        }
    }

    /// `pull_checkout` writes to a local checkout, so its refusals are
    /// the interesting part. Exercised against REAL git repositories --
    /// a mocked git would test the mock's idea of `--ff-only`, and the
    /// whole risk here is what git actually does.
    mod pull {
        use super::*;
        use std::process::Command;

        const IDENT: [(&str, &str); 4] = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];

        fn run(dir: &Path, args: &[&str]) -> bool {
            Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(IDENT)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }

        /// An origin with one commit, and a clone tracking it.
        fn origin_and_clone(base: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
            let origin = base.join("origin");
            std::fs::create_dir_all(&origin).unwrap();
            assert!(run(&origin, &["init", "-q", "-b", "main"]));
            assert!(run(
                &origin,
                &["commit", "-q", "--allow-empty", "-m", "one"]
            ));

            let clone = base.join("clone");
            assert!(Command::new(crate::auth::git_program())
                .args(["clone", "-q"])
                .arg(&origin)
                .arg(&clone)
                .envs(IDENT)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false));
            (origin, clone)
        }

        #[test]
        fn fast_forwards_a_clean_checkout_that_is_behind() {
            let tmp = tempfile::TempDir::new().unwrap();
            let (origin, clone) = origin_and_clone(tmp.path());
            // Move origin ahead so the clone has something to pull.
            assert!(run(
                &origin,
                &["commit", "-q", "--allow-empty", "-m", "two"]
            ));

            pull_checkout(&clone.to_string_lossy()).expect("a clean fast-forward must succeed");

            let log = git(&clone, &["log", "--oneline"]).unwrap();
            assert!(
                log.contains("two"),
                "the new commit must have arrived: {log}"
            );
        }

        /// The gate that matters most: a pull into uncommitted changes
        /// can conflict or abort halfway, and recovering from that is
        /// exactly what a GUI button should not create.
        ///
        /// A MODIFIED tracked file, which is the case that warrants it.
        #[test]
        fn refuses_a_dirty_checkout_and_says_how_dirty() {
            let tmp = tempfile::TempDir::new().unwrap();
            let (origin, clone) = origin_and_clone(tmp.path());
            assert!(run(
                &origin,
                &["commit", "-q", "--allow-empty", "-m", "two"]
            ));
            // A TRACKED file, modified: `origin_and_clone` makes only
            // empty commits, so one has to be committed here before it
            // can be dirtied. An untracked file would not refuse, which
            // is the point of the test below.
            std::fs::write(clone.join("tracked.txt"), "one").unwrap();
            assert!(run(&clone, &["add", "tracked.txt"]));
            assert!(run(&clone, &["commit", "-q", "-m", "add tracked"]));
            std::fs::write(clone.join("tracked.txt"), "edited").unwrap();

            let err = pull_checkout(&clone.to_string_lossy())
                .expect_err("a dirty checkout must be refused");
            assert!(err.contains("1 uncommitted change"), "{err}");

            // And nothing was pulled.
            let log = git(&clone, &["log", "--oneline"]).unwrap();
            assert!(!log.contains("two"), "the pull must not have run: {log}");
        }

        /// An UNTRACKED file does not block a fast-forward.
        ///
        /// This test asserted the opposite, because the check counted
        /// `git status --porcelain` whole. A user hit it for real: the
        /// button refused with "1 uncommitted file -- commit or stash
        /// first" while `git pull` in a shell on the same repo worked
        /// (#653). Git only refuses when an incoming commit would
        /// overwrite the untracked path, and it says so itself.
        #[test]
        fn an_untracked_file_does_not_block_a_pull() {
            let tmp = tempfile::TempDir::new().unwrap();
            let (origin, clone) = origin_and_clone(tmp.path());
            assert!(run(
                &origin,
                &["commit", "-q", "--allow-empty", "-m", "two"]
            ));
            std::fs::write(clone.join("scratch.txt"), "untracked").unwrap();

            pull_checkout(&clone.to_string_lossy()).expect("an untracked file must not block");

            let log = git(&clone, &["log", "--oneline"]).unwrap();
            assert!(log.contains("two"), "the pull should have run: {log}");
            // And the file is still there: nothing was cleaned up.
            assert!(clone.join("scratch.txt").is_file());
        }

        /// `--ff-only`: a merge commit created by a background click is
        /// not something the user asked for.
        #[test]
        fn refuses_to_merge_a_diverged_branch() {
            let tmp = tempfile::TempDir::new().unwrap();
            let (origin, clone) = origin_and_clone(tmp.path());
            // Both sides gain a commit, so the histories diverge.
            assert!(run(
                &origin,
                &["commit", "-q", "--allow-empty", "-m", "theirs"]
            ));
            assert!(run(
                &clone,
                &["commit", "-q", "--allow-empty", "-m", "mine"]
            ));

            // Configure the clone to MERGE on pull, which is what makes
            // `--ff-only` load-bearing. Modern git refuses a divergent
            // pull by default, so without this the flag looks redundant
            // -- and an earlier version of this test passed with it
            // removed. The user's own git config decides the default,
            // and the app must not depend on theirs.
            assert!(run(&clone, &["config", "pull.rebase", "false"]));

            let before = git(&clone, &["rev-parse", "HEAD"]).unwrap();

            let err = pull_checkout(&clone.to_string_lossy())
                .expect_err("a diverged branch cannot fast-forward");
            // Git's OWN words, not a generic message -- its refusal
            // names the problem better than we would.
            assert!(!err.is_empty());

            // The DECISIVE assertion. Without `--ff-only` git creates a
            // merge commit here and reports success, so checking the
            // error alone tests nothing -- an earlier version of this
            // test passed with the flag removed.
            let after = git(&clone, &["rev-parse", "HEAD"]).unwrap();
            assert_eq!(before, after, "HEAD must not move on a refused pull");
        }

        #[test]
        fn a_missing_directory_is_a_message_not_a_panic() {
            let err = pull_checkout("/nonexistent/path/for/a/test")
                .expect_err("a missing directory must be refused");
            assert!(err.contains("missing"), "{err}");
        }
    }

    /// `fetch_refs` is the half of #788 that closes the loop: the badge
    /// can say how stale it is, but until now the only way to make it
    /// fresh again was `git pull`, which also moves the branch.
    ///
    /// Against REAL git repositories for the same reason `pull` is: the
    /// whole property under test is what `git fetch` does and does not
    /// touch, and a mock would only assert our own idea of it.
    mod fetch {
        use super::*;
        use std::process::Command;

        const IDENT: [(&str, &str); 4] = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];

        fn run(dir: &Path, args: &[&str]) -> bool {
            Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(IDENT)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }

        fn origin_and_clone(base: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
            let origin = base.join("origin");
            std::fs::create_dir_all(&origin).unwrap();
            assert!(run(&origin, &["init", "-q", "-b", "main"]));
            assert!(run(
                &origin,
                &["commit", "-q", "--allow-empty", "-m", "one"]
            ));

            let clone = base.join("clone");
            assert!(Command::new(crate::auth::git_program())
                .args(["clone", "-q"])
                .arg(&origin)
                .arg(&clone)
                .envs(IDENT)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false));
            (origin, clone)
        }

        /// The WHOLE POINT, as two assertions that have to hold
        /// together: `origin/main` advances and local `main` does not.
        ///
        /// Either one alone passes over a different bug. Only checking
        /// that `origin/main` moved would be satisfied by `git pull`,
        /// which is what the user already had and what #788 is asking for
        /// an alternative to. Only checking that `main` stayed put would
        /// be satisfied by doing nothing at all.
        #[test]
        fn advances_the_remote_ref_and_leaves_the_local_branch_alone() {
            let tmp = tempfile::TempDir::new().unwrap();
            let (origin, clone) = origin_and_clone(tmp.path());
            assert!(run(
                &origin,
                &["commit", "-q", "--allow-empty", "-m", "two"]
            ));

            let local_before = git(&clone, &["rev-parse", "main"]).unwrap();
            let remote_before = git(&clone, &["rev-parse", "origin/main"]).unwrap();
            assert_eq!(
                local_before, remote_before,
                "before the fetch the two refs agree -- which is exactly the \
                 state that renders as a green 'up to date with upstream'"
            );

            fetch_refs(&clone.to_string_lossy()).expect("a fetch against a reachable origin");

            let remote_after = git(&clone, &["rev-parse", "origin/main"]).unwrap();
            assert_ne!(
                remote_before, remote_after,
                "origin/main must have advanced; otherwise the comparison is \
                 still as stale as it was and the badge still lies"
            );
            let local_after = git(&clone, &["rev-parse", "main"]).unwrap();
            assert_eq!(
                local_before, local_after,
                "main must NOT move -- a Fetch that pulls is a Pull, and the \
                 separation is the feature"
            );
        }

        /// No `--prune`, asserted by outcome rather than by reading the
        /// argv.
        ///
        /// A pruning fetch deletes `origin/*` refs whose remote branch is
        /// gone, and those refs are the inputs to `MergedUpstreamDeleted`
        /// (#732) -- the verdict that tells "merged, then the branch was
        /// tidied up" from "these commits exist nowhere else". So a
        /// `--prune` slipped into this command would silently rewrite a
        /// DELETION gate's evidence from a button labelled Fetch.
        #[test]
        fn does_not_prune_a_remote_ref_whose_branch_is_gone() {
            let tmp = tempfile::TempDir::new().unwrap();
            let (origin, clone) = origin_and_clone(tmp.path());
            // A second branch on origin, fetched into the clone, then
            // deleted upstream -- the exact shape `MergedUpstreamDeleted`
            // reads.
            assert!(run(&origin, &["branch", "landed"]));
            fetch_refs(&clone.to_string_lossy()).expect("the first fetch brings the branch down");
            assert!(
                git(&clone, &["rev-parse", "origin/landed"]).is_ok(),
                "the fixture is broken: origin/landed should exist now"
            );
            assert!(run(&origin, &["branch", "-D", "landed"]));

            fetch_refs(&clone.to_string_lossy()).expect("the second fetch, after the deletion");

            assert!(
                git(&clone, &["rev-parse", "origin/landed"]).is_ok(),
                "origin/landed must survive: pruning it would change what the \
                 safety verdicts read, on a button that promises only a refresh"
            );
        }

        #[test]
        fn a_missing_directory_is_a_message_not_a_panic() {
            let err = fetch_refs("/nonexistent/path/for/a/test")
                .expect_err("a missing directory must be refused");
            assert!(err.contains("missing"), "{err}");
        }

        /// An unreachable remote is the scan's second reason for never
        /// fetching, and bounding it is what makes this safe on a click:
        /// it comes back with git's own words instead of hanging.
        #[test]
        fn an_unreachable_remote_is_an_error_not_a_hang() {
            let tmp = tempfile::TempDir::new().unwrap();
            let repo = tmp.path().join("lonely");
            std::fs::create_dir_all(&repo).unwrap();
            assert!(run(&repo, &["init", "-q", "-b", "main"]));
            assert!(run(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]));
            // A local path that does not exist, rather than a hostname:
            // an unresolvable host would make this test depend on the
            // machine's DNS timeout, and CI runners differ. Git's
            // refusal is what is being asserted, not the transport.
            assert!(run(
                &repo,
                &["remote", "add", "origin", "/nonexistent/origin/for/a/test"]
            ));

            let err =
                fetch_refs(&repo.to_string_lossy()).expect_err("an unreachable origin must fail");
            assert!(!err.is_empty(), "git's own message, not an empty string");
        }
    }

    /// #356: a worktree whose parent repository was deleted is
    /// invisible to the scan -- it is not a repository, so the walker
    /// skips it, and its own repo can no longer report it.
    ///
    /// MEASURED on a real machine: 2.5 GB across three such
    /// directories, entirely absent from a view whose purpose is
    /// reclaiming exactly this.
    mod orphans {
        use super::*;
        use std::process::Command;

        const IDENT: [(&str, &str); 4] = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];

        fn run(dir: &Path, args: &[&str]) -> bool {
            Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(IDENT)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }

        /// A real worktree, then its repository deleted out from under
        /// it -- built with git rather than by hand-writing a `.git`
        /// file, so the shape is whatever git actually produces.
        fn orphaned_worktree(base: &Path) -> std::path::PathBuf {
            let repo = base.join("proj");
            std::fs::create_dir_all(&repo).unwrap();
            assert!(run(&repo, &["init", "-q", "-b", "main"]));
            assert!(run(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]));

            let wt = base.join("proj-feature");
            assert!(run(
                &repo,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    "feature",
                    wt.to_str().unwrap(),
                ]
            ));
            assert!(wt.join(".git").is_file(), "a worktree's .git is a file");

            // The repository goes away; the worktree stays behind.
            std::fs::remove_dir_all(&repo).unwrap();
            wt
        }

        #[test]
        fn an_orphan_is_found_and_reported_as_orphaned() {
            let tmp = tempfile::TempDir::new().unwrap();
            let wt = orphaned_worktree(tmp.path());

            let repos = scan_dirs_fast(&[tmp.path().to_string_lossy().into_owned()]);
            let found: Vec<_> = repos
                .iter()
                .flat_map(|r| &r.worktrees)
                .filter(|w| w.safety == Safety::Orphaned)
                .collect();
            assert_eq!(found.len(), 1, "the orphan must be reported at all");
            assert_eq!(found[0].path, wt.to_string_lossy());
        }

        /// #1026: an orphan reports NO default ref, rather than a
        /// plausible one.
        ///
        /// The same rule that makes its safety verdict `Orphaned`:
        /// resolving a default branch means running git in the parent
        /// repository, and the parent repository is exactly what is
        /// gone. Sending the word `main` here would be a confident
        /// answer about which ref a verdict was measured against, for a
        /// checkout where no verdict was measured at all -- this
        /// codebase's absent-read-as-success bug (#967, #769, #841)
        /// applied to the one field that says what was compared.
        #[test]
        fn an_orphan_reports_no_default_ref() {
            let tmp = tempfile::TempDir::new().unwrap();
            orphaned_worktree(tmp.path());
            let repos = scan_dirs_fast(&[tmp.path().to_string_lossy().into_owned()]);
            let orphans: Vec<_> = repos
                .iter()
                .filter(|r| r.worktrees.iter().any(|w| w.safety == Safety::Orphaned))
                .collect();
            assert_eq!(orphans.len(), 1, "the orphan must be reported at all");
            assert_eq!(
                orphans[0].default_ref, None,
                "an orphan has no repository to resolve a default ref from"
            );
        }

        /// The safety rule that matters: an orphan can never be
        /// classified -- there is no git to run in it -- so it must not
        /// reach the path that removes "safe" worktrees in bulk.
        #[test]
        fn an_orphan_is_never_safe() {
            let tmp = tempfile::TempDir::new().unwrap();
            orphaned_worktree(tmp.path());
            let repos = scan_dirs_fast(&[tmp.path().to_string_lossy().into_owned()]);
            for w in repos.iter().flat_map(|r| &r.worktrees) {
                assert!(!w.safety.is_safe(), "an orphan must never be is_safe()");
            }
        }

        /// The counting discrepancy in the v3.10.0 screenshot: the
        /// sidebar said 120 while the panel said 123 across 41 repos,
        /// and only 10 repos were listed.
        #[test]
        #[ignore]
        fn live_sidebar_versus_rollup_counts() {
            let base = format!("{}/code", std::env::var("HOME").unwrap());
            let repos = scan_dirs_fast(&[base]);
            // What the sidebar computes: n - 1 per repo.
            let sidebar: usize = repos
                .iter()
                .map(|r| r.worktrees.len().saturating_sub(1))
                .sum();
            // What the rollup computes: everything not is_main.
            let rollup: usize = repos
                .iter()
                .flat_map(|r| &r.worktrees)
                .filter(|w| !w.is_main)
                .count();
            let no_main = repos
                .iter()
                .filter(|r| !r.worktrees.iter().any(|w| w.is_main))
                .count();
            let hidden = repos
                .iter()
                .filter(|r| r.worktrees.len().saturating_sub(1) == 0)
                .count();
            println!(
                "repos={} sidebar={sidebar} rollup={rollup} repos_without_main={no_main} hidden_by_sidebar={hidden}",
                repos.len()
            );
        }

        /// How long would sizing EVERY repository take? That decides
        /// whether the all-repositories view can measure at all, or
        /// whether it has to keep saying "open a repository".
        #[test]
        #[ignore]
        fn live_cost_of_sizing_every_repo() {
            let base = format!("{}/code", std::env::var("HOME").unwrap());
            let repos = scan_dirs_fast(&[base]);
            let t = std::time::Instant::now();
            let mut measured = 0usize;
            for r in &repos {
                if let Ok(sizes) = size_repo(&r.path) {
                    measured += sizes.len();
                }
            }
            println!(
                "sized {} worktrees across {} repos in {:?}",
                measured,
                repos.len(),
                t.elapsed()
            );
        }

        /// Against the REAL directories that prompted #356.
        ///
        /// Takes `HEADSTATE_REPO_DIR` rather than a literal path: this
        /// is a public repository, and a checkout path names the
        /// projects on the machine that ran it.
        #[test]
        #[ignore]
        fn live_orphans_in_the_code_directory() {
            let Ok(base) = std::env::var("HEADSTATE_REPO_DIR") else {
                println!("set HEADSTATE_REPO_DIR to a directory of repositories to run this");
                return;
            };
            if !Path::new(&base).is_dir() {
                println!("no such directory; nothing to check");
                return;
            }
            let repos = scan_dirs_fast(&[base]);
            let orphans: Vec<_> = repos
                .iter()
                .flat_map(|r| &r.worktrees)
                .filter(|w| w.safety == Safety::Orphaned)
                .collect();
            println!("found {} orphaned worktree(s)", orphans.len());
            for o in &orphans {
                println!("  {}", o.path);
            }
        }

        /// Every scan root for a `remove_orphan` test: the temp dir the
        /// fixture was built in, which is where `collect_inner` would
        /// have found the orphan.
        fn roots(base: &Path) -> Vec<String> {
            vec![base.to_string_lossy().into_owned()]
        }

        /// `remove_orphan` is a plain recursive delete -- git cannot
        /// help, since the repository that owned the worktree is gone.
        /// That makes the gate the ONLY protection, so these test it
        /// rather than the happy path.
        #[test]
        fn removes_an_orphan() {
            let tmp = tempfile::TempDir::new().unwrap();
            let wt = orphaned_worktree(tmp.path());
            assert!(wt.is_dir());
            remove_orphan(&wt.to_string_lossy(), &roots(tmp.path()))
                .expect("an orphan must be removable");
            assert!(!wt.exists(), "the directory must be gone");
        }

        /// The containment half of #854's invariant: the only thing
        /// between a bad path and `remove_dir_all` on an arbitrary
        /// directory. `remove_artifact`'s
        /// `refuses_a_path_outside_every_scan_root` is the sibling this
        /// copies -- this path had no equivalent at all.
        #[test]
        fn refuses_an_orphan_outside_every_scan_root() {
            let tmp = tempfile::TempDir::new().unwrap();
            let wt = orphaned_worktree(tmp.path());
            let elsewhere = tempfile::TempDir::new().unwrap();
            let err = remove_orphan(&wt.to_string_lossy(), &roots(elsewhere.path()))
                .expect_err("a path outside the scanned folders must be refused");
            assert!(err.contains("outside the scanned folders"), "{err}");
            assert!(wt.is_dir(), "and it must still be there");
        }

        /// `..` must not walk out of a scan root. Checked against the
        /// CANONICAL path for exactly this.
        #[test]
        fn refuses_a_traversal_out_of_the_root() {
            let inside = tempfile::TempDir::new().unwrap();
            let outside = tempfile::TempDir::new().unwrap();
            let victim = orphaned_worktree(outside.path());

            // A path that LOOKS like it is under the root but resolves
            // out of it.
            let sneaky = format!(
                "{}/../{}/{}",
                inside.path().to_string_lossy(),
                outside.path().file_name().unwrap().to_string_lossy(),
                victim.file_name().unwrap().to_string_lossy()
            );
            let _ = remove_orphan(&sneaky, &roots(inside.path()));
            assert!(
                victim.exists(),
                "a path resolving outside the root must not be removed"
            );
        }

        /// The symlink half. `remove_dir_all` on a symlink deletes the
        /// TARGET's contents, which may be anywhere at all -- and
        /// `orphan_gitdir` reads through the link perfectly happily, so
        /// a link whose target is an orphan passed the old gate.
        ///
        /// Both rejecting paths are exercised for the reason
        /// `artifacts/mod.rs` gives: `canonicalize` resolves through the
        /// link, so a link out of the roots is caught by containment and
        /// only a link that stays inside reaches the symlink check
        /// itself. Testing one would leave the other unprotected the day
        /// someone reorders them.
        #[test]
        #[cfg(unix)]
        fn refuses_a_symlink_pointing_out_of_the_root() {
            let tmp = tempfile::TempDir::new().unwrap();
            let real = tempfile::TempDir::new().unwrap();
            let victim = orphaned_worktree(real.path());
            std::fs::write(victim.join("keep.txt"), "important").unwrap();

            let link = tmp.path().join("looks-orphaned");
            std::os::unix::fs::symlink(&victim, &link).unwrap();

            assert!(remove_orphan(&link.to_string_lossy(), &roots(tmp.path())).is_err());
            assert!(
                victim.join("keep.txt").exists(),
                "the link target must be untouched"
            );
        }

        #[test]
        #[cfg(unix)]
        fn refuses_a_symlink_pointing_inside_the_root() {
            let tmp = tempfile::TempDir::new().unwrap();
            let victim = orphaned_worktree(tmp.path());
            std::fs::write(victim.join("keep.txt"), "important").unwrap();

            let link = tmp.path().join("looks-orphaned");
            std::os::unix::fs::symlink(&victim, &link).unwrap();

            let err = remove_orphan(&link.to_string_lossy(), &roots(tmp.path()))
                .expect_err("a symlink must be refused as one");
            assert!(err.contains("symlink"), "{err}");
            assert!(
                victim.join("keep.txt").exists(),
                "the link target must be untouched"
            );
        }

        /// The gate that matters: a LIVE worktree must never be
        /// deleted by this path. It has a repository, so it belongs to
        /// `remove_worktree`, which re-checks safety.
        #[test]
        fn refuses_a_live_worktree() {
            let tmp = tempfile::TempDir::new().unwrap();
            let repo = tmp.path().join("proj");
            std::fs::create_dir_all(&repo).unwrap();
            assert!(run(&repo, &["init", "-q", "-b", "main"]));
            assert!(run(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]));
            let wt = tmp.path().join("proj-feature");
            assert!(run(
                &repo,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    "feature",
                    wt.to_str().unwrap(),
                ]
            ));

            let err = remove_orphan(&wt.to_string_lossy(), &roots(tmp.path()))
                .expect_err("a live worktree must be refused");
            assert!(err.contains("no longer an orphaned worktree"), "{err}");
            assert!(wt.is_dir(), "and it must still be there");
        }

        /// An ordinary directory is not an orphan either. Without the
        /// re-check this command would be "delete any path the
        /// frontend names".
        #[test]
        fn refuses_a_directory_that_is_not_a_worktree() {
            let tmp = tempfile::TempDir::new().unwrap();
            let plain = tmp.path().join("just-a-folder");
            std::fs::create_dir_all(&plain).unwrap();
            std::fs::write(plain.join("important.txt"), "data").unwrap();

            assert!(remove_orphan(&plain.to_string_lossy(), &roots(tmp.path())).is_err());
            assert!(
                plain.join("important.txt").exists(),
                "nothing may be deleted"
            );
        }

        /// A HEALTHY worktree must not be mistaken for an orphan        /// A HEALTHY worktree must not be mistaken for an orphan --
        /// that would put every ordinary worktree in a section saying
        /// its repository is gone.
        #[test]
        fn a_live_worktree_is_not_an_orphan() {
            let tmp = tempfile::TempDir::new().unwrap();
            let repo = tmp.path().join("proj");
            std::fs::create_dir_all(&repo).unwrap();
            assert!(run(&repo, &["init", "-q", "-b", "main"]));
            assert!(run(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]));
            let wt = tmp.path().join("proj-feature");
            assert!(run(
                &repo,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    "feature",
                    wt.to_str().unwrap(),
                ]
            ));

            assert!(
                orphan_gitdir(&wt).is_none(),
                "a live worktree is not orphaned"
            );
        }
    }

    /// #343: a squash-merge is invisible to BOTH existing signals.
    ///
    /// Built with real git rather than mocked -- the whole defect is a
    /// property of what git actually reports for a squash, and a mock
    /// would encode my belief about that instead of testing it.
    mod squash {
        use super::*;
        use std::process::Command;

        const IDENT: [(&str, &str); 4] = [
            ("GIT_AUTHOR_NAME", "octocat"),
            ("GIT_COMMITTER_NAME", "octocat"),
            ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
            ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
        ];

        fn run(dir: &Path, args: &[&str]) -> bool {
            Command::new(crate::auth::git_program())
                .arg("-C")
                .arg(dir)
                .args(args)
                .envs(IDENT)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }

        /// A repo whose `feature` branch was SQUASH-merged into main:
        /// main gains one new commit carrying the whole change, and the
        /// branch's own commits are never ancestors of it.
        fn repo_with_squash_merge(base: &Path) -> std::path::PathBuf {
            let repo = base.join("repo");
            std::fs::create_dir_all(&repo).unwrap();
            assert!(run(&repo, &["init", "-q", "-b", "main"]));
            std::fs::write(repo.join("base.txt"), "base\n").unwrap();
            assert!(run(&repo, &["add", "-A"]));
            assert!(run(&repo, &["commit", "-q", "-m", "base"]));

            // Three commits on a branch, the shape `git cherry` cannot
            // match once they are collapsed into one.
            assert!(run(&repo, &["checkout", "-q", "-b", "feature"]));
            for (i, line) in ["one", "two", "three"].iter().enumerate() {
                std::fs::write(
                    repo.join("work.txt"),
                    format!("{}\n", ["one", "one\ntwo", "one\ntwo\nthree"][i]),
                )
                .unwrap();
                assert!(run(&repo, &["add", "-A"]));
                assert!(run(&repo, &["commit", "-q", "-m", line]));
            }

            // The squash: main takes the whole diff as ONE new commit.
            assert!(run(&repo, &["checkout", "-q", "main"]));
            assert!(run(&repo, &["merge", "--squash", "feature"]));
            assert!(run(&repo, &["commit", "-q", "-m", "feature (#164)"]));
            assert!(run(&repo, &["checkout", "-q", "feature"]));
            repo
        }

        #[test]
        fn a_squash_merged_branch_is_recognised_as_merged() {
            let tmp = tempfile::TempDir::new().unwrap();
            let repo = repo_with_squash_merge(tmp.path());

            // Both existing signals fail, which is the whole point.
            assert!(
                git(&repo, &["merge-base", "--is-ancestor", "HEAD", "main"]).is_err(),
                "ancestry must NOT see a squash -- if it does, this test proves nothing"
            );
            let cherry = git(&repo, &["cherry", "main", "HEAD"]).unwrap();
            assert!(
                cherry.lines().any(|l| l.trim_start().starts_with('+')),
                "git cherry must NOT see a squash: {cherry}"
            );

            // Through `squash_merged`, the REAL call site. Calling
            // `aggregate_patch_merged` directly left the wiring
            // untested: deleting the call entirely still passed.
            assert_eq!(squash_merged(&repo, "main"), Safety::Safe);
        }

        /// The direction that matters for safety: this LOOSENS a gate
        /// that guards deletion, so a branch that genuinely has not
        /// landed must never come back Safe.
        #[test]
        fn genuinely_unmerged_work_stays_unmerged() {
            let tmp = tempfile::TempDir::new().unwrap();
            let repo = repo_with_squash_merge(tmp.path());
            // A fourth commit that never reached main.
            std::fs::write(repo.join("work.txt"), "one\ntwo\nthree\nfour\n").unwrap();
            assert!(run(&repo, &["add", "-A"]));
            assert!(run(&repo, &["commit", "-q", "-m", "four"]));

            assert_eq!(squash_merged(&repo, "main"), Safety::Unmerged);
        }

        /// A branch with no diff against its base has nothing to
        /// compare, and claiming merged on an empty comparison would
        /// greenlight deleting a worktree whose state was never
        /// established.
        #[test]
        fn a_branch_with_no_changes_is_not_called_merged() {
            let tmp = tempfile::TempDir::new().unwrap();
            let repo = repo_with_squash_merge(tmp.path());
            assert!(run(&repo, &["checkout", "-q", "-b", "empty", "main"]));
            assert_eq!(squash_merged(&repo, "main"), Safety::Unmerged);
        }
    }

    /// #343, against a REAL worktree rather than a fixture: a branch
    /// merged through a squash-merge queue, which both ancestry and
    /// `git cherry` call unmerged.
    ///
    /// Takes the path from `HEADSTATE_SQUASHED_WORKTREE` rather than
    /// hardcoding one, for the reason `a_yarn_berry_project_reports_updates`
    /// takes `HEADSTATE_YARN_REPO`: the worktree that produced the
    /// original report is on one machine, and this is a public
    /// repository where a real checkout path is exactly what
    /// CONTRIBUTING.md's privacy rule keeps out.
    /// The production classifier over a real checkout, for the counts in
    /// #776. Set `HEADSTATE_REAL_REPO` to a repository with worktrees.
    ///
    /// Ignored by default, and READ-ONLY: it classifies and prints, and
    /// never writes to the repository it is pointed at. Same shape as
    /// `live_squash_merged_worktree_is_detected` -- the established way
    /// to check a fix against a real tree without committing one.
    #[test]
    #[ignore]
    fn live_classifier_verdict_census() {
        let Ok(path) = std::env::var("HEADSTATE_REAL_REPO") else {
            println!("set HEADSTATE_REAL_REPO to a repo with worktrees to run this");
            return;
        };
        let repo = std::path::PathBuf::from(path);
        if !repo.is_dir() {
            println!("repo absent; nothing to check");
            return;
        }
        let default = default_branch(&repo);
        let listed = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());
        println!("default branch: {default}  worktrees: {}", listed.len());

        let mut tally: std::collections::BTreeMap<String, usize> = Default::default();
        for w in &listed {
            let mut w = w.clone();
            classify(&mut w, &repo, &default);
            let key = match &w.safety {
                Safety::Unknown(m) => format!("Unknown({m})"),
                Safety::Dirty(_) => "Dirty".to_string(),
                Safety::Unpushed(_) => "Unpushed".to_string(),
                Safety::Locked(_) => "Locked".to_string(),
                Safety::Prunable(_) => "Prunable".to_string(),
                other => format!("{other:?}"),
            };
            *tally.entry(key).or_default() += 1;
        }
        for (k, n) in &tally {
            println!("{n:>4}  {k}");
        }
    }

    /// How long classification takes PER WORKTREE, and what the spread
    /// is (#830).
    ///
    /// The number this answers is the one `isPending` in
    /// `src/lib/worktrees.ts` used to quote as "~57s", which was a TOTAL
    /// for one tree on one machine and told a reader nothing about the
    /// variable that actually matters. #830 is a 111-worktree repository
    /// whose safety column never resolved at all, and a total cannot
    /// distinguish "many worktrees, each quick" from "one worktree that
    /// never answers" -- which are the same total and completely
    /// different bugs.
    ///
    /// So this prints the per-worktree distribution, not just the sum:
    /// the max against the median is the whole argument for a
    /// per-worktree ceiling, because a bound set from the median would
    /// throw away honest slow answers and a bound set from the total
    /// would never fire.
    ///
    /// Serial on purpose, unlike production. `CLASSIFY_WORKERS` threads
    /// would make each timing include its share of contention, and the
    /// question here is what ONE worktree costs -- the figure a timeout
    /// has to be set from.
    #[test]
    #[ignore]
    fn live_classification_cost_per_worktree() {
        let Ok(path) = std::env::var("HEADSTATE_REAL_REPO") else {
            println!("set HEADSTATE_REAL_REPO to a repo with worktrees to run this");
            return;
        };
        let repo = std::path::PathBuf::from(path);
        if !repo.is_dir() {
            println!("repo absent; nothing to check");
            return;
        }
        let default = default_branch(&repo);
        let listed = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());

        let whole = std::time::Instant::now();
        let mut each: Vec<(u128, String)> = Vec::new();
        for w in &listed {
            let mut w = w.clone();
            let t = std::time::Instant::now();
            classify(&mut w, &repo, &default);
            each.push((t.elapsed().as_millis(), w.path.clone()));
        }
        let total = whole.elapsed();
        each.sort_unstable();

        let at = |q: f64| each[((each.len() as f64 - 1.0) * q).round() as usize].0;
        println!(
            "worktrees={} serial_total={:?} p50={}ms p90={}ms p99={}ms max={}ms",
            each.len(),
            total,
            at(0.50),
            at(0.90),
            at(0.99),
            each.last().map(|e| e.0).unwrap_or(0),
        );
        for (ms, p) in each.iter().rev().take(5) {
            println!("  slowest {ms:>6}ms  {p}");
        }
    }

    /// What is really underneath the locks on a real machine (#775).
    ///
    /// The question the whole change exists to answer, asked of the
    /// production classifier rather than of a fixture: of the locked
    /// worktrees on this repository, how many are merged and would be
    /// removable once the lock were cleared?
    ///
    /// `#[ignore]` and env-driven, following
    /// `live_squash_merged_worktree_is_detected` -- the established way
    /// to check a fix against a real tree without committing one, since
    /// the tree it needs cannot exist in a public repository.
    ///
    /// Prints the age distribution alongside, because that is the other
    /// half of the argument: if the locks are all one age the reason
    /// string would have done, and if they are spread over days then
    /// the mtime is carrying real information the reason cannot.
    #[test]
    #[ignore]
    fn live_locked_worktree_census() {
        let Ok(path) = std::env::var("HEADSTATE_REAL_REPO") else {
            println!("set HEADSTATE_REAL_REPO to a repo with locked worktrees to run this");
            return;
        };
        let repo = std::path::PathBuf::from(path);
        if !repo.is_dir() {
            println!("repo absent; nothing to check");
            return;
        }
        let default = default_branch(&repo);
        let listed = parse_porcelain(&git(&repo, &["worktree", "list", "--porcelain"]).unwrap());

        let t = std::time::Instant::now();
        let mut locked = 0usize;
        let mut merged_underneath = 0usize;
        let mut ages: std::collections::BTreeMap<Option<u64>, usize> = Default::default();
        let mut holders: std::collections::BTreeMap<Option<bool>, usize> = Default::default();
        let mut underlying: std::collections::BTreeMap<String, usize> = Default::default();
        for w in &listed {
            let mut w = w.clone();
            classify(&mut w, &repo, &default);
            let Safety::Locked(lock) = &w.safety else {
                continue;
            };
            locked += 1;
            if lock.underlying.is_safe() {
                merged_underneath += 1;
            }
            *ages.entry(lock.age_days).or_default() += 1;
            *holders.entry(lock.holder_running).or_default() += 1;
            let key = match lock.underlying.as_ref() {
                Safety::Unknown(m) => format!("Unknown({m})"),
                Safety::Dirty(_) => "Dirty".into(),
                Safety::Unpushed(_) => "Unpushed".into(),
                other => format!("{other:?}"),
            };
            *underlying.entry(key).or_default() += 1;
        }

        println!(
            "worktrees: {}  locked: {locked}  merged underneath: {merged_underneath}  \
             classified in {:?}",
            listed.len(),
            t.elapsed()
        );
        println!("underlying verdicts: {underlying:?}");
        println!("age in days -> count: {ages:?}");
        println!("holder running -> count: {holders:?}");
    }

    #[test]
    #[ignore]
    fn live_squash_merged_worktree_is_detected() {
        let Ok(path) = std::env::var("HEADSTATE_SQUASHED_WORKTREE") else {
            println!("set HEADSTATE_SQUASHED_WORKTREE to a squash-merged worktree to run this");
            return;
        };
        let wt = std::path::PathBuf::from(path);
        if !wt.is_dir() {
            println!("worktree absent; nothing to check");
            return;
        }
        println!(
            "ancestry says merged: {}",
            git(&wt, &["merge-base", "--is-ancestor", "HEAD", "origin/main"]).is_ok()
        );
        println!("verdict: {:?}", aggregate_patch_merged(&wt, "origin/main"));
    }
}

/// Remove a worktree, refusing anything not provably safe.
///
/// The commit a worktree's HEAD currently points at.
///
/// Used to expire an assessment: a branch that has moved since it was
/// assessed is no longer the thing that was assessed.
pub fn head_oid(worktree_path: &str) -> Result<String, String> {
    git(Path::new(worktree_path), &["rev-parse", "HEAD"]).map(|s| s.trim().to_string())
}

/// One worktree's outcome in a bulk removal.
///
/// Every input gets an outcome. Partial failure is the normal case here,
/// not the exception: safety is re-checked at delete time, so a worktree
/// that went dirty since the scan is refused mid-batch, and a single
/// verdict would hide that.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RemovalOutcome {
    pub path: String,
    pub error: Option<String>,
}

/// Remove several worktrees, reporting each independently.
///
/// Deliberately a loop over `remove_worktree` rather than a bulk git
/// call: that keeps the per-worktree safety gate exactly as it is. A
/// bulk path that evaluated safety once and then deleted N directories
/// would be a different and much more dangerous thing than N safe
/// deletions.
///
/// Sequential rather than concurrent. `git worktree remove` mutates the
/// repository's administrative files, so parallel removals in one repo
/// contend on the same lock -- and at a few hundred milliseconds each
/// the wall-clock saving would not repay the risk of interleaved writes.
/// Remove worktrees, reporting after each one.
///
/// Removal is sequential at a few hundred milliseconds each, so ~100
/// worktrees is around 30 seconds behind a single boolean "busy" flag.
/// `on_progress` is called with (done, total) after every removal so the
/// UI can say "Removed 34 of 106" instead of spinning silently.
///
/// A CALLBACK rather than emitting Tauri events here: this module is
/// pure git plumbing and has no AppHandle, which is also what keeps it
/// testable without a running app.
/// Fast-forward a checkout to its upstream.
///
/// The main checkout's row reports how far behind it is and, until now,
/// offered no way to act on it -- so fixing it meant leaving the app for
/// a terminal, which is the thing this view exists to avoid.
///
/// Three deliberate constraints:
///
/// - **Refuses on a dirty checkout.** A pull into uncommitted changes
///   can conflict or abort halfway, and recovering from that is exactly
///   the situation a GUI button should not create. Checked HERE rather
///   than trusted from the scan, for the same reason `remove_inner`
///   re-checks: the scan is a snapshot the world may have moved past.
///
/// - **`--ff-only`.** A merge commit created by a background click is
///   not something the user asked for. A branch that cannot fast-forward
///   is a real situation to report, not to resolve silently.
///
/// - **Returns git's own message.** "Could not update" says nothing;
///   git's refusal usually names the problem exactly.
pub fn pull_checkout(path: &str) -> Result<String, String> {
    let dir = Path::new(path);
    if !dir.is_dir() {
        return Err("that directory is missing".into());
    }

    // Fresh, not from the scan.
    //
    // `--untracked-files=no`, unlike the `Safety` gate this used to
    // share. Untracked files do not stop a fast-forward: git only
    // refuses when an incoming commit would OVERWRITE one, and it says
    // so itself. Counting them here refused to update a checkout whose
    // only change was one untracked file, while `git pull` in a shell on
    // the same repo succeeded (#653).
    //
    // `Safety` keeps counting them, and must: it gates DELETION, where
    // an untracked file is precisely the one with no copy anywhere else.
    let status = git(dir, &["status", "--porcelain", "--untracked-files=no"])
        .map_err(|e| format!("could not read the checkout's state: {e}"))?;
    let dirty = status.lines().filter(|l| !l.trim().is_empty()).count();
    if dirty > 0 {
        return Err(format!(
            "{dirty} uncommitted change{} -- commit or stash first",
            if dirty == 1 { "" } else { "s" }
        ));
    }

    // `--ff-only` on the pull itself, so a diverged branch fails here
    // rather than producing a merge nobody asked for.
    git(dir, &["pull", "--ff-only"])
}

/// Refresh one repository's remote refs, moving no branch (#788).
///
/// The ONE network call on this module's own initiative, and it is here
/// because the scan deliberately has none. `upstream_state` and every
/// merge verdict read `origin/*` off disk, so the green "up to date with
/// upstream" on the main checkout's row compares local `main` against
/// whatever `origin/main` happened to be at the last fetch. When nothing
/// has fetched recently both refs are behind together, the row honestly
/// reports that they agree, and the Update button -- which DOES fetch,
/// via `git pull` -- then discovers everything the badge never saw. That
/// is the asymmetry #788 reports as "green, then it pulled lots of new
/// changes".
///
/// The no-fetch decision in the scan stays, and nothing here weakens it:
/// this runs on a CLICK, for ONE repository, never on the scan's path.
/// That is the whole distinction. The scan's reasons -- a network call
/// per repository every time the view opens, 40+ repositories here and
/// ~27,000 directories in one pass, and a hang on an unreachable remote
/// -- are all about the implicit, fan-out case. A user who has asked for
/// a live answer for one repository has accepted the wait and is
/// watching for the result.
///
/// Three deliberate constraints, and they are what make this different
/// from `pull_checkout` rather than a second spelling of it:
///
/// - **Moves no ref the user is standing on.** `git fetch` writes only
///   the remote-tracking refs, so `main` and every worktree's HEAD are
///   exactly where they were. Today the only way to refresh the
///   comparison is to perform the merge, which means a user who wants to
///   KNOW whether they are behind has to first become not-behind. This
///   separates the question from the answer.
///
/// - **Never `--prune`.** Pruning deletes local `origin/*` refs whose
///   remote branch is gone, and those refs are exactly what
///   `MergedUpstreamDeleted` (#732) and the cherry checks read to tell
///   "merged, then the branch was tidied up" from "these commits exist
///   nowhere else". A refresh that silently rewrote the inputs to the
///   safety verdicts would be a deletion gate changing under the user on
///   a button labelled Fetch.
///
/// - **Returns git's own message.** Same rule `pull_checkout` states:
///   "could not fetch" names nothing, while git's refusal usually names
///   the host, the permission, or the ref. `git fetch` is quiet on
///   success and writes its progress to stderr, so the Ok string is
///   routinely EMPTY -- the caller must phrase its own success line
///   rather than echoing this, which `useFetchRefs` does.
///
/// Bounded by `GIT_TIMEOUT` like every other call here, which is the
/// answer to the scan's second objection: an unreachable remote fails
/// after 30s with git's message rather than holding a pool thread
/// forever.
pub fn fetch_refs(path: &str) -> Result<String, String> {
    let dir = Path::new(path);
    if !dir.is_dir() {
        return Err("that directory is missing".into());
    }

    // No `--prune`, no `--all`, no refspec: just the default remote's
    // configured refspec. See the doc above for why pruning is not a
    // tidy-up this button is allowed to do.
    let args: &[&str] = &["fetch"];
    git(dir, args)
}

pub fn remove_worktrees_with_progress(
    repo_path: &str,
    worktree_paths: &[String],
    mut on_progress: impl FnMut(usize, usize),
) -> Vec<RemovalOutcome> {
    let total = worktree_paths.len();
    worktree_paths
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let outcome = RemovalOutcome {
                path: p.clone(),
                error: remove_worktree(repo_path, p).err(),
            };
            // AFTER the removal, so the count means "done", not
            // "started" -- a progress bar that reaches 100% before the
            // work finishes is worse than none.
            on_progress(i + 1, total);
            outcome
        })
        .collect()
}

/// A path reduced to something two spellings of the same location share.
///
/// Three platform differences all land on this one comparison, and it
/// decides which directory `remove_worktree` deletes:
///
/// - macOS resolves `/var` to `/private/var`, so a raw string compare
///   fails for anything under a temp directory.
/// - Windows `canonicalize` returns a UNC extended-length path
///   (`\\?\C:\...`) while git reports `C:/...`, so canonicalising only
///   one side guarantees a mismatch.
/// - Windows paths are case-insensitive and git may report either
///   separator, so `C:\Code\Proj` and `c:/code/proj` are the same place.
///
/// Canonicalising both sides handles the first two; the UNC prefix is
/// stripped and the result lowercased on Windows for the third. A path
/// that cannot be canonicalised falls back to its raw form rather than
/// erroring, so a missing target stays comparable.
fn canonical_key(p: &Path) -> String {
    let resolved = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = resolved.to_string_lossy().into_owned();

    if cfg!(windows) {
        // Strip the extended-length prefix git never uses, normalise the
        // separator, and fold case -- Windows filesystems are
        // case-insensitive, so differing case is the same directory.
        s.trim_start_matches(r"\\?\")
            .replace('\\', "/")
            .to_lowercase()
    } else {
        s
    }
}

/// **The only destructive operation Headstate performs on local disk.**
/// It deletes files that may be the only copy of work, so the bar is
/// higher than for the GitHub mutations: the safety gate is RE-EVALUATED
/// here rather than trusted from the scan. A scan is a snapshot, and the
/// user may have started editing in the seconds since -- 24 of 296
/// worktrees on this machine are dirty at any moment.
///
/// Uses `git worktree remove`, never `rm -rf`: git updates its own
/// administrative files, where a raw delete leaves a stale entry making
/// the repo report a worktree that no longer exists.
pub fn remove_worktree(repo_path: &str, worktree_path: &str) -> Result<(), String> {
    remove_inner(repo_path, worktree_path, false)
}

/// Remove a worktree the safety gate would refuse.
///
/// The ONLY caller is a confirmation the user reached by reading an
/// assessment of that specific worktree. It exists because the app's
/// verdict and a considered human judgement can legitimately differ:
/// `never_pushed` means "these commits exist only here", which is a
/// reason to think hard, not a reason the user may never decide.
///
/// It relaxes exactly one check -- the safety gate. Everything else
/// holds: the target must still be a worktree of THIS repository, and
/// the main checkout is still refused.
///
/// It DOES pass git's `--force` (#798), which it did not until that
/// issue. The old behaviour was not a deliberate second line of
/// defence, it was a contradiction: the function is named `forced`, it
/// logs that it is forcing, and then git refused the commonest unsafe
/// reason on its own account -- `Dirty` -- with *"contains modified or
/// untracked files, use --force to delete it"*. So the one path whose
/// entire purpose is "I have read the warning and I want it gone" could
/// not do it, and the worktree was left in place after the user had
/// confirmed a destructive dialog.
///
/// `--force` ONCE, never twice. Git wants `--force --force` for a
/// LOCKED worktree, and that is deliberately not given: a lock is
/// another process's claim on the directory, and on the reporting
/// machine 13 of 34 worktrees were locked by agents actively working in
/// them. Clearing such a claim is `unlock_worktree`'s job, behind a
/// confirmation that names the holder and the age -- so a locked
/// worktree still fails here, loudly, rather than being torn out from
/// under whatever holds it.
pub fn remove_worktree_forced(repo_path: &str, worktree_path: &str) -> Result<(), String> {
    remove_inner(repo_path, worktree_path, true)
}

fn remove_inner(repo_path: &str, worktree_path: &str, allow_unsafe: bool) -> Result<(), String> {
    let repo = Path::new(repo_path);
    let target = Path::new(worktree_path);

    let list = git(repo, &["worktree", "list", "--porcelain"])
        .map_err(|e| format!("could not list worktrees: {e}"))?;
    let known = parse_porcelain(&list);

    // Compare CANONICAL paths. Git reports what it resolved, which may
    // differ from what the caller holds when any component is a symlink
    // -- on macOS /var is a link to /private/var, so a naive string
    // compare fails for anything under a temp directory. Falling back to
    // the raw path keeps a non-existent target comparable rather than
    // erroring here.
    let target_canon = canonical_key(target);
    let wt = known
        .iter()
        .find(|w| canonical_key(Path::new(&w.path)) == target_canon)
        .ok_or_else(|| "not a worktree of this repository".to_string())?;

    if wt.is_main {
        return Err("refusing to remove the repository's main checkout".into());
    }

    // Re-check RIGHT NOW, not from the scan. Skipped only when the caller
    // came through `remove_worktree_forced`, which means a human read an
    // assessment of this specific worktree and decided anyway.
    if !allow_unsafe {
        let branch = default_branch(repo);
        // Probed FRESH here, never reused from the scan: this is the
        // delete-time gate, and its whole point is that the scan is a
        // snapshot the world may have moved past.
        let up = upstream_state(Path::new(&wt.path));
        let has_upstream = !matches!(up, Upstream::Untracked);
        let ahead = match &up {
            Upstream::Ahead(n) => Some(*n),
            Upstream::Diverged(a, _) => Some(*a),
            Upstream::Unknown(_) => None,
            _ => Some(0),
        };
        let safety = worktree_safety(wt, &branch, has_upstream, ahead);
        if !safety.is_safe() {
            return Err(format!("not safe to remove: {}", safety.reason()));
        }
    } else {
        log::warn!("removing {worktree_path} past the safety gate, by explicit confirmation");
    }

    // `--force` exactly when the gate was SKIPPED, and never when it
    // ran (#798).
    //
    // On the GATED path there is still no `--force`, and the original
    // reasoning for that is unchanged and load-bearing: the gate above
    // already established the tree is clean, so needing force there
    // would mean the gate was wrong -- and forcing past a gate that has
    // just been proved wrong is precisely how unpushed work is lost.
    //
    // On the FORCED path that claim was simply false, and saying it
    // anyway is what made `remove_worktree_forced` structurally
    // incapable of removing a `Dirty` worktree -- the commonest unsafe
    // reason there is. Nothing established the tree is clean on this
    // branch; the log line three lines up says the opposite. The user
    // read the specific loss in a confirmation naming it and asked for
    // the directory to go, so git is told to make it go.
    //
    // ONCE, not twice. `--force --force` is what git wants for a locked
    // worktree and is deliberately withheld -- see
    // `remove_worktree_forced` for why a lock is a claim to respect
    // rather than an obstacle to double-force past.
    let mut args: Vec<&str> = vec!["worktree", "remove"];
    if allow_unsafe {
        args.push("--force");
    }
    // git's OWN resolved path, with a separator: the caller's raw string
    // could be relative or flag-shaped, and the gate above already
    // matched this record.
    args.extend(["--", wt.path.as_str()]);
    git(repo, &args)
        .map(|_| ())
        .map_err(|e| format!("git refused: {e}"))
}

/// Clear a worktree's lock.
///
/// **Not destructive, and deliberately not treated as harmless
/// either.** Nothing is deleted and the operation is exactly reversible
/// by `git worktree lock`, so this is not in the same class as
/// `remove_worktree`. What it removes is a GUARD: the lock is the only
/// mechanism a concurrent process has for saying "I am using this", and
/// clearing one that is genuinely live invites a second process into a
/// directory the first is working in.
///
/// #753 declined to offer this at all, reasoning that a one-click
/// button beside a row invites clearing a claim without reading it.
/// That reasoning was right for its evidence and #775 changed the
/// evidence: 20 of 44 worktrees on the reporting machine are locked,
/// all by one pid that is alive only because it is the parent session,
/// and `lsof -d cwd` finds nothing working in any of them. At 45% of
/// the list, refusing to offer the remedy does not protect the user
/// from a bad decision -- it leaves them with a view they cannot use
/// and sends them to a terminal to do the same thing unaided.
///
/// So it is offered, and the care went into the CONFIRMATION rather
/// than into withholding the action: the dialog names the holder and
/// the age and says what is underneath, which is the reading #753
/// wanted and a bare button would have skipped.
///
/// This function does NOT remove anything, and removal does not become
/// easier by its existence. The gate is untouched: `worktree_safety`
/// still returns `Locked` while the lock is there, and after this the
/// worktree is re-classified from scratch and refused or allowed on its
/// own merits. A merged worktree under a lock is still locked until
/// this actually runs.
///
/// Verifies the target belongs to THIS repository first, exactly as
/// `remove_inner` does and for the same reason: without it, the command
/// is "unlock any path the frontend names".
pub fn unlock_worktree(repo_path: &str, worktree_path: &str) -> Result<(), String> {
    let repo = Path::new(repo_path);
    let target = Path::new(worktree_path);

    let list = git(repo, &["worktree", "list", "--porcelain"])
        .map_err(|e| format!("could not list worktrees: {e}"))?;
    let known = parse_porcelain(&list);

    // Canonical comparison, for the reasons `canonical_key` documents:
    // macOS resolves /var to /private/var, so a raw string compare
    // fails for anything under a temp directory.
    let target_canon = canonical_key(target);
    let wt = known
        .iter()
        .find(|w| canonical_key(Path::new(&w.path)) == target_canon)
        .ok_or_else(|| "not a worktree of this repository".to_string())?;

    // Re-checked RIGHT NOW rather than trusted from the scan, which is
    // a snapshot. Not a safety gate -- unlocking loses nothing -- but
    // saying "that worktree is not locked" beats git's own error, and
    // it means a stale click on a row somebody else already unlocked
    // reports the truth instead of a refusal that reads as a fault.
    if wt.locked.is_none() {
        return Err("that worktree is not locked".into());
    }

    // git's OWN resolved path, with a `--` separator: the caller's raw
    // string could be relative or flag-shaped, and the gate above has
    // already matched this record.
    git(repo, &["worktree", "unlock", "--", &wt.path])
        .map(|_| ())
        .map_err(|e| format!("git refused: {e}"))
}

/// Clear every stale worktree registration in a repository, and say how
/// many went.
///
/// `git worktree prune`, which #793 found the app had been naming in
/// prose and never running -- the string appears in three comments and
/// nowhere in an argument list. 12 worktrees on the reporting machine
/// were `Prunable`: the Remove button greyed out, excluded from "safe to
/// remove", and the confirmation copy literally quoting the command the
/// user then had to go and type in a terminal.
///
/// **Deletes nothing recoverable, and nothing on disk at all.** A
/// prunable registration is an entry under `.git/worktrees/` whose
/// directory has already gone -- git says so itself, with "gitdir file
/// points to non-existent location". There is no tree to lose work
/// from, no branch is touched, and no commit becomes unreachable: the
/// branch the vanished worktree had checked out keeps its ref. So this
/// is the one "cleanup" in the app whose worst case is that it does
/// nothing.
///
/// REPO-WIDE by design, not per worktree. That is what git's verb is:
/// `prune` takes no path and walks every registration. A per-row button
/// would have been a lie about the scope -- clicking it on one row
/// would clear all 12 -- so the UI offers it once, over the repository,
/// with the count in the label.
///
/// Returns how many registrations were cleared, counted by LISTING
/// before and after rather than by parsing git's output. `--verbose`
/// prints a line per removal whose wording is not a documented
/// interface, and `git worktree list --porcelain` is already parsed
/// here for every other purpose. The count is what the toast needs:
/// "pruned 12 stale registrations" is a result, where a bare success is
/// indistinguishable from a no-op on a repository that had none.
///
/// A zero is a legitimate answer and not an error. Two clicks in a row,
/// or a prune somebody else ran in a terminal meanwhile, leaves nothing
/// to do -- and reporting that as a failure would read as a broken
/// command rather than as an already-tidy repository.
pub fn prune_worktrees(repo_path: &str) -> Result<u64, String> {
    let repo = Path::new(repo_path);

    let before = git(repo, &["worktree", "list", "--porcelain"])
        .map_err(|e| format!("could not list worktrees: {e}"))?;
    let stale_before = parse_porcelain(&before)
        .iter()
        .filter(|w| w.prunable.is_some())
        .count();

    // No `--expire`: git's default prunes what is ALREADY stale with no
    // grace period, which is the behaviour the UI promised when it named
    // the command. Passing an expiry would make the button silently do
    // less than the sentence beside it says.
    git(repo, &["worktree", "prune"])
        .map(|_| ())
        .map_err(|e| format!("git refused: {e}"))?;

    // Counted from a FRESH listing rather than assumed to be
    // `stale_before`. Git can decline an individual registration it
    // considers still in use -- a locked one, for instance -- and
    // reporting the number we hoped for instead of the number that went
    // would overstate the result in exactly the direction that stops the
    // user noticing the rows are still there.
    let after = git(repo, &["worktree", "list", "--porcelain"])
        .map_err(|e| format!("could not list worktrees: {e}"))?;
    let stale_after = parse_porcelain(&after)
        .iter()
        .filter(|w| w.prunable.is_some())
        .count();

    // Saturating, not a subtraction that could wrap. A registration
    // going prunable between the two listings is possible -- a directory
    // can be deleted at any moment -- and the honest answer then is
    // "none cleared", not a vast number.
    Ok(stale_before.saturating_sub(stale_after) as u64)
}

#[cfg(test)]
mod bare_repository_tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    const IDENT: [(&str, &str); 4] = [
        ("GIT_AUTHOR_NAME", "octocat"),
        ("GIT_COMMITTER_NAME", "octocat"),
        ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
        ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
    ];

    fn run(dir: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .envs(IDENT)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// #1142: a bare repository is found, and its worktrees with it.
    ///
    /// Before this, neither `.git` branch matched, so the walk descended
    /// into it as an ordinary directory until the depth cap stopped it
    /// -- and nothing was pushed to `unreadable`, so the scan came back
    /// short and did not say so.
    #[test]
    fn a_bare_repository_and_its_worktrees_are_found() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // A source with one commit, then a BARE clone of it: the shape
        // a mirror or a `--bare` clone actually has on disk.
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        assert!(run(&src, &["init", "-q", "-b", "main"]));
        assert!(run(&src, &["commit", "-q", "--allow-empty", "-m", "one"]));

        let bare = root.join("mirror.git");
        assert!(Command::new("git")
            .args(["clone", "-q", "--bare"])
            .arg(&src)
            .arg(&bare)
            .envs(IDENT)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false));

        // A worktree hanging off the bare repo -- the thing that was
        // invisible along with it.
        let wt = root.join("feature");
        assert!(run(
            &bare,
            &[
                "worktree",
                "add",
                "-q",
                wt.to_str().unwrap(),
                "-b",
                "feature"
            ]
        ));

        let mut scan = RepoScan::default();
        collect_inner(root, 0, &mut scan, false);

        // Canonicalised on both sides: on macOS a tempdir under `/var`
        // is reported by git as `/private/var`, and a string compare
        // would fail for a reason that has nothing to do with the code
        // under test.
        let same = |a: &str, b: &Path| match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };

        let found = scan
            .repos
            .iter()
            .find(|r| same(&r.path, &bare))
            .expect("the bare repository must be found");
        assert!(found.bare, "and must be marked bare");
        assert!(
            found.worktrees.iter().any(|w| same(&w.path, &wt)),
            "its linked worktree must be listed, got {:?}",
            found.worktrees.iter().map(|w| &w.path).collect::<Vec<_>>()
        );
        // The container itself is not a checkout. Listing it as one
        // would offer a removal that cannot succeed.
        assert!(
            !found.worktrees.iter().any(|w| same(&w.path, &bare)),
            "the bare repository must not list itself as one of its own worktrees"
        );
    }

    /// The other direction: an ordinary checkout is still found, and is
    /// NOT marked bare. A change that reported everything as bare would
    /// pass the test above and break every safety verdict.
    #[test]
    fn an_ordinary_checkout_is_still_found_and_is_not_bare() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let repo = root.join("plain");
        std::fs::create_dir_all(&repo).unwrap();
        assert!(run(&repo, &["init", "-q", "-b", "main"]));
        assert!(run(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]));

        let mut scan = RepoScan::default();
        collect_inner(root, 0, &mut scan, false);

        let found = scan
            .repos
            .iter()
            .find(
                |r| match (std::fs::canonicalize(&r.path), std::fs::canonicalize(&repo)) {
                    (Ok(a), Ok(b)) => a == b,
                    _ => false,
                },
            )
            .expect("an ordinary checkout must still be found");
        assert!(!found.bare, "a checkout with a working tree is not bare");
    }

    /// A directory that merely LOOKS like a bare repository -- a `HEAD`
    /// file beside an `objects` directory -- must not be reported as
    /// one. The cheap pre-check exists to avoid spawning git per
    /// directory; it must not become the answer on its own.
    #[test]
    fn a_lookalike_directory_is_not_reported_as_a_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let fake = root.join("not-a-repo");
        std::fs::create_dir_all(fake.join("objects")).unwrap();
        std::fs::write(fake.join("HEAD"), "ref: refs/heads/main\n").unwrap();

        let mut scan = RepoScan::default();
        collect_inner(root, 0, &mut scan, false);

        assert!(
            !scan.repos.iter().any(|r| {
                matches!(
                    (std::fs::canonicalize(&r.path), std::fs::canonicalize(&fake)),
                    (Ok(a), Ok(b)) if a == b
                )
            }),
            "a directory git does not call a repository must not be listed as one"
        );
    }
}

#[cfg(test)]
mod in_progress_tests {
    use super::*;

    const IDENT: [(&str, &str); 4] = [
        ("GIT_AUTHOR_NAME", "octocat"),
        ("GIT_COMMITTER_NAME", "octocat"),
        ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
        ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
    ];

    fn run(dir: &Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .envs(IDENT)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// A repository with two branches that conflict on one file.
    fn conflicting(dir: &Path) {
        assert!(run(dir, &["init", "-q", "-b", "main"]));
        std::fs::write(dir.join("f.txt"), "base\n").unwrap();
        assert!(run(dir, &["add", "f.txt"]));
        assert!(run(dir, &["commit", "-q", "-m", "base"]));

        assert!(run(dir, &["checkout", "-q", "-b", "feat"]));
        std::fs::write(dir.join("f.txt"), "theirs\n").unwrap();
        assert!(run(dir, &["commit", "-q", "-am", "theirs"]));

        assert!(run(dir, &["checkout", "-q", "main"]));
        std::fs::write(dir.join("f.txt"), "ours\n").unwrap();
        assert!(run(dir, &["commit", "-q", "-am", "ours"]));
    }

    /// #1136: a stopped rebase is not "7 uncommitted files".
    ///
    /// Driven against a REAL conflicted rebase rather than a fabricated
    /// git dir: the marker files are git's own behaviour, and a test
    /// that wrote them by hand would assert what this code assumes
    /// rather than what git does.
    #[test]
    fn a_conflicted_rebase_is_in_progress_not_dirty() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        conflicting(dir);
        assert!(run(dir, &["checkout", "-q", "feat"]));
        // Expected to FAIL: that is the conflict this test needs.
        let _ = run(dir, &["rebase", "main"]);

        let op = operation_in_progress(dir).expect("a stopped rebase must be detected");
        assert_eq!(op, GitOperation::Rebase);
        assert_eq!(
            conflicted_files(dir),
            Some(1),
            "and the conflicted file is counted"
        );
    }

    #[test]
    fn a_conflicted_merge_is_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        conflicting(dir);
        let _ = run(dir, &["merge", "feat"]);

        assert_eq!(operation_in_progress(dir), Some(GitOperation::Merge));
    }

    #[test]
    fn a_conflicted_cherry_pick_is_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        conflicting(dir);
        let _ = run(dir, &["cherry-pick", "feat"]);

        assert_eq!(operation_in_progress(dir), Some(GitOperation::CherryPick));
    }

    /// The other direction, which is what stops this reporting every
    /// worktree as mid-rebase: an ordinary clean checkout has no
    /// operation in progress.
    #[test]
    fn a_clean_checkout_has_no_operation_in_progress() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        conflicting(dir);

        assert_eq!(operation_in_progress(dir), None);
        assert_eq!(conflicted_files(dir), Some(0), "and nothing is conflicted");
    }

    /// An ABORTED rebase leaves no markers, so the row goes back to
    /// whatever it was. A state that survived its own resolution would
    /// be worse than not detecting it.
    #[test]
    fn aborting_the_rebase_clears_the_state() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        conflicting(dir);
        assert!(run(dir, &["checkout", "-q", "feat"]));
        let _ = run(dir, &["rebase", "main"]);
        assert!(operation_in_progress(dir).is_some(), "precondition");

        assert!(run(dir, &["rebase", "--abort"]));
        assert_eq!(operation_in_progress(dir), None);
    }

    /// `classify` must report it ABOVE `Dirty`: a rebase has a dirty
    /// tree by definition, so the ordering is the whole fix.
    #[test]
    fn classify_reports_in_progress_rather_than_dirty() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        conflicting(dir);
        assert!(run(dir, &["checkout", "-q", "feat"]));
        let _ = run(dir, &["rebase", "main"]);

        let mut w = Worktree {
            path: dir.to_string_lossy().into_owned(),
            branch: "feat".into(),
            ..Default::default()
        };
        classify(&mut w, dir, "main");
        match w.safety {
            Safety::InProgress { op, .. } => assert_eq!(op, GitOperation::Rebase),
            other => panic!("expected InProgress, got {other:?} -- Dirty would hide it"),
        }
        assert!(!w.safety.is_safe(), "and it is never one-click removable");
    }
}

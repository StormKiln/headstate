//! Listing branches and deciding which are deletable.
//!
//! The shape here comes from measurement on a 507-branch repository,
//! not from guessing (recorded on #411):
//!
//! - Every piece of METADATA -- name, upstream, ahead/behind, date,
//!   author, tip -- comes from ONE `for-each-ref`. 1057 branches in
//!   72ms. None of it needs a worker.
//! - `--merged` settles only 49 of 507. The other 458 are squash
//!   merges, so patch-id comparison is the main path here, not a
//!   fallback.
//! - Per-branch patch-id work cannot be batched: `git diff` has no
//!   `--stdin`, and merge-bases are effectively unique (110 distinct
//!   across 120 branches). Serial costs 18s; the existing 8-worker
//!   pool brings it to 3.5s.
//!
//! # Streaming (#657)
//!
//! The metadata being one cheap call and the verdicts being expensive
//! and independent is not just a performance note -- it is the shape
//! the scan reports in. [`scan_with_progress`] hands a caller every
//! row before any verdict exists, then verdicts as the threads settle
//! them, so the page has a full list to render while it waits for the
//! part that is slow.
//!
//! Measured on a 582-branch, 1784-commit repository: every row at
//! **257ms**, every verdict at **10.5s**. The gap is the blank page
//! the issue was about. Getting the first number required moving
//! `mainline_patch_ids` BELOW the listing -- it is ~5s of work the
//! listing does not depend on, and it used to run first.

use std::collections::HashSet;
use std::path::Path;

use super::model::{Branch, Deletable, Location, MergedHow};
use crate::worktrees::scan::git;

/// Matches the worktree classifier. Measured 18.0s serial against
/// 3.5s at 8 workers on 458 branches; beyond 8 the win flattens while
/// contention with the rest of the app does not.
const WORKERS: usize = 8;

/// Fields pulled in one pass. Tab-separated because a branch name may
/// contain almost anything else, including `|` and spaces.
/// `%(upstream:track)` is the gone marker git already computes (#1139).
/// It renders as `[gone]` for a configured upstream the remote no longer
/// has -- the ordinary state after a PR merges and the head branch is
/// deleted. Asking for it costs nothing: it is the same `for-each-ref`
/// pass.
const REF_FORMAT: &str = "%(refname:short)\t%(upstream:short)\t%(committerdate:iso8601)\t%(authorname)\t%(objectname:short)\t%(upstream:track)";

/// Spawn a git command, retrying only the SPAWN.
///
/// Same reason as `git()`: spawning git intermittently fails with
/// ENOENT under load. It matters more here than anywhere else, because the
/// callers below treat a spawn failure as "no patch-ids" -- which
/// reports every branch as unmerged, quietly, as "nothing is
/// deletable" rather than as an error.
fn spawn_git(dir: &Path, args: &[&str], stdin: bool) -> Option<std::process::Child> {
    use std::process::Stdio;
    retrying(|| {
        let mut cmd = std::process::Command::new(crate::auth::git_program());
        cmd.arg("-C")
            .arg(dir)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if stdin {
            cmd.stdin(Stdio::piped());
        }
        cmd.spawn()
    })
}

/// How many times a spawn is attempted before giving up.
///
/// Three, not two: the observed failures were single transient
/// ENOENTs, and a second attempt already covers those. The third is
/// headroom for a burst, and it costs nothing when the first succeeds.
const SPAWN_ATTEMPTS: usize = 3;

/// Run `attempt` until it succeeds, up to `SPAWN_ATTEMPTS` times.
///
/// Split out from `spawn_git` so the retry itself is testable without
/// having to provoke a real ENOENT from the kernel, which is exactly
/// the thing that cannot be summoned on demand.
fn retrying<T>(mut attempt: impl FnMut() -> std::io::Result<T>) -> Option<T> {
    for _ in 0..SPAWN_ATTEMPTS {
        if let Ok(v) = attempt() {
            return Some(v);
        }
    }
    None
}

/// The default branch, read from the remote rather than assumed.
///
/// Assuming `main` here would mean classifying every branch against a
/// ref that may not exist, and reporting the whole repository as
/// unmerged -- quietly, as "nothing is deletable".
pub fn default_branch(dir: &Path) -> Result<String, DefaultBranchError> {
    match git(
        dir,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        Ok(head) => {
            let head = head.trim();
            if head.is_empty() {
                Err(DefaultBranchError::NotSet)
            } else if !crate::worktrees::scan::is_safe_ref(
                head.strip_prefix("origin/").unwrap_or(head),
            ) {
                // `origin/HEAD` is written by the REMOTE, so the name it
                // yields is remote-controlled and must clear the
                // flag-shape check before it reaches an argv (#854). It
                // goes on to `rev-list <default>` (`mainline_patch_ids`)
                // and `merge-base <branch> <default>` (`branch_patch_id`)
                // as a bare argument with no `--`, where `--output=/path`
                // is an arbitrary file write with the app's privileges.
                //
                // `worktrees::scan::default_branch` has validated exactly
                // this since it was written, with the reasoning in its own
                // comment; this sibling did not, because `is_safe_ref` was
                // `pub(super)` and unreachable from here.
                //
                // The BARE name is what is checked, after the `origin/`
                // prefix is stripped -- and this is not a refinement, it
                // is the whole check. `symbolic-ref --short` returns
                // `origin/<name>`, so a hostile `origin/--output=/tmp/x`
                // starts with `o` and passes `is_safe_ref` unchanged. The
                // first version of this fix validated the prefixed string
                // and was therefore worthless; the test below caught it.
                // `worktrees::scan::default_branch`'s comment warns about
                // exactly this -- "Prefixing first would hide
                // `--output=EVIL` behind a name that no longer starts with
                // `-`" -- and the trap is the same whether the prefix is
                // re-added or never removed.
                //
                // Reported as `NotSet` rather than `Unavailable`: git
                // ANSWERED, and the answer is a name this code will not
                // use -- which is a fact about the repository, the
                // distinction #481 turned on.
                log::warn!("ignoring a default branch that reads as a flag: {head:?}");
                Err(DefaultBranchError::NotSet)
            } else {
                Ok(head.to_string())
            }
        }
        // git ANSWERED, and the answer is that there is no such symref.
        // A repository with no remote, or one whose origin/HEAD was
        // never set. That is a fact about the repository.
        Err(e) if e.contains("is not a symbolic ref") || e.contains("not a valid ref") => {
            Err(DefaultBranchError::NotSet)
        }
        // git could NOT be asked. Not a fact about anything.
        Err(e) => Err(DefaultBranchError::Unavailable(e)),
    }
}

/// Why the default branch is not known.
///
/// The distinction #481 turned on. Both used to be `None`, so a git
/// call that failed under load produced the same answer as a
/// repository that genuinely has no `origin/HEAD` -- and the caller
/// rendered it as a verdict about the branches.
///
/// A failed CHECK is not a finding. The Docker path already works this
/// way: `images_in_use` refuses rather than proceeding when it cannot
/// tell, because failing open there would delete an image in use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefaultBranchError {
    /// git answered: this repository has no `origin/HEAD`.
    NotSet,
    /// git could not be asked, and might succeed on a retry.
    Unavailable(String),
}

impl std::fmt::Display for DefaultBranchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DefaultBranchError::NotSet => {
                write!(f, "this repository has no origin/HEAD to compare against")
            }
            DefaultBranchError::Unavailable(e) => {
                write!(f, "could not read origin/HEAD: {e}")
            }
        }
    }
}

/// Branches currently checked out in a worktree, by branch name.
///
/// Queried here rather than shared with the Worktrees view. The two
/// views then cannot disagree: a cache that is invalidated but not
/// refreshed is exactly what made #435 delete against stale state.
fn checked_out(dir: &Path) -> Vec<(String, String)> {
    let Ok(out) = git(dir, &["worktree", "list", "--porcelain"]) else {
        return Vec::new();
    };

    let mut pairs = Vec::new();
    let mut path: Option<String> = None;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(p.trim().to_string());
        } else if let Some(b) = line.strip_prefix("branch ") {
            let name = b.trim().strip_prefix("refs/heads/").unwrap_or(b.trim());
            if let Some(p) = path.clone() {
                pairs.push((name.to_string(), p));
            }
        }
    }
    pairs
}

/// Parse one `for-each-ref` line into the metadata half of a branch.
///
/// Returns None rather than a partial branch: a row we cannot read is
/// not a branch we should offer to delete.
fn parse_ref(
    line: &str,
    remote: bool,
) -> Option<(String, Option<String>, String, String, String, bool)> {
    let mut f = line.split('\t');
    let name = f.next()?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    let upstream = f.next().unwrap_or("").trim();
    let upstream = (!upstream.is_empty()).then(|| upstream.to_string());
    let committed = f.next().unwrap_or("").trim().to_string();
    let author = f.next().unwrap_or("").trim().to_string();
    let tip = f.next().unwrap_or("").trim().to_string();
    // `[gone]` appears inside a track field that can also carry
    // `[ahead 3, behind 1]`, so this is `contains`, not equality.
    //
    // An ABSENT field is not "not gone": a git too old for
    // `%(upstream:track)` emits nothing here, and reading that as a live
    // upstream would be a confident wrong answer. The caller keeps
    // `Local` in that case, which is what it reported before this
    // change.
    let gone = f.next().unwrap_or("").contains("[gone]");

    // `origin/HEAD` is a symref, not a branch anyone deletes.
    if remote && name.ends_with("/HEAD") {
        return None;
    }
    Some((name, upstream, committed, author, tip, gone))
}

/// Ahead/behind counts for every local branch, in one process.
///
/// `%(ahead-behind:)` needs git 2.41. On an older git the field comes
/// back empty and every branch reports 0/0 -- which would make an
/// unmerged branch look like it had nothing to lose. So a missing
/// count is treated as unknown by the caller, never as zero.
///
/// That last sentence was true of the intent and false of the code until
/// #967: both callers did `.unwrap_or(0)`, and the row read "Not merged
/// -- 0 commits not on the default branch" beside a delete checkbox. The
/// counts are `Option` on `Branch` and on `Deletable::Unmerged` now, so
/// an absent entry in this map stays absent all the way to the screen.
/// `emptyStateGuard.test.ts` names this failure mode exactly: a guard
/// that passed on a comment rather than on behaviour.
fn ahead_behind(dir: &Path, default: &str) -> std::collections::HashMap<String, (u64, u64)> {
    let fmt = format!("%(refname:short)\t%(ahead-behind:{default})");
    let mut map = std::collections::HashMap::new();
    let Ok(out) = git(dir, &["for-each-ref", "--format", &fmt, "refs/heads"]) else {
        return map;
    };
    for line in out.lines() {
        let mut f = line.split('\t');
        let (Some(name), Some(counts)) = (f.next(), f.next()) else {
            continue;
        };
        let mut n = counts.split_whitespace();
        let (Some(a), Some(b)) = (n.next(), n.next()) else {
            continue;
        };
        let (Ok(a), Ok(b)) = (a.parse(), b.parse()) else {
            continue;
        };
        map.insert(name.trim().to_string(), (a, b));
    }
    map
}

/// The mainline patch-ids, or the reason they could not be built.
///
/// `Err` is NOT an empty set (#1125). Every failure arm below used to
/// `return set` with nothing in it, which is indistinguishable from a
/// repository that genuinely has no matching patch-ids -- and the
/// consequence was the worst available: a squash-merged branch failed
/// its `mainline.contains(&pid)` test and was reported `Unmerged`, so a
/// scan where git could not be spawned rendered as "nothing here is
/// deletable" rather than as a failure. That is the same shape as #967
/// one field over, and the same remedy: the unread case gets its own
/// state instead of borrowing a real one.
type Mainline = Result<HashSet<String>, String>;

/// The patch-ids of every commit on the default branch since `since`.
///
/// Built ONCE per repository and shared by every branch. Measured at
/// 4.2s for 1729 commits; doing it per branch is what made the naive
/// version take minutes.
fn mainline_patch_ids(dir: &Path, default: &str) -> Mainline {
    use std::io::Write;

    let mut set = HashSet::new();
    let shas = git(dir, &["rev-list", default, "-n", "5000"])
        .map_err(|e| format!("could not list commits on {default}: {e}"))?;
    // A genuinely empty history IS an empty set -- the check ran and
    // found nothing, which is a real answer and not a failure.
    if shas.trim().is_empty() {
        return Ok(set);
    }

    let Some(mut log) = spawn_git(
        dir,
        &["log", "--stdin", "--no-walk", "-p", "--format=commit %H"],
        true,
    ) else {
        return Err(format!("could not run git log in {}", dir.display()));
    };
    if let Some(mut stdin) = log.stdin.take() {
        let _ = stdin.write_all(shas.as_bytes());
    }
    let log_out = log
        .wait_with_output()
        .map_err(|e| format!("git log did not complete: {e}"))?;

    let Some(mut pid) = spawn_git(dir, &["patch-id", "--stable"], true) else {
        return Err(format!("could not run git patch-id in {}", dir.display()));
    };
    // Feed stdin from ITS OWN THREAD.
    //
    // The obvious form -- `write_all` into stdin, THEN
    // `wait_with_output` -- deadlocks on a real repository. Verified by
    // A/B on a 1779-commit history whose `git log -p` is 60MB: the
    // simple form hung past 90s with no output and no error, the form
    // below completes the same work in 4.5s. Isolated to this function;
    // no concurrency involved.
    //
    // The mechanism is the classic one: both processes block, us on
    // writing input and `patch-id` on writing output that nobody is
    // reading. What I could NOT establish is a byte threshold -- a
    // synthetic fixture pushing 58MB through the same pipeline does not
    // reproduce it, so rate and diff shape matter, not volume alone.
    // That is why there is no unit test here: a fixture that passes
    // against the broken code would assert nothing. The guard is this
    // comment plus the shape of the code.
    //
    // `batch_contains_patch` in `worktrees::scan` has the simple form
    // and is fine, because its input is bounded by a merge-base --
    // measured around 300 commits. Do not copy it here.
    let writer = {
        let mut stdin = pid.stdin.take();
        std::thread::spawn(move || {
            if let Some(stdin) = stdin.as_mut() {
                let _ = stdin.write_all(&log_out.stdout);
            }
            // Dropping stdin closes the pipe, which is what tells
            // patch-id there is no more input.
            drop(stdin);
        })
    };
    let out = pid
        .wait_with_output()
        .map_err(|e| format!("git patch-id did not complete: {e}"))?;
    let _ = writer.join();

    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(id) = line.split_whitespace().next() {
            set.insert(id.to_string());
        }
    }
    Ok(set)
}

/// The patch-id of everything `branch` adds on top of the default.
fn branch_patch_id(dir: &Path, branch: &str, default: &str) -> Option<String> {
    use std::io::Write;

    let base = git(dir, &["merge-base", branch, default]).ok()?;
    let base = base.trim();
    if base.is_empty() {
        return None;
    }

    let diff = spawn_git(dir, &["diff", base, branch], false)?
        .wait_with_output()
        .ok()?;
    // An empty diff means there is nothing to compare. Claiming merged
    // on that would greenlight a deletion nothing established.
    if !diff.status.success() || diff.stdout.is_empty() {
        return None;
    }

    let mut child = spawn_git(dir, &["patch-id", "--stable"], true)?;
    child.stdin.take()?.write_all(&diff.stdout).ok()?;
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(str::to_string)
}

/// Decide one branch's deletability.
///
/// Order matters: the cheap and certain checks come first, so the
/// patch-id comparison only runs for branches nothing else settled.
fn classify(
    dir: &Path,
    name: &str,
    default: &str,
    ancestors: &HashSet<String>,
    checked_out: &[(String, String)],
    ahead: Option<(u64, u64)>,
    mainline: &Mainline,
) -> Deletable {
    // The default branch, under either its local or remote name.
    let bare_default = default.rsplit('/').next().unwrap_or(default);
    if name == default || name == bare_default {
        return Deletable::DefaultBranch;
    }

    // Checked out beats merged: git refuses regardless, so reporting
    // "merged" here would offer a deletion that cannot succeed.
    if let Some((_, path)) = checked_out.iter().find(|(b, _)| b == name) {
        return Deletable::CheckedOut { path: path.clone() };
    }

    if ancestors.contains(name) {
        return Deletable::Merged {
            how: MergedHow::Ancestor,
        };
    }

    // The mainline set failing to build is NOT "this branch is not
    // squash-merged" (#1125). Nothing was established, so nothing is
    // claimed -- and critically this must be checked BEFORE the
    // patch-id comparison below, which would otherwise read a missing
    // set as a negative result and report the branch `Unmerged`.
    let mainline = match mainline {
        Ok(set) => set,
        Err(reason) => {
            return Deletable::Unknown {
                reason: format!("could not check whether this branch was squash-merged: {reason}"),
            };
        }
    };

    match branch_patch_id(dir, name, default) {
        Some(pid) if mainline.contains(&pid) => Deletable::Merged {
            how: MergedHow::Squash,
        },
        // The verdict stays `Unmerged` -- that part was measured, by the
        // patch-id comparison right above, and is independent of whether
        // the COUNT could be read. Only the number becomes absent (#967).
        // Routing this to `Unknown` instead would widen nothing (both are
        // already non-deletable) but would lose the one fact we do have.
        Some(_) => Deletable::Unmerged {
            ahead: ahead.map(|(a, _)| a),
        },
        // No diff against the merge-base, and not an ancestor. Nothing
        // was established either way, so this is a failed check rather
        // than a negative result.
        None => Deletable::Unknown {
            reason: "could not compare this branch against the default branch".into(),
        },
    }
}

/// What a caller wants told while a scan runs.
///
/// Two calls, in this order and never the other: `listed` once with
/// every row as metadata alone, then `classified` repeatedly as
/// verdicts settle. `listed` carries the TOTAL, which is what makes an
/// interrupted stream legible -- a page told to expect 512 that has
/// received 47 knows it is unfinished, where a page fed only verdicts
/// cannot tell a dead stream from a finished one (#657).
///
/// A trait rather than two closures because the classification pass
/// runs inside `std::thread::scope`, so the sink is shared across
/// eight threads and must be `Sync`. Implementations must therefore
/// tolerate `classified` arriving from several threads at once, in no
/// particular order, and interleaved.
pub trait Progress: Sync {
    /// Every branch, metadata only, all verdicts `Pending`.
    fn listed(&self, branches: &[Branch]);
    /// A batch of settled verdicts, by branch name.
    ///
    /// Batched rather than one call per branch: the remote event hub
    /// buffers 256 frames per subscriber and ends the stream of a phone
    /// that falls further behind than that, so 512 single-verdict
    /// frames would kill the connection this exists to serve.
    fn classified(&self, verdicts: &[(String, Deletable)]);
}

/// The sink for callers that want only the answer -- `scan`, and the
/// delete gate underneath it.
struct Silent;

impl Progress for Silent {
    fn listed(&self, _: &[Branch]) {}
    fn classified(&self, _: &[(String, Deletable)]) {}
}

/// How many verdicts a worker accumulates before reporting them.
///
/// Picked against the two failure modes rather than rounded off: at
/// ~64ms per branch a worker fills eight verdicts in about half a
/// second, which is often enough that the count on screen visibly
/// moves, and 512 branches then cost ~64 frames against the hub's
/// 256-frame budget instead of 512 against it.
const BATCH: usize = 8;

/// Every branch in a repository, classified.
pub fn scan(dir: &Path) -> Result<Vec<Branch>, String> {
    scan_with_progress(dir, &Silent)
}

/// `scan`, telling `progress` what it finds as it finds it.
///
/// The work is identical and so is the answer: this reports on the
/// pass that was already there rather than adding one. Without it
/// every result waits for the slowest branch before any of them
/// render, though each is complete the moment its thread finishes
/// (#657).
pub fn scan_with_progress(dir: &Path, progress: &dyn Progress) -> Result<Vec<Branch>, String> {
    // Deliberately NOT cached.
    //
    // `delete_local` calls this to re-check the merge gate immediately
    // before deleting, and its doc comment states the property that
    // makes the gate trustworthy: the check runs "against the
    // repository as it stands, never against what the UI last
    // displayed". A cached answer IS what the UI last displayed, so
    // serving one here would quietly convert the strongest safety
    // property in this module into the exact thing it rules out.
    //
    // The cache is one layer up, in `scan_cached`, which only the
    // read-only listing path uses. See #657.
    let key = super::cache::ref_state(dir);

    let default = default_branch(dir).map_err(|e| e.to_string())?;

    let locals = git(dir, &["for-each-ref", "--format", REF_FORMAT, "refs/heads"])?;
    let remotes = git(
        dir,
        &["for-each-ref", "--format", REF_FORMAT, "refs/remotes"],
    )?;

    let ancestors: HashSet<String> = git(
        dir,
        &[
            "for-each-ref",
            "--format",
            "%(refname:short)",
            "--merged",
            &default,
            "refs/heads",
        ],
    )
    .unwrap_or_default()
    .lines()
    .map(|l| l.trim().to_string())
    .filter(|l| !l.is_empty())
    .collect();

    let wt = checked_out(dir);
    let ab = ahead_behind(dir, &default);

    // Which local branches have a remote counterpart, so a local-only
    // branch is not reported as tracked just because a same-named
    // remote ref exists without an upstream configured.
    let remote_names: HashSet<String> = remotes
        .lines()
        .filter_map(|l| parse_ref(l, true))
        .map(|(n, ..)| n)
        .collect();

    let mut out: Vec<Branch> = Vec::new();
    let mut local_upstreams: HashSet<String> = HashSet::new();

    for line in locals.lines() {
        let Some((name, upstream, committed, author, tip, gone)) = parse_ref(line, false) else {
            continue;
        };
        if let Some(u) = &upstream {
            local_upstreams.insert(u.clone());
        }
        let location = match &upstream {
            // Checked FIRST: a gone upstream is still an upstream, so
            // `remote_names.contains` can legitimately miss it while the
            // branch is neither tracked nor local-only. Ordering the
            // other way would let a live-but-unlisted remote mask it.
            Some(_) if gone => Location::Gone,
            Some(u) if remote_names.contains(u) => Location::Tracked,
            // Unchanged, and the fallback that keeps this safe: a git
            // too old for `%(upstream:track)` emits no marker, so its
            // branches stay `Local` exactly as before.
            _ => Location::Local,
        };
        // NOT `.unwrap_or((0, 0))` (#967). An empty map is what an old
        // git, a missing git, or a `safe.directory` refusal produces, and
        // that fallback made every branch claim it was level with the
        // default branch.
        let (ahead, behind) = match ab.get(&name).copied() {
            Some((a, b)) => (Some(a), Some(b)),
            None => (None, None),
        };
        out.push(Branch {
            name,
            location,
            upstream,
            ahead,
            behind,
            committed,
            author,
            tip,
            deletable: Deletable::Pending,
        });
    }

    // Remote branches with no local counterpart. These clean up by a
    // push to a shared remote, which is why they are a separate case
    // rather than a flag on the local row.
    for line in remotes.lines() {
        let Some((name, _, committed, author, tip, _gone)) = parse_ref(line, true) else {
            continue;
        };
        if local_upstreams.contains(&name) {
            continue;
        }
        out.push(Branch {
            name,
            location: Location::Remote,
            upstream: None,
            // `None`, not `0` (#967). `ahead_behind` only queries
            // `refs/heads`, so a remote-only branch was never measured at
            // all -- and a 0 here was an answer to a question nobody
            // asked.
            ahead: None,
            behind: None,
            committed,
            author,
            tip,
            deletable: Deletable::Pending,
        });
    }

    // Sorted BEFORE the classification pass, not after.
    //
    // This order is what the page renders in, and `listed` hands it
    // the whole list up front -- so sorting afterwards would reshuffle
    // every row under the user as verdicts arrived. The sort is on
    // `committed`, which no classification touches, so moving it here
    // cannot change the answer.
    out.sort_by(|a, b| b.committed.cmp(&a.committed));

    // Every row, in its final order, with nothing decided yet. This is
    // the frame that turns a blank page into a full one, and it costs
    // nothing: the metadata came from a `for-each-ref` that has
    // already run.
    progress.listed(&out);

    // AFTER the listing, deliberately, and this is not cosmetic.
    //
    // `mainline_patch_ids` is the single most expensive step in the
    // whole scan -- ~5s of `log -p` piped through `patch-id` on a
    // 1784-commit history -- and only the classification below needs
    // it. Built before the listing, as it was, it put its five seconds
    // in front of a frame that depends on none of it: measured on a
    // 603-branch repository, every row reached the page at 5.5s
    // instead of 0.6s, for no reason but statement order.
    let mainline = mainline_patch_ids(dir, &default);

    // The expensive pass, 8 ways. Measured 18.0s serial / 3.5s here.
    let chunk = out.len().div_ceil(WORKERS).max(1);
    std::thread::scope(|scope| {
        for part in out.chunks_mut(chunk) {
            let (default, ancestors, wt, ab, mainline) =
                (&default, &ancestors, &wt, &ab, &mainline);
            scope.spawn(move || {
                let mut batch: Vec<(String, Deletable)> = Vec::with_capacity(BATCH);
                for b in part {
                    b.deletable = classify(
                        dir,
                        &b.name,
                        default,
                        ancestors,
                        wt,
                        ab.get(&b.name).copied(),
                        mainline,
                    );
                    batch.push((b.name.clone(), b.deletable.clone()));
                    if batch.len() >= BATCH {
                        progress.classified(&batch);
                        batch.clear();
                    }
                }
                // The tail. Without it a worker whose share is not a
                // multiple of BATCH keeps its last verdicts, and the
                // page sits permanently short of its total -- the
                // "stalled" state the count exists to make visible,
                // reported for a scan that actually finished.
                if !batch.is_empty() {
                    progress.classified(&batch);
                }
            });
        }
    });

    // Stored against the refs as they were when the scan STARTED. If
    // something moved while it ran, the next call's key will not match
    // this one and the answer is recomputed -- which is the safe
    // direction. Re-reading the refs here to store a fresher key would
    // stamp a just-computed answer as describing a repository state it
    // was not computed from.
    if let Some(k) = key {
        super::cache::put(dir, k, &out);
    }
    Ok(out)
}

/// `scan`, served from the cache when the refs have not moved.
///
/// For the LISTING path only. The scan costs ~64ms per branch --
/// 10-13s on a 512-branch repository -- and every visit after the
/// query's 10s `staleTime` paid it again, with a blank page throughout
/// (#657).
///
/// The 10s `staleTime` is deliberately short because a stale "safe to
/// delete" is the expensive wrong answer, and that reasoning survives
/// here untouched: the page still refetches on the same schedule. What
/// changes is that a refetch against unmoved refs costs one
/// `for-each-ref` instead of a full rescan. If ANY ref moved -- a new
/// branch, a fetch, a force-push -- the key differs and the work is
/// redone.
///
/// Deletion does not come through here, by design. `delete_local`
/// re-checks against `scan` directly, so the gate still runs against
/// the repository as it stands.
pub fn scan_cached(dir: &Path) -> Result<Vec<Branch>, String> {
    scan_cached_with_progress(dir, &Silent)
}

/// `scan_cached`, streaming on a MISS only.
///
/// A hit emits nothing at all, and deliberately: the answer is already
/// complete and returns in one `for-each-ref`, so there is no gap to
/// fill. Emitting a `listed` frame there would put every row back to
/// `Pending` on screen for the instant before the return value
/// replaced them -- a flicker whose only content is a lie about what
/// is known.
pub fn scan_cached_with_progress(
    dir: &Path,
    progress: &dyn Progress,
) -> Result<Vec<Branch>, String> {
    if let Some(k) = super::cache::ref_state(dir) {
        if let Some(hit) = super::cache::get(dir, &k) {
            return Ok(hit);
        }
    }
    scan_with_progress(dir, progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Synthetic identity: these fixtures must never carry a real one.
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

    /// A repo with a remote, so `origin/HEAD` resolves the way the
    /// scan requires.
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

    /// A flag-shaped `origin/HEAD` is refused rather than passed to git.
    ///
    /// `origin/HEAD` is written by the REMOTE, so the name it yields is
    /// remote-controlled -- and it goes on to `rev-list <default>` and
    /// `merge-base <branch> <default>` as a bare argv element with no
    /// `--`, where `--output=/path` is an arbitrary file write with this
    /// app's privileges.
    ///
    /// `worktrees::scan` has guarded its own `default_branch` since it was
    /// written; this sibling did not, because `is_safe_ref` was
    /// `pub(super)` (#854). The symref is planted with `git symbolic-ref`
    /// directly rather than through `set-head`, which validates the name
    /// -- a hostile remote is under no such obligation.
    #[test]
    fn a_flag_shaped_remote_head_is_refused() {
        let (_t, repo) = fixture();
        run(
            &repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/--output=/tmp/pwned",
            ],
        );
        assert_eq!(
            default_branch(&repo),
            Err(DefaultBranchError::NotSet),
            "a name git would read as an option must not be returned as a branch"
        );
    }

    /// And the ordinary case still works, so the guard above is not
    /// refusing everything -- which would read as "this repository has no
    /// origin/HEAD" on every repository.
    #[test]
    fn an_ordinary_remote_head_is_still_returned() {
        let (_t, repo) = fixture();
        assert_eq!(default_branch(&repo).as_deref(), Ok("origin/main"));
    }

    fn find<'a>(bs: &'a [Branch], name: &str) -> &'a Branch {
        bs.iter().find(|b| b.name == name).unwrap_or_else(|| {
            panic!(
                "no branch {name} in {:?}",
                bs.iter().map(|b| &b.name).collect::<Vec<_>>()
            )
        })
    }

    #[test]
    fn the_default_branch_is_never_offered_for_deletion() {
        let (_t, repo) = fixture();
        let out = scan(&repo).unwrap();
        assert_eq!(find(&out, "main").deletable, Deletable::DefaultBranch);
    }

    /// THE case this view exists for. `git branch --merged` misses a
    /// squash merge entirely, and every merged PR in these repos is a
    /// squash -- so without patch-id comparison the view would report
    /// almost nothing as deletable.
    #[test]
    fn a_squash_merged_branch_is_detected_as_merged() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "feature"]);
        commit(&repo, "feature-work");
        run(&repo, &["checkout", "-q", "main"]);
        // Squash: same content, different commit -- not an ancestor.
        run(&repo, &["merge", "-q", "--squash", "feature"]);
        run(&repo, &["commit", "-q", "-m", "squashed feature"]);
        run(&repo, &["push", "-q", "origin", "main"]);

        let out = scan(&repo).unwrap();
        assert_eq!(
            find(&out, "feature").deletable,
            Deletable::Merged {
                how: MergedHow::Squash
            },
            "a squash merge must be recognised; --merged alone never sees it"
        );
    }

    #[test]
    fn a_fast_forward_merged_branch_reports_as_an_ancestor() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "ff"]);
        commit(&repo, "ff-work");
        run(&repo, &["checkout", "-q", "main"]);
        run(&repo, &["merge", "-q", "--ff-only", "ff"]);
        run(&repo, &["push", "-q", "origin", "main"]);

        let out = scan(&repo).unwrap();
        assert_eq!(
            find(&out, "ff").deletable,
            Deletable::Merged {
                how: MergedHow::Ancestor
            }
        );
    }

    #[test]
    fn an_unmerged_branch_is_not_deletable_and_says_how_far_ahead() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "wip"]);
        commit(&repo, "wip-one");
        commit(&repo, "wip-two");
        run(&repo, &["checkout", "-q", "main"]);

        let out = scan(&repo).unwrap();
        assert_eq!(
            find(&out, "wip").deletable,
            Deletable::Unmerged { ahead: Some(2) }
        );
        assert!(!find(&out, "wip").deletable.is_deletable());
        // A real count, not the `Some(0)` an old git would have produced.
        assert_eq!(find(&out, "wip").ahead, Some(2));
    }

    /// #967. `ahead_behind`'s doc says "a missing count is treated as
    /// unknown by the caller, never as zero", and for a long time the
    /// caller did `.unwrap_or(0)` anyway.
    ///
    /// An empty `ahead_behind` map is precisely the state git older than
    /// 2.41 produces -- `%(ahead-behind:)` comes back empty and every line
    /// is skipped -- and it is also what a failed `for-each-ref` produces,
    /// since that path returns the map empty. Both are ordinary
    /// fresh-machine conditions: Debian stable and several LTS images ship
    /// 2.39, and a GUI-launched app may not find git on its PATH at all.
    ///
    /// The verdict must stay `Unmerged` (that part WAS measured, by the
    /// patch-id comparison) while the count becomes absent. What must not
    /// happen is `Unmerged { ahead: 0 }`, which `BranchesPage` renders as
    /// "Not merged — 0 commits not on the default branch" beside a delete
    /// checkbox: the warning and the reassurance that cancels it, in one
    /// sentence, resolving in the dangerous direction.
    /// #1125: a mainline set that could not be BUILT is not a mainline
    /// set that is empty.
    ///
    /// The distinction is load-bearing in the dangerous direction. A
    /// squash-merged branch is recognised only by finding its patch-id
    /// in the mainline set; if the set failed to build and that failure
    /// renders as an empty set, the branch fails the lookup and is
    /// reported `Unmerged`. Repeated across a repository, a scan where
    /// git could not be spawned rendered as "nothing here is deletable"
    /// -- a confident wrong answer, not an error.
    #[test]
    fn a_mainline_that_could_not_be_built_is_unknown_not_unmerged() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "wip"]);
        commit(&repo, "wip-one");
        run(&repo, &["checkout", "-q", "main"]);

        let default = default_branch(&repo).unwrap();

        let verdict = classify(
            &repo,
            "wip",
            &default,
            &HashSet::new(),
            &[],
            None,
            &Err("git could not be spawned".to_string()),
        );

        match &verdict {
            Deletable::Unknown { reason } => {
                assert!(
                    reason.contains("squash-merged"),
                    "the reason must say what could not be checked, got {reason:?}"
                );
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
        assert!(!verdict.is_deletable());
    }

    /// The other direction, so the fix cannot be "return Unknown
    /// always": a repository whose history genuinely yields an empty
    /// mainline set still gets a real verdict.
    #[test]
    fn an_empty_mainline_is_still_a_real_answer() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "wip"]);
        commit(&repo, "wip-one");
        run(&repo, &["checkout", "-q", "main"]);

        let default = default_branch(&repo).unwrap();
        let verdict = classify(
            &repo,
            "wip",
            &default,
            &HashSet::new(),
            &[],
            Some((1, 0)),
            &Ok(HashSet::new()),
        );

        assert!(
            !matches!(verdict, Deletable::Unknown { .. }),
            "an empty-but-successful mainline must not be reported as a failed check, got {verdict:?}"
        );
    }

    #[test]
    fn a_branch_whose_ahead_count_could_not_be_read_does_not_report_zero() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "wip"]);
        commit(&repo, "wip-one");
        run(&repo, &["checkout", "-q", "main"]);

        let default = default_branch(&repo).unwrap();
        let mainline = mainline_patch_ids(&repo, &default);

        // No ancestors, no worktrees, and -- the point -- NO entry in the
        // ahead/behind map, which is what `ahead_behind` returns wholesale
        // on an old or absent git.
        let verdict = classify(
            &repo,
            "wip",
            &default,
            &HashSet::new(),
            &[],
            None,
            &mainline,
        );

        assert_ne!(
            verdict,
            Deletable::Unmerged { ahead: Some(0) },
            "an unread count must not render as a branch with nothing to lose"
        );
        match verdict {
            Deletable::Unmerged { ahead: None } | Deletable::Unknown { .. } => {}
            other => panic!("expected an absent count or Unknown, got {other:?}"),
        }

        // And the branch is still refused, which was never in doubt: the
        // fix is to what is REPORTED, not to what is permitted.
        assert!(!verdict.is_deletable());
    }

    /// The other half: a genuinely level branch keeps reporting a real
    /// zero (#967). Only the UNREAD case became absent, and a fix that
    /// erased the measured zero would have traded one wrong answer for
    /// another.
    #[test]
    fn a_measured_zero_ahead_is_still_a_measured_zero() {
        let (_t, repo) = fixture();
        // Branch at the tip of main: nothing ahead, and `--merged` will
        // settle it as an ancestor.
        run(&repo, &["branch", "level"]);

        let out = scan(&repo).unwrap();
        assert_eq!(
            find(&out, "level").ahead,
            Some(0),
            "a branch level with the default branch has a MEASURED zero"
        );
        assert_eq!(find(&out, "level").behind, Some(0));
    }

    /// Checked-out beats merged: git refuses to delete a branch that is
    /// checked out, so reporting it as deletable would offer an action
    /// that cannot succeed.
    #[test]
    fn a_branch_checked_out_in_a_worktree_is_reported_as_checked_out() {
        let (tmp, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "live"]);
        commit(&repo, "live-work");
        run(&repo, &["checkout", "-q", "main"]);
        run(&repo, &["merge", "-q", "--squash", "live"]);
        run(&repo, &["commit", "-q", "-m", "squashed live"]);
        run(&repo, &["push", "-q", "origin", "main"]);

        let wt = tmp.path().join("live-wt");
        run(
            &repo,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "live"],
        );

        let out = scan(&repo).unwrap();
        match &find(&out, "live").deletable {
            Deletable::CheckedOut { path } => assert!(path.contains("live-wt"), "{path}"),
            other => panic!("merged-but-checked-out must report CheckedOut, got {other:?}"),
        }
    }

    /// A remote branch with no local counterpart is its own case: it is
    /// removed by a push to a shared remote, not by deleting a ref here.
    #[test]
    fn a_remote_only_branch_is_listed_and_marked_remote() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "only-remote"]);
        commit(&repo, "remote-work");
        run(&repo, &["push", "-q", "origin", "only-remote"]);
        run(&repo, &["checkout", "-q", "main"]);
        run(&repo, &["branch", "-q", "-D", "only-remote"]);

        let out = scan(&repo).unwrap();
        let b = find(&out, "origin/only-remote");
        assert_eq!(b.location, Location::Remote);
    }

    /// #1139: a branch whose upstream was deleted is not a branch that
    /// never had one.
    ///
    /// Those are OPPOSITE claims -- one is work existing only on this
    /// machine, the other is work that was pushed, merged and cleaned up
    /// -- and they shared the word "local" because `%(upstream:track)`
    /// was never asked for and the configured-but-vanished upstream fell
    /// to the `_` arm.
    #[test]
    fn a_gone_upstream_is_not_reported_as_local_only() {
        let (name, upstream, _, _, _, gone) = parse_ref(
            "feature\torigin/feature\t2026-09-01T10:00:00+00:00\tsam\tabc1234\t[gone]",
            false,
        )
        .expect("a well-formed ref parses");
        assert_eq!(name, "feature");
        assert_eq!(upstream.as_deref(), Some("origin/feature"));
        assert!(gone, "the [gone] marker must be read");
    }

    /// The live case, so the fix cannot degenerate into marking
    /// everything gone.
    #[test]
    fn a_live_upstream_is_not_gone() {
        let (_, upstream, _, _, _, gone) = parse_ref(
            "feature\torigin/feature\t2026-09-01T10:00:00+00:00\tsam\tabc1234\t[ahead 3, behind 1]",
            false,
        )
        .unwrap();
        assert_eq!(upstream.as_deref(), Some("origin/feature"));
        assert!(!gone, "an ahead/behind branch is tracked, not gone");
    }

    /// A git too old for `%(upstream:track)` emits nothing in that
    /// field. Reading an ABSENT marker as "not gone" is correct -- those
    /// branches stay `Local`, exactly as before this change -- but
    /// reading it as gone would be a confident wrong answer, so it is
    /// pinned.
    #[test]
    fn an_absent_track_field_is_not_gone() {
        let (_, _, _, _, _, gone) = parse_ref(
            "feature\torigin/feature\t2026-09-01T10:00:00+00:00\tsam\tabc1234",
            false,
        )
        .unwrap();
        assert!(
            !gone,
            "an unread marker must not be reported as a deleted upstream"
        );
    }

    #[test]
    fn a_branch_with_an_upstream_is_tracked_not_local_only() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "shared"]);
        commit(&repo, "shared-work");
        run(&repo, &["push", "-q", "-u", "origin", "shared"]);
        run(&repo, &["checkout", "-q", "main"]);

        let out = scan(&repo).unwrap();
        let b = find(&out, "shared");
        assert_eq!(b.location, Location::Tracked);
        assert_eq!(b.upstream.as_deref(), Some("origin/shared"));
        // And it is listed once, not twice.
        assert_eq!(out.iter().filter(|x| x.name == "origin/shared").count(), 0);
    }

    /// The retry is the fix for the CI failure that stopped v4.6.0,
    /// so it gets a test of its own rather than resting on "the flake
    /// stopped happening".
    ///
    /// A real ENOENT under load cannot be summoned on demand, which is
    /// why `retrying` is split out: the transient failure is injected
    /// instead.
    #[test]
    fn a_spawn_that_fails_twice_still_answers() {
        let mut calls = 0;
        let got = retrying(|| {
            calls += 1;
            if calls <= 2 {
                Err(std::io::Error::from(std::io::ErrorKind::NotFound))
            } else {
                Ok("answered")
            }
        });
        assert_eq!(got, Some("answered"));
        assert_eq!(calls, 3, "it must keep trying, not give up after one");
    }

    /// A spawn that never works must report failure, not spin.
    #[test]
    fn a_spawn_that_always_fails_gives_up_and_says_so() {
        let mut calls = 0;
        let got: Option<()> = retrying(|| {
            calls += 1;
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        });
        assert_eq!(got, None);
        assert_eq!(calls, SPAWN_ATTEMPTS);
    }

    /// The common path pays nothing: one call, no retries.
    #[test]
    fn a_spawn_that_works_is_attempted_once() {
        let mut calls = 0;
        let got = retrying(|| {
            calls += 1;
            Ok::<_, std::io::Error>(7)
        });
        assert_eq!(got, Some(7));
        assert_eq!(calls, 1);
    }

    /// `origin/HEAD` is a symref, not something anyone deletes.
    #[test]
    fn the_remote_head_symref_is_not_listed_as_a_branch() {
        let (_t, repo) = fixture();
        let out = scan(&repo).unwrap();
        assert!(
            !out.iter().any(|b| b.name.ends_with("/HEAD")),
            "origin/HEAD must not appear as a deletable branch"
        );
    }

    /// Without a resolvable default branch every classification would
    /// be measured against a ref that does not exist, quietly reporting
    /// the whole repository as unmerged.
    #[test]
    fn a_repository_with_no_remote_head_is_an_error_not_an_empty_answer() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("solo");
        std::fs::create_dir_all(&repo).unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        commit(&repo, "base");

        assert!(scan(&repo).is_err());
    }

    // ---- streaming (#657) ------------------------------------------

    /// Records what a scan reported, from whichever thread reported it.
    #[derive(Default)]
    struct Recorder {
        listed: std::sync::Mutex<Vec<Branch>>,
        verdicts: std::sync::Mutex<Vec<(String, Deletable)>>,
        /// How many separate `classified` calls arrived, as distinct
        /// from how many verdicts they carried. Batching is the thing
        /// keeping the phone's stream alive, so it is asserted on.
        batches: std::sync::atomic::AtomicUsize,
    }

    impl Progress for Recorder {
        fn listed(&self, branches: &[Branch]) {
            self.listed.lock().unwrap().extend_from_slice(branches);
        }
        fn classified(&self, verdicts: &[(String, Deletable)]) {
            self.batches
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.verdicts.lock().unwrap().extend_from_slice(verdicts);
        }
    }

    /// The whole point: the page can render every row before any
    /// verdict exists, because the metadata comes from a
    /// `for-each-ref` that has already run.
    #[test]
    fn every_branch_is_listed_as_pending_before_any_verdict_arrives() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "wip"]);
        commit(&repo, "wip-work");
        run(&repo, &["checkout", "-q", "main"]);

        let rec = Recorder::default();
        let out = scan_with_progress(&repo, &rec).unwrap();

        let listed = rec.listed.lock().unwrap();
        assert_eq!(listed.len(), out.len(), "the listing must be complete");
        assert!(
            listed.iter().all(|b| b.deletable == Deletable::Pending),
            "a listed row carries no verdict; anything else would show \
             an answer nothing established"
        );
        // Same rows, same order, so the page never reshuffles beneath
        // the user as verdicts land.
        let names = |bs: &[Branch]| bs.iter().map(|b| b.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&listed), names(&out));
    }

    /// Every listed row eventually gets exactly one verdict, and it is
    /// the verdict the returned answer carries. A stream that
    /// disagreed with the answer would let the page offer a deletion
    /// the scan itself did not.
    #[test]
    fn the_stream_settles_every_row_and_agrees_with_the_answer() {
        let (_t, repo) = fixture();
        run(&repo, &["checkout", "-q", "-b", "squashed"]);
        commit(&repo, "squashed-work");
        run(&repo, &["checkout", "-q", "main"]);
        run(&repo, &["merge", "-q", "--squash", "squashed"]);
        run(&repo, &["commit", "-q", "-m", "squashed"]);
        run(&repo, &["push", "-q", "origin", "main"]);
        run(&repo, &["checkout", "-q", "-b", "open-work"]);
        commit(&repo, "open");
        run(&repo, &["checkout", "-q", "main"]);

        let rec = Recorder::default();
        let out = scan_with_progress(&repo, &rec).unwrap();

        let verdicts = rec.verdicts.lock().unwrap();
        assert_eq!(
            verdicts.len(),
            out.len(),
            "one verdict per branch, no more and no fewer -- a missing \
             one leaves the page short of its total forever"
        );
        for b in &out {
            let streamed = verdicts
                .iter()
                .find(|(n, _)| *n == b.name)
                .unwrap_or_else(|| panic!("no streamed verdict for {}", b.name));
            assert_eq!(streamed.1, b.deletable, "{} disagreed", b.name);
        }
        assert_eq!(
            find(&out, "squashed").deletable,
            Deletable::Merged {
                how: MergedHow::Squash
            }
        );
    }

    /// The tail flush. A worker whose share is not a multiple of
    /// `BATCH` must still report its last verdicts: the failure mode
    /// is a scan that finished while the page reads "47 of 512", which
    /// is indistinguishable from the dead stream the count exists to
    /// expose.
    #[test]
    fn a_partial_final_batch_is_still_reported() {
        let (_t, repo) = fixture();
        // One branch, so no worker can fill a batch of eight.
        run(&repo, &["checkout", "-q", "-b", "lonely"]);
        commit(&repo, "lonely-work");
        run(&repo, &["checkout", "-q", "main"]);

        let rec = Recorder::default();
        let out = scan_with_progress(&repo, &rec).unwrap();
        assert_eq!(rec.verdicts.lock().unwrap().len(), out.len());
        assert!(out.len() < BATCH, "the fixture must not fill a batch");
    }

    /// Batching is what keeps a phone's stream alive: the hub buffers
    /// 256 frames and ends the stream of a subscriber further behind
    /// than that, so a frame per branch would kill the connection this
    /// feature exists to serve.
    #[test]
    fn verdicts_are_batched_rather_than_sent_one_frame_per_branch() {
        let (_t, repo) = fixture();
        // Enough branches that a single worker's share exceeds BATCH.
        for i in 0..(BATCH * WORKERS * 2) {
            let name = format!("b{i}");
            run(&repo, &["branch", "-q", &name, "main"]);
        }

        let rec = Recorder::default();
        let out = scan_with_progress(&repo, &rec).unwrap();
        let batches = rec.batches.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(rec.verdicts.lock().unwrap().len(), out.len());
        assert!(
            batches < out.len(),
            "{batches} frames for {} branches is one per branch",
            out.len()
        );
    }

    /// A cache hit emits NOTHING. The answer is already complete, so a
    /// `listed` frame would put every row back to `Pending` on screen
    /// for the instant before the return value replaced it -- a
    /// flicker whose only content is a lie about what is known.
    #[test]
    fn a_cache_hit_streams_nothing_because_there_is_no_gap_to_fill() {
        // The fixture is a fresh temporary path, so nothing else can
        // have primed it -- but `cache.rs`'s tests call `clear()`,
        // which wipes the WHOLE map and would turn this hit into a
        // miss. The shared lock keeps them apart.
        let _serial = super::super::cache::serialised();
        let (_t, repo) = fixture();

        // Prime it, then ask again with the refs unmoved.
        let first = scan_cached(&repo).unwrap();
        let rec = Recorder::default();
        let second = scan_cached_with_progress(&repo, &rec).unwrap();

        assert_eq!(first, second);
        assert!(rec.listed.lock().unwrap().is_empty());
        assert!(rec.verdicts.lock().unwrap().is_empty());
    }
}

#[cfg(test)]
/// Measurement, not verification. `#[ignore]` because it needs a real
/// repository: `REPO=~/path cargo test -- --ignored live_scan`.
///
/// It is what produced the numbers this module's design rests on --
/// 675 branches in 9.4s, of which 489 merges were squashes and only 47
/// were ancestors. Kept so the cost can be re-checked on a different
/// machine rather than taken on faith from a comment.
mod live {
    /// What streaming actually buys, measured rather than asserted:
    /// how long until the page has EVERY row, against how long until
    /// it has every verdict. Same invocation as the scan above --
    /// `REPO=~/path cargo test -- --ignored live_stream`.
    ///
    /// The gap between the two numbers is the blank page #657 was
    /// about.
    #[test]
    #[ignore]
    fn live_stream_of_a_real_repository() {
        use crate::branches::{Branch, Deletable, Progress};
        use std::sync::Mutex;
        use std::time::Instant;

        struct Timing {
            start: Instant,
            first_row: Mutex<Option<(std::time::Duration, usize)>>,
            settled: Mutex<Vec<(std::time::Duration, usize)>>,
            done: std::sync::atomic::AtomicUsize,
        }
        impl Progress for Timing {
            fn listed(&self, branches: &[Branch]) {
                *self.first_row.lock().unwrap() = Some((self.start.elapsed(), branches.len()));
            }
            fn classified(&self, verdicts: &[(String, Deletable)]) {
                let n = self
                    .done
                    .fetch_add(verdicts.len(), std::sync::atomic::Ordering::Relaxed)
                    + verdicts.len();
                self.settled.lock().unwrap().push((self.start.elapsed(), n));
            }
        }

        let dir = std::path::PathBuf::from(std::env::var("REPO").unwrap());
        // A cache hit would measure nothing, so start from a miss.
        crate::branches::cache::clear();
        let t = Timing {
            start: Instant::now(),
            first_row: Mutex::new(None),
            settled: Mutex::new(Vec::new()),
            done: std::sync::atomic::AtomicUsize::new(0),
        };
        let out = super::scan_cached_with_progress(&dir, &t).unwrap();
        let total_time = t.start.elapsed();

        let (listed_at, listed_n) = t.first_row.lock().unwrap().expect("a listing frame");
        let settled = t.settled.lock().unwrap();
        let quarter = settled.iter().find(|(_, n)| *n * 4 >= out.len());
        eprintln!("BRANCHES        {}", out.len());
        eprintln!("every row       {listed_at:?}  ({listed_n} rows, no verdicts)");
        if let Some((at, n)) = quarter {
            eprintln!("a quarter       {at:?}  ({n} classified)");
        }
        eprintln!("every verdict   {total_time:?}");
        eprintln!(
            "frames          {} (batched at {})",
            settled.len(),
            super::BATCH
        );
    }

    #[test]
    #[ignore]
    fn live_scan_of_a_real_repository() {
        let dir = std::path::PathBuf::from(std::env::var("REPO").unwrap());
        let t = std::time::Instant::now();
        let out = super::scan(&dir).unwrap();
        let elapsed = t.elapsed();
        use crate::branches::{Deletable, Location, MergedHow};
        let n = |f: &dyn Fn(&crate::branches::Branch) -> bool| out.iter().filter(|b| f(b)).count();
        eprintln!("TOTAL {} branches in {:?}", out.len(), elapsed);
        eprintln!(
            "  local {}  tracked {}  remote {}",
            n(&|b| b.location == Location::Local),
            n(&|b| b.location == Location::Tracked),
            n(&|b| b.location == Location::Remote)
        );
        eprintln!(
            "  merged/ancestor {}",
            n(&|b| matches!(
                b.deletable,
                Deletable::Merged {
                    how: MergedHow::Ancestor
                }
            ))
        );
        eprintln!(
            "  merged/squash   {}",
            n(&|b| matches!(
                b.deletable,
                Deletable::Merged {
                    how: MergedHow::Squash
                }
            ))
        );
        eprintln!(
            "  unmerged        {}",
            n(&|b| matches!(b.deletable, Deletable::Unmerged { .. }))
        );
        eprintln!(
            "  checked out     {}",
            n(&|b| matches!(b.deletable, Deletable::CheckedOut { .. }))
        );
        eprintln!(
            "  unknown         {}",
            n(&|b| matches!(b.deletable, Deletable::Unknown { .. }))
        );
        eprintln!(
            "  pending         {}",
            n(&|b| matches!(b.deletable, Deletable::Pending))
        );
    }
}

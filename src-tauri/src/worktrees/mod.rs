//! Local git worktree discovery.
//!
//! Agents create worktrees; pull requests merge; the worktrees stay. On
//! this machine that is 202 worktrees across 6 repositories in a `~/code`
//! totalling 270 GB, which is how a disk fills up silently.
//!
//! Discovery asks GIT rather than scanning for `.git` files: git owns the
//! truth about which worktrees exist, so there is no orphan-guessing and
//! no false positives from a stale directory.
//!
//! Nothing here talks to GitHub. It is local disk work and deliberately
//! runs off the poll loop.

mod assess;
mod model;
pub(crate) mod scan;
/// Submodule state per worktree (#1138).
pub mod submodule;
mod update;

pub use assess::{assess, Assessment};
// `Repo` is still the payload `RepoScan` carries, so it stays exported
// even though #951 left `list_worktrees`' signature naming only the scan.
#[allow(unused_imports)]
pub use model::{Repo, Worktree};
// `git` is the ONE bounded git invocation in this crate, and the
// repository browser (#1031) needs it for exactly the reason `branches`
// already did: a git call that hangs must become an answer rather than
// park a `spawn_blocking` thread forever. Re-exported rather than
// re-implemented, so `GIT_TIMEOUT` and the spawn retry cover the browser
// too.
pub(crate) use scan::git;
pub use scan::{
    classify_main_checkout, classify_repo_streaming, fetch_refs, head_oid, prune_worktrees,
    pull_checkout, remove_orphan, remove_worktree, remove_worktree_forced,
    remove_worktrees_with_progress, repo_identity, scan_dirs_fast_reporting, size_repo_streaming,
    unlock_worktree, RemovalOutcome, RepoScan,
};
// Update All (#1012). A loop over `pull_checkout` above, deliberately in
// its own module: the refusal set is the feature, and it is long enough
// to argue in one place rather than as a wing of the scan.
//
// `RepoUpdateOutcome` is the payload `UpdateAllReport` carries, so it
// stays exported even though production code only ever names the report
// -- the same reason `Repo` above carries the allow. A caller that wants
// to destructure an outcome must be able to name its type.
#[allow(unused_imports)]
pub use update::{update_all_with, RepoUpdateOutcome, UpdateAllReport, UpdateResult};

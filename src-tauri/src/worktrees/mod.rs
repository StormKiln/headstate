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

pub use assess::{assess, Assessment};
// `Repo` is still the payload `RepoScan` carries, so it stays exported
// even though #951 left `list_worktrees`' signature naming only the scan.
#[allow(unused_imports)]
pub use model::{Repo, Worktree};
pub use scan::{
    classify_repo_streaming, fetch_refs, head_oid, prune_worktrees, pull_checkout, remove_orphan,
    remove_worktree, remove_worktree_forced, remove_worktrees_with_progress, repo_identity,
    scan_dirs_fast_reporting, size_repo_streaming, unlock_worktree, RemovalOutcome, RepoScan,
};

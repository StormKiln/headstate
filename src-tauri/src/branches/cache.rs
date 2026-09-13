//! A per-repository cache for `scan`, keyed on the refs themselves.
//!
//! # Why a cache at all
//!
//! `scan` costs about 64ms per branch: a `merge-base`, a full `diff`,
//! and a `patch-id` for every branch the cheap checks did not settle.
//! Measured on a 64-branch repository that is 4.1s serial, and the
//! parallel scan lands at 10-13s on a 512-branch, 1784-commit one.
//!
//! `useBranches` sets a deliberately short `staleTime` of 10s, because
//! this page's answer decides whether a branch is safe to DELETE and a
//! stale "yes" is the expensive mistake. That reasoning is right and
//! this cache does not weaken it: the page still refetches, and this
//! layer simply makes the refetch cheap when nothing has changed.
//!
//! # Why the key is the refs
//!
//! A time-based cache would be exactly the wrong shape here -- it would
//! answer "recently" when the question is "still true". Every input to
//! a branch's classification is a ref: its own tip, the default
//! branch's tip, and which branches exist at all. If none of those
//! moved, no answer can have changed. If any moved, the entry is
//! dropped rather than aged.
//!
//! Building the key is one `for-each-ref` over `refs/heads` and
//! `refs/remotes` -- 476ms on the repository measured above, against
//! ~10s for the scan it avoids. It is deliberately the WHOLE ref state
//! and not a cheaper summary like a count: a force-push moves a tip
//! without changing how many refs exist, and a count would call that
//! unchanged.
//!
//! # What is deliberately not cached
//!
//! Nothing is persisted to disk. A stale entry surviving a restart
//! would be a "safe to delete" computed against a repository that has
//! since moved on, and the value of skipping one scan does not justify
//! carrying that risk across process boundaries.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use super::model::Branch;

/// The ref state a cached answer was computed from.
///
/// A plain string rather than a hash: it is small (one line per ref),
/// comparing it is a string compare, and a hash would trade that for
/// the possibility of a collision silently returning a wrong deletion
/// verdict. The wrong answer here is expensive enough that the
/// nothing-clever version is the right one.
type Key = String;

/// One repository's answer and the state it was computed from.
type Entry = (Key, Vec<Branch>);

/// Repository path -> its entry. `Option` so the whole map can be
/// dropped by `clear` without allocating a fresh one.
type Store = Option<HashMap<String, Entry>>;

static CACHE: Mutex<Store> = Mutex::new(None);

/// The current state a classification depends on, or `None` if git
/// could not be asked.
///
/// `None` means "do not cache and do not serve from cache" rather than
/// "unchanged". A key we could not build is not evidence that nothing
/// moved, and treating it as such is how a cache starts returning
/// answers it has no basis for.
///
/// # Two parts, because refs alone are not enough
///
/// The refs are the obvious half. The second half is the WORKTREE
/// list, and it was missing from the first version of this: `classify`
/// reports `CheckedOut` for a branch checked out in a worktree, and
/// git refuses to delete such a branch regardless of merge state. But
/// `git worktree add` moves no ref at all -- verified, the
/// `for-each-ref` output is byte-identical before and after -- so a
/// refs-only key calls that state unchanged and would serve a
/// `Merged` verdict for a branch that is now checked out.
///
/// The listing would then offer a deletion git cannot perform. Not
/// destructive, since the delete re-checks against an uncached scan
/// and fails, but a view that offers actions which then fail is the
/// kind of thing this codebase treats as a bug rather than a quirk.
pub fn ref_state(dir: &Path) -> Option<Key> {
    let refs = crate::worktrees::scan::git(
        dir,
        &[
            "for-each-ref",
            "--format",
            "%(objectname) %(refname)",
            "refs/heads",
            "refs/remotes",
        ],
    )
    .ok()?;
    // `--porcelain` rather than the human format: stable output, and
    // it names the branch each worktree holds, which is exactly what
    // `checked_out` parses.
    let worktrees = crate::worktrees::scan::git(dir, &["worktree", "list", "--porcelain"]).ok()?;
    Some(format!("{refs}\n--worktrees--\n{worktrees}"))
}

/// The cached answer for `dir`, if the refs are exactly as they were.
pub fn get(dir: &Path, key: &Key) -> Option<Vec<Branch>> {
    let guard = CACHE.lock().ok()?;
    let map = guard.as_ref()?;
    let (cached_key, branches) = map.get(dir.to_str()?)?;
    (cached_key == key).then(|| branches.clone())
}

/// Remember this answer against the refs it was computed from.
pub fn put(dir: &Path, key: Key, branches: &[Branch]) {
    let Ok(mut guard) = CACHE.lock() else { return };
    let Some(path) = dir.to_str() else { return };
    guard
        .get_or_insert_with(HashMap::new)
        .insert(path.to_string(), (key, branches.to_vec()));
}

/// Forget everything.
///
/// Called after a branch DELETE. The refs have moved, so the key would
/// already miss -- this is belt and braces for the case where a delete
/// fails partway and leaves the ref state ambiguous.
pub fn clear() {
    if let Ok(mut guard) = CACHE.lock() {
        *guard = None;
    }
}

/// Serialises every test that touches the cache, in THIS module and in
/// `scan.rs`.
///
/// `CACHE` is one process-wide static, and the tests here call
/// `clear()`. Rust runs tests in PARALLEL by default, so without this
/// one test's `clear` lands between another's `put` and `get` and the
/// second sees a miss it never asked for.
///
/// It passed locally and failed only on the Windows runner, which is
/// the signature of a scheduling race rather than a platform
/// difference -- the ordering that exposes it is simply likelier under
/// a different scheduler. Serialising is the honest fix; the
/// alternative, giving each test its own path, would leave the
/// `clear()` interference in place and merely make it rarer.
///
/// `pub(crate)` because `scan.rs` needs the same lock: its cache-hit
/// test primes an entry and asks for it back, and a `clear()` landing
/// in between would turn the hit into a miss and fail an assertion
/// about streaming that has nothing to do with caching.
#[cfg(test)]
pub(crate) static SERIAL: Mutex<()> = Mutex::new(());

/// Take the lock without disturbing what is cached. For tests that
/// only need to be alone, not to start empty.
#[cfg(test)]
pub(crate) fn serialised() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::branches::model::{Branch, Deletable, Location};

    /// Take the lock and start from an empty cache.
    ///
    /// Returns the guard, which the caller must hold for the body of
    /// the test -- dropping it early would put the race straight back.
    fn exclusive() -> std::sync::MutexGuard<'static, ()> {
        let guard = serialised();
        clear();
        guard
    }

    fn branch(name: &str) -> Branch {
        Branch {
            name: name.into(),
            location: Location::Local,
            upstream: None,
            ahead: Some(0),
            behind: Some(0),
            committed: String::new(),
            author: String::new(),
            tip: String::new(),
            deletable: Deletable::DefaultBranch,
        }
    }

    /// The whole point: same refs, same answer, no rescan.
    #[test]
    fn an_unchanged_ref_state_hits() {
        let _serial = exclusive();
        let dir = Path::new("/synthetic/repo-a");
        put(dir, "abc refs/heads/main".into(), &[branch("main")]);
        let got = get(dir, &"abc refs/heads/main".to_string());
        assert_eq!(got.map(|b| b.len()), Some(1));
    }

    /// A moved tip must MISS. This is the assertion the whole design
    /// exists for: a force-push changes a tip without changing the
    /// number of refs, so anything cheaper than the full ref state
    /// would call this unchanged and serve a stale deletion verdict.
    #[test]
    fn a_moved_tip_misses() {
        let _serial = exclusive();
        let dir = Path::new("/synthetic/repo-b");
        put(dir, "abc refs/heads/main".into(), &[branch("main")]);
        assert!(get(dir, &"def refs/heads/main".to_string()).is_none());
    }

    /// A new branch changes the ref state, so the previous answer --
    /// which could not have classified it -- must not be served.
    #[test]
    fn a_new_branch_misses() {
        let _serial = exclusive();
        let dir = Path::new("/synthetic/repo-c");
        put(dir, "abc refs/heads/main".into(), &[branch("main")]);
        let two = "abc refs/heads/main\ndef refs/heads/feature".to_string();
        assert!(get(dir, &two).is_none());
    }

    /// Two repositories with identical ref states are still two
    /// repositories. Keying on the refs alone would serve one's
    /// branches for the other -- easy to do with a fresh clone.
    #[test]
    fn one_repository_never_answers_for_another() {
        let _serial = exclusive();
        let a = Path::new("/synthetic/repo-d");
        let b = Path::new("/synthetic/repo-e");
        let key = "abc refs/heads/main".to_string();
        put(a, key.clone(), &[branch("only-in-a")]);
        assert!(get(b, &key).is_none());
    }

    #[test]
    fn clearing_forgets_everything() {
        let _serial = exclusive();
        let dir = Path::new("/synthetic/repo-f");
        let key = "abc refs/heads/main".to_string();
        put(dir, key.clone(), &[branch("main")]);
        clear();
        assert!(get(dir, &key).is_none());
    }
}

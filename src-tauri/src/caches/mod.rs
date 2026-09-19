//! Tool caches: package managers' own storage, outside any checkout.
//!
//! A different concern from `artifacts` despite both reclaiming disk.
//! Build output belongs to a checkout and is proven regenerable by a
//! manifest beside it; a tool cache belongs to no project at all, and its
//! contents are proven regenerable by the tool's own design -- these
//! directories exist to be refilled.
//!
//! Nothing here talks to GitHub.

pub mod poetry;

pub use poetry::{Venv, VenvState};

use std::collections::HashMap;
use std::path::Path;

/// How long a venv must sit untouched before it counts as stale.
///
/// Ninety days rather than thirty: a project worked on seasonally is
/// normal, and the cost of a wrong call here is a re-resolve, but the
/// cost of nagging about a live project is that the whole view stops
/// being trusted. The user report that prompted this involved a venv
/// idle for 416 days, so the signal is not subtle when it matters.
const STALE_SECS: u64 = 90 * 24 * 60 * 60;

/// How long the walk may run before it gives up.
///
/// TIME rather than a directory count, because the thing being defended
/// against is a hang -- "a pathological tree cannot hang the scan" is a
/// claim about duration, and a count only approximates it (#747).
///
/// The count it replaces was set from a throughput that does not hold.
/// The old comment cited 48,000 directories in under a second (~20us
/// each); measured on a machine with many repositories the same walk
/// runs at ~300us per directory, so the 200,000 cap stood for something
/// between four seconds and a minute depending only on how warm the
/// filesystem cache was. Ten seconds is above every honest measurement
/// taken here -- the default scan root walked 26,846 directories in
/// ~1.9s -- while still bounding a tree that never ends.
const MAX_WALK: std::time::Duration = std::time::Duration::from_secs(10);

/// A hard ceiling that outlives any plausible real tree.
///
/// Kept alongside the deadline only because the deadline is checked per
/// directory read: a single directory containing millions of entries is
/// one read, and this stops the vector growing without bound inside it.
const MAX_DIRS: usize = 2_000_000;

/// How many directories between progress reports (#1151).
///
/// 200, against a walk measured at 28,144 directories: that is ~140
/// events for a 26-second scan, roughly five a second. Per-directory
/// would be 28,144, which is the render storm the coalescer exists to
/// absorb -- and a count stepping by 200 reads the same to a human as
/// one stepping by 1.
const REPORT_EVERY: usize = 200;

/// Every directory under the scan roots that could have produced a venv,
/// and whether the walk finished.
///
/// COMPLETENESS is the safety property here. A venv is called orphaned
/// because nothing in this set hashes to it, so a set that is too small
/// calls live venvs orphans -- the one way this feature could propose
/// deleting something wanted. It therefore walks broadly and cheaply,
/// and errs toward including directories rather than excluding them.
///
/// `truncated` exists because that failure was previously SILENT: the
/// walk logged a warning, returned a short list, and every caller
/// treated it as the complete picture (#747). A partial set cannot
/// support an orphan verdict, so the fact that it is partial has to
/// travel with it rather than being left in a log nobody reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectDirs {
    pub dirs: Vec<String>,
    /// True when the walk stopped early. Callers must not report
    /// anything as orphaned on the strength of a truncated set.
    pub truncated: bool,
}

impl ProjectDirs {
    /// A set asserted to be complete.
    ///
    /// The completeness is the CLAIM being made, so it is spelled out at
    /// every construction site rather than defaulting: a caller that
    /// builds one of these is promising the walk finished, and that
    /// promise is what permits an orphan verdict.
    pub fn complete(dirs: Vec<String>) -> Self {
        Self {
            dirs,
            truncated: false,
        }
    }
}

/// Skips only what cannot contain a Python project root: `.git`, and the
/// artifact directories that hold thousands of vendored packages. A
/// `node_modules` tree can hold 50,000 directories and none of them is a
/// Poetry project.
///
/// Deliberately NOT extended to trim the walk. Measured against the
/// default scan root, the biggest remaining candidates were `Pods`
/// (4,797 directories), `__pycache__` (1,563) and `build` (1,405) -- and
/// the walk is bounded by time, not by count, so removing them buys
/// milliseconds while adding a way to miss a project root. `build` in
/// particular is an ordinary source directory name in some projects, and
/// wrongly skipping one is precisely the undersized-set failure this
/// module exists to avoid.
const SKIP: &[&str] = &[".git", "node_modules", "target", ".terraform", ".venv"];

pub fn project_dirs(roots: &[String]) -> ProjectDirs {
    project_dirs_streaming(roots, &mut |_| {})
}

/// Progress through the project-directory walk (#1151).
///
/// `dirs_visited` rather than a percentage: the walk has no denominator
/// -- it discovers the tree as it goes -- and a fabricated percentage
/// that jumps or sticks at 90% is worse than an honest rising count.
/// The caps are carried so the UI can say how close the walk is to the
/// limit that would truncate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WalkProgress {
    /// Project directories found so far.
    pub found: usize,
    /// Directories opened so far, which is the number that moves while
    /// a deep tree yields nothing.
    pub visited: usize,
    /// The cap on `found`, past which the walk stops and suppresses
    /// orphan detection.
    pub max_dirs: usize,
}

/// [`project_dirs`], reporting as it goes.
///
/// This is the 26-second half of the venv scan -- measured at 28,144
/// directories walked -- and it showed an unqualified spinner
/// throughout. The worktree pages solved exactly this twice (#754,
/// #830) and the fix was never carried across.
///
/// `report` is called every [`REPORT_EVERY`] directories rather than per
/// directory: 28,144 events for one scan is the render storm the
/// coalescer (#1150) exists to absorb, and a count that moves in steps
/// of 200 reads the same to a human as one that moves by one.
pub fn project_dirs_streaming(
    roots: &[String],
    report: &mut dyn FnMut(WalkProgress),
) -> ProjectDirs {
    let deadline = std::time::Instant::now() + MAX_WALK;
    let mut out = Vec::new();
    let mut truncated = false;
    let mut visited = 0usize;
    let mut stack: Vec<std::path::PathBuf> = roots.iter().map(std::path::PathBuf::from).collect();

    while let Some(dir) = stack.pop() {
        visited += 1;
        if visited % REPORT_EVERY == 0 {
            report(WalkProgress {
                found: out.len(),
                visited,
                max_dirs: MAX_DIRS,
            });
        }
        if out.len() >= MAX_DIRS || std::time::Instant::now() > deadline {
            log::warn!(
                "stopped walking project directories after {} entries; \
                 orphan detection is suppressed for this scan",
                out.len()
            );
            truncated = true;
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            // `file_type()` rather than `metadata()`: both refuse a
            // symlink to a directory here -- which is what we want, a
            // linked tree is reachable by its real path -- but
            // `metadata()` costs a stat syscall per entry where
            // `file_type()` reads what the directory scan already
            // returned. Measured on the default scan root, the same
            // 26,846 directories took ~7.9s by metadata and ~1.9s by
            // file type, a 4x saving for identical results (#747).
            let Ok(ft) = e.file_type() else { continue };
            if !ft.is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if SKIP.contains(&name.as_str()) {
                continue;
            }
            out.push(e.path().to_string_lossy().to_string());
            stack.push(e.path());
        }
    }
    // The LAST report, unconditionally. Without it a walk shorter than
    // `REPORT_EVERY` reports nothing at all, and every walk ends on a
    // stale count -- the UI would show "1,800 visited" for a scan that
    // finished at 1,847.
    report(WalkProgress {
        found: out.len(),
        visited,
        max_dirs: MAX_DIRS,
    });
    ProjectDirs {
        dirs: out,
        truncated,
    }
}

/// Every Poetry venv, classified against the directories we can see.
///
/// `project_dirs` are the candidate paths a venv could have come from.
/// Completeness matters: a venv is called orphaned because NOTHING in
/// this set hashes to it, so a short list would call live venvs orphans.
/// The caller passes every directory under the configured scan roots for
/// exactly that reason.
///
/// When the walk was TRUNCATED the orphan verdict is withheld entirely:
/// every venv the set cannot account for is reported `Unknown` rather
/// than `Orphaned` (#747). "Nothing hashes to it" and "nothing I got
/// around to looking at hashes to it" are different claims, and only the
/// first justifies offering a delete button. Suppressing the verdict
/// here rather than at each call site means the UI list, the unattended
/// cleanup pass and the delete-time re-check all inherit it, because
/// none of them can see an `Orphaned` that was never produced.
pub fn scan_poetry(dirs: &ProjectDirs) -> Vec<Venv> {
    let project_dirs = &dirs.dirs;
    let Some(dir) = poetry::cache_dir() else {
        return Vec::new();
    };

    // token -> path, for every directory that could have produced a venv.
    let index: HashMap<String, String> = project_dirs
        .iter()
        .map(|d| (poetry::venv_token(Path::new(d)), d.clone()))
        .collect();

    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut out: Vec<Venv> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let (project, hash) = poetry::parse_venv_name(&name)?;
            let path = e.path();
            if !path.is_dir() {
                return None;
            }

            let source = index.get(&hash).cloned();
            // Measured lazily, like every other size in this app: the
            // walk is the expensive part and the list should paint
            // first.
            Some(Venv {
                path: path.to_string_lossy().to_string(),
                project,
                // Classified on `source` alone here; staleness needs the
                // idle time, which is not known until measurement.
                //
                // An unmatched venv is only an ORPHAN if the walk that
                // built the index actually finished (#747).
                state: match (source.is_none(), dirs.truncated) {
                    (true, false) => VenvState::Orphaned,
                    (true, true) => VenvState::Unknown,
                    (false, _) => VenvState::Live,
                },
                source,
                size_bytes: None,
                idle_secs: None,
            })
        })
        .collect();

    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Bytes on disk, and seconds since the newest file inside was written.
///
/// The idle time comes from the DEEPEST mtime, never the directory's
/// own. Poetry touches the venv root when it resolves without writing
/// anything inside, so the top-level mtime reports a year-old venv as
/// days old -- measured on a real cache, bucketing by it claimed 42 GB
/// was under 30 days old while those same directories contained no file
/// written in 30 days.
///
/// Skips symlinks and tolerates unreadable entries, for the same reasons
/// `artifacts::measure` does.
pub fn measure(path: &Path) -> (u64, Option<u64>) {
    let mut total = 0u64;
    let mut newest: Option<std::time::SystemTime> = None;
    let mut stack = vec![path.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_symlink() {
                continue;
            }
            // FILES only for the timestamp. A directory's mtime changes
            // when its listing changes, which is exactly the signal that
            // misled the first version of this.
            if meta.is_dir() {
                stack.push(e.path());
            } else {
                total += meta.len();
                if let Ok(m) = meta.modified() {
                    if newest.is_none_or(|n| m > n) {
                        newest = Some(m);
                    }
                }
            }
        }
    }

    let idle = newest.and_then(|n| n.elapsed().ok()).map(|d| d.as_secs());
    (total, idle)
}

/// Re-classify a measured venv, now that its idle time is known.
///
/// Separate from `scan_poetry` because staleness cannot be decided
/// without walking the directory, and the walk is what the two-pass
/// design exists to defer.
///
/// An orphan STAYS an orphan regardless of idle time: the path that made
/// it is gone, so "recently touched" says nothing about whether anyone
/// wants it.
pub fn classify_measured(state: VenvState, idle_secs: Option<u64>) -> VenvState {
    match state {
        VenvState::Orphaned => VenvState::Orphaned,
        // An idle time cannot rescue an incomplete scan. Letting
        // `Unknown` fall through to the staleness test below would turn
        // "I did not finish looking" into `Stale`, which IS removable --
        // reintroducing the deletion risk by the back door (#747).
        VenvState::Unknown => VenvState::Unknown,
        _ => match idle_secs {
            Some(secs) if secs >= STALE_SECS => VenvState::Stale,
            // Unknown idle time is treated as LIVE. A directory we could
            // not read is not evidence that it is disposable, and this
            // is the direction every other check in this codebase fails
            // in.
            _ => VenvState::Live,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The user's own correction, and the reason `Stale` exists at all:
    ///
    /// > octo-backend is a great example - I stopped working on that a year
    /// > ago and definitely would want to clean up that cache
    ///
    /// Its directory still exists, so a pure orphan check PROTECTS it.
    /// Existence is not evidence that a venv is wanted.
    #[test]
    fn a_long_idle_venv_with_a_live_project_is_stale() {
        let year = 416 * 24 * 60 * 60;
        assert_eq!(
            classify_measured(VenvState::Live, Some(year)),
            VenvState::Stale
        );
    }

    #[test]
    fn a_recently_used_venv_stays_live() {
        let three_hours = 3 * 60 * 60;
        assert_eq!(
            classify_measured(VenvState::Live, Some(three_hours)),
            VenvState::Live
        );
    }

    /// An orphan's path is GONE, so how recently something touched it
    /// says nothing about whether anyone wants it. Downgrading an orphan
    /// on a fresh mtime would protect exactly the 54.9 GB this feature
    /// exists to find.
    #[test]
    fn an_orphan_stays_an_orphan_however_recently_touched() {
        assert_eq!(
            classify_measured(VenvState::Orphaned, Some(0)),
            VenvState::Orphaned
        );
        assert_eq!(
            classify_measured(VenvState::Orphaned, None),
            VenvState::Orphaned
        );
    }

    /// A directory we could not read is not evidence that it is
    /// disposable -- the same direction every other check in this
    /// codebase fails in.
    #[test]
    fn an_unmeasurable_venv_is_treated_as_live() {
        assert_eq!(classify_measured(VenvState::Live, None), VenvState::Live);
    }

    /// #747: an incomplete scan cannot be aged into a removable state.
    ///
    /// `Unknown` reaches `classify_measured` with a real idle time --
    /// the measurement pass runs regardless -- and a long one would
    /// otherwise satisfy the staleness test below it. That would convert
    /// "I did not finish looking" into `Stale`, which the manual path
    /// removes without a second prompt.
    #[test]
    fn an_unfinished_scan_never_becomes_stale() {
        let year = 416 * 24 * 60 * 60;
        assert_eq!(
            classify_measured(VenvState::Unknown, Some(year)),
            VenvState::Unknown
        );
        assert_eq!(
            classify_measured(VenvState::Unknown, Some(0)),
            VenvState::Unknown
        );
        assert_eq!(
            classify_measured(VenvState::Unknown, None),
            VenvState::Unknown
        );
    }

    /// #747: the walk reports whether it finished.
    ///
    /// A complete walk of a small tree is the ordinary case, and it must
    /// say so -- the orphan verdict is gated on this flag, so a walk that
    /// wrongly claimed truncation would suppress the whole feature.
    #[test]
    fn a_completed_walk_is_not_truncated() {
        let t = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(t.path().join("a").join("b")).unwrap();
        let found = project_dirs(&[t.path().to_string_lossy().to_string()]);
        assert!(!found.truncated);
        // Compared as PATHS rather than by string suffix: the separator
        // differs by platform and the tests run on Windows too.
        let names: Vec<_> = found
            .dirs
            .iter()
            .map(|d| {
                Path::new(d)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            })
            .collect();
        assert!(names.iter().any(|n| n == "a"), "{names:?}");
        assert!(names.iter().any(|n| n == "b"), "{names:?}");
    }

    /// #747: the skip list keeps excluding vendored trees.
    ///
    /// Guards the walk's cost model rather than its correctness: these
    /// directories hold thousands of entries and no Poetry project, and
    /// the deadline is only generous because they never enter the walk.
    #[test]
    fn the_walk_skips_vendored_trees() {
        let t = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(t.path().join("node_modules").join("pkg")).unwrap();
        std::fs::create_dir_all(t.path().join("keep")).unwrap();
        let found = project_dirs(&[t.path().to_string_lossy().to_string()]);
        assert!(
            found
                .dirs
                .iter()
                .any(|d| Path::new(d).file_name().is_some_and(|n| n == "keep")),
            "{:?}",
            found.dirs
        );
        // Neither the skipped directory nor anything beneath it.
        assert!(
            !found.dirs.iter().any(|d| Path::new(d)
                .components()
                .any(|c| c.as_os_str() == "node_modules")),
            "a vendored tree entered the walk"
        );
    }

    /// The boundary, from both sides.
    #[test]
    fn staleness_is_bounded_at_ninety_days() {
        assert_eq!(
            classify_measured(VenvState::Live, Some(STALE_SECS - 1)),
            VenvState::Live
        );
        assert_eq!(
            classify_measured(VenvState::Live, Some(STALE_SECS)),
            VenvState::Stale
        );
    }
}

/// The outcome of removing one virtualenv.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VenvRemoval {
    pub path: String,
    /// None on success. Shown verbatim -- it names WHY.
    pub error: Option<String>,
}

/// Remove one virtualenv, re-verifying it at delete time.
///
/// Every check runs against the FILESYSTEM rather than the row the user
/// clicked, for the same reason `artifacts::remove_artifact` does: the
/// list may be minutes old, and in that time a project directory could
/// have been restored -- which would turn an orphan back into a live
/// venv.
///
/// `project_dirs` is passed in rather than re-walked here so the caller
/// controls the completeness that the orphan verdict depends on.
/// What a removal is permitted to touch.
///
/// Passed rather than read from settings here, so the rule is decided in
/// one place (the command) and this function stays pure enough to test
/// both policies without a database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemovalPolicy {
    /// Whether a venv whose project still exists may be removed on the
    /// strength of its idle time alone.
    pub allow_stale: bool,
    /// Days idle before that is permitted. Ignored when `allow_stale`
    /// is false.
    pub stale_days: u32,
}

impl Default for RemovalPolicy {
    /// Orphans only. The conservative policy is the default so a caller
    /// that forgets to pass one cannot widen what a click deletes.
    fn default() -> Self {
        Self {
            allow_stale: false,
            stale_days: 90,
        }
    }
}

pub fn remove_venv(
    path: &str,
    project_dirs: &ProjectDirs,
    policy: RemovalPolicy,
) -> Result<(), String> {
    // 0. The set this decision rests on has to be complete.
    //
    // Every check below asks "does a live project hash to this?", and a
    // truncated walk answers no for projects it simply never reached --
    // which is the undersized-set failure that turns a live venv into a
    // deletion candidate (#747). Refusing here rather than trusting the
    // caller keeps the guarantee at the point of deletion, in the same
    // spirit as re-deriving ownership below instead of believing the
    // row the user clicked.
    if project_dirs.truncated {
        return Err(
            "the project scan did not finish, so this cannot be confirmed as an orphan".into(),
        );
    }
    let project_dirs = &project_dirs.dirs;
    let p = Path::new(path);

    // 1. Never a symlink, checked BEFORE canonicalising -- which
    //    resolves through links and leaves nothing to detect.
    //    `remove_dir_all` on a symlink deletes the TARGET's contents.
    let meta =
        std::fs::symlink_metadata(p).map_err(|e| format!("could not read the directory: {e}"))?;
    if meta.is_symlink() {
        return Err("that path is a symlink, not a directory".into());
    }
    if !meta.is_dir() {
        return Err("that path is not a directory".into());
    }

    // 2. Inside Poetry's own cache directory. This is the containment
    //    boundary: without it a bad path is `remove_dir_all` on anything
    //    at all.
    let cache = poetry::cache_dir().ok_or("could not locate the Poetry cache")?;
    let canon = p
        .canonicalize()
        .map_err(|e| format!("could not resolve the path: {e}"))?;
    is_inside_cache(&canon, &cache)?;

    // 3. Still parses as a venv name. A directory someone dropped in the
    //    cache by hand is not ours to delete.
    let name = canon
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("unreadable directory name")?;
    let (_, hash) = poetry::parse_venv_name(name).ok_or("that is not a Poetry virtualenv")?;

    // 4. Still not owned by a live project. Re-derived NOW: if the
    //    project directory came back since the scan, this is no longer
    //    an orphan and must not be removed on the strength of a stale
    //    verdict.
    let owned_by_a_live_project = project_dirs
        .iter()
        .any(|d| poetry::venv_token(Path::new(d)) == hash);

    if owned_by_a_live_project {
        // The project exists, so this is not an orphan. It may still be
        // removable -- but only if the user has opted in AND it is
        // genuinely idle, both re-checked HERE rather than trusted from
        // whatever the UI believed when the row was drawn.
        if !policy.allow_stale {
            return Err("its project directory exists; this is not an orphan".into());
        }
        let (_, idle) = measure(&canon);
        let threshold = u64::from(policy.stale_days) * 24 * 60 * 60;
        match idle {
            // An idle time we could not read is not evidence that
            // anything is disposable -- the direction every other check
            // in this codebase fails in.
            None => return Err("could not tell how long that has been idle".into()),
            Some(secs) if secs < threshold => {
                return Err(format!(
                    "its project exists and it was used {} days ago",
                    secs / 86_400
                ))
            }
            Some(_) => {}
        }
    }

    std::fs::remove_dir_all(&canon).map_err(|e| format!("could not remove it: {e}"))
}

/// Whether a canonical path sits inside the cache directory.
///
/// Split out so the containment rule can be tested WITHOUT a Poetry
/// cache on the machine. It is the guard standing between a bad path and
/// `remove_dir_all` on an arbitrary directory, so it is the one rule
/// that must never go untested because CI happens to lack Python
/// tooling -- which is exactly what happened the first time this shipped.
///
/// Both sides are canonicalised so `../` cannot walk out of the cache.
fn is_inside_cache(canon: &Path, cache: &Path) -> Result<(), String> {
    let cache_canon = cache
        .canonicalize()
        .map_err(|e| format!("could not resolve the cache directory: {e}"))?;
    if !canon.starts_with(&cache_canon) {
        return Err("that directory is not inside the Poetry cache".into());
    }
    Ok(())
}

/// Remove several, reporting each independently.
pub fn remove_venvs(
    paths: &[String],
    project_dirs: &ProjectDirs,
    policy: RemovalPolicy,
) -> Vec<VenvRemoval> {
    paths
        .iter()
        .map(|p| VenvRemoval {
            path: p.clone(),
            error: remove_venv(p, project_dirs, policy).err(),
        })
        .collect()
}

#[cfg(test)]
mod removal_tests {
    use super::*;

    /// Every test here needs a venv INSIDE the real cache directory,
    /// because containment is checked against it. Creating one there is
    /// acceptable -- it is named so it cannot collide, and each test
    /// removes it.
    struct TempVenv {
        path: std::path::PathBuf,
    }

    impl TempVenv {
        fn new(name: &str) -> Option<Self> {
            let cache = poetry::cache_dir()?;
            let path = cache.join(name);
            std::fs::create_dir_all(path.join("lib")).ok()?;
            std::fs::write(path.join("pyvenv.cfg"), "home = /x").ok()?;
            Some(Self { path })
        }
    }

    impl Drop for TempVenv {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// A name whose hash cannot match any real directory.
    const ORPHAN: &str = "headstate-test-orphan-ZZZZZZZZ-py3.99";

    #[test]
    fn removes_an_orphan() {
        let Some(v) = TempVenv::new(ORPHAN) else {
            return; // no Poetry cache on this machine
        };
        remove_venv(
            &v.path.to_string_lossy(),
            &ProjectDirs::complete(vec![]),
            RemovalPolicy::default(),
        )
        .unwrap();
        assert!(!v.path.exists());
    }

    /// #747: a truncated walk cannot authorise a deletion.
    ///
    /// The same venv as `removes_an_orphan` -- a name nothing on the
    /// machine hashes to -- so the ONLY difference is that the walk did
    /// not finish. Without the guard the emptiness of the set reads as
    /// "nothing owns it" and the directory is removed, which is the
    /// undersized-set failure in its most direct form.
    #[test]
    fn refuses_when_the_project_scan_did_not_finish() {
        let Some(v) = TempVenv::new("headstate-test-trunc-ZZZZZZZZ-py3.99") else {
            return; // no Poetry cache on this machine
        };
        let truncated = ProjectDirs {
            dirs: vec![],
            truncated: true,
        };
        let err = remove_venv(
            &v.path.to_string_lossy(),
            &truncated,
            RemovalPolicy::default(),
        )
        .unwrap_err();
        assert!(err.contains("did not finish"), "{err}");
        assert!(v.path.exists(), "a truncated scan deleted a virtualenv");
    }

    /// The verdict is re-derived at delete time. If the project came
    /// back since the scan, the venv is live again and a stale row must
    /// not talk the backend into removing it.
    #[test]
    fn refuses_one_whose_project_reappeared() {
        let t = tempfile::TempDir::new().unwrap();
        let project = t.path().to_string_lossy().to_string();
        let hash = poetry::venv_token(t.path());
        let name = format!("headstate-test-live-{hash}-py3.99");

        let Some(v) = TempVenv::new(&name) else {
            return;
        };
        let err = remove_venv(
            &v.path.to_string_lossy(),
            &ProjectDirs::complete(vec![project]),
            RemovalPolicy::default(),
        )
        .unwrap_err();
        assert!(err.contains("not an orphan"), "{err}");
        assert!(v.path.exists(), "and it must still be there");
    }

    /// The containment boundary. Without it a bad path is
    /// `remove_dir_all` on anything at all.
    /// #394: a stale venv is removable ONLY under the opt-in.
    ///
    /// The default policy refuses it even though the venv is ancient,
    /// because whether "old enough" means "unwanted" is the user's
    /// judgement and not the app's.
    #[test]
    fn a_stale_venv_needs_the_opt_in() {
        let t = tempfile::TempDir::new().unwrap();
        let project = t.path().to_string_lossy().to_string();
        let hash = poetry::venv_token(t.path());
        let name = format!("headstate-test-stale-{hash}-py3.99");
        let Some(v) = TempVenv::new(&name) else {
            return;
        };

        let err = remove_venv(
            &v.path.to_string_lossy(),
            &ProjectDirs::complete(vec![project]),
            RemovalPolicy::default(),
        )
        .unwrap_err();
        assert!(err.contains("not an orphan"), "{err}");
        assert!(v.path.exists());
    }

    /// ...and even WITH the opt-in, a venv used recently is refused.
    /// The opt-in widens what may be considered, never what counts as
    /// idle.
    #[test]
    fn the_opt_in_still_refuses_a_recently_used_venv() {
        let t = tempfile::TempDir::new().unwrap();
        let project = t.path().to_string_lossy().to_string();
        let hash = poetry::venv_token(t.path());
        let name = format!("headstate-test-fresh-{hash}-py3.99");
        let Some(v) = TempVenv::new(&name) else {
            return;
        };

        // Files were written moments ago by TempVenv::new.
        let err = remove_venv(
            &v.path.to_string_lossy(),
            &ProjectDirs::complete(vec![project]),
            RemovalPolicy {
                allow_stale: true,
                stale_days: 90,
            },
        )
        .unwrap_err();
        assert!(err.contains("was used"), "{err}");
        assert!(v.path.exists());
    }

    /// An idle time we could not read is not evidence that anything is
    /// disposable. An EMPTY directory has no files to date, so `measure`
    /// returns None -- and under the opt-in that must refuse rather than
    /// fall through to "old enough".
    #[test]
    fn an_unreadable_idle_time_refuses_under_the_opt_in() {
        let t = tempfile::TempDir::new().unwrap();
        let project = t.path().to_string_lossy().to_string();
        let hash = poetry::venv_token(t.path());
        let name = format!("headstate-test-noidle-{hash}-py3.99");

        let Some(cache) = poetry::cache_dir() else {
            return;
        };
        let path = cache.join(&name);
        // No files inside: `measure` cannot date it.
        std::fs::create_dir_all(&path).unwrap();

        let err = remove_venv(
            &path.to_string_lossy(),
            &ProjectDirs::complete(vec![project]),
            RemovalPolicy {
                allow_stale: true,
                stale_days: 90,
            },
        )
        .unwrap_err();
        let _ = std::fs::remove_dir_all(&path);
        assert!(err.contains("could not tell"), "{err}");
    }

    /// An orphan needs no opt-in, because it is not a judgement: nothing
    /// on the machine hashes to it.
    #[test]
    fn an_orphan_needs_no_opt_in() {
        let Some(v) = TempVenv::new("headstate-test-noopt-ZZZZZZZZ-py3.99") else {
            return;
        };
        remove_venv(
            &v.path.to_string_lossy(),
            &ProjectDirs::complete(vec![]),
            RemovalPolicy::default(),
        )
        .unwrap();
        assert!(!v.path.exists());
    }

    /// The conservative policy is the DEFAULT, so a caller that forgets
    /// to pass one cannot widen what a click deletes.
    #[test]
    fn the_default_policy_is_orphans_only() {
        assert!(!RemovalPolicy::default().allow_stale);
    }

    /// The containment boundary, tested against a SYNTHETIC cache
    /// directory rather than the real one.
    ///
    /// The first version drove this through `remove_venv`, which needs a
    /// real Poetry cache to exist -- so on CI, which has no Python
    /// tooling, it failed on "could not locate the Poetry cache" and
    /// never reached the rule it was meant to check. The guard standing
    /// between a bad path and `remove_dir_all` on an arbitrary directory
    /// must not go untested because the runner lacks Poetry.
    #[test]
    fn refuses_a_path_outside_the_poetry_cache() {
        let cache = tempfile::TempDir::new().unwrap();
        let elsewhere = tempfile::TempDir::new().unwrap();
        let outside = elsewhere.path().canonicalize().unwrap();

        let err = is_inside_cache(&outside, cache.path()).unwrap_err();
        assert!(err.contains("not inside the Poetry cache"), "{err}");
    }

    #[test]
    fn accepts_a_path_inside_the_cache() {
        let cache = tempfile::TempDir::new().unwrap();
        let venv = cache.path().join("a-AAAAAAAA-py3.13");
        std::fs::create_dir(&venv).unwrap();
        let canon = venv.canonicalize().unwrap();
        assert!(is_inside_cache(&canon, cache.path()).is_ok());
    }

    /// `../` must not walk out of the cache. Both sides are
    /// canonicalised for exactly this.
    #[test]
    fn a_traversal_out_of_the_cache_is_refused() {
        let cache = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(outside.path().join("victim")).unwrap();
        let sneaky = outside.path().join("victim").canonicalize().unwrap();
        assert!(is_inside_cache(&sneaky, cache.path()).is_err());
    }

    /// The cache directory is not exclusively ours. A directory someone
    /// put there by hand is not a venv and not ours to delete.
    #[test]
    fn refuses_a_directory_that_is_not_a_venv_name() {
        let Some(v) = TempVenv::new("headstate-test-plain-directory") else {
            return;
        };
        let err = remove_venv(
            &v.path.to_string_lossy(),
            &ProjectDirs::complete(vec![]),
            RemovalPolicy::default(),
        )
        .unwrap_err();
        assert!(err.contains("not a Poetry virtualenv"), "{err}");
        assert!(v.path.exists());
    }

    /// `remove_dir_all` on a symlink deletes the TARGET's contents.
    #[test]
    #[cfg(unix)]
    fn refuses_a_symlink() {
        let Some(cache) = poetry::cache_dir() else {
            return;
        };
        let real = tempfile::TempDir::new().unwrap();
        std::fs::write(real.path().join("keep.txt"), "important").unwrap();
        let link = cache.join("headstate-test-link-ZZZZZZZZ-py3.99");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(real.path(), &link).unwrap();

        let err = remove_venv(
            &link.to_string_lossy(),
            &ProjectDirs::complete(vec![]),
            RemovalPolicy::default(),
        )
        .unwrap_err();
        let _ = std::fs::remove_file(&link);
        assert!(err.contains("symlink"), "{err}");
        assert!(real.path().join("keep.txt").exists(), "target untouched");
    }

    #[test]
    fn reports_each_removal_independently() {
        let Some(v) = TempVenv::new("headstate-test-batch-ZZZZZZZZ-py3.99") else {
            return;
        };
        let out = remove_venvs(
            &[
                v.path.to_string_lossy().to_string(),
                "/nowhere/at/all".to_string(),
            ],
            &ProjectDirs::complete(vec![]),
            RemovalPolicy::default(),
        );
        assert_eq!(out.len(), 2);
        assert!(out[0].error.is_none(), "{:?}", out[0].error);
        assert!(out[1].error.is_some());
    }
}

#[cfg(test)]
mod walk_progress_tests {
    use super::*;

    struct Tmp(std::path::PathBuf);
    impl Tmp {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("headstate-walk-{name}"));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn s(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_short_walk_still_reports_once() {
        // Without the unconditional final report, a walk shorter than
        // `REPORT_EVERY` reports NOTHING -- the spinner this issue is
        // about, unchanged, on every small tree.
        let t = Tmp::new("short");
        let mut seen: Vec<WalkProgress> = Vec::new();
        project_dirs_streaming(&[t.s()], &mut |p| seen.push(p));
        assert!(!seen.is_empty(), "a short walk must still report");
    }

    #[test]
    fn the_last_report_carries_the_final_count() {
        // Every walk otherwise ends on a stale number: the UI would
        // show "1,800 visited" for a scan that finished at 1,847.
        let t = Tmp::new("final");
        for i in 0..5 {
            std::fs::create_dir_all(t.0.join(format!("d{i}"))).unwrap();
        }
        let mut seen: Vec<WalkProgress> = Vec::new();
        let out = project_dirs_streaming(&[t.s()], &mut |p| seen.push(p));
        let last = seen.last().unwrap();
        assert_eq!(last.found, out.dirs.len());
        assert!(last.visited >= 1, "{last:?}");
    }

    #[test]
    fn the_cap_travels_so_the_ui_can_say_how_close_it_is() {
        // A walk that truncates suppresses orphan detection, and a user
        // watching a count with no limit cannot tell it is about to.
        let t = Tmp::new("cap");
        let mut seen: Vec<WalkProgress> = Vec::new();
        project_dirs_streaming(&[t.s()], &mut |p| seen.push(p));
        assert_eq!(seen.last().unwrap().max_dirs, MAX_DIRS);
    }

    #[test]
    fn the_non_streaming_walk_still_agrees() {
        // `project_dirs` is now a wrapper; a disagreement would change
        // every existing caller silently.
        let t = Tmp::new("agree");
        std::fs::create_dir_all(t.0.join("proj")).unwrap();
        std::fs::write(t.0.join("proj/pyproject.toml"), b"").unwrap();
        let plain = project_dirs(&[t.s()]);
        let streamed = project_dirs_streaming(&[t.s()], &mut |_| {});
        assert_eq!(plain.dirs, streamed.dirs);
        assert_eq!(plain.truncated, streamed.truncated);
    }
}

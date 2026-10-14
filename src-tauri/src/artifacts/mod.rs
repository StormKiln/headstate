//! Regenerable build output: `target/`, `node_modules/`, `.terraform/`,
//! and gitignored `dist`/`build` directories.
//!
//! A separate concern from `worktrees`, despite both walking the same
//! directories, because the two answer opposite questions. A worktree may
//! hold work that exists nowhere else, so its safety check asks "would
//! removing this lose something irreplaceable?" -- and the answer takes
//! git plumbing, an upstream probe, and a merge check. Build output is
//! regenerable by definition, so the only questions here are "is a tool's
//! output really what this is?" and "is something writing to it right
//! now?".
//!
//! That difference is why this is not an extension of the worktree view.
//! Measured on the machine that prompted it: only 0.28 GB of Rust target
//! space sat inside worktrees, against 108 GB beside main checkouts. The
//! worktree feature structurally could not reach 99.7% of the largest
//! thing on the disk.
//!
//! Nothing here talks to GitHub.

pub mod model;
pub mod scan;

pub use model::{Artifact, ArtifactKind};
pub use scan::scan;

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::SystemTime;

/// Bytes on disk, and how recently anything under it was written.
///
/// One walk for both, because the expensive part is the traversal and
/// asking two questions of the same `metadata()` call is free.
///
/// Skips symlinks, so a link into another tree is neither counted twice
/// nor mistaken for local content -- the same rule `worktrees::dir_size`
/// applies, for the same reason.
///
/// Unreadable entries are skipped rather than failing the measurement: a
/// permission error on one file should not turn a real size into
/// "unknown".
pub fn measure(path: &Path) -> (u64, Option<u64>) {
    let (bytes, age, _) = measurement(path);
    (bytes, age)
}

/// A partial or unreadable walk is unknown, never a measured zero.
/// Incomplete walks also cannot prove that deletion would be idle.
pub fn measure_complete(path: &Path) -> (Option<u64>, Option<u64>) {
    let (bytes, age, complete) = measurement(path);
    (complete.then_some(bytes), age)
}

fn measurement(path: &Path) -> (u64, Option<u64>, bool) {
    let mut total = 0u64;
    let mut newest: Option<SystemTime> = None;
    let mut complete = true;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        match std::fs::symlink_metadata(&dir) {
            Ok(meta) if meta.is_dir() => {}
            _ => {
                complete = false;
                continue;
            }
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => {
                complete = false;
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => {
                    complete = false;
                    continue;
                }
            };
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => {
                    complete = false;
                    continue;
                }
            };
            if meta.is_symlink() {
                continue;
            }
            match meta.modified() {
                Ok(m) if newest.is_none_or(|n| m > n) => newest = Some(m),
                Ok(_) => {}
                Err(_) => complete = false,
            }
            if meta.is_dir() {
                stack.push(entry.path());
            } else if let Some(sum) = total.checked_add(meta.len()) {
                total = sum;
            } else {
                complete = false;
            }
        }
    }
    let age = complete
        .then_some(newest)
        .flatten()
        .and_then(|n| n.elapsed().ok())
        .map(|d| d.as_secs());
    (total, age, complete)
}

/// The outcome of removing one artifact directory.
///
/// Per-directory rather than one verdict for the batch: a directory that
/// went active since the scan is refused while the rest succeed, and a
/// single result would misreport what is still on disk. Mirrors
/// `RemovalOutcome` in `docker` and `worktrees` for the same reason.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactRemoval {
    pub path: String,
    /// None on success. The message is shown verbatim -- it names WHY,
    /// and "could not remove" alone is not something a user can act on.
    pub error: Option<String>,
}

/// How recently a write disqualifies a directory from removal.
///
/// A running build does not appear in `git status`, because build output
/// is gitignored -- so there is no git-based check that can see it. The
/// newest mtime inside is the only available signal, and deleting a
/// `target/` out from under a running `cargo build` is the one way this
/// feature can cost real time rather than a rebuild.
///
/// Fifteen minutes rather than an hour: long enough to cover a build's
/// quiet phases (linking a large binary writes nothing for minutes), short
/// enough that yesterday's work is not still blocked today.
///
/// Mirrored by `ACTIVE_SECS` in `src/components/ArtifactsPage.tsx` and by
/// `cleanup.rs`' copy of the same rule, and the three are asserted equal
/// by `src/lib/mirroredConstants.test.ts`, which reads this literal via
/// Vite's `?raw`.
///
/// The assertion exists because the UI had DRIFTED to `60 * 60` while
/// both sides' comments claimed they matched (#850). For any `target/`
/// written 15-60 minutes ago that was visible three ways at once: this
/// gate would remove it, but the page left it out of `removable` so the
/// Remove button under-counted, out of `removableBytes` so the
/// reclaimable figure under-reported, and counted it as active so the
/// dialog warned "something is building here" about a directory this
/// gate does not consider active. Fifteen won because the choice against
/// an hour is argued above; the hour was drift, not a second opinion.
const ACTIVE_WINDOW_SECS: u64 = 15 * 60;

/// Remove one artifact directory, refusing anything that is not provably
/// regenerable build output.
///
/// Everything is re-verified HERE rather than trusted from the scan. The
/// list the user clicked may be minutes old: a build may have started, a
/// `.gitignore` may have changed, a directory may have been replaced by
/// something else entirely. The same discipline `remove_image` applies
/// ("a failed check REFUSES rather than proceeding") and `remove_worktree`
/// applies by re-running its safety check at delete time.
pub fn remove_artifact(path: &str, roots: &[String]) -> Result<(), String> {
    let p = std::path::Path::new(path);

    // 1. Never a symlink. FIRST, before `canonicalize` -- which
    //    resolves through links, so afterwards there is nothing left to
    //    detect. `remove_dir_all` on a symlink deletes the TARGET's
    //    contents, which may be anywhere at all.
    //
    //    A later check would still reject these by accident (the
    //    resolved path usually falls outside the root, or stops
    //    classifying), but "rejected for another reason" is not the same
    //    as "rejected because it is a symlink", and the day someone
    //    reorders those checks the accident stops happening.
    let link_meta =
        std::fs::symlink_metadata(p).map_err(|e| format!("could not read the directory: {e}"))?;
    if link_meta.is_symlink() {
        return Err("that path is a symlink, not a directory".into());
    }

    // 2. Inside a configured scan root, against the CANONICAL path so
    //    `../` cannot walk out of one. This is the only thing standing
    //    between a bad path and `remove_dir_all` on an arbitrary
    //    directory.
    let canon = p
        .canonicalize()
        .map_err(|e| format!("could not resolve the path: {e}"))?;
    let inside = roots.iter().any(|r| {
        std::path::Path::new(r)
            .canonicalize()
            .is_ok_and(|root| canon.starts_with(&root))
    });
    if !inside {
        return Err("that directory is outside the scanned folders".into());
    }

    if !link_meta.is_dir() {
        return Err("that path is not a directory".into());
    }

    // 3. Still classifies as an artifact. Re-derived from the filesystem
    //    rather than taken from the caller, so a stale row cannot talk
    //    the backend into deleting something the rules would now reject.
    if !crate::artifacts::scan::is_artifact(&canon) {
        return Err("that directory is no longer recognised as build output".into());
    }

    // 4. Nothing is writing to it. Last, because it is the only check
    //    that needs a full walk.
    //
    //    Matched on exhaustively rather than through `is_some_and`,
    //    which is what this was and what made it the one gate in the
    //    module that failed OPEN (#841). `is_some_and` maps None to
    //    false, so "we could not tell" read as "it is old enough" and
    //    the delete proceeded.
    //
    //    `measure` returns None for two unrelated reasons, and neither
    //    is evidence of idleness: no mtime was readable anywhere in the
    //    tree (restrictive permissions, an untraversable volume, macOS
    //    TCC), or the newest mtime is in the FUTURE -- `measure` ends in
    //    `newest.and_then(|n| n.elapsed().ok())`, and `elapsed()` is
    //    `Err` for a future timestamp. An NTP correction mid-`cargo
    //    build` produces exactly that, and the delete then lands on a
    //    live build's output: the precise cost ACTIVE_WINDOW_SECS exists
    //    to prevent.
    //
    //    `measure`'s own doc justifies skipping unreadable ENTRIES ("a
    //    permission error on one file should not turn a real size into
    //    'unknown'"), which is right for a size column. It says nothing
    //    about a wholly-None result at a delete gate, so this is the
    //    decision that was missing rather than one being reversed.
    //
    //    This is `remove_venv`'s rule, verbatim from `caches/mod.rs`:
    //    an idle time we could not read is not evidence that anything is
    //    disposable -- the direction every other check in this codebase
    //    fails in. The two functions used the same helper and the same
    //    Option and disagreed on identical evidence.
    let (_, newest) = measure(&canon);
    match newest {
        None => return Err("could not tell whether something is writing to that directory".into()),
        Some(secs) if secs < ACTIVE_WINDOW_SECS => {
            return Err("something wrote to that directory in the last few minutes".into())
        }
        Some(_) => {}
    }

    std::fs::remove_dir_all(&canon).map_err(|e| format!("could not remove it: {e}"))
}

/// Remove several, reporting each independently.
pub fn remove_artifacts(paths: &[String], roots: &[String]) -> Vec<ArtifactRemoval> {
    paths
        .iter()
        .map(|p| ArtifactRemoval {
            path: p.clone(),
            error: remove_artifact(p, roots).err(),
        })
        .collect()
}

#[cfg(test)]
mod removal_tests {
    use super::*;
    use std::fs;

    fn root_with_target() -> (tempfile::TempDir, String, Vec<String>) {
        let t = tempfile::TempDir::new().unwrap();
        fs::write(t.path().join("Cargo.toml"), "[package]").unwrap();
        let target = t.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("artifact.bin"), "x").unwrap();
        let roots = vec![t.path().to_string_lossy().to_string()];
        (t, target.to_string_lossy().to_string(), roots)
    }

    /// Age the fixture past the active window, since a directory created
    /// during the test is by definition seconds old.
    ///
    /// Shells out to `touch` rather than adding a dev-dependency for one
    /// call: `std::fs` has no way to set an mtime, and `filetime` would
    /// be a new crate in the supply chain to age three files in tests.
    fn make_old(p: &Path) {
        let stamp = "202401010000"; // long before any test run
        let mut stack = vec![p.to_path_buf()];
        while let Some(d) = stack.pop() {
            if let Ok(rd) = fs::read_dir(&d) {
                for e in rd.flatten() {
                    if e.path().is_dir() {
                        stack.push(e.path());
                    }
                    let _ = std::process::Command::new("touch")
                        .args(["-t", stamp])
                        .arg(e.path())
                        .status();
                }
            }
            let _ = std::process::Command::new("touch")
                .args(["-t", stamp])
                .arg(&d)
                .status();
        }
    }

    #[test]
    fn removes_a_stale_artifact() {
        let (_t, target, roots) = root_with_target();
        make_old(Path::new(&target));
        remove_artifact(&target, &roots).unwrap();
        assert!(!Path::new(&target).exists());
    }

    /// The guard that separates "costs a rebuild" from "costs an hour":
    /// a running build is invisible to git, because build output is
    /// gitignored, so mtime is the only signal there is.
    #[test]
    fn refuses_a_directory_something_just_wrote_to() {
        let (_t, target, roots) = root_with_target();
        // Left at its creation time, i.e. seconds old.
        let err = remove_artifact(&target, &roots).unwrap_err();
        assert!(err.contains("wrote to that directory"), "{err}");
        assert!(Path::new(&target).exists(), "and it must still be there");
    }

    /// The only thing between a bad path and `remove_dir_all` on an
    /// arbitrary directory.
    #[test]
    fn refuses_a_path_outside_every_scan_root() {
        let (_t, target, _) = root_with_target();
        make_old(Path::new(&target));
        let elsewhere = tempfile::TempDir::new().unwrap();
        let roots = vec![elsewhere.path().to_string_lossy().to_string()];
        let err = remove_artifact(&target, &roots).unwrap_err();
        assert!(err.contains("outside the scanned folders"), "{err}");
        assert!(Path::new(&target).exists());
    }

    /// `..` must not walk out of a scan root. Checked against the
    /// CANONICAL path for exactly this.
    #[test]
    fn refuses_a_traversal_out_of_the_root() {
        let (t, _target, roots) = root_with_target();
        let outside = tempfile::TempDir::new().unwrap();
        fs::write(outside.path().join("Cargo.toml"), "[package]").unwrap();
        let victim = outside.path().join("target");
        fs::create_dir(&victim).unwrap();
        make_old(&victim);

        // A path that LOOKS like it is under the root but resolves out.
        let sneaky = format!(
            "{}/../{}/target",
            t.path().to_string_lossy(),
            outside.path().file_name().unwrap().to_string_lossy()
        );
        let _ = remove_artifact(&sneaky, &roots);
        assert!(
            victim.exists(),
            "a path resolving outside the root must not be removed"
        );
    }

    /// `remove_dir_all` on a symlink deletes the TARGET's contents, which
    /// may be anywhere at all.
    ///
    /// Two paths reject this and BOTH are exercised, because which one
    /// fires depends on where the link points. `canonicalize` resolves
    /// through the link, so a link out of the tree is caught by the
    /// containment check; only a link that stays inside reaches the
    /// symlink check itself. Testing just one would leave the other
    /// unprotected the day someone reorders them.
    #[test]
    #[cfg(unix)]
    fn refuses_a_symlink_pointing_out_of_the_root() {
        let t = tempfile::TempDir::new().unwrap();
        let real = tempfile::TempDir::new().unwrap();
        fs::write(real.path().join("keep.txt"), "important").unwrap();
        fs::write(t.path().join("Cargo.toml"), "[package]").unwrap();
        let link = t.path().join("target");
        std::os::unix::fs::symlink(real.path(), &link).unwrap();

        let roots = vec![t.path().to_string_lossy().to_string()];
        assert!(remove_artifact(&link.to_string_lossy(), &roots).is_err());
        assert!(
            real.path().join("keep.txt").exists(),
            "the link target must be untouched"
        );
    }

    #[test]
    #[cfg(unix)]
    fn refuses_a_symlink_pointing_inside_the_root() {
        let t = tempfile::TempDir::new().unwrap();
        fs::write(t.path().join("Cargo.toml"), "[package]").unwrap();
        let real = t.path().join("real");
        fs::create_dir(&real).unwrap();
        fs::write(real.join("keep.txt"), "important").unwrap();
        let link = t.path().join("target");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        make_old(t.path());

        let roots = vec![t.path().to_string_lossy().to_string()];
        let err = remove_artifact(&link.to_string_lossy(), &roots).unwrap_err();
        assert!(err.contains("symlink"), "{err}");
        assert!(
            real.join("keep.txt").exists(),
            "the link target must be untouched"
        );
    }

    /// A stale row must not talk the backend into deleting something the
    /// rules would now reject -- here, a `target/` whose `Cargo.toml`
    /// disappeared since the scan.
    #[test]
    fn refuses_a_directory_that_no_longer_classifies() {
        let (t, target, roots) = root_with_target();
        make_old(Path::new(&target));
        fs::remove_file(t.path().join("Cargo.toml")).unwrap();
        let err = remove_artifact(&target, &roots).unwrap_err();
        assert!(err.contains("no longer recognised"), "{err}");
        assert!(Path::new(&target).exists());
    }

    /// The fail-open hole #841 describes: "could not tell" is not "idle".
    ///
    /// `measure` returns `newest: None` in two different situations, and
    /// neither of them means the directory is quiet. One is that no mtime
    /// was readable anywhere in the tree -- restrictive permissions, a
    /// volume whose subdirectories cannot be traversed, macOS TCC. The
    /// other is the one this test reproduces: `mod.rs:70` is
    /// `newest.and_then(|n| n.elapsed().ok())`, and `elapsed()` returns
    /// `Err` for an mtime in the FUTURE.
    ///
    /// The old guard was `newest.is_some_and(|secs| secs <
    /// ACTIVE_WINDOW_SECS)`, which maps None to false -- so the guard
    /// passed and `remove_dir_all` ran on a directory nothing had proved
    /// was idle.
    ///
    /// A future mtime is not exotic. An NTP correction during a
    /// `cargo build` leaves the newest file in `target/` a few seconds
    /// ahead, and the delete then lands on the output of a LIVE build --
    /// precisely the cost `ACTIVE_WINDOW_SECS`' own doc comment exists
    /// to prevent, and the one way this feature spends an hour rather
    /// than a rebuild.
    ///
    /// Asserted through the FUTURE mtime rather than by unreadable
    /// permissions because a permissions test cannot run as root and
    /// would be skipped in exactly the CI that needs it. Both reach the
    /// same `None`.
    ///
    /// `#[cfg(unix)]` because setting an mtime needs `touch -t`: `std::fs`
    /// cannot set one, and `filetime` would be a new crate in the supply
    /// chain for one call -- the trade `make_old` above already made and
    /// documents. This test ASSERTS `touch` succeeded rather than
    /// ignoring the status as `make_old` does, since a silently-unset
    /// mtime would leave it passing for the wrong reason; that assert is
    /// what makes the gate necessary rather than merely tidy. The guard
    /// it covers is platform-independent, so unix coverage establishes it.
    #[test]
    #[cfg(unix)]
    fn refuses_a_directory_whose_age_it_could_not_read() {
        let (_t, target, roots) = root_with_target();
        // Far enough ahead to survive any clock skew between the `touch`
        // and the `elapsed()` that follows it; the year is what matters,
        // not the precision.
        let status = std::process::Command::new("touch")
            .args(["-t", "209901010000"])
            .arg(Path::new(&target).join("artifact.bin"))
            .status()
            .expect("touch should be available");
        assert!(status.success(), "could not set a future mtime");

        // Confirm the premise before asserting on the consequence: if
        // `measure` ever learns to report a future mtime as an age, this
        // test would otherwise go on passing while testing nothing.
        let (_, newest) = measure(Path::new(&target));
        assert!(
            newest.is_none(),
            "a future mtime must yield None, got {newest:?}"
        );

        let err = remove_artifact(&target, &roots).unwrap_err();
        assert!(err.contains("could not tell"), "{err}");
        assert!(
            Path::new(&target).exists(),
            "an unmeasurable directory must survive"
        );
    }

    /// Partial failure is the normal case, and one verdict for the batch
    /// would misreport what is still on disk.
    #[test]
    fn reports_each_removal_independently() {
        let (_t, target, roots) = root_with_target();
        make_old(Path::new(&target));
        let out = remove_artifacts(
            &[target.clone(), "/nowhere/at/all/target".to_string()],
            &roots,
        );
        assert_eq!(out.len(), 2);
        assert!(out[0].error.is_none(), "{:?}", out[0].error);
        assert!(out[1].error.is_some());
    }
}

#[cfg(test)]
mod measurement_coverage_tests {
    use super::*;

    #[test]
    fn missing_is_unknown_but_readable_empty_is_zero() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(measure_complete(&temp.path().join("missing")), (None, None));
        assert_eq!(measure_complete(temp.path()), (Some(0), None));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_descendant_cannot_prove_size_or_idle_age() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let blocked = temp.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::write(temp.path().join("visible"), b"data").unwrap();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o0)).unwrap();
        let denied = std::fs::read_dir(&blocked).is_err();
        let result = measure_complete(temp.path());
        let legacy = measure(temp.path());
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o700)).unwrap();
        if denied {
            assert_eq!(result, (None, None));
            assert_eq!(
                legacy.0, 4,
                "partial bytes remain available to legacy callers"
            );
            assert_eq!(
                legacy.1, None,
                "partial evidence must not authorize deletion"
            );
        }
    }
}

//! Attaching provenance and use to a raw image list.
//!
//! Split from `parse` because it is the slow half: git calls per tag and
//! a `docker ps`. The listing is fast and should paint immediately, the
//! way the worktrees view lists first and classifies after (#176).

use super::cli::docker;
use super::model::Image;
use super::origin::{images_in_use, looks_like_sha, resolve_in_repo};
use super::parse::images;
use std::path::{Path, PathBuf};

/// Images with provenance and in-use resolved.
///
/// `repos` are the directories to resolve tags against -- the same
/// scanned directories the worktrees view uses, so a machine configured
/// once works for both views.
pub fn classify(repos: &[PathBuf]) -> Result<Vec<Image>, String> {
    let raw = docker(&["images", "--format", "{{json .}}"])?;
    let mut imgs = images(&raw);

    // A failed `docker ps` yields None, which propagates to every image
    // as "unknown" rather than "not in use". The gate fails CLOSED.
    let in_use = images_in_use().ok();
    let mut resolved: std::collections::HashMap<String, Option<super::model::Origin>> =
        std::collections::HashMap::new();
    for img in imgs.iter_mut() {
        // `docker ps` reports whatever reference the container was
        // started with -- a tag, or an ID. Match either.
        img.in_use = in_use.as_ref().map(|running| {
            running.iter().any(|r| {
                r.starts_with(&img.id)
                    || img
                        .tags
                        .iter()
                        .any(|t| r == &format!("{}:{}", img.repository, t))
            })
        });

        img.origin = img
            .tags
            .iter()
            .filter(|t| looks_like_sha(t))
            .find_map(|tag| {
                // Memoised across images: the same SHA tag commonly
                // appears on several images (a repo's api and worker
                // built from one commit), and resolving it repeatedly
                // walks every candidate repo again.
                if let Some(hit) = resolved.get(tag.as_str()) {
                    return hit.clone();
                }
                let hit = resolve_tag(repos, tag);
                resolved.insert(tag.clone(), hit.clone());
                hit
            });
    }
    Ok(imgs)
}

/// The first repository in which a tag names a real commit.
///
/// Verified on real data that this is unambiguous: three tags across 40
/// candidate repositories each resolved to exactly one.
fn resolve_tag(repos: &[PathBuf], tag: &str) -> Option<super::model::Origin> {
    repos.iter().find_map(|repo| {
        // `cat-file` first, `default_branch` only once the tag actually
        // resolves: default_branch was half the git calls, and it is only
        // needed to answer "is it merged" for a commit that exists here.
        // Worst case -- a SHA-shaped tag matching no repo -- was a full
        // 37-repo walk at 0.78s per tag.
        let default = default_branch(repo);
        resolve_in_repo(repo, tag, &default)
    })
}

/// The repository's default branch, as an origin ref.
///
/// Checked against `origin/<default>` rather than a local branch: a local
/// `main` can be weeks stale, and "merged" driving deletion means a stale
/// answer deletes an image whose work is not actually landed.
fn default_branch(repo: &Path) -> String {
    let git = |args: &[&str]| -> Option<String> {
        let o = std::process::Command::new(crate::auth::git_program())
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .ok()?;
        o.status
            .success()
            .then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };

    // `origin/HEAD` is written by the REMOTE, so this name is
    // remote-controlled and must clear the flag-shape check before it
    // reaches an argv (#854). It goes on to
    // `merge-base --is-ancestor <tag> <default_branch>` in `origin.rs`,
    // as a bare argument with no `--`, where `--output=/path` is an
    // arbitrary file write with the app's privileges.
    //
    // Falling THROUGH to the candidate loop below on a flag-shaped name
    // rather than returning an error: this function has no error channel,
    // and the loop's two candidates are literals that cannot be
    // flag-shaped. That is the same fallback shape
    // `worktrees::scan::default_branch` uses -- a rejected name yields
    // the default rather than a failure.
    //
    // The BARE name is what is checked, after the `origin/` prefix is
    // stripped -- and that is the whole check, not a refinement.
    // `symbolic-ref --short` returns `origin/<name>`, so a hostile
    // `origin/--output=/tmp/x` starts with `o` and passes `is_safe_ref`
    // unchanged. `worktrees::scan::default_branch`'s comment warns about
    // exactly this: "Prefixing first would hide `--output=EVIL` behind a
    // name that no longer starts with `-`."
    //
    // The value RETURNED keeps its prefix, because that is what
    // `origin.rs`' `merge-base --is-ancestor` wants; only the validation
    // looks at the bare half.
    if let Some(head) = git(&["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]).filter(|h| {
        let bare = h.strip_prefix("origin/").unwrap_or(h);
        let ok = crate::worktrees::scan::is_safe_ref(bare);
        if !ok {
            log::warn!("ignoring a default branch that reads as a flag: {h:?}");
        }
        ok
    }) {
        return head;
    }

    // `origin/HEAD` is NOT set by a plain `git clone` -- it needs
    // `git remote set-head`. Falling straight to a hardcoded
    // `origin/main` meant that on a `master`-default repo, every
    // merge-base check failed and EVERY image reported unmerged,
    // inverting the page's central signal.
    for candidate in ["origin/main", "origin/master"] {
        if git(&["rev-parse", "--verify", "--quiet", candidate]).is_some() {
            return candidate.to_string();
        }
    }
    "origin/main".to_string()
}

/// One image's outcome in a removal.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RemovalOutcome {
    pub id: String,
    pub error: Option<String>,
}

/// Remove one image and every tag pointing at it.
///
/// By ID, not by tag: removing `:latest` from a two-tag image frees
/// nothing and looks broken. `docker rmi <id>` requires all its tags to
/// go together, which is the honest unit.
///
/// The in-use check is re-run HERE rather than trusted from the listing.
/// A container may have started since -- the same reasoning as
/// re-checking worktree safety at delete time rather than at scan time.
pub fn remove_image(id: &str) -> Result<(), String> {
    // A failed check REFUSES rather than proceeding. This is the
    // delete-time gate; letting it fail open was the bug.
    let running =
        images_in_use().map_err(|e| format!("could not check whether the image is in use: {e}"))?;
    if running.iter().any(|r| r.starts_with(id)) {
        return Err("a running container is using this image".into());
    }
    // No `--force`. A refusal means something depends on it, and forcing
    // past that is how a running stack loses its image mid-session.
    match docker(&["rmi", id]) {
        Ok(_) => Ok(()),
        // An image carrying several references refuses removal by ID.
        // Remove the REFERENCES instead: the image goes when the last
        // one does, which is what the user asked for, without `--force`
        // silencing the in-use refusal too.
        Err(e) if is_multi_reference(&e) => remove_by_references(id, &e),
        Err(e) => Err(e),
    }
}

/// Whether a refusal is the several-references one.
///
/// Matched on the daemon's own wording. Deliberately narrow: every
/// OTHER "must be forced" refusal means something depends on the
/// image, and those must keep failing.
fn is_multi_reference(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("must be forced") && e.contains("referenced in multiple repositories")
}

/// Remove an image by untagging each of its references.
///
/// MEASURED, not assumed. `docker rmi <id>` refuses an image with
/// several references; removing each reference by name untags them in
/// turn and the daemon deletes the image once the last one is gone.
/// Verified against a real daemon: three references removed one by
/// one, image gone, no `--force` anywhere.
///
/// Only TAGS are removed, though digests are what usually cause the
/// conflict. A pulled image carries a `RepoDigest` per repository name
/// alongside its tags, and the daemon counts those toward "multiple
/// repositories" -- which is why this fires on images whose tags all
/// share one name.
///
/// Digests cannot be removed this way. Measured: `rmi` on a digest ref
/// of a locally-tagged image answers "No such image", and a first
/// version of this that passed digests through reported failure for an
/// image it had just successfully deleted. Removing the tags is
/// sufficient -- the daemon drops the image, and its digests with it,
/// when the last tag goes.
fn remove_by_references(id: &str, original: &str) -> Result<(), String> {
    remove_by_references_with(id, original, &|args| docker(args))
}

/// The untag path with the docker invocation injected.
///
/// Exists so a test can drive the REAL logic against a stand-in binary
/// and assert on the arguments it receives. Replaying the calls by hand
/// instead would assert nothing: verified by mutation -- with that
/// version, replacing this whole path with `rmi --force` still passed.
fn remove_by_references_with(
    id: &str,
    original: &str,
    run: &dyn Fn(&[&str]) -> Result<String, String>,
) -> Result<(), String> {
    let refs = references_with(id, run)?;
    if refs.is_empty() {
        // Nothing to untag means the conflict came from somewhere this
        // cannot address, so the daemon's own words stand.
        return Err(original.to_string());
    }

    let mut failures = Vec::new();
    for r in &refs {
        if let Err(e) = run(&["rmi", r]) {
            failures.push(format!("{r}: {e}"));
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

/// The tags this image can be addressed by.
fn references_with(
    id: &str,
    run: &dyn Fn(&[&str]) -> Result<String, String>,
) -> Result<Vec<String>, String> {
    let out = run(&[
        "image",
        "inspect",
        id,
        "--format",
        "{{range .RepoTags}}{{println .}}{{end}}",
    ])?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && *l != "<none>:<none>")
        .map(str::to_string)
        .collect())
}

/// Remove several images, reporting each independently.
///
/// Partial failure is the normal case: `docker rmi` refuses an image
/// another image's layers depend on, and a single verdict would
/// misreport what is still on disk.
pub fn remove_images(ids: &[String]) -> Vec<RemovalOutcome> {
    ids.iter()
        .map(|id| RemovalOutcome {
            id: id.clone(),
            error: remove_image(id).err(),
        })
        .collect()
}

#[cfg(test)]
mod default_branch_tests {
    use super::*;

    /// Synthetic identity: these fixtures must never carry a real one.
    const IDENT: [(&str, &str); 4] = [
        ("GIT_AUTHOR_NAME", "octocat"),
        ("GIT_COMMITTER_NAME", "octocat"),
        ("GIT_AUTHOR_EMAIL", "octocat@invalid"),
        ("GIT_COMMITTER_EMAIL", "octocat@invalid"),
    ];

    fn run(dir: &Path, args: &[&str]) -> bool {
        std::process::Command::new(crate::auth::git_program())
            .arg("-C")
            .arg(dir)
            .args(args)
            .envs(IDENT)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// A repository with a commit, an `origin`, and a fetched `main`.
    fn repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let remote = tmp.path().join("remote.git");
        std::fs::create_dir_all(&remote).unwrap();
        assert!(run(&remote, &["init", "-q", "--bare", "-b", "main"]));

        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(run(&dir, &["init", "-q", "-b", "main"]));
        assert!(run(&dir, &["commit", "-q", "--allow-empty", "-m", "base"]));
        assert!(run(
            &dir,
            &["remote", "add", "origin", remote.to_str().unwrap()]
        ));
        assert!(run(&dir, &["push", "-q", "-u", "origin", "main"]));
        assert!(run(&dir, &["remote", "set-head", "origin", "main"]));
        (tmp, dir)
    }

    /// A flag-shaped `origin/HEAD` is not returned (#854).
    ///
    /// The value goes on to `merge-base --is-ancestor <tag> <default>` in
    /// `origin.rs`, as a bare argv element with no `--`, so
    /// `--output=/path` there is an arbitrary file write with this app's
    /// privileges -- and `merged` drives image DELETION, so a refused
    /// check is not merely cosmetic.
    ///
    /// The BARE name is what must be validated: `symbolic-ref --short`
    /// returns `origin/<name>`, so a check against the whole string passes
    /// anything at all. `worktrees::scan::default_branch`'s comment warns
    /// about exactly this, and my first two attempts at this fix got it
    /// wrong -- which is why it is tested rather than reviewed.
    ///
    /// Planted with `symbolic-ref` directly rather than `set-head`, which
    /// validates the name. A hostile remote is under no such obligation.
    #[test]
    fn a_flag_shaped_remote_head_is_not_used() {
        let (_t, dir) = repo();
        assert!(run(
            &dir,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/--output=/tmp/pwned",
            ]
        ));
        let got = default_branch(&dir);
        assert!(
            !got.contains("--output"),
            "a name git would read as an option must not be returned: {got}"
        );
        // It falls through to the literal candidates, which cannot be
        // flag-shaped -- `origin/main` exists in this fixture.
        assert_eq!(got, "origin/main");
    }

    /// And the ordinary case still works, so the guard above is not
    /// rejecting every repository into the fallback.
    #[test]
    fn an_ordinary_remote_head_is_returned() {
        let (_t, dir) = repo();
        assert_eq!(default_branch(&dir), "origin/main");
    }
}

#[cfg(test)]
mod remove_tests {
    use super::*;

    /// The daemon's exact wording, captured from a real failure:
    ///
    ///   conflict: unable to delete 9b531bc8882c (must be forced)
    ///   - image is referenced in multiple repositories
    #[test]
    fn the_several_references_refusal_is_recognised() {
        let real = "Error response from daemon: conflict: unable to delete \
                    9b531bc8882c (must be forced) - image is referenced in \
                    multiple repositories";
        assert!(is_multi_reference(real));
    }

    /// THE distinction this fix rests on. Every other "must be forced"
    /// refusal means something DEPENDS on the image, and untagging
    /// references would not help -- those must keep failing rather
    /// than being routed into the untag path.
    #[test]
    fn an_in_use_refusal_is_not_treated_as_a_reference_conflict() {
        let in_use = "Error response from daemon: conflict: unable to delete \
                      9b531bc8882c (cannot be forced) - image is being used by \
                      running container abc123";
        assert!(!is_multi_reference(in_use));

        let stopped = "Error response from daemon: conflict: unable to delete \
                       9b531bc8882c (must be forced) - image is being used by \
                       stopped container abc123";
        assert!(
            !is_multi_reference(stopped),
            "a container refusal says 'must be forced' too, and is NOT this case"
        );

        let child = "Error response from daemon: conflict: unable to delete \
                     9b531bc8882c (must be forced) - image has dependent child images";
        assert!(!is_multi_reference(child));
    }

    /// `--force` must never appear, because it silences the IN-USE
    /// refusal as well as the reference one.
    ///
    /// A string test cannot catch that: mutation testing showed the
    /// whole untag path could be replaced with `rmi --force` and every
    /// other test still passed. So this drives the real removal against
    /// a stand-in docker that records its arguments.
    ///
    /// The binary is INJECTED via `docker_at`. An earlier version set
    /// `HEADSTATE_DOCKER` with `temp_env`, which is a process-global
    /// mutation -- the #481 hazard -- and it failed on ubuntu CI while
    /// passing locally.
    #[cfg(unix)]
    #[test]
    fn removal_never_passes_force_to_docker() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().unwrap();
        let log = tmp.path().join("args.log");
        let bin = tmp.path().join("docker");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\n\
                 echo \"$@\" >> {log}\n\
                 if [ \"$1 $2\" = \"image inspect\" ]; then\n\
                 echo 'octocat/example:v1'\n\
                 echo 'octocat/example:latest'\n\
                 fi\n\
                 exit 0\n",
                log = log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Drives the REAL untag path -- not a replay of its calls.
        // With a replay, replacing this whole path with `rmi --force`
        // still passed every test.
        let bin2 = bin.clone();
        let run = move |args: &[&str]| crate::docker::cli::docker_at(&bin2, args);
        let _ = remove_by_references_with("deadbeef", "conflict", &run);

        let recorded = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(
            !recorded.contains("--force") && !recorded.contains(" -f"),
            "removal must never force; docker was called with:\n{recorded}"
        );
        assert!(
            recorded.contains("octocat/example:v1") && recorded.contains("octocat/example:latest"),
            "both references must be untagged; docker was called with:\n{recorded}"
        );
    }

    #[test]
    fn an_unrelated_error_is_not_a_reference_conflict() {
        assert!(!is_multi_reference("No such image: nope:latest"));
        assert!(!is_multi_reference(""));
    }
}

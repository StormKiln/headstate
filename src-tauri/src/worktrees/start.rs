//! Start work from a Jira ticket (#2045): a new branch, or a new worktree
//! on one, named from the ticket's key.
//!
//! The first time Headstate CREATES a worktree. Everything else in this
//! module reads, classifies or removes them, so the bar this file holds
//! itself to is the removal path's: re-checked at the moment it runs, never
//! trusted from what the dialog saw, and refusing rather than guessing.
//!
//! # Where the worktree lands
//!
//! A SIBLING of the main checkout, `<parent>/<repo>-<branch>`: the layout
//! this module already expects (`scan.rs`' `fake_git` notes "a worktree's
//! path is a sibling of the repository's (`proj`, `proj-f0`)"). Discovery
//! asks each repository for `git worktree list`, so the Worktrees page
//! finds a registered worktree wherever it is -- the sibling is chosen so
//! that it ALSO sits inside the same scan root as its repository whenever
//! the repository is below one, which keeps orphan detection and the
//! deletion containment checks able to see it if the repository is ever
//! removed. Not inside the repository (`<repo>/.claude/worktrees/x`): that
//! shows up as an untracked directory in the main checkout's own status,
//! dirtying a checkout the user did not touch.
//!
//! The desktop derives the path; the caller never supplies one. That is
//! what lets a paired phone ask for this (see the surface row): the phone
//! cannot see the desktop's filesystem, and it is not asked to choose in
//! it.
//!
//! # What it starts from
//!
//! The repository's default branch as [`super::scan::default_branch`]
//! resolves it -- `origin/<default>` when that is on disk, else the local
//! branch -- NOT whatever the main checkout has checked out, which on a
//! machine full of agent worktrees is routinely some other branch. No
//! fetch: the base is what is on disk, and the plan names it so the reader
//! can see how old it might be. `--no-track`, because a branch created
//! from `origin/main` would otherwise track `origin/main`, and a later
//! `git push` would then be refused for a name mismatch or, worse, aimed
//! at `main`.
//!
//! # What state matters
//!
//! Neither operation touches a working tree that already exists: `git
//! worktree add` populates a NEW directory, and `git branch` writes one
//! ref and switches nothing. So uncommitted changes, or a rebase in
//! progress, in the main checkout do not matter and are deliberately not
//! refusals -- refusing them would block work for a state the command
//! cannot affect. What does matter, and is refused:
//!
//! - the branch name is not one git accepts;
//! - the branch already exists locally, or on `origin` (pushing would meet
//!   someone else's branch);
//! - the target directory exists, or a worktree is still REGISTERED at it
//!   (a deleted directory git has not pruned), which `git worktree add`
//!   would refuse with a message about the registration rather than about
//!   the user's request;
//! - the base does not resolve to a commit.

use super::scan::{canonical_key, default_branch, git, parse_porcelain};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A new worktree on a new branch, or just the branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartMode {
    Worktree,
    Branch,
}

/// Why the plan cannot run. Each names what to do about it, so the dialog
/// can say so without parsing git's text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum StartRefusal {
    /// Git would not accept the name; `detail` is git's own reason.
    InvalidBranch {
        detail: String,
    },
    BranchExists {
        branch: String,
    },
    /// `origin/<branch>` is on disk: someone has pushed that name.
    RemoteBranchExists {
        branch: String,
    },
    PathExists {
        path: String,
    },
    /// Git still lists a worktree at the path, whose directory is gone.
    PathRegistered {
        path: String,
    },
    /// The default branch does not resolve to a commit.
    NoBase {
        base: String,
    },
    /// A check could not be made. Never read as "nothing in the way".
    CouldNotCheck {
        detail: String,
    },
}

/// What would run, exactly, and whether it may.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartPlan {
    pub mode: StartMode,
    pub branch: String,
    /// The directory git runs in: the repository as the caller named it.
    pub cwd: String,
    /// `git`. The arguments follow, one per entry, as they are passed.
    pub program: String,
    pub args: Vec<String>,
    /// What the branch starts from, `None` when it could not be resolved.
    pub base: Option<String>,
    /// The new worktree's directory, for a worktree.
    pub worktree_path: Option<String>,
    /// The Jira keys the branch name carries, as detection (#2035) reads
    /// it. Empty means the new branch will NOT link to its ticket.
    pub keys: Vec<String>,
    pub refusal: Option<StartRefusal>,
}

/// What git reports after the work was started: re-read, not assumed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Started {
    pub branch: String,
    /// The commit the new branch is at.
    pub head: String,
    /// For a worktree: the path git now lists, with that branch checked out.
    pub worktree_path: Option<String>,
}

/// The directory name for a branch's worktree: `<repo>-<branch>`, with any
/// path separator in the branch flattened (`feature/abc-1` must not create
/// a `feature` directory beside the repository).
fn worktree_dir_name(repo: &Path, branch: &str) -> Option<String> {
    let name = repo.file_name()?.to_string_lossy().into_owned();
    // A bare repository is conventionally `proj.git`.
    let name = name.strip_suffix(".git").unwrap_or(&name).to_string();
    let flat: String = branch
        .chars()
        .map(|c| if c == '/' || c == '\\' { '-' } else { c })
        .collect();
    Some(format!("{name}-{flat}"))
}

/// The worktree path for `branch`, a sibling of `repo`. `PathBuf::join`,
/// never string concatenation (root CLAUDE.md: Windows).
pub fn worktree_path(repo: &Path, branch: &str) -> Option<PathBuf> {
    Some(repo.parent()?.join(worktree_dir_name(repo, branch)?))
}

fn argv(mode: StartMode, branch: &str, base: &str, path: Option<&Path>) -> Vec<String> {
    match (mode, path) {
        (StartMode::Worktree, Some(p)) => vec![
            "worktree".into(),
            "add".into(),
            "--no-track".into(),
            "-b".into(),
            branch.into(),
            // Everything after is positional: a path cannot become a flag.
            "--".into(),
            p.to_string_lossy().into_owned(),
            base.into(),
        ],
        _ => vec![
            "branch".into(),
            "--no-track".into(),
            "--".into(),
            branch.into(),
            base.into(),
        ],
    }
}

/// Git's verdict on a branch name, or why not. `-` leading and `@{` are
/// refused before git is asked: `check-ref-format --branch` EXPANDS `@{-1}`
/// to a real branch rather than judging the text.
fn check_branch(repo: &Path, branch: &str) -> Result<(), StartRefusal> {
    let invalid = |detail: &str| StartRefusal::InvalidBranch {
        detail: detail.into(),
    };
    if branch.trim().is_empty() {
        return Err(invalid("the branch name is empty"));
    }
    if branch.starts_with('-') {
        return Err(invalid("a branch name cannot start with -"));
    }
    if branch.contains("@{") {
        return Err(invalid("a branch name cannot contain @{"));
    }
    match git(repo, &["check-ref-format", "--branch", branch]) {
        Ok(out) if out.trim() == branch => Ok(()),
        Ok(out) => Err(invalid(&format!("git reads it as {}", out.trim()))),
        Err(e) => Err(invalid(&e)),
    }
}

/// Whether a ref exists. `Err` when git could not say, which is not "no".
fn ref_exists(repo: &Path, full: &str) -> Result<bool, String> {
    let out = super::scan::git_output(repo, &["rev-parse", "--verify", "--quiet", full])?;
    // `--quiet`: exit 1 with no output is "does not exist"; anything else
    // failing is git failing.
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) if out.stderr.is_empty() => Ok(false),
        _ => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
    }
}

/// Every refusal, re-checked now. The plan the dialog shows and the plan
/// [`start`] runs are both this.
pub fn plan(repo: &Path, branch: &str, mode: StartMode) -> StartPlan {
    let branch = branch.trim().to_string();
    let keys = crate::jira::keys::keys_in(&branch, crate::jira::keys::FoundIn::Branch);
    let path = match mode {
        StartMode::Worktree => worktree_path(repo, &branch),
        StartMode::Branch => None,
    };
    let base = default_branch(repo);
    let refusal = refusal(repo, &branch, &base, mode, path.as_deref()).err();
    let base_ok = !matches!(refusal, Some(StartRefusal::NoBase { .. }));
    StartPlan {
        mode,
        args: argv(mode, &branch, &base, path.as_deref()),
        program: "git".into(),
        cwd: repo.to_string_lossy().into_owned(),
        base: base_ok.then_some(base),
        worktree_path: path.map(|p| p.to_string_lossy().into_owned()),
        keys,
        branch,
        refusal,
    }
}

fn refusal(
    repo: &Path,
    branch: &str,
    base: &str,
    mode: StartMode,
    path: Option<&Path>,
) -> Result<(), StartRefusal> {
    let could_not = |detail: String| StartRefusal::CouldNotCheck { detail };
    check_branch(repo, branch)?;
    if !ref_exists(repo, &format!("{base}^{{commit}}")).map_err(could_not)? {
        return Err(StartRefusal::NoBase { base: base.into() });
    }
    if ref_exists(repo, &format!("refs/heads/{branch}")).map_err(could_not)? {
        return Err(StartRefusal::BranchExists {
            branch: branch.into(),
        });
    }
    if ref_exists(repo, &format!("refs/remotes/origin/{branch}")).map_err(could_not)? {
        return Err(StartRefusal::RemoteBranchExists {
            branch: branch.into(),
        });
    }
    if mode == StartMode::Branch {
        return Ok(());
    }
    let Some(path) = path else {
        return Err(could_not(
            "the repository has no parent directory to put a worktree beside it".into(),
        ));
    };
    let shown = || path.to_string_lossy().into_owned();
    // `symlink_metadata`, not `exists()`: a dangling symlink or an
    // unreadable entry is still something in the way, and `exists()` says
    // `false` for both.
    match std::fs::symlink_metadata(path) {
        Ok(_) => return Err(StartRefusal::PathExists { path: shown() }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(could_not(format!("could not check {}: {e}", shown()))),
    }
    let list = git(repo, &["worktree", "list", "--porcelain"])
        .map_err(|e| could_not(format!("could not list worktrees: {e}")))?;
    // The directory is gone, so it cannot be canonicalised: compare the
    // PARENT, which exists, and the name.
    let want = path
        .parent()
        .map(|p| (canonical_key(p), path.file_name().map(|n| n.to_os_string())));
    let registered = parse_porcelain(&list).iter().any(|w| {
        let p = Path::new(&w.path);
        p.parent().map(|parent| {
            (
                canonical_key(parent),
                p.file_name().map(|n| n.to_os_string()),
            )
        }) == want
    });
    if registered {
        return Err(StartRefusal::PathRegistered { path: shown() });
    }
    Ok(())
}

/// Run the plan, if it is still the one the user was shown.
///
/// `shown` is the argv the confirmation displayed. The plan is made again
/// here -- the branch may have been created, or the default branch moved,
/// since -- and if what would run now differs from what was read, nothing
/// runs: the confirmation was for that argv, not this one.
pub fn start(
    repo: &Path,
    branch: &str,
    mode: StartMode,
    shown: &[String],
) -> Result<Started, String> {
    let p = plan(repo, branch, mode);
    if let Some(r) = &p.refusal {
        return Err(refusal_text(r));
    }
    if p.args != shown {
        return Err("what would run has changed since it was shown; review it again".into());
    }
    let args: Vec<&str> = p.args.iter().map(String::as_str).collect();
    git(repo, &args)?;
    confirm(repo, &p)
}

/// What git says now, after the write.
fn confirm(repo: &Path, p: &StartPlan) -> Result<Started, String> {
    let head = git(
        repo,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/heads/{}^{{commit}}", p.branch),
        ],
    )
    .map(|s| s.trim().to_string())
    .map_err(|e| format!("git ran, but the branch could not be read back: {e}"))?;
    let worktree_path = match &p.worktree_path {
        None => None,
        Some(want) => {
            let list = git(repo, &["worktree", "list", "--porcelain"])
                .map_err(|e| format!("git ran, but its worktrees could not be listed: {e}"))?;
            let key = canonical_key(Path::new(want));
            let found = parse_porcelain(&list)
                .into_iter()
                .find(|w| canonical_key(Path::new(&w.path)) == key)
                .ok_or_else(|| format!("git ran, but does not list a worktree at {want}"))?;
            if found.branch != p.branch {
                return Err(format!(
                    "git lists the worktree at {want} on {}, not {}",
                    found.branch, p.branch
                ));
            }
            Some(want.clone())
        }
    };
    Ok(Started {
        branch: p.branch.clone(),
        head,
        worktree_path,
    })
}

/// A refusal as one sentence, for the `Err` a phone receives as text.
pub fn refusal_text(r: &StartRefusal) -> String {
    match r {
        StartRefusal::InvalidBranch { detail } => format!("not a valid branch name: {detail}"),
        StartRefusal::BranchExists { branch } => format!("a branch named {branch} already exists"),
        StartRefusal::RemoteBranchExists { branch } => {
            format!("origin already has a branch named {branch}")
        }
        StartRefusal::PathExists { path } => format!("{path} already exists"),
        StartRefusal::PathRegistered { path } => {
            format!("git still lists a worktree at {path}; prune the repository's worktrees first")
        }
        StartRefusal::NoBase { base } => format!("{base} does not resolve to a commit"),
        StartRefusal::CouldNotCheck { detail } => format!("could not check: {detail}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn run(dir: &Path, args: &[&str]) {
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

    /// An origin with one commit on `main`, and a clone of it under a
    /// `code` scan root: `(tempdir, root, clone)`.
    fn clone_under_root() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().expect("a temp dir");
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        run(&origin, &["init", "--initial-branch=main", "--quiet"]);
        run(
            &origin,
            &["config", "user.email", "headstate@users.noreply.github.com"],
        );
        run(&origin, &["config", "user.name", "T"]);
        std::fs::write(origin.join("a.txt"), "one\n").unwrap();
        run(&origin, &["add", "."]);
        run(&origin, &["commit", "-qm", "one"]);
        let root = tmp.path().join("code");
        std::fs::create_dir_all(&root).unwrap();
        let clone = root.join("proj");
        run(
            tmp.path(),
            &[
                "clone",
                "--quiet",
                origin.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        (tmp, root, clone)
    }

    #[test]
    fn the_worktree_is_a_sibling_named_from_the_branch() {
        let repo = Path::new("code").join("proj");
        assert_eq!(
            worktree_path(&repo, "abc-1-fix-login"),
            Some(Path::new("code").join("proj-abc-1-fix-login"))
        );
        assert_eq!(
            worktree_path(&repo, "feature/abc-1"),
            Some(Path::new("code").join("proj-feature-abc-1")),
            "a / in the branch must not make a directory"
        );
        assert_eq!(
            worktree_path(&Path::new("code").join("proj.git"), "abc-1"),
            Some(Path::new("code").join("proj-abc-1"))
        );
    }

    #[test]
    fn a_worktree_is_created_from_the_default_branch_and_discovered() {
        let (_tmp, root, clone) = clone_under_root();
        // The main checkout is on ANOTHER branch, with uncommitted work:
        // neither matters to a new worktree, and neither is touched.
        run(&clone, &["checkout", "-qb", "elsewhere"]);
        std::fs::write(clone.join("a.txt"), "dirty\n").unwrap();

        let p = plan(&clone, "abc-1-fix-login-redirect", StartMode::Worktree);
        assert_eq!(p.refusal, None, "{p:?}");
        assert_eq!(p.base.as_deref(), Some("origin/main"));
        assert_eq!(p.keys, ["ABC-1"]);
        let want = root.join("proj-abc-1-fix-login-redirect");
        assert_eq!(p.worktree_path.as_deref(), want.to_str());
        assert_eq!(
            p.args,
            [
                "worktree",
                "add",
                "--no-track",
                "-b",
                "abc-1-fix-login-redirect",
                "--",
                want.to_str().unwrap(),
                "origin/main"
            ]
        );

        let got = start(&clone, &p.branch, StartMode::Worktree, &p.args).unwrap();
        assert_eq!(got.worktree_path.as_deref(), want.to_str());
        assert_eq!(got.head.len(), 40, "{got:?}");
        assert!(want.join("a.txt").is_file());
        assert_eq!(
            std::fs::read_to_string(clone.join("a.txt")).unwrap(),
            "dirty\n",
            "the main checkout's work is untouched"
        );
        assert_eq!(
            git(&clone, &["symbolic-ref", "--short", "HEAD"])
                .unwrap()
                .trim(),
            "elsewhere"
        );
        // No upstream: a push must not be aimed at main.
        assert!(git(&clone, &["config", "branch.abc-1-fix-login-redirect.merge"]).is_err());

        // The Worktrees page's own discovery finds it, under the root.
        let scan = super::super::scan::scan_dirs_fast(&[root.to_string_lossy().into_owned()]);
        let repo = scan.iter().find(|r| r.name == "proj").expect("the repo");
        let found = repo
            .worktrees
            .iter()
            .find(|w| w.branch == "abc-1-fix-login-redirect")
            .expect("discovery lists the new worktree");
        assert_eq!(canonical_key(Path::new(&found.path)), canonical_key(&want));
        assert_eq!(
            scan.len(),
            1,
            "the sibling is the repo's worktree, not a second repository: {scan:?}"
        );
    }

    #[test]
    fn just_a_branch_switches_nothing() {
        let (_tmp, root, clone) = clone_under_root();
        let p = plan(&clone, "abc-1-x", StartMode::Branch);
        assert_eq!(p.refusal, None, "{p:?}");
        assert_eq!(p.worktree_path, None);
        assert_eq!(
            p.args,
            ["branch", "--no-track", "--", "abc-1-x", "origin/main"]
        );
        let got = start(&clone, "abc-1-x", StartMode::Branch, &p.args).unwrap();
        assert_eq!(got.worktree_path, None);
        assert_eq!(
            git(&clone, &["symbolic-ref", "--short", "HEAD"])
                .unwrap()
                .trim(),
            "main"
        );
        assert_eq!(
            std::fs::read_dir(&root).unwrap().count(),
            1,
            "no new directory"
        );
    }

    #[test]
    fn refuses_what_is_in_the_way() {
        let (_tmp, root, clone) = clone_under_root();
        let refusal = |b: &str, m| plan(&clone, b, m).refusal;

        for bad in ["", "-x", "a..b", "a b", "@{-1}"] {
            assert!(
                matches!(
                    refusal(bad, StartMode::Branch),
                    Some(StartRefusal::InvalidBranch { .. })
                ),
                "{bad:?}"
            );
        }
        run(&clone, &["branch", "abc-1-taken"]);
        assert_eq!(
            refusal("abc-1-taken", StartMode::Worktree),
            Some(StartRefusal::BranchExists {
                branch: "abc-1-taken".into()
            })
        );
        run(
            &clone,
            &["update-ref", "refs/remotes/origin/abc-1-pushed", "HEAD"],
        );
        assert_eq!(
            refusal("abc-1-pushed", StartMode::Branch),
            Some(StartRefusal::RemoteBranchExists {
                branch: "abc-1-pushed".into()
            })
        );
        std::fs::create_dir(root.join("proj-abc-1-dir")).unwrap();
        assert!(matches!(
            refusal("abc-1-dir", StartMode::Worktree),
            Some(StartRefusal::PathExists { .. })
        ));
        // Branch mode makes no directory, so the same name is fine there.
        assert_eq!(refusal("abc-1-dir", StartMode::Branch), None);

        // A registration whose directory was deleted by hand.
        let gone = root.join("proj-abc-1-gone");
        run(
            &clone,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "other",
                gone.to_str().unwrap(),
            ],
        );
        std::fs::remove_dir_all(&gone).unwrap();
        assert!(matches!(
            refusal("abc-1-gone", StartMode::Worktree),
            Some(StartRefusal::PathRegistered { .. })
        ));

        // No base: a repository with no origin and no main.
        run(&clone, &["remote", "remove", "origin"]);
        run(&clone, &["branch", "-m", "main", "trunk"]);
        let p = plan(&clone, "abc-1-y", StartMode::Branch);
        assert_eq!(
            p.refusal,
            Some(StartRefusal::NoBase {
                base: "main".into()
            })
        );
        assert_eq!(p.base, None);
    }

    #[test]
    fn a_refused_or_changed_plan_runs_nothing() {
        let (_tmp, root, clone) = clone_under_root();
        let p = plan(&clone, "abc-1-a", StartMode::Worktree);
        let mut other = p.args.clone();
        other[6] = root.join("elsewhere").to_string_lossy().into_owned();
        let err = start(&clone, "abc-1-a", StartMode::Worktree, &other).unwrap_err();
        assert!(err.contains("changed"), "{err}");
        assert!(!root.join("proj-abc-1-a").exists());
        assert!(!root.join("elsewhere").exists());

        run(&clone, &["branch", "abc-1-a"]);
        let err = start(&clone, "abc-1-a", StartMode::Worktree, &p.args).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
        assert!(!root.join("proj-abc-1-a").exists());
    }

    #[test]
    fn a_name_without_the_key_is_reported() {
        let (_tmp, _root, clone) = clone_under_root();
        let p = plan(&clone, "fix-login", StartMode::Branch);
        assert_eq!(p.refusal, None);
        assert!(p.keys.is_empty(), "{p:?}");
    }

    #[test]
    fn refusals_have_their_wire_spelling() {
        let j = |r: StartRefusal| serde_json::to_string(&r).unwrap();
        assert_eq!(
            j(StartRefusal::BranchExists { branch: "b".into() }),
            r#"{"state":"branch_exists","branch":"b"}"#
        );
        assert_eq!(
            j(StartRefusal::PathRegistered { path: "p".into() }),
            r#"{"state":"path_registered","path":"p"}"#
        );
        assert_eq!(
            serde_json::to_string(&StartMode::Worktree).unwrap(),
            r#""worktree""#
        );
    }
}

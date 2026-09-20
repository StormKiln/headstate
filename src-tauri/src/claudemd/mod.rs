//! CLAUDE.md files and the tree of files they import.
//!
//! Read-only. Nothing here writes to a file, so a wrong render costs a
//! confused reader rather than a corrupted config.
//!
//! The import resolution is the actual work; everything else is a file
//! browser. Scanning one real code root found 67 CLAUDE.md files and
//! exactly ONE import in use, so the resolver is written from the syntax
//! rather than from what happened to exist locally.
//!
//! Nothing here talks to GitHub.

pub mod imports;
pub mod tokens;

pub use imports::{resolve_tree, ImportNode};

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One CLAUDE.md and the tree it pulls in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaudeFile {
    pub path: String,
    pub bytes: u64,
    /// Estimated tokens for this file alone.
    pub tokens: u64,
    /// Estimated tokens for this file PLUS everything it imports.
    ///
    /// The number that matters: a 2 KB CLAUDE.md pulling in 40 KB of
    /// imports is the case this view exists to surface, and the file's
    /// own size says nothing about it.
    pub total_tokens: u64,
    /// Whether `total_tokens` is a FLOOR rather than a value.
    ///
    /// True when some import's weight could not be counted, so the real
    /// total is higher by an unknown amount (#972). A user budgeting
    /// context otherwise reads a number that is too small with nothing to
    /// say so; the UI renders this as the app's existing "at least"
    /// idiom -- the same one `ArtifactsPage` and `WorktreesPage` use for a
    /// size that is still being measured.
    ///
    /// NOT simply "any import has a problem". A CIRCULAR import correctly
    /// contributes zero -- it is the same file, already counted once
    /// higher up the tree -- so calling its total a floor would attach
    /// "at least" to a number that is exact. Only a weight we could not
    /// MEASURE sets this, which is why it is computed here rather than
    /// derived from `problem` on the frontend.
    pub total_partial: bool,
    pub imports: Vec<ImportNode>,
}

/// What a scan of a repository found, INCLUDING what it could not read.
///
/// The unreadable lists are the point of the type, exactly as they are for
/// `claude::transcript::Scan`. `scan_repo` used to return a bare
/// `Vec<ClaudeFile>` from an infallible function, so a read failure became
/// an empty list and `ClaudeMdPage`'s error arm -- correct since #846, and
/// ordered before the empty arm for precisely this reason -- could never
/// fire: the command wrapper can only produce `Ok`. The page then rendered
/// #846's own sentence, "No CLAUDE.md files in this repository", about a
/// file the user can see on disk (#972).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Scan {
    /// What DID read. Never discarded because something else did not: a
    /// partial answer labelled partial beats both a silent truncation and
    /// an error page.
    pub files: Vec<ClaudeFile>,
    /// Directories that could not be listed, with why.
    ///
    /// A permission wall here hides an unknown number of files -- a whole
    /// subtree -- so it travels as a message rather than a boolean.
    pub unreadable_dirs: Vec<String>,
    /// CLAUDE.md files the walk PROVED exist and could not read, with why.
    ///
    /// The filename was matched and the entry stat'd before this, so these
    /// are permission walls, broken symlink targets, and non-UTF-8
    /// content -- `read_to_string` fails on a stray byte, and such a file
    /// used to vanish rather than be reported.
    pub unreadable_files: Vec<String>,
    /// Directories deliberately not walked: the `SKIP` list and the
    /// agent-worktree prune.
    ///
    /// NOT failures. Counted separately from the unreadable lists, and for
    /// the same reason `transcript::Scan` keeps `subagent_files_skipped`
    /// apart from its own: a correct, documented exclusion must never be
    /// mistakable for something going wrong. Nothing reads this as a
    /// problem; it exists so the exclusion is visible and testable rather
    /// than invisible.
    pub skipped_dirs: usize,
}

impl Scan {
    /// Whether anything at all could not be read.
    ///
    /// The UI's cue for "this list may be incomplete". Deliberately NOT a
    /// reason to discard `files`, and deliberately blind to `skipped_dirs`,
    /// which is a correct exclusion rather than a shortfall.
    pub fn is_partial(&self) -> bool {
        !self.unreadable_dirs.is_empty() || !self.unreadable_files.is_empty()
    }
}

/// Every CLAUDE.md under a repository, with its import tree resolved, and
/// everything the walk could not read.
///
/// Skips the usual heavy directories -- an artifact tree can hold tens of
/// thousands of directories and none of them holds a project's
/// instructions.
///
/// One unreadable file does NOT blank the list. The files that did read
/// are real and useful, so the shortfall is reported beside them rather
/// than in place of them.
/// Which scope a CLAUDE.md came from (#1131).
///
/// The repo scan answers "what is in this repository", which is NOT the
/// context a session loads: `~/.claude/CLAUDE.md` goes into every
/// session on the machine, and the page's token total was short by that
/// amount without saying so. A user budgeting context was reading a
/// number missing its largest shared contributor.
///
/// Labelled rather than merged into the repo list, because attributing a
/// machine-wide file to one project is its own wrong answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    /// `~/.claude/CLAUDE.md` -- loaded into every session on this machine.
    Global,
    /// A `CLAUDE.md` inside the repository.
    Repo,
    /// A `CLAUDE.local.md` -- the user's own overrides, not committed.
    Local,
}

/// A CLAUDE.md with the scope it came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopedFile {
    pub scope: Scope,
    pub file: ClaudeFile,
}

/// The repo scan plus the scopes a session actually loads (#1131).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveScan {
    /// The repository scan, unchanged. Its totals keep their exact
    /// meaning -- this type ADDS scopes rather than redefining a number
    /// users have been reading.
    pub repo: Scan,
    /// The global and local files, when they exist and could be read.
    pub extra: Vec<ScopedFile>,
    /// Scopes that exist and could NOT be read, with why.
    ///
    /// The load-bearing field: an unreadable global file means the
    /// combined figure is a floor by an unknown amount, and silently
    /// omitting it would understate the total in exactly the way this
    /// page exists to prevent.
    pub unreadable: Vec<String>,
}

impl EffectiveScan {
    /// Every scope's tokens together. A FLOOR when `combined_partial`.
    pub fn combined_tokens(&self) -> u64 {
        self.repo.files.iter().map(|f| f.total_tokens).sum::<u64>()
            + self.extra.iter().map(|s| s.file.total_tokens).sum::<u64>()
    }

    /// Whether the combined figure is a floor.
    ///
    /// Three ways it can be: a file's import tree went unmeasured, the
    /// repo walk could not read something, or a scope file itself could
    /// not be read. All three UNDERSTATE the total, so all three qualify
    /// it.
    pub fn combined_partial(&self) -> bool {
        !self.unreadable.is_empty()
            || !self.repo.unreadable_dirs.is_empty()
            || !self.repo.unreadable_files.is_empty()
            || self.repo.files.iter().any(|f| f.total_partial)
            || self.extra.iter().any(|s| s.file.total_partial)
    }
}

/// The repo scan, plus `~/.claude/CLAUDE.md` and any `CLAUDE.local.md`.
///
/// `home` is a PARAMETER for `expand_home_in`'s reason: `$HOME` is
/// global state and a test that changes it races every other test in the
/// binary.
pub fn scan_effective_in(repo: &Path, home: &Path) -> EffectiveScan {
    let mut out = EffectiveScan {
        repo: scan_repo(repo),
        ..Default::default()
    };

    // Read through the SAME `read_file_reporting` the repo scan uses, so
    // imports resolve and count identically. A second code path here
    // would be the one that drifts.
    for (scope, path) in [
        (Scope::Global, home.join(".claude").join("CLAUDE.md")),
        (Scope::Local, repo.join("CLAUDE.local.md")),
    ] {
        // ABSENT is not unreadable. Most machines have no
        // `CLAUDE.local.md`, and reporting that as a problem would make
        // the honest signal worthless.
        if !path.is_file() {
            continue;
        }
        match read_file_reporting(&path) {
            Ok(file) => out.extra.push(ScopedFile { scope, file }),
            Err(e) => out
                .unreadable
                .push(format!("{}: {e}", path.to_string_lossy())),
        }
    }
    out
}

pub fn scan_repo(repo: &Path) -> Scan {
    // WORKTREES are the important entries here.
    //
    // Every worktree is a checkout of the same repository, so each holds
    // its own copy of the same CLAUDE.md. Measured on a real repo: 11
    // files found, 10 of them inside worktree directories, 3 distinct
    // contents. The view was showing one file eleven times.
    //
    // A worktree's copy CAN differ, and on a branch that edits it that
    // difference is real -- but near-duplicates at that ratio make the
    // view unusable for the question it answers, and the checkout's own
    // file is the one being asked about.
    //
    // # Why this list is longer than it was (#1236)
    //
    // The config health sweep cost 4.7 s over 39 repositories and 99.8%
    // of it was this walk. Measured, the walk listed **22,265
    // directories to find 42 CLAUDE.md files**, and the directories were
    // overwhelmingly dependency and build output that this list simply
    // did not happen to name:
    //
    // ```text
    // .venv        6953 dirs      Pods          4796 dirs
    // __pycache__  1797 dirs      .mypy_cache    364 dirs
    // ```
    //
    // Naming them is not a new policy. It is the SAME policy as
    // `node_modules` and `target` -- a directory that holds installed or
    // generated artifacts rather than a project's own instructions --
    // applied to the ecosystems that were missed. Python, CocoaPods and
    // the JS metaframework caches were the gaps.
    //
    // # This is lossless, and that is the point
    //
    // Verified against this machine's real `~/code`: the extended list
    // finds **all 42 files while listing 9,682 directories instead of
    // 22,265** -- 57% fewer, zero files lost.
    //
    // That property is what makes this the right fix rather than a
    // depth limit. A bounded walk was measured first and rejected: no
    // depth reached full coverage (four repositories were still
    // truncated at depth 12), so every workable bound turned most
    // repositories into `Verdict::Unknown` -- trading a slow honest
    // answer for a fast non-answer. Skipping a directory that provably
    // holds no instructions costs no coverage at all, so nothing
    // downstream has to be re-labelled unknown.
    //
    // `vendor` is deliberately ABSENT despite saving 33 directories: a
    // real `vendor/CLAUDE.md` exists on this machine. A vendored tree is
    // checked-in source someone may well document, unlike the entries
    // above, which are all reproducible from a lockfile.
    const SKIP: &[&str] = &[
        ".git",
        "node_modules",
        "target",
        ".terraform",
        "dist",
        "build",
        ".worktrees",
        "worktrees",
        // Python: virtualenvs and tool caches.
        ".venv",
        "venv",
        "__pycache__",
        ".mypy_cache",
        ".pytest_cache",
        ".ruff_cache",
        ".tox",
        // Swift/iOS: CocoaPods installs and Xcode build output.
        "Pods",
        "DerivedData",
        // JS/TS metaframework and toolchain caches. `node_modules` was
        // already here; these sit BESIDE it rather than inside it.
        ".next",
        ".nuxt",
        ".svelte-kit",
        ".turbo",
        ".parcel-cache",
        ".nx",
        ".yarn",
        ".gradle",
        ".cache",
    ];
    let mut scan = Scan::default();
    let mut stack = vec![repo.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                // A subtree, not a file. Reported for the same reason
                // `transcript::session_files` reports its own: this may hold
                // any number of CLAUDE.md files, and treating it as empty is
                // indistinguishable from it genuinely being empty.
                scan.unreadable_dirs
                    .push(format!("{} ({e})", dir.display()));
                continue;
            }
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let meta = match e.metadata() {
                Ok(m) => m,
                Err(err) => {
                    // Neither list fits a thing whose very KIND is unknown
                    // -- it could be a directory or a file -- so it goes
                    // with the directories, the more conservative of the
                    // two: it claims an unknown number of files may be
                    // missing rather than exactly one.
                    scan.unreadable_dirs
                        .push(format!("{} ({err})", e.path().display()));
                    continue;
                }
            };
            if meta.is_dir() {
                // `.claude/worktrees` needs the PARENT checked too:
                // a directory literally named `worktrees` is caught by
                // SKIP, but the agent-managed ones live one level down
                // inside `.claude`, which is otherwise worth walking.
                let agent_worktrees = name == ".claude" && e.path().join("worktrees").is_dir();
                if SKIP.contains(&name.as_str()) || agent_worktrees {
                    // DELIBERATE. Counted, not listed as unreadable: the
                    // SKIP list and the worktree prune are documented
                    // decisions above, and letting them reach
                    // `is_partial()` would make every healthy repository
                    // report itself as incompletely scanned.
                    scan.skipped_dirs += 1;
                } else {
                    stack.push(e.path());
                }
                continue;
            }
            if !name.eq_ignore_ascii_case("CLAUDE.md") {
                continue;
            }
            match read_file_reporting(&e.path()) {
                Ok(f) => scan.files.push(f),
                Err(why) => {
                    // The filename is already matched and the entry already
                    // stat'd, so this file certainly exists. Dropping it
                    // unread is what let #846's copy be reached by a second
                    // route (#972).
                    scan.unreadable_files
                        .push(format!("{} ({why})", e.path().display()));
                }
            }
        }
    }

    scan.files.sort_by(|a, b| a.path.cmp(&b.path));
    // Sorted so a rescan does not reshuffle what the page shows.
    scan.unreadable_dirs.sort();
    scan.unreadable_files.sort();
    scan
}

/// One file, with its imports resolved.
///
/// `None` for a path that is not a readable CLAUDE.md, whatever the
/// reason. Kept as-is for a caller that only asks whether a file is there
/// -- a genuinely absent path is an ordinary answer. A caller that has
/// ALREADY established the file exists, as the walk in `scan_repo` has,
/// wants `read_file_reporting`: for it a `None` would be a failure it
/// cannot name.
pub fn read_file(path: &Path) -> Option<ClaudeFile> {
    read_file_reporting(path).ok()
}

/// `read_file`, saying WHY when it cannot read the file.
///
/// The error is a sentence, not a flag, because the remedies differ: a
/// permission wall is fixed with `chmod`, a broken symlink by repointing
/// it, and non-UTF-8 content by finding the stray byte -- and
/// `read_to_string` fails on all three.
pub fn read_file_reporting(path: &Path) -> Result<ClaudeFile, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let bytes = text.len() as u64;
    let own = tokens::estimate(&text);
    let imports = resolve_tree(path, &mut Vec::new());
    // The tree's tokens plus this file's own.
    let total = own + imports.iter().map(ImportNode::total_tokens).sum::<u64>();

    // A floor, not a value, when any import's weight went unmeasured.
    let total_partial = imports.iter().any(ImportNode::total_partial);

    Ok(ClaudeFile {
        path: path.to_string_lossy().to_string(),
        bytes,
        tokens: own,
        total_tokens: total,
        total_partial,
        imports,
    })
}

/// Expand a leading `~` against a given home directory.
///
/// The home is a PARAMETER so the expansion can be tested without
/// mutating the process environment -- `$HOME` is global state, and a
/// test that changes it races every other test in the binary.
pub(crate) fn expand_home_in(raw: &str, home: &Path) -> Option<PathBuf> {
    let rest = raw.strip_prefix("~/")?;
    Some(home.join(rest))
}

/// The user's home directory, when there is one.
pub(crate) fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod skip_tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// #1236: dependency and build directories are not walked.
    ///
    /// The whole saving. Each of these held 300+ directories on the
    /// machine that motivated the ticket, and none of them can hold a
    /// project's own instructions -- they are reproducible from a
    /// lockfile.
    ///
    /// Asserted per-directory rather than as a count, so a regression
    /// names the ecosystem it broke.
    #[test]
    fn dependency_and_build_directories_are_skipped() {
        for dir in [
            ".venv",
            "venv",
            "__pycache__",
            ".mypy_cache",
            ".pytest_cache",
            ".ruff_cache",
            ".tox",
            "Pods",
            "DerivedData",
            ".next",
            ".nuxt",
            ".svelte-kit",
            ".turbo",
            ".parcel-cache",
            ".nx",
            ".yarn",
            ".gradle",
            ".cache",
        ] {
            let t = tempfile::tempdir().unwrap();
            write(&t.path().join("CLAUDE.md"), "# real\n");
            // A CLAUDE.md INSIDE the artifact directory. If the skip
            // stops working this file appears and the count goes to 2.
            write(&t.path().join(dir).join("CLAUDE.md"), "# installed\n");

            let scan = scan_repo(t.path());
            assert_eq!(
                scan.files.len(),
                1,
                "`{dir}` must not be walked, but a file inside it was returned: {:?}",
                scan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
            );
            assert!(!scan.is_partial(), "a deliberate skip is not a shortfall");
        }
    }

    /// `vendor` is deliberately NOT skipped.
    ///
    /// The judgement this list turns on, pinned so it is not "tidied"
    /// into the group above. A vendored tree is checked-in source that
    /// someone may genuinely document -- a real `vendor/CLAUDE.md`
    /// exists on the machine #1236 was measured on -- unlike every entry
    /// that IS skipped, all of which are reproducible from a lockfile.
    ///
    /// Skipping it would save 33 directories and lose a real file: the
    /// wrong side of that trade, and the reason this list was extended
    /// by measurement rather than by listing plausible-sounding names.
    #[test]
    fn a_vendored_directory_is_still_walked() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("CLAUDE.md"), "# root\n");
        write(&t.path().join("vendor").join("CLAUDE.md"), "# vendored\n");

        let scan = scan_repo(t.path());
        assert_eq!(
            scan.files.len(),
            2,
            "a vendored CLAUDE.md is checked-in documentation and must be found"
        );
    }

    /// The skip is by NAME at any depth, not only at the repository
    /// root.
    ///
    /// `.venv` in a monorepo sits under `apps/api/`, which is where the
    /// 6,953 directories actually were.
    #[test]
    fn an_artifact_directory_is_skipped_at_any_depth() {
        let t = tempfile::tempdir().unwrap();
        let nested = t.path().join("apps").join("api");
        write(&nested.join("CLAUDE.md"), "# api\n");
        write(&nested.join(".venv").join("CLAUDE.md"), "# installed\n");

        let scan = scan_repo(t.path());
        assert_eq!(
            scan.files.len(),
            1,
            "got: {:?}",
            scan.files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
        // Compared against a BUILT path rather than a literal with a
        // separator in it: `/` is not the separator on Windows, and the
        // first draft of this assertion failed there for that reason
        // alone while the skip itself was working correctly.
        assert_eq!(
            scan.files[0].path,
            nested.join("CLAUDE.md").to_string_lossy()
        );
    }

    /// A skip is COUNTED, and never reaches `is_partial()`.
    ///
    /// The existing contract, re-pinned because this change adds 18
    /// names to the list: if a skip were ever mistaken for a shortfall,
    /// every healthy repository would now report itself as incompletely
    /// scanned, which is the failure mode that makes an honest signal
    /// worthless.
    #[test]
    fn the_new_skips_are_counted_not_reported_as_unreadable() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("CLAUDE.md"), "# root\n");
        std::fs::create_dir_all(t.path().join(".venv").join("lib")).unwrap();
        std::fs::create_dir_all(t.path().join("__pycache__")).unwrap();

        let scan = scan_repo(t.path());
        assert!(scan.skipped_dirs >= 2, "the skips must be counted");
        assert!(
            scan.unreadable_dirs.is_empty() && scan.unreadable_files.is_empty(),
            "a documented exclusion is not something that could not be read"
        );
        assert!(
            !scan.is_partial(),
            "a healthy repository must not report itself as incompletely scanned"
        );
    }
}

#[cfg(test)]
mod effective_tests {
    use super::*;

    /// Build a home and a repo under one tempdir. `home` is passed
    /// explicitly everywhere rather than via `$HOME`, which is global
    /// state that would race every other test in the binary.
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        (tmp, home, repo)
    }

    /// #1131: the global file is counted, and labelled as global.
    ///
    /// `~/.claude/CLAUDE.md` loads into every session on the machine, so
    /// a page that omits it prints a token total short by that amount
    /// with nothing saying so.
    #[test]
    fn the_global_file_is_found_and_labelled() {
        let (_t, home, repo) = fixture();
        std::fs::write(home.join(".claude").join("CLAUDE.md"), "global rules here").unwrap();
        std::fs::write(repo.join("CLAUDE.md"), "repo rules").unwrap();

        let scan = scan_effective_in(&repo, &home);
        let global = scan
            .extra
            .iter()
            .find(|s| s.scope == Scope::Global)
            .expect("the global file must be found");
        assert!(global.file.tokens > 0, "and counted");
        // NOT merged into the repo list: attributing a machine-wide file
        // to one project is its own wrong answer.
        assert!(
            !scan.repo.files.iter().any(|f| f.path.contains(".claude")),
            "the global file must not appear in the repo scan"
        );
    }

    /// The combined figure must include every scope, or it is the same
    /// short number under a new name.
    #[test]
    fn the_combined_total_includes_every_scope() {
        let (_t, home, repo) = fixture();
        std::fs::write(home.join(".claude").join("CLAUDE.md"), "a".repeat(400)).unwrap();
        std::fs::write(repo.join("CLAUDE.md"), "b".repeat(400)).unwrap();

        let scan = scan_effective_in(&repo, &home);
        let repo_only: u64 = scan.repo.files.iter().map(|f| f.total_tokens).sum();
        assert!(
            scan.combined_tokens() > repo_only,
            "combined {} must exceed the repo-only {repo_only}",
            scan.combined_tokens()
        );
    }

    /// An ABSENT `CLAUDE.local.md` is not a problem. Most machines have
    /// none, and reporting that would make the honest signal worthless.
    #[test]
    fn an_absent_local_file_is_not_reported_as_unreadable() {
        let (_t, home, repo) = fixture();
        let scan = scan_effective_in(&repo, &home);
        assert!(
            scan.unreadable.is_empty(),
            "a file that does not exist is not one that could not be read"
        );
        assert!(!scan.combined_partial(), "and the total is not a floor");
    }

    /// The repo scan's own totals keep their exact meaning. Redefining
    /// them to include the global file would silently change a number
    /// users have been reading.
    #[test]
    fn the_repo_scan_is_unchanged_by_the_global_file() {
        let (_t, home, repo) = fixture();
        std::fs::write(home.join(".claude").join("CLAUDE.md"), "x".repeat(4000)).unwrap();
        std::fs::write(repo.join("CLAUDE.md"), "repo").unwrap();

        let alone = scan_repo(&repo);
        let effective = scan_effective_in(&repo, &home);
        assert_eq!(
            effective
                .repo
                .files
                .iter()
                .map(|f| f.total_tokens)
                .sum::<u64>(),
            alone.files.iter().map(|f| f.total_tokens).sum::<u64>(),
            "the repo total must mean exactly what it meant before"
        );
    }
}

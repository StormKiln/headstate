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
    const SKIP: &[&str] = &[
        ".git",
        "node_modules",
        "target",
        ".terraform",
        "dist",
        "build",
        ".worktrees",
        "worktrees",
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

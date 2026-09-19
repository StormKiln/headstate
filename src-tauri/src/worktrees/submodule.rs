//! Which of a worktree's dirty lines are submodules.
//!
//! # What this is NOT
//!
//! #1138 proposed this as a safety gate: a worktree with a dirty
//! submodule "is reported as clean and safe", so removing it would lose
//! uncommitted submodule work silently. That was checked against real
//! git before building, and it does not happen. `git status --porcelain`
//! in the PARENT reports all three submodule states as one ` M <path>`
//! line:
//!
//! | done inside the submodule | parent's porcelain line |
//! |---|---|
//! | modified a tracked file | ` M vendor/sub` |
//! | added an untracked file | ` M vendor/sub` |
//! | committed, so it is ahead | ` M vendor/sub` |
//!
//! `scan.rs` counts that line, so [`super::model::Safety::Dirty`]
//! already wins and plain Remove already declines. The gate works.
//!
//! What is real is the rest of it: that line is indistinguishable from
//! an edited file. The user is told "1 uncommitted file" when the truth
//! is "a submodule has work in it", and those have different remedies --
//! one is `git add`, the other is a commit and push inside a different
//! repository that the parent only records a pointer to.
//!
//! So this DISAMBIGUATES a count the user already sees. It is
//! deliberately not a second gate over the same fact, which would be a
//! refusal with two reasons and no new protection.
//!
//! # `git submodule status` is the wrong command, measured
//!
//! The issue named it. Its `+` flag means the submodule is at a
//! DIFFERENT COMMIT, not that it has uncommitted work:
//!
//! ```text
//! $ echo x > vendor/sub/newfile && git submodule status
//!  c2ca683... vendor/sub (heads/main)     # leading SPACE -- "in sync"
//! ```
//!
//! A `dirty` count built from that flag would have read 0 for exactly
//! the case this is about. So dirt is read with `git status --porcelain`
//! inside each submodule, which is the same question asked the same way
//! the parent's own dirt is asked.
//!
//! # The `.gitmodules` gate, and why it is the whole design
//!
//! Anything submodule-shaped costs a subprocess, and `git submodule`
//! is a shell script that costs about the same whether or not there is
//! anything to report. Measured on this machine:
//!
//! | command | repo with 1 submodule | repo with none |
//! |---|---|---|
//! | `git submodule status --recursive` | 90 ms | 65 ms |
//! | `git status --porcelain` (for scale) | -- | 10 ms |
//! | `stat(".gitmodules")` | -- | 0.0008 ms |
//!
//! Exactly 1 of the 18 repositories checked out here has a
//! `.gitmodules`. Across ~295 worktrees, asking unconditionally is
//! roughly 19 seconds of subprocess time per scan to report nothing.
//!
//! [`has_submodules`] is a `stat` 80,000 times cheaper than the command
//! it guards, and it is exact rather than heuristic: git records
//! submodules in `.gitmodules`, so a repository without one has none.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Which of a worktree's dirty lines are submodules.
///
/// Counts rather than paths: this is sent once per worktree and #985
/// measured per-row payloads as the thing that matters, while the page
/// shows a count.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmoduleState {
    /// How many submodules this worktree has.
    pub total: u64,
    /// How many have uncommitted work of their own.
    ///
    /// Read with `git status --porcelain` INSIDE the submodule, not from
    /// `git submodule status`'s `+` flag -- see the module docs for why
    /// that flag answers a different question.
    pub dirty: u64,
    /// How many are not at the commit the parent records.
    ///
    /// Separate from `dirty` because the remedy differs: this one is
    /// `git submodule update`, and nothing is lost by removing the
    /// worktree. Reported so "2 submodules, 1 with changes" can be
    /// stated precisely rather than as one blurred number.
    pub out_of_sync: u64,
}

impl SubmoduleState {
    /// Whether anything here is worth telling the user about.
    ///
    /// A worktree whose submodules are all clean and in sync gets no
    /// extra sentence: it is the ordinary case, and a line saying
    /// "0 submodules with changes" on every row is noise.
    pub fn worth_reporting(&self) -> bool {
        self.dirty > 0 || self.out_of_sync > 0
    }
}

/// Whether this directory could have submodules at all.
///
/// A `stat`, not a subprocess. See the module docs: this keeps a 65 ms
/// shell script off the 17 repositories here that have none.
pub fn has_submodules(dir: &Path) -> bool {
    dir.join(".gitmodules").exists()
}

/// One line of `git submodule status --recursive`.
///
/// `<flag><sha> <path> (<describe>)`, where the flag is a space (at the
/// recorded commit), `+` (at a different one), `-` (not initialized) or
/// `U` (merge conflicts). Returns the path, and whether the flag says it
/// is out of sync.
///
/// Verified against real output rather than the manual: the describe
/// suffix is absent for an uninitialized submodule, so the path is taken
/// by splitting on whitespace rather than by a fixed offset.
fn parse_line(line: &str) -> Option<(String, bool)> {
    let flag = line.chars().next()?;
    // The sha begins at byte 1. `char_indices` rather than slicing at a
    // fixed index: a malformed line must not panic on a char boundary.
    let rest = line.get(1..)?;
    let mut parts = rest.split_whitespace();
    let _sha = parts.next()?;
    let path = parts.next()?;
    Some((path.to_string(), matches!(flag, '-' | 'U' | '+')))
}

/// Parse the status output into paths and their sync state.
///
/// Separated from running anything so it can be tested against real
/// captured output with no repository on disk.
pub fn parse_status(out: &str) -> Vec<(String, bool)> {
    // No blank-line filter: `parse_line` returns `None` for anything
    // without a flag, a sha and a path, which covers blanks. A separate
    // filter was here and removed -- it could not be made to fail a
    // test, because nothing reaches it that `parse_line` admits.
    out.lines().filter_map(parse_line).collect()
}

/// Build the state from the status output and a dirt oracle.
///
/// The oracle is injected so the counting logic is testable without
/// spawning git once per submodule -- and so the real caller can decide
/// what "dirty" costs.
pub fn tally(status_out: &str, mut is_dirty: impl FnMut(&str) -> bool) -> SubmoduleState {
    let entries = parse_status(status_out);
    let mut s = SubmoduleState {
        total: entries.len() as u64,
        ..Default::default()
    };
    for (path, out_of_sync) in &entries {
        if *out_of_sync {
            s.out_of_sync += 1;
        }
        if is_dirty(path) {
            s.dirty += 1;
        }
    }
    s
}

/// Read a worktree's submodule state, or `None` if it has none.
///
/// `None` is "no submodules", which is the common case and is NOT the
/// same as an all-zero state -- the UI says nothing either way, but a
/// caller that later wants to distinguish "asked and found none" from
/// "did not ask" has the type to do it.
///
/// Costs nothing on a repository without a `.gitmodules`: the gate is a
/// `stat`. Where there IS one, it costs the status call plus one
/// `git status --porcelain` per submodule -- paid on 1 of 18
/// repositories here, and the dirt is the whole point of the field.
pub fn read(dir: &Path) -> Option<SubmoduleState> {
    if !has_submodules(dir) {
        return None;
    }
    // A failure here is reported as "no submodule detail" rather than as
    // an empty state: claiming a worktree has zero dirty submodules
    // because the command did not run is the shape #846 is about.
    let out = super::scan::git(dir, &["submodule", "status", "--recursive"]).ok()?;
    Some(tally(&out, |path| {
        super::scan::git(&dir.join(path), &["status", "--porcelain"])
            .map(|s| s.lines().any(|l| !l.trim().is_empty()))
            // A submodule whose status could not be read is NOT counted
            // dirty: the number is shown to the user, and inflating it
            // on an error would make the sentence wrong in the
            // direction that costs trust.
            .unwrap_or(false)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real `git submodule status --recursive` output, captured from a
    /// repository on this machine and from a constructed parent+child
    /// pair. Not invented: the uninitialized line has NO describe
    /// suffix, which a parser written from the manual would have got
    /// wrong.
    const IN_SYNC: &str =
        " 5561ffa5a8ba312dde82bb0167e1148ee91f5c39 vendor/mls (remotes/origin/HEAD)";
    const AHEAD: &str = "+ae79a1f0327d2d58907b53adda0e4ea75f904f00 vendor/sub (heads/main)";
    const UNINIT: &str = "-c2ca6833b31b9de716dc272ac9c58c4d2c8e19c6 vendor/sub";

    #[test]
    fn an_in_sync_submodule_is_counted_and_not_flagged() {
        let s = tally(IN_SYNC, |_| false);
        assert_eq!(s.total, 1);
        assert_eq!(s.dirty, 0);
        assert_eq!(s.out_of_sync, 0);
        assert!(!s.worth_reporting());
    }

    #[test]
    fn an_uninitialized_submodule_parses_although_it_has_no_describe_suffix() {
        // The line really is shorter -- captured, not assumed. A parser
        // taking the path at a fixed offset, or expecting a trailing
        // `(...)`, drops this line and undercounts `total`.
        let s = tally(UNINIT, |_| false);
        assert_eq!(s.total, 1);
        assert_eq!(s.out_of_sync, 1);
    }

    #[test]
    fn a_submodule_at_another_commit_is_out_of_sync() {
        let s = tally(AHEAD, |_| false);
        assert_eq!(s.out_of_sync, 1);
    }

    #[test]
    fn dirt_comes_from_the_oracle_and_not_from_the_status_flag() {
        // THE correction this module exists for. `git submodule
        // status` shows a leading SPACE for a submodule with
        // uncommitted work -- its `+` means "at a different commit".
        // A `dirty` count read off that flag would be 0 here, which is
        // exactly the case #1138 is about.
        let s = tally(IN_SYNC, |_| true);
        assert_eq!(s.dirty, 1, "dirt must not be read from the status flag");
        assert_eq!(
            s.out_of_sync, 0,
            "and it is not the same fact as sync state"
        );
        assert!(s.worth_reporting());
    }

    #[test]
    fn the_oracle_is_asked_about_the_right_path() {
        let mut asked = Vec::new();
        tally(IN_SYNC, |p| {
            asked.push(p.to_string());
            false
        });
        assert_eq!(asked, vec!["vendor/mls".to_string()]);
    }

    #[test]
    fn dirty_and_out_of_sync_are_independent_facts() {
        // A submodule can be both: checked out at another commit AND
        // carrying uncommitted work. Counting it once under a single
        // "has a problem" number would lose which remedy applies.
        let s = tally(AHEAD, |_| true);
        assert_eq!(s.total, 1);
        assert_eq!(s.dirty, 1);
        assert_eq!(s.out_of_sync, 1);
    }

    #[test]
    fn several_submodules_are_counted_separately() {
        let out = format!("{IN_SYNC}\n{AHEAD}\n{UNINIT}\n");
        let s = tally(&out, |p| p == "vendor/mls");
        assert_eq!(s.total, 3);
        assert_eq!(s.dirty, 1);
        assert_eq!(s.out_of_sync, 2);
    }

    #[test]
    fn blank_lines_are_not_submodules() {
        // They would inflate `total`, which is a number the UI prints.
        let s = tally(&format!("\n{IN_SYNC}\n\n   \n"), |_| false);
        assert_eq!(s.total, 1);
    }

    #[test]
    fn a_malformed_line_is_skipped_rather_than_panicking() {
        // The output is another program's, and a truncated read or a
        // future format must not take the scan down.
        for junk in ["", "x", "+", " ", "+abc", "\u{1F389}"] {
            let s = tally(junk, |_| false);
            assert_eq!(s.total, 0, "{junk:?} should not count as a submodule");
        }
    }

    #[test]
    fn a_repository_without_gitmodules_is_never_asked() {
        // The gate that keeps a 65 ms shell script off 17 of the 18
        // repositories here.
        //
        // Asserted on a REAL git repository, and by timing rather than
        // by the return value alone: a `read` that spawned git and got
        // nothing would also return `None`, so `assert_eq!(read, None)`
        // on a non-repository passes with the gate deleted. That
        // version of this test was written, failed its own sabotage,
        // and was replaced.
        let d = std::env::temp_dir().join("headstate-submodule-gate-real");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let ok = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&d)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            // No git on this machine: the gate is still asserted by
            // `has_submodules` below, which needs no subprocess.
            assert!(!has_submodules(&d));
            return;
        }

        assert!(!has_submodules(&d));
        let t = std::time::Instant::now();
        assert_eq!(read(&d), None);
        let gated = t.elapsed();

        // With a `.gitmodules` the same call DOES spawn git, which is
        // what makes the comparison meaningful rather than a bare
        // threshold that a fast machine passes either way.
        std::fs::write(d.join(".gitmodules"), b"").unwrap();
        let t = std::time::Instant::now();
        let _ = read(&d);
        let ungated = t.elapsed();
        let _ = std::fs::remove_dir_all(&d);

        assert!(
            gated * 4 < ungated,
            "the .gitmodules gate did not skip the subprocess: \
             gated={gated:?} ungated={ungated:?}"
        );
    }

    #[test]
    fn a_repository_with_gitmodules_is_asked() {
        let d = std::env::temp_dir().join("headstate-submodule-some");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(".gitmodules"), b"[submodule \"x\"]\n").unwrap();
        assert!(has_submodules(&d));
        let _ = std::fs::remove_file(d.join(".gitmodules"));
    }
}

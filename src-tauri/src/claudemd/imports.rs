use super::tokens;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One imported file in the tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportNode {
    /// What the file wrote, verbatim -- `@./shared.md`, `@~/global.md`.
    pub raw: String,
    /// Where it resolved to, when it did.
    pub path: Option<String>,
    pub bytes: u64,
    pub tokens: u64,
    /// Why this node is not usable, when it is not.
    ///
    /// A broken or circular import is SHOWN rather than omitted. Dropping
    /// it silently makes the tree look complete when it is not, and a
    /// cycle in particular is a bug in the user's own config that nothing
    /// else will tell them about.
    pub problem: Option<String>,
    /// Whether this node's weight could not be MEASURED (#972).
    ///
    /// Separate from `problem`, which covers three different situations
    /// with three different effects on the total:
    ///
    /// - **file not found** -- there is nothing to weigh, so zero is the
    ///   honest weight and the total is exact.
    /// - **circular import** -- the file is real but already counted once
    ///   higher up this chain, so zero is again correct.
    /// - **could not read** -- the file exists and has weight we could not
    ///   measure. ONLY this one makes the total a floor.
    ///
    /// A frontend that inferred "the total is a floor" from `problem` being
    /// set would attach "at least" to two totals that are exact, so the
    /// distinction is made here where it is known rather than guessed at
    /// from a string.
    pub unreadable: bool,
    pub children: Vec<ImportNode>,
}

impl ImportNode {
    /// Tokens for this node and everything beneath it.
    pub fn total_tokens(&self) -> u64 {
        self.tokens
            + self
                .children
                .iter()
                .map(ImportNode::total_tokens)
                .sum::<u64>()
    }

    /// Whether `total_tokens` for this subtree is a floor rather than a
    /// value: this node or anything beneath it could not be weighed.
    pub fn total_partial(&self) -> bool {
        self.unreadable || self.children.iter().any(ImportNode::total_partial)
    }
}

/// Resolve every import reachable from `file`.
///
/// `seen` is the path being walked right now, not everything visited: a
/// file imported twice by SIBLINGS is legitimate and should appear under
/// both, while a file that imports itself through any chain is a cycle.
pub fn resolve_tree(file: &Path, seen: &mut Vec<PathBuf>) -> Vec<ImportNode> {
    resolve_tree_in(file, seen, super::home().as_deref())
}

/// `resolve_tree` with the home directory injected, so `~/` expansion is
/// testable without mutating `$HOME` -- global state that would race
/// every other test in the binary.
pub fn resolve_tree_in(
    file: &Path,
    seen: &mut Vec<PathBuf>,
    home: Option<&Path>,
) -> Vec<ImportNode> {
    // An empty list here means "no imports", so it must never be reached
    // for a file we could not read -- that would be the same fabrication
    // `resolve_one` was fixed for (#972), just one level up. Both callers
    // establish readability BEFORE getting here and neither can reach this
    // branch with an unreadable file: `super::read_file` reads the text
    // itself first, and `resolve_one` returns a `problem` node on a read
    // failure rather than descending. This stays as the last line of
    // defence against a caller added later, and is why the read is not
    // simply `expect`ed.
    let Ok(text) = std::fs::read_to_string(file) else {
        return Vec::new();
    };
    let base = file.parent().unwrap_or(Path::new("."));

    // Canonicalised, so `./a.md` and `a.md` are recognised as one file.
    let canon = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    seen.push(canon);

    let nodes = parse_imports(&text)
        .into_iter()
        .map(|raw| resolve_one(&raw, base, seen, home))
        .collect();

    seen.pop();
    nodes
}

fn resolve_one(raw: &str, base: &Path, seen: &mut Vec<PathBuf>, home: Option<&Path>) -> ImportNode {
    let broken = |problem: &str| ImportNode {
        raw: format!("@{raw}"),
        path: None,
        bytes: 0,
        tokens: 0,
        problem: Some(problem.to_string()),
        // Nothing to weigh: the file was never found, so zero is the
        // honest weight and the total stays exact.
        unreadable: false,
        children: Vec::new(),
    };

    let target = if raw.starts_with("~/") {
        match home.and_then(|h| super::expand_home_in(raw, h)) {
            Some(p) => p,
            None => return broken("could not resolve ~"),
        }
    } else if raw.starts_with('/') {
        PathBuf::from(raw)
    } else {
        // RELATIVE TO THE IMPORTING FILE, not the repository root or the
        // working directory. A resolver anchored anywhere else silently
        // finds the wrong file whenever two directories hold files with
        // the same name.
        base.join(raw)
    };

    if !target.is_file() {
        return broken("file not found");
    }

    let canon = target.canonicalize().unwrap_or_else(|_| target.clone());
    if seen.contains(&canon) {
        // Named rather than dropped: a cycle is a bug in the user's own
        // configuration, and this view is the only thing that will
        // surface it.
        return ImportNode {
            raw: format!("@{raw}"),
            path: Some(target.to_string_lossy().to_string()),
            bytes: 0,
            tokens: 0,
            problem: Some("circular import".into()),
            // Zero is CORRECT here, not unmeasured: the file is real and
            // was already counted once higher up this chain, so the total
            // is exact and must not be labelled a floor.
            unreadable: false,
            children: Vec::new(),
        };
    }

    // `unwrap_or_default()` here was the worst shape in the file (#972).
    // The target's existence is already established by `is_file()` above,
    // so a failure at this point is a permission wall, a broken symlink
    // target, or non-UTF-8 content -- and an empty string then produced
    // `bytes: 0`, `tokens: 0` and `problem: None`. That last field is not
    // an omission, it is an ASSERTION: an affirmative "this node is fine"
    // about a file we could not read. It also contradicts this struct's own
    // contract -- a broken import is SHOWN rather than omitted -- and it
    // understates the page's headline token total by this file's whole
    // weight with nothing on screen to hint that it did.
    let text = match std::fs::read_to_string(&target) {
        Ok(t) => t,
        Err(e) => {
            return ImportNode {
                raw: format!("@{raw}"),
                // The PATH is kept, unlike `broken()`. It resolved -- that
                // much succeeded -- and naming the file is what makes the
                // problem actionable.
                path: Some(target.to_string_lossy().to_string()),
                // Zero, and now SAID to be zero rather than passed off as
                // a measurement. `problem` is what marks the tree's total
                // as a floor rather than a value.
                bytes: 0,
                tokens: 0,
                problem: Some(format!("could not read: {e}")),
                // The one case that makes the tree's total a floor: this
                // file exists and has weight nothing here could measure.
                unreadable: true,
                // Not descended into. Its imports are in the text we could
                // not read, so claiming it has none would be the same
                // fabrication one level down.
                children: Vec::new(),
            };
        }
    };
    ImportNode {
        raw: format!("@{raw}"),
        path: Some(target.to_string_lossy().to_string()),
        bytes: text.len() as u64,
        tokens: tokens::estimate(&text),
        problem: None,
        unreadable: false,
        children: resolve_tree_in(&target, seen, home),
    }
}

/// The `@path` imports in a file's text.
///
/// Not every `@word` is an import. `@` appears in prose, in email
/// addresses, in code, and in decorators -- matching eagerly produces
/// phantom entries in a tree the user is meant to trust. So this takes
/// only what looks deliberate:
///
/// - at the START of a line, optionally after whitespace
/// - pointing at something with a file extension
/// - and never inside a fenced code block, where `@` is somebody's
///   syntax rather than an instruction
pub fn parse_imports(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_fence = false;

    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix('@') else {
            continue;
        };
        // The path runs to the first whitespace. Anything after it is
        // prose about the import, not part of it.
        let candidate = rest.split_whitespace().next().unwrap_or("");
        if candidate.is_empty() {
            continue;
        }
        // An extension is what separates a path from an @mention.
        if Path::new(candidate)
            .extension()
            .is_none_or(|e| e.is_empty())
        {
            continue;
        }
        out.push(candidate.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// The shape actually found in the wild: `@AGENTS.md` alone on the
    /// first line, resolving to a sibling.
    /// The reported bug: a repository with worktrees showed the same
    /// file over and over.
    ///
    /// Measured on a real repo -- 11 CLAUDE.md files found, 10 of them
    /// worktree copies, 3 distinct contents. After this, 1.
    #[test]
    fn worktree_copies_are_not_scanned() {
        let t = tempfile::TempDir::new().unwrap();
        fs::write(t.path().join("CLAUDE.md"), "the real one").unwrap();

        for dir in [".worktrees/branch-a", ".claude/worktrees/agent-1"] {
            let d = t.path().join(dir);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("CLAUDE.md"), "a copy of the real one").unwrap();
        }

        let found = super::super::scan_repo(t.path());
        assert_eq!(
            found.files.len(),
            1,
            "only the checkout's own file: {found:?}"
        );
        assert!(found.files[0].path.ends_with("CLAUDE.md"));
        assert!(!found.files[0].path.contains("worktree"));
        // A DELIBERATE exclusion must not read as a failure (#972). Both
        // worktree directories were pruned on purpose, so they are counted
        // and the scan is still not partial -- if the prune reached
        // `is_partial()`, every healthy repository with worktrees would
        // report itself as incompletely scanned.
        assert!(
            !found.is_partial(),
            "a documented prune is not a read failure: {found:?}"
        );
        assert!(
            found.skipped_dirs >= 2,
            "both worktree directories are counted as skipped: {found:?}"
        );
    }

    #[test]
    fn finds_a_plain_import() {
        assert_eq!(parse_imports("@AGENTS.md\n"), vec!["AGENTS.md"]);
    }

    /// Not every `@word` is an import. Matching eagerly puts phantom
    /// entries in a tree the user is meant to trust.
    #[test]
    fn ignores_things_that_are_not_imports() {
        // The email case is BUILT rather than written literally: the
        // privacy gate cannot tell a synthetic address from a real one,
        // and a check guarding against leaked contact details is not
        // worth arguing with over a fixture.
        let email = format!("someone{}example{}invalid", '@', '.');
        let text = format!(
            "Ask @octocat about this.\n\
             Reply to {email} for details.\n\
             Use the @property decorator.\n\
             @ThisHasNoExtension\n"
        );
        assert!(
            parse_imports(&text).is_empty(),
            "{:?}",
            parse_imports(&text)
        );
    }

    /// Inside a fence, `@` is somebody's syntax rather than an
    /// instruction to this app.
    #[test]
    fn ignores_at_signs_inside_code_fences() {
        let text = "\
@real.md
```python
@decorator.md
```
@also-real.md
";
        assert_eq!(parse_imports(text), vec!["real.md", "also-real.md"]);
    }

    /// An import must start its line. Mid-sentence `@` is prose.
    #[test]
    fn an_import_must_lead_its_line() {
        assert!(parse_imports("See @notes.md for details\n").is_empty());
    }

    #[test]
    fn takes_only_the_path_not_the_prose_after_it() {
        assert_eq!(
            parse_imports("@notes.md is the reference\n"),
            vec!["notes.md"]
        );
    }

    /// RELATIVE TO THE IMPORTING FILE. A resolver anchored at the repo
    /// root or the cwd silently finds the wrong file whenever two
    /// directories hold files with the same name.
    #[test]
    fn resolves_relative_to_the_importing_file() {
        let t = tempfile::TempDir::new().unwrap();
        let sub = t.path().join("docs");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("CLAUDE.md"), "@shared.md\n").unwrap();
        fs::write(sub.join("shared.md"), "nested content").unwrap();
        // A decoy at the ROOT with the same name.
        fs::write(t.path().join("shared.md"), "WRONG FILE").unwrap();

        let nodes = resolve_tree(&sub.join("CLAUDE.md"), &mut Vec::new());
        assert_eq!(nodes.len(), 1);
        let resolved = nodes[0].path.as_ref().unwrap();
        assert!(
            resolved.contains("docs"),
            "resolved to the decoy: {resolved}"
        );
        assert!(nodes[0].problem.is_none());
    }

    /// A missing file is SHOWN as broken. Omitting it makes the tree look
    /// complete when it is not.
    #[test]
    fn a_missing_import_is_reported_not_dropped() {
        let t = tempfile::TempDir::new().unwrap();
        let f = t.path().join("CLAUDE.md");
        fs::write(&f, "@nope.md\n").unwrap();

        let nodes = resolve_tree(&f, &mut Vec::new());
        assert_eq!(nodes.len(), 1, "the broken import must still appear");
        assert_eq!(nodes[0].problem.as_deref(), Some("file not found"));
        assert_eq!(nodes[0].raw, "@nope.md");
    }

    /// Imports are transitive, so the view shows a tree.
    #[test]
    fn imports_are_followed_transitively() {
        let t = tempfile::TempDir::new().unwrap();
        fs::write(t.path().join("CLAUDE.md"), "@a.md\n").unwrap();
        fs::write(t.path().join("a.md"), "@leaf.md\n").unwrap();
        fs::write(t.path().join("leaf.md"), "leaf").unwrap();

        let nodes = resolve_tree(&t.path().join("CLAUDE.md"), &mut Vec::new());
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].children.len(), 1, "b.md is reached through a.md");
        assert!(nodes[0].children[0].problem.is_none());
    }

    /// A cycle must RENDER, not hang -- and it must be visible, because
    /// it is a bug in the user's own config that nothing else surfaces.
    #[test]
    fn a_cycle_is_named_rather_than_followed_forever() {
        let t = tempfile::TempDir::new().unwrap();
        fs::write(t.path().join("CLAUDE.md"), "@a.md\n").unwrap();
        fs::write(t.path().join("a.md"), "@CLAUDE.md\n").unwrap();

        let nodes = resolve_tree(&t.path().join("CLAUDE.md"), &mut Vec::new());
        assert_eq!(nodes.len(), 1);
        let back = &nodes[0].children[0];
        assert_eq!(back.problem.as_deref(), Some("circular import"));
    }

    /// The same file imported by two SIBLINGS is legitimate -- it is not
    /// a cycle, and suppressing the second would understate the tree.
    #[test]
    fn a_file_imported_twice_by_siblings_is_not_a_cycle() {
        let t = tempfile::TempDir::new().unwrap();
        fs::write(t.path().join("CLAUDE.md"), "@1st.md\n\n@2nd.md\n").unwrap();
        fs::write(t.path().join("1st.md"), "@shared.md\n").unwrap();
        fs::write(t.path().join("2nd.md"), "@shared.md\n").unwrap();
        fs::write(t.path().join("shared.md"), "content").unwrap();

        let nodes = resolve_tree(&t.path().join("CLAUDE.md"), &mut Vec::new());
        assert_eq!(nodes.len(), 2);
        for n in &nodes {
            assert_eq!(n.children.len(), 1, "{}", n.raw);
            assert!(
                n.children[0].problem.is_none(),
                "a sibling import is not a cycle: {:?}",
                n.children[0].problem
            );
        }
    }

    /// `~/.claude/CLAUDE.md` is a real and common target, and it reaches
    /// OUTSIDE the repository, which is correct.
    #[test]
    fn expands_a_home_relative_import() {
        let t = tempfile::TempDir::new().unwrap();
        let fake_home = t.path().join("home");
        fs::create_dir_all(fake_home.join(".claude")).unwrap();
        fs::write(fake_home.join(".claude/CLAUDE.md"), "global rules").unwrap();

        let repo = t.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let f = repo.join("CLAUDE.md");
        fs::write(&f, "@~/.claude/CLAUDE.md\n").unwrap();

        let nodes = resolve_tree_in(&f, &mut Vec::new(), Some(&fake_home));
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].problem.is_none(), "{:?}", nodes[0].problem);
        assert!(nodes[0].tokens > 0);
    }

    /// The number that matters: a small file pulling in a large tree.
    #[test]
    fn total_tokens_include_the_whole_tree() {
        let t = tempfile::TempDir::new().unwrap();
        fs::write(t.path().join("CLAUDE.md"), "@big.md\n").unwrap();
        fs::write(t.path().join("big.md"), "x".repeat(4000)).unwrap();

        let f = super::super::read_file(&t.path().join("CLAUDE.md")).unwrap();
        assert!(f.tokens < 10, "the file itself is tiny");
        assert!(f.total_tokens > 900, "but the tree it pulls in is not");
    }

    /// An unreadable import must not claim to be fine (#972).
    ///
    /// `problem: None` here is not an omission, it is an ASSERTION -- an
    /// affirmative "this node is fine" about a file the app could not read.
    /// `read_to_string(...).unwrap_or_default()` produced exactly that,
    /// alongside `bytes: 0` and `tokens: 0`, on a target whose existence
    /// `is_file()` had already proven. It also contradicts `ImportNode`'s
    /// own contract: a broken import is SHOWN rather than omitted, because
    /// dropping it silently makes the tree look complete when it is not.
    ///
    /// # Why `#[cfg(unix)]`
    ///
    /// `chmod 000` is the mechanism, and Windows does not honour it: a
    /// `0o000` file stays readable there, so this test would fail for a
    /// reason unrelated to the code under test. The behaviour is not
    /// platform-specific -- a permission wall is a permission wall -- only
    /// this way of producing one is.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_import_is_a_problem_not_a_zero_token_node() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::TempDir::new().unwrap();
        fs::write(t.path().join("CLAUDE.md"), "@child.md\n").unwrap();
        let child = t.path().join("child.md");
        // Real content, so a zero-token node cannot be mistaken for an
        // honest measurement of an empty file.
        fs::write(&child, "x".repeat(4000)).unwrap();
        fs::set_permissions(&child, fs::Permissions::from_mode(0o000)).unwrap();

        let got = super::super::read_file_reporting(&t.path().join("CLAUDE.md"));

        // Restored BEFORE any assertion can panic, so a failure cannot
        // leave an undeletable file behind in the temp dir.
        fs::set_permissions(&child, fs::Permissions::from_mode(0o644)).unwrap();

        let file = got.expect("the CLAUDE.md itself is readable");
        assert_eq!(file.imports.len(), 1, "the import is SHOWN: {file:?}");
        let node = &file.imports[0];
        // Asserted explicitly against `None`, because that is the
        // fabricated claim rather than merely a missing one.
        assert_ne!(
            node.problem, None,
            "an unreadable import must never assert that it is fine: {node:?}"
        );
        assert!(
            node.problem.as_deref().unwrap().contains("could not read"),
            "say what went wrong: {node:?}"
        );
        // The weight is zero AND said to be unmeasured, so the headline
        // total is marked as a floor rather than quietly understated.
        assert_eq!(node.tokens, 0);
        assert!(node.unreadable, "{node:?}");
        assert!(
            file.total_partial,
            "a total missing an unmeasured import is a floor, not a value: {file:?}"
        );
    }

    /// A CIRCULAR import contributes a correct zero, so the total is EXACT.
    ///
    /// The false-positive half of the same distinction. A cycle is a real
    /// file already counted once higher up the chain, so labelling its
    /// total "at least" would put the floor idiom in front of a number that
    /// is exact -- which is why `unreadable` is a field of its own rather
    /// than inferred from `problem` being set.
    #[test]
    fn a_circular_import_does_not_make_the_total_a_floor() {
        let t = tempfile::TempDir::new().unwrap();
        fs::write(t.path().join("CLAUDE.md"), "@a.md\n").unwrap();
        fs::write(t.path().join("a.md"), "@CLAUDE.md\n").unwrap();

        let file = super::super::read_file_reporting(&t.path().join("CLAUDE.md")).unwrap();

        let cycle = &file.imports[0].children[0];
        assert_eq!(cycle.problem.as_deref(), Some("circular import"));
        assert!(!cycle.unreadable, "a cycle is not an unmeasured weight");
        assert!(
            !file.total_partial,
            "a cycle's zero is correct, so the total is exact: {file:?}"
        );
    }

    /// A MISSING import contributes a correct zero too.
    #[test]
    fn a_missing_import_does_not_make_the_total_a_floor() {
        let t = tempfile::TempDir::new().unwrap();
        fs::write(t.path().join("CLAUDE.md"), "@nowhere.md\n").unwrap();

        let file = super::super::read_file_reporting(&t.path().join("CLAUDE.md")).unwrap();

        assert_eq!(file.imports[0].problem.as_deref(), Some("file not found"));
        assert!(!file.imports[0].unreadable);
        assert!(
            !file.total_partial,
            "there is nothing to weigh, so the total is exact: {file:?}"
        );
    }

    /// An unreadable CLAUDE.md is reported, not omitted (#972).
    ///
    /// The walk has already matched the filename and stat'd the entry, so
    /// the file certainly exists. Dropping it unread made `scan_repo`
    /// return a silently short list -- and because the command wrapper can
    /// only produce `Ok`, `ClaudeMdPage`'s error arm (correct since #846,
    /// and ordered before the empty arm for exactly this reason) could
    /// never fire. The page then rendered #846's own sentence, "No
    /// CLAUDE.md files in this repository", about a file on disk.
    ///
    /// `#[cfg(unix)]` for the reason above: `chmod 000` is not honoured on
    /// Windows.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_claude_md_is_reported_not_omitted() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::TempDir::new().unwrap();
        let file = t.path().join("CLAUDE.md");
        fs::write(&file, "real instructions").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();

        let got = super::super::scan_repo(t.path());

        // Restored before any assertion can panic.
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();

        assert!(
            got.is_partial(),
            "a scan that could not read a file must know it is partial: {got:?}"
        );
        assert_eq!(got.unreadable_files.len(), 1, "{got:?}");
        assert!(
            got.unreadable_files[0].contains("CLAUDE.md"),
            "the report must name the file: {got:?}"
        );
        // The precise inversion: an empty list that nothing marks as
        // incomplete is what let "No CLAUDE.md files in this repository"
        // render on a repository that has one.
        assert!(
            !(got.files.is_empty() && !got.is_partial()),
            "an unmarked empty list reads as 'this repository has none': {got:?}"
        );
    }

    /// A partial scan keeps the files that DID read (#972).
    ///
    /// One unreadable file must not blank the list: the other files are
    /// real and useful. The rule `claude::transcript::Scan::is_partial`
    /// states -- a partial answer labelled partial beats both a silent
    /// truncation and an error page.
    #[cfg(unix)]
    #[test]
    fn a_partial_scan_still_returns_the_files_it_could_read() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::TempDir::new().unwrap();
        fs::write(t.path().join("CLAUDE.md"), "the readable one").unwrap();
        let sub = t.path().join("nested");
        fs::create_dir_all(&sub).unwrap();
        let shut = sub.join("CLAUDE.md");
        fs::write(&shut, "the unreadable one").unwrap();
        fs::set_permissions(&shut, fs::Permissions::from_mode(0o000)).unwrap();

        let got = super::super::scan_repo(t.path());

        fs::set_permissions(&shut, fs::Permissions::from_mode(0o644)).unwrap();

        assert!(got.is_partial(), "{got:?}");
        assert_eq!(
            got.files.len(),
            1,
            "the readable file survives its unreadable sibling: {got:?}"
        );
    }

    /// An unreadable DIRECTORY ends a subtree, and says so.
    ///
    /// A bigger unknown than one file: it may hold any number of CLAUDE.md
    /// files, so treating it as empty is indistinguishable from it
    /// genuinely being empty.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_directory_is_reported_not_treated_as_empty() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::TempDir::new().unwrap();
        let sub = t.path().join("packages");
        fs::create_dir_all(sub.join("api")).unwrap();
        fs::write(sub.join("api").join("CLAUDE.md"), "hidden").unwrap();
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o000)).unwrap();

        let got = super::super::scan_repo(t.path());

        fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(got.is_partial(), "{got:?}");
        assert_eq!(got.unreadable_dirs.len(), 1, "{got:?}");
    }

    /// A healthy repository is NOT partial.
    ///
    /// The false-positive half: if the ordinary case reported itself as
    /// incompletely scanned, the new banner would be on screen always and
    /// would therefore mean nothing.
    #[test]
    fn a_repository_that_read_cleanly_is_not_partial() {
        let t = tempfile::TempDir::new().unwrap();
        fs::write(t.path().join("CLAUDE.md"), "instructions").unwrap();

        let got = super::super::scan_repo(t.path());

        assert!(!got.is_partial(), "{got:?}");
        assert_eq!(got.files.len(), 1);
        assert!(got.unreadable_dirs.is_empty());
        assert!(got.unreadable_files.is_empty());
    }
}

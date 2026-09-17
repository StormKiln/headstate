//! Rules this codebase already states, asserted over its own source.
//!
//! # Why this module exists (#854)
//!
//! Six parallel audits of v5.13.0 produced about fifty findings, and the
//! thing every auditor noticed independently was that almost none was a
//! conceptual gap. The security auditor put it best: *"In every case the
//! correct implementation already exists elsewhere in this same codebase,
//! usually with a doc comment explaining the threat. None of these are
//! conceptual gaps in the authors' understanding; they are places a known
//! rule was not propagated to a sibling."*
//!
//! Fifty patches fix fifty instances and prevent none. The rule was
//! known every time -- written down, often measured -- and it still did
//! not reach the sibling, so the fifty-first arrives next release. What
//! closes that is a test that fails the moment the sibling is written.
//!
//! # Why the checks here are source scans
//!
//! Each property below is a statement about EVERY call site rather than
//! about observable behaviour at one of them. A behavioural test proves
//! the mechanism works -- which is rarely the thing in doubt -- while
//! saying nothing about the next call site somebody adds. That argument
//! is not new here: `stats::fetch`'s
//! `every_stats_read_goes_through_the_process_wide_permit` makes it at
//! length, and `stats::budget`'s `every_stats_query_meters_itself` is the
//! model this module follows throughout.
//!
//! # Derived, not enumerated
//!
//! The one hard lesson of #844, #842 and #847 is that a hand-written list
//! cannot cover the item nobody remembered to add to it -- all three
//! defects were exactly that, and #844's metering guard found two
//! uncovered documents the moment it stopped naming six. So every scan
//! here DISCOVERS its subjects:
//!
//! - the files, by walking the crate tree at runtime rather than by
//!   `include_str!` on a fixed list, so a brand-new file is covered
//!   without anyone remembering to add it. `include_str!` is the existing
//!   idiom and it is the one thing it cannot do: `every_query_document`
//!   names two paths, which is why `stats/tree.rs`' two documents sit
//!   outside it to this day.
//! - the call sites, by scanning that text.
//!
//! # What a source scan cannot see, stated rather than glossed
//!
//! These limits are real and shared by every check below:
//!
//! - **It cannot follow a call.** A check performed in a helper is
//!   invisible, so where a guard would otherwise report a defect at a
//!   location that does not have one, it offers a NAMED exemption with a
//!   recorded reason rather than a loose pattern.
//! - **It reads text, not semantics.** `"remove_dir_all"` inside a string
//!   literal or a doc comment looks like a call. Comment lines are
//!   skipped explicitly for that reason.
//! - **It sees only this crate tree.** A removal performed by a
//!   dependency, or through `std::process::Command`, is outside every
//!   check here.
//! - **It cannot prove a check is CORRECT**, only that one is present.
//!   `symlink_metadata` called and its answer ignored would pass. The
//!   behavioural tests beside each gate are what cover that, and they
//!   are cited from the checks that depend on them.
//!
//! A guard that cries wolf gets disabled -- `check-privacy.sh:120`
//! records ~40 false positives from one unanchored pattern as the reason
//! every pattern in it is anchored. So each check here prefers an
//! explicit, commented allowlist over a broader regex, and every entry in
//! one says why it is there.

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    /// The crates this repository builds, and where their Rust lives.
    ///
    /// Walked at runtime from `CARGO_MANIFEST_DIR` (the idiom
    /// `packages::detect` and `tray` already use to reach repository
    /// files from a test) rather than listed as `include_str!` paths,
    /// which is the whole point: a new file under any of these is
    /// covered the moment it is written.
    ///
    /// `src-mobile` and `crates/headstate-stepup` are separate crates
    /// that `cargo test` here does not compile, and that is exactly why
    /// they are read as TEXT. The alternative -- a copy of each check in
    /// each crate -- is the duplication this module exists to stop.
    fn crate_roots() -> Vec<(&'static str, PathBuf)> {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        vec![
            ("src-tauri", manifest.join("src")),
            ("src-mobile", manifest.join("../src-mobile/src")),
            ("stepup", manifest.join("../crates/headstate-stepup/src")),
        ]
    }

    /// Every `.rs` file under `dir`, recursively.
    ///
    /// Sorted, so a failure message names the same file every run: an
    /// unstable order in a guard's output makes two identical failures
    /// look like two different ones.
    fn rust_files(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        out.sort();
        out
    }

    /// The PRODUCTION code in a file: everything outside a
    /// `#[cfg(test)]` item.
    ///
    /// Every check here is about shipped code. Test code legitimately
    /// does the things these guards forbid -- a fixture deletes its own
    /// temp directory, a test names a command to assert it is refused --
    /// and a guard that failed on its own fixtures would be turned off
    /// within a day.
    ///
    /// # Why this is not a split on the first marker
    ///
    /// `stats::fetch`'s permit guard does exactly that
    /// (`src.split_once("\n#[cfg(test)]")`), and it is correct for the
    /// four files it reads, each of which ends in one test module. It is
    /// WRONG in general, and this guard caught it the first time it ran:
    /// `caches/mod.rs` has a test module at line 291 and more production
    /// code after it, including the `remove_dir_all` in `remove_venv` at
    /// 565. Splitting on the first marker hid one of the three call sites
    /// the invariant exists to check -- and the only thing that said so
    /// was the `checked >= 3` self-guard at the bottom of the test.
    /// Without that assertion this would have shipped green while seeing
    /// two thirds of the code.
    ///
    /// So each `#[cfg(test)]` item is skipped individually, and the end
    /// of one is found by INDENTATION rather than by counting braces.
    ///
    /// Brace-counting was written first and was wrong, which is worth
    /// recording because the failure is not obvious: `scan.rs`' test
    /// module contains `"@{u}"` and `"@{{u}}"` -- git's upstream syntax,
    /// in assertion messages -- and a counter that does not lex string
    /// literals reads those as real braces. It closed the 3,600-line
    /// module 1,400 lines early and then judged the test code in the
    /// remainder by production rules, reporting
    /// `reasons_are_display_ready_and_pluralised` as an unguarded
    /// recursive delete. A guard that cries wolf gets disabled, so the
    /// mechanism had to change rather than the message.
    ///
    /// Indentation is reliable here for a reason specific to this
    /// codebase rather than by luck: `cargo fmt --check` runs on all
    /// three crates in `make lint`, so every top-level item begins at
    /// column 0 and everything inside a module is indented. A
    /// `#[cfg(test)]` item therefore ends at the next line that starts
    /// with a non-space, non-`}` character.
    ///
    /// Column 0 alone is not enough, which was the second wrong version:
    /// `scan.rs`' `SAMPLE` fixture is a line-continuation string literal
    /// whose CONTENT starts at column 0 (`worktree /home/u/code/...`),
    /// and that ended the test module 5,000 lines early. So the
    /// terminator must also LOOK like a Rust item -- one of the keywords
    /// a top-level item can begin with, or an attribute. A string
    /// literal's contents do not, and neither does prose.
    ///
    /// The limits that survive, and they are real: a `#[cfg(test)]` on a
    /// nested (indented) item is not recognised at all, and a raw string
    /// whose content begins with one of those keywords at column 0 would
    /// still end a block early. Neither occurs in this tree, and both
    /// fail in the direction of seeing MORE code rather than less -- a
    /// false positive somebody reads, not a silent gap. The `checked`
    /// self-guard at the bottom of each test is what catches the other
    /// direction.
    fn production(src: &str) -> String {
        let mut out = String::new();
        // `Some(true)` while the skipped item's own header line
        // (`mod tests {`, `fn helper() {`) is still to be consumed: that
        // line is itself at column 0, so looking for the terminator
        // before eating it ends every block after one line -- which is
        // how this first reported a test in `auth.rs` as production code.
        let mut skipping: Option<bool> = None;
        for line in src.lines() {
            let top_level = !line.starts_with([' ', '\t']) && !line.is_empty();
            match skipping {
                Some(true) => {
                    skipping = Some(false);
                    continue;
                }
                Some(false) => {
                    // The first top-level line that begins a new Rust
                    // ITEM ends the block. "Looks like an item" rather
                    // than merely "is at column 0", because a string
                    // literal's content can sit at column 0 too.
                    if top_level && starts_an_item(line) {
                        skipping = None;
                    } else {
                        continue;
                    }
                }
                None => {}
            }
            if line.trim_start().starts_with("#[cfg(test)]") {
                skipping = Some(true);
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        out
    }

    /// Whether a line begins a top-level Rust item.
    ///
    /// The vocabulary a `.rs` file's column 0 can legitimately start
    /// with, which is short and closed. Used to tell the end of a
    /// `#[cfg(test)]` block from a line of string-literal content that
    /// merely happens to be unindented -- see [`production`].
    fn starts_an_item(line: &str) -> bool {
        const ITEM: &[&str] = &[
            "fn ",
            "pub ",
            "mod ",
            "use ",
            "const ",
            "static ",
            "struct ",
            "enum ",
            "impl ",
            "trait ",
            "type ",
            "macro_rules!",
            "extern ",
            "unsafe ",
            "async ",
            "#[",
            "#!",
            "///",
            "//!",
            "//",
        ];
        ITEM.iter().any(|k| line.starts_with(k))
    }

    /// Whether a line is a comment, and so a mention rather than code.
    ///
    /// Load-bearing for every scan here: this codebase documents its
    /// rules at length directly above the code that implements them, so
    /// the name a guard greps for appears in prose far more often than in
    /// a call. Without this every check below would fire on the very doc
    /// comments that state the rule it enforces.
    fn is_comment(line: &str) -> bool {
        let t = line.trim_start();
        t.starts_with("//") || t.starts_with("*") || t.starts_with("#!")
    }

    /// The body of the function containing byte offset `at`, as text.
    ///
    /// Scoped between the nearest preceding `fn` and the next top-level
    /// item, so a check has to be in THIS function rather than merely
    /// somewhere in a file that has many. `every_stats_query_meters_itself`
    /// records what getting this wrong costs: an earlier version anchored
    /// on the first mention of a name instead of its definition, read the
    /// wrong region, and reported a defect at a location that did not have
    /// one.
    ///
    /// Returns the name as well, so a failure can say which function.
    fn enclosing_fn(src: &str, at: usize) -> (String, String) {
        let before = &src[..at];
        let start = ["\nfn ", "\npub fn ", "\n    fn ", "\n    pub fn "]
            .iter()
            .filter_map(|m| before.rfind(m))
            .max()
            .unwrap_or(0);
        let body = &src[start..];
        // To the next item, searched FROM the hit rather than from the
        // start of the body.
        //
        // `body.find(m)` finds each pattern's FIRST occurrence, which may
        // already be behind the hit -- and filtering those out discards
        // the pattern entirely instead of looking for its next occurrence,
        // so the body runs on to whichever pattern happens to appear
        // later. The same mistake in `client.rs`' refusal guard gave one
        // function a body spanning six others, which made that guard pass
        // over the defect it was written for. It was caught by reverting
        // the fix and watching the guard NOT fail.
        let rel = at - start;
        let end = ["\nfn ", "\npub fn ", "\n}\n"]
            .iter()
            .filter_map(|m| body[rel..].find(m).map(|e| rel + e))
            .min()
            .unwrap_or(body.len());
        let body = &body[..end];
        let name = body
            .split_once("fn ")
            .and_then(|(_, r)| r.split(['(', '<']).next())
            .unwrap_or("<unknown>")
            .trim()
            .to_string();
        (name, body.to_string())
    }

    // ---- Invariant 1: recursive deletion ---------------------------------

    /// `remove_dir_all` on an externally-supplied path sits behind a
    /// symlink check AND a containment check.
    ///
    /// # What it enforces
    ///
    /// Every production call to `std::fs::remove_dir_all` must be in a
    /// function that also calls `symlink_metadata` (and tests
    /// `is_symlink`) and that checks the canonical path is inside a root
    /// it was GIVEN, rather than one the caller named.
    ///
    /// # The finding it would have caught (#854, the #841 family)
    ///
    /// `worktrees::remove_orphan` had NEITHER. Its gate was
    /// `orphan_gitdir`, which asks for a readable `<dir>/.git` whose
    /// `gitdir:` target does not exist -- a two-line file any caller can
    /// write, and not a containment boundary in any case. `dir.is_dir()`
    /// follows symlinks, so a link whose target held such a `.git`
    /// passed, and `remove_dir_all` then deleted the TARGET's contents.
    /// The path was never canonicalised and never compared to anything,
    /// and `remove_orphan` is exposed on the remote surface as
    /// `Class::Destructive`, so it could arrive from a paired peer.
    ///
    /// Both rules were already written down twice, which is the whole
    /// point of this guard. `artifacts::remove_artifact` and
    /// `caches::remove_venv` each carry a paragraph on why the symlink
    /// check must precede `canonicalize`, and `commands::remove_artifacts`
    /// states the containment rule outright: *"containment is the only
    /// thing between a bad path and `remove_dir_all` on an arbitrary
    /// directory, so the boundary it checks against must come from
    /// settings, not from the request."* `remove_orphan` sat one file
    /// away from both and had neither.
    ///
    /// # What it cannot see
    ///
    /// - **Whether the checks are right**, only that they are there. A
    ///   `symlink_metadata` whose answer is discarded passes. The three
    ///   gates' own behavioural tests cover that --
    ///   `refuses_a_symlink_pointing_inside_the_root`,
    ///   `refuses_a_traversal_out_of_the_root` and their siblings -- and
    ///   this guard is what stops a FOURTH gate shipping without them.
    /// - **A check in a helper.** `remove_venv` delegates containment to
    ///   `is_inside_cache`, so containment is recognised by any of several
    ///   spellings rather than one. That breadth is deliberate and is the
    ///   reason the symlink half is asserted separately and strictly: it
    ///   has no helper form in this codebase.
    /// - **A deletion that is not `remove_dir_all`.** A hand-rolled
    ///   recursive walk calling `remove_file`, or `Command::new("rm")`,
    ///   is outside this check. Nothing in the tree does either today.
    #[test]
    fn every_recursive_delete_checks_symlinks_and_containment() {
        let mut checked = 0usize;
        for (crate_name, root) in crate_roots() {
            for file in rust_files(&root) {
                let Ok(src) = std::fs::read_to_string(&file) else {
                    continue;
                };
                let prod = &production(&src);
                let rel = file.strip_prefix(&root).unwrap_or(&file).display();
                let mut at = 0usize;
                while let Some(i) = prod[at..].find("remove_dir_all(") {
                    let hit = at + i;
                    at = hit + 1;
                    // The line it sits on, so a doc comment explaining
                    // the rule is not mistaken for a call that breaks it.
                    let line_start = prod[..hit].rfind('\n').map_or(0, |j| j + 1);
                    let line_end = prod[hit..].find('\n').map_or(prod.len(), |j| hit + j);
                    let line = &prod[line_start..line_end];
                    if is_comment(line) {
                        continue;
                    }
                    let (fn_name, body) = enclosing_fn(prod, hit);
                    checked += 1;

                    assert!(
                        body.contains("symlink_metadata") && body.contains("is_symlink"),
                        "{crate_name}/{rel}: `{fn_name}` calls remove_dir_all with no symlink \
                         check. `remove_dir_all` on a symlink deletes the TARGET's contents, \
                         which may be anywhere at all -- and `is_dir()` follows links, so it \
                         sees the target's type and not the link. Call `symlink_metadata` and \
                         reject `is_symlink()` BEFORE `canonicalize`, which resolves through \
                         links and leaves nothing to detect. `artifacts::remove_artifact` and \
                         `caches::remove_venv` both document this at length; \
                         `worktrees::remove_orphan` is the sibling that did not get it (#854).\n\
                         \x20   {}",
                        line.trim()
                    );

                    // Containment, in any of the spellings this codebase
                    // uses. Broad on purpose: `remove_venv` delegates to
                    // `is_inside_cache`, so demanding a literal
                    // `starts_with` here would fail a gate that is
                    // correct. What every spelling has in common is that
                    // the canonical path is compared against a boundary.
                    let contains = body.contains("starts_with")
                        || body.contains("is_inside_cache")
                        || body.contains("outside the scanned folders");
                    assert!(
                        body.contains("canonicalize") && contains,
                        "{crate_name}/{rel}: `{fn_name}` calls remove_dir_all without checking \
                         the CANONICAL path is inside a root it was given. Without it the \
                         command is `remove_dir_all` on any directory its caller names, and \
                         `..` walks out of any root compared before canonicalising. The \
                         boundary must come from settings rather than from the request -- \
                         `commands::remove_artifacts` states exactly that, and \
                         `commands::remove_orphan` passed the path alone until #854.\n\
                         \x20   {}",
                        line.trim()
                    );
                }
            }
        }
        // The scan is asserted to have FOUND something, so a rename or a
        // moved file fails loudly rather than passing vacuously over an
        // empty list -- which is how a derived guard dies quietly.
        // `every_stats_query_meters_itself` guards itself the same way.
        assert!(
            checked >= 3,
            "only {checked} production remove_dir_all call(s) found; the scan is broken, \
             not the code. There are three (artifacts, caches, worktrees)."
        );
    }

    // ---- Invariant 2: refs reaching a git argv ---------------------------

    /// Every reader of `refs/remotes/origin/HEAD` validates what it got.
    ///
    /// # What it enforces
    ///
    /// A production function that asks git for `refs/remotes/origin/HEAD`
    /// must pass the answer through `is_safe_ref` before returning it.
    ///
    /// # Why this shape, and not the one #854 asked for
    ///
    /// #854 states the invariant as "every ref or name reaching a
    /// `git`/`docker` argv is behind a flag-shape validator or a `--`".
    /// That was written as a PER-SINK rule, and this codebase deliberately
    /// rejected the per-sink form. `is_safe_ref`'s own doc comment says
    /// so: validation is done "at the BOUNDARIES where remote-controlled
    /// refs enter ... rather than at each of the ten call sites, because a
    /// boundary cannot be forgotten. The `--` separators at the sinks are
    /// the second layer, not the only one."
    ///
    /// A guard demanding a validator or a `--` at each of roughly forty
    /// `git` spawn sites would therefore report the architecture as the
    /// defect. It would fire on `merge-base --is-ancestor HEAD <default>`,
    /// which is correct by construction -- the ref was validated at its
    /// boundary and git's own `merge-base` takes no `--` -- and on
    /// `config --get branch.<b>.remote`, where the name is embedded in a
    /// prefix and cannot be read as a flag at all. Dozens of findings,
    /// none of them real. That is the unanchored-pattern mistake
    /// `check-privacy.sh:120` records forty false positives from, and a
    /// gate that cries wolf is a gate someone disables.
    ///
    /// So the invariant is asserted where the codebase actually places it:
    /// at the boundary. The property "a remote-controlled ref is validated
    /// as it enters" is the one that makes every downstream sink safe, and
    /// it is checkable precisely because the boundaries are few and
    /// identifiable by the ref they read.
    ///
    /// # The finding it catches (#854)
    ///
    /// FOUR functions named `default_branch` read
    /// `refs/remotes/origin/HEAD`, and before #854 exactly ONE validated
    /// it -- `worktrees::scan::default_branch`, which carries the
    /// reasoning in its own comment: *"`origin/HEAD` is written by the
    /// remote, so the short name it yields is validated BEFORE the
    /// `origin/` prefix is put back on. Prefixing first would hide
    /// `--output=EVIL` behind a name that no longer starts with `-`."*
    ///
    /// `branches::scan`, `packages::apply` and `docker::classify` each
    /// grew their own and returned the name unvalidated. It then reached
    /// `rev-list <default>`, `merge-base <branch> <default>` and
    /// `merge-base --is-ancestor <tag> <default>` as a bare argv element
    /// with no `--`, where `--output=/path` is an arbitrary file write
    /// with the app's privileges.
    ///
    /// The reach of the validator was the mechanism of the failure:
    /// `is_safe_ref` was `pub(super)`, so the other three modules could
    /// not call it even had they wanted to. A shared rule only one module
    /// can see is a rule with one user.
    ///
    /// # What it cannot see
    ///
    /// - **Every other remote-controlled value.** This asserts ONE
    ///   boundary, the one with four implementations and three defects.
    ///   `parse_porcelain`'s branch names are the other, guarded and
    ///   tested since it was written. A tag read from a docker image
    ///   label (`docker::origin`) is a third, gated by `looks_like_sha`
    ///   instead -- a stricter check, not a missing one.
    /// - **Whether the validation is correctly ORDERED** -- only that it
    ///   is ordered at all. `symbolic-ref --short` returns
    ///   `origin/<name>`, so `is_safe_ref` on the PREFIXED string is
    ///   worthless: `origin/--output=/tmp/x` begins with `o`. This is not
    ///   hypothetical. Two of the three fixes #854 wrote made exactly that
    ///   mistake, and this guard passed on both -- it was the behavioural
    ///   test `a_flag_shaped_remote_head_is_refused` that caught them. So
    ///   the assertion below also requires a prefix strip in the same
    ///   function, which narrows the hole without closing it: a strip of
    ///   the wrong prefix, or of the right one applied to the wrong value,
    ///   still passes. The per-site behavioural tests are what cover that,
    ///   and they are named here so the pairing is not accidental.
    /// - **The sinks themselves.** A new `git` call passing an
    ///   unvalidated ref from somewhere else entirely is outside this.
    ///   That is the per-sink question, and it is the one judged
    ///   unenforceable above.
    #[test]
    fn every_reader_of_the_remote_head_validates_it() {
        const REF: &str = "refs/remotes/origin/HEAD";
        let mut checked = 0usize;
        for (crate_name, root) in crate_roots() {
            for file in rust_files(&root) {
                let Ok(src) = std::fs::read_to_string(&file) else {
                    continue;
                };
                let prod = &production(&src);
                let rel = file.strip_prefix(&root).unwrap_or(&file).display();
                let mut at = 0usize;
                while let Some(i) = prod[at..].find(REF) {
                    let hit = at + i;
                    at = hit + 1;
                    let line_start = prod[..hit].rfind('\n').map_or(0, |j| j + 1);
                    let line_end = prod[hit..].find('\n').map_or(prod.len(), |j| hit + j);
                    let line = &prod[line_start..line_end];
                    if is_comment(line) {
                        continue;
                    }
                    let (fn_name, body) = enclosing_fn(prod, hit);
                    checked += 1;
                    assert!(
                        body.contains("is_safe_ref"),
                        "{crate_name}/{rel}: `{fn_name}` reads {REF} and never passes the \
                         answer through `is_safe_ref`. That symref is written by the \
                         REMOTE, so the name it yields is remote-controlled -- and git \
                         ref names may legitimately begin with `-`. Passed as a bare \
                         argv element, `--output=/path` makes `git log` write to an \
                         arbitrary file, with this app's privileges. \
                         `worktrees::scan::default_branch` has validated it since it \
                         was written and states why; three siblings did not, because \
                         the validator was `pub(super)` (#854). Validate the BARE name, \
                         before any `origin/` prefix is put back on.\n\x20   {}",
                        line.trim()
                    );
                    // And the name validated must be the BARE one.
                    //
                    // `--short` returns `origin/<name>`, so `is_safe_ref`
                    // on that string is worthless -- `origin/--output=/x`
                    // begins with `o`. Two of #854's own three fixes made
                    // this mistake and this guard passed both until the
                    // check was added, so it is asserted rather than
                    // trusted to review.
                    assert!(
                        body.contains("strip_prefix(\"origin/\")") || body.contains("rsplit('/')"),
                        "{crate_name}/{rel}: `{fn_name}` calls `is_safe_ref` but never \
                         strips the `origin/` prefix, so it is validating a string that \
                         starts with `o` whatever the remote named the branch. \
                         `symbolic-ref --short` returns `origin/<name>`; validate \
                         `<name>` (#854)."
                    );
                }
            }
        }
        // Guards the guard: four readers exist, and a scan that found
        // fewer has stopped seeing one of them.
        assert!(
            checked >= 4,
            "only {checked} reader(s) of {REF} found; the scan is broken, not the \
             code. There are four (worktrees, branches, packages, docker)."
        );
    }

    // ---- Invariant 5: mirrored constants ---------------------------------

    /// A constant declared in more than one crate has a test that reads
    /// both copies.
    ///
    /// # What it enforces
    ///
    /// Every `const NAME` that appears in the production half of two
    /// different crates must either be asserted by a test that reads both
    /// sides, or appear in `COINCIDENTAL` below with a reason.
    ///
    /// # The finding it would have caught (#850)
    ///
    /// #850 found five pairs whose doc comments said the agreement was
    /// asserted and which no test checked; one had already drifted --
    /// `15 * 60` in Rust against `60 * 60` in the UI, both comments
    /// claiming they matched, with three user-visible consequences. The
    /// remedy, `src/lib/mirroredConstants.test.ts`, is excellent and
    /// ENUMERATED: it covers exactly the five pairs that audit found.
    ///
    /// So this asks the general question instead, and the answer is that
    /// the Rust-to-Rust pairs were never in scope of that file at all.
    /// Worst among them, and the reason this is worth a guard rather than
    /// five more assertions: `ECDSA_SIG_LEN` and `MLDSA_SIG_LEN` are
    /// declared in `crates/headstate-stepup` -- whose module doc says in
    /// so many words that it holds what "both ends must agree on" -- and
    /// then DECLARED AGAIN in `src-mobile/src/keys.rs` rather than
    /// imported from it. `keys.rs` then asserts against its own local
    /// copies, so a change to the shared crate is invisible on the phone.
    /// These are signature lengths on a security boundary.
    ///
    /// # Why the pairs are derived structurally, not from the prose
    ///
    /// The obvious derivation is to grep doc comments for "must match" /
    /// "mirrors" and demand a test per hit. It was tried and rejected:
    /// over this tree that phrase matches mostly ordinary prose
    /// ("singular and plural must agree with the number", "the default
    /// branch must agree"), which is the unanchored-pattern mistake
    /// `check-privacy.sh` records ~40 false positives from. A repeated
    /// NAME is a fact about the code rather than about how carefully
    /// somebody worded a comment, and it also catches the pair whose
    /// comment says nothing at all -- which `VALIDITY_YEARS` very nearly
    /// is.
    ///
    /// # What it cannot see
    ///
    /// - **A pair with different names on each side.** `SEED_LEN` (32) in
    ///   the desktop and `VAULT_KEY_LEN` (32) on the phone are the same
    ///   32 bytes and this cannot tell. Name-matching is the price of not
    ///   matching on prose.
    /// - **A value duplicated as a bare literal** rather than as a named
    ///   constant. `PORT` is `41919` in the desktop and an unnamed
    ///   `41919` five times over on the phone, and only the named side is
    ///   visible here.
    /// - **Whether the covering test is any GOOD.** It checks that a test
    ///   names the constant and reads both files, not that it compares
    ///   them correctly. `mirroredConstants.test.ts` guards its own
    ///   extractor for this reason, and `NONCE_LEN` is the live example
    ///   of a test that names a constant and still reads one side twice
    ///   (`assert_eq!(NONCE_LEN, 16)`).
    /// - **TypeScript.** The Rust-to-TS pairs are
    ///   `mirroredConstants.test.ts`' business and stay there; this is
    ///   its Rust-to-Rust counterpart, not its replacement.
    #[test]
    fn every_cross_crate_constant_is_read_from_both_sides() {
        /// Names that collide by coincidence rather than by mirroring.
        ///
        /// An explicit, reviewed list with a reason per entry, which is
        /// the form this codebase insists on over a looser pattern:
        /// `surfaceGuard.test.ts`' `DESKTOP_ONLY_WRAPPERS` makes the same
        /// argument, that adding to such a list should be a deliberate
        /// act somebody reads.
        ///
        /// Every entry here is a name two crates use for DIFFERENT
        /// things, verified by reading both. A pair that merely happens
        /// to agree today does not belong here -- that is the thing being
        /// guarded.
        const COINCIDENTAL: &[(&str, &str)] = &[
            // 120s for a companion's whole HTTP call to the desktop,
            // 20s for one `docker` subprocess. Unrelated budgets that
            // would be wrong to tie together.
            (
                "CALL_TIMEOUT",
                "unrelated timeouts: an HTTP call vs a docker subprocess",
            ),
            // The phone's key record is at schema 1, the desktop's
            // identity record at 2. Separate formats on separate
            // migration paths; forcing them equal would be meaningless.
            (
                "STORED_VERSION",
                "independent on-disk record schemas, versioned separately",
            ),
            // The desktop's is the mDNS TXT string `"1"`, the phone's the
            // integer 1 in its own pairing record. Different types,
            // different records.
            (
                "RECORD_VERSION",
                "a TXT string on one side, a record schema integer on the other",
            ),
        ];

        let mut by_name: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        for (crate_name, root) in crate_roots() {
            for file in rust_files(&root) {
                let Ok(src) = std::fs::read_to_string(&file) else {
                    continue;
                };
                for line in production(&src).lines() {
                    if is_comment(line) {
                        continue;
                    }
                    // `const NAME:` at any indentation, with or without
                    // `pub`. SCREAMING_CASE only, which is what
                    // distinguishes a constant from a local binding.
                    let t = line.trim_start();
                    let rest = t
                        .strip_prefix("pub const ")
                        .or_else(|| t.strip_prefix("const "))
                        .or_else(|| {
                            t.strip_prefix("pub(crate) const ")
                                .or_else(|| t.strip_prefix("pub(super) const "))
                        });
                    let Some(rest) = rest else { continue };
                    let name: String = rest
                        .chars()
                        .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
                        .collect();
                    if name.len() < 3 || !rest[name.len()..].trim_start().starts_with(':') {
                        continue;
                    }
                    let file_name = file
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    by_name
                        .entry(name)
                        .or_default()
                        .push((crate_name.to_string(), file_name));
                }
            }
        }

        // Every FUNCTION that reads another file's source as text, as a
        // list of its bodies. A covering assertion has to be inside one
        // of these, because reading the other side's source is the only
        // way an assertion can fail when the other side alone changes --
        // which is the whole property. `mirroredConstants.test.ts`' own
        // header makes the argument: a test comparing a constant to a
        // literal reads ONE side twice and passes at any value.
        //
        // Scoped to the function rather than to the file for the reason
        // the first version of this check got wrong: a file-wide search
        // for `include_str!` matched every file in the tree, so the
        // condition was true for every constant and the whole assertion
        // was vacuous. It passed green over sixteen uncovered pairs.
        let mut readers: Vec<String> = Vec::new();
        for (_, root) in crate_roots() {
            for file in rust_files(&root) {
                let Ok(src) = std::fs::read_to_string(&file) else {
                    continue;
                };
                // Line endings normalised before any byte pattern runs.
                //
                // A Windows checkout with `core.autocrlf` has CRLF, so a
                // pattern containing a bare `\n` -- which is how
                // `enclosing_fn` finds a function's start -- matches
                // nothing there. It would return a body beginning at
                // offset 0, i.e. the whole file, making every constant
                // look covered: a silent pass, on one platform only.
                //
                // This hazard is not hypothetical here. `health::runaway`
                // and `src-mobile::background` each carry a paragraph on
                // it, both recording that it was OBSERVED on the
                // `platform (windows-latest)` job. `production()` above
                // normalises for the same reason, by rebuilding its
                // output line by line.
                let src = src.replace("\r\n", "\n");
                let mut at = 0usize;
                while let Some(i) = src[at..].find("include_str!") {
                    let hit = at + i;
                    at = hit + 1;
                    // Only a read of a DIFFERENT crate's source. A file
                    // reading its own text (`include_str!("scan.rs")`)
                    // is a self-scan, not a mirror.
                    let line_end = src[hit..].find('\n').map_or(src.len(), |j| hit + j);
                    if !src[hit..line_end].contains("..") {
                        continue;
                    }
                    readers.push(enclosing_fn(&src, hit).1);
                }
            }
        }
        // The frontend's mirror test is one reader too: it is where a
        // Rust-to-TypeScript pair's assertion belongs, and a constant
        // asserted there is covered.
        let ts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/lib/mirroredConstants.test.ts");
        if let Ok(s) = std::fs::read_to_string(&ts) {
            readers.push(s);
        }
        assert!(
            readers.len() >= 2,
            "only {} cross-file source reader(s) found; the scan is broken. \
             `src-mobile/src/surface.rs` and `src/lib/mirroredConstants.test.ts` \
             are two of them.",
            readers.len()
        );

        let mut pairs = 0usize;
        let mut unguarded = Vec::new();
        for (name, sites) in &by_name {
            let crates: std::collections::BTreeSet<&str> =
                sites.iter().map(|(c, _)| c.as_str()).collect();
            if crates.len() < 2 {
                continue;
            }
            pairs += 1;
            if COINCIDENTAL.iter().any(|(n, _)| *n == name) {
                continue;
            }
            // Covered when some function that reads another file's
            // source also NAMES this constant. Both halves in the same
            // scope is the point: naming it without reading across is
            // what `top_n_is_five` did while only ever seeing Rust, and
            // reading across without naming it says nothing about this
            // constant.
            let covered = readers.iter().any(|body| body.contains(name.as_str()));
            if !covered {
                let where_ = sites
                    .iter()
                    .map(|(c, f)| format!("{c}/{f}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                unguarded.push(format!("{name} ({where_})"));
            }
        }

        // Guards the guard, both directions. A pattern that stopped
        // matching would report every pair as covered; one that matched
        // nothing would report no pairs at all.
        assert!(
            pairs >= 10,
            "only {pairs} cross-crate constant(s) found; the scan is broken, not the code"
        );
        assert!(
            unguarded.is_empty(),
            "these constants are declared in two crates and no test reads both copies:\n  {}\n\n\
             A doc comment saying two values must agree cannot enforce itself -- #850 found \
             five such comments and one pair had already drifted while both went on claiming \
             they matched. Add an assertion that reads the OTHER side's SOURCE \
             (`include_str!` in Rust, `?raw` from TypeScript), the way \
             `src-mobile/src/surface.rs` reads the desktop's table and \
             `src/lib/mirroredConstants.test.ts` reads Rust constants. Asserting one copy \
             against a literal reads one side twice and cannot fail when the other moves. \
             If the collision is coincidental, add it to COINCIDENTAL above with a reason \
             (#854).",
            unguarded.join("\n  ")
        );
    }

    // ---- Shared: reading the TEST half ------------------------------------

    /// The line index just past the item that starts at `lines[start]` and
    /// is indented `indent` columns.
    ///
    /// The terminator is the next line that is exactly `}` at the item's OWN
    /// column, which is reliable in this tree for the reason [`production`]
    /// sets out at length: `cargo fmt --check` runs over all three crates in
    /// `make lint`, so a closing brace sits at the column its `fn` keyword
    /// started on. Nested test modules are why the column has to be a
    /// parameter rather than zero -- `scan.rs` has `mod tests { mod
    /// classifying { #[test] fn ... } }`, so the bodies the guards below read
    /// are three levels in.
    ///
    /// Falls back to the end of the file when no such brace is found, which
    /// errs toward seeing MORE code than the item really spans. That is the
    /// safe direction here: a guard reading too much produces a false
    /// positive somebody investigates, where one reading too little passes
    /// silently over the defect. `production`'s doc makes the same argument
    /// about the same trade.
    fn item_end(lines: &[&str], start: usize, indent: usize) -> usize {
        lines
            .iter()
            .enumerate()
            .skip(start + 1)
            .find(|(_, l)| l.trim() == "}" && l.len() - l.trim_start().len() == indent)
            .map_or(lines.len(), |(j, _)| j + 1)
    }

    /// One `#[test]` function, as the three things a guard needs about it.
    ///
    /// The guards below (invariants 7 and 8) are the first here to assert
    /// about TEST code rather than production code, which inverts
    /// [`production`]: where the earlier six strip `#[cfg(test)]` items
    /// because a fixture legitimately does what they forbid, these two are
    /// about the fixtures themselves.
    ///
    /// That is not a change of heart about scope. The two defects #869
    /// collects (#861, #868) were both in test code, both cost a release
    /// tag, and both were a rule stated in the same file that the body one
    /// screen down contradicted -- so the thing to guard is the
    /// consistency between a test's prose and its body, which only exists
    /// inside a test.
    struct TestFn {
        /// `crate/path/file.rs`, for a failure message that names a file
        /// somebody can open.
        where_: String,
        /// The function name, as `fn NAME(` spells it.
        name: String,
        /// The `///` lines immediately above the `#[test]` attribute, with
        /// the slashes stripped and joined by SPACES, so a phrase
        /// `rustfmt` wrapped across two lines is still one string to match
        /// on. Empty when the test has no doc comment.
        doc: String,
        /// The body text, from the `fn` line to the closing brace at the
        /// same indentation.
        body: String,
        /// Whether this is an `async` test (`#[tokio::test]`).
        ///
        /// Load-bearing for invariant 7 and not a convenience: the lock it
        /// enforces is a `std::sync::MutexGuard`, which clippy's
        /// `await_holding_lock` forbids holding across an `.await` -- and
        /// `-D warnings` is what CI's `lint` job runs, so an async test
        /// CANNOT comply with the rule as the rule is currently built. See
        /// that invariant's "What it cannot see".
        is_async: bool,
    }

    /// Every `#[test]` function in `src`, with its doc comment and body.
    ///
    /// # Why the body is bounded by INDENTATION and not by brace counting
    ///
    /// [`production`]'s doc records the full argument and it applies
    /// unchanged here: `scan.rs`' test module contains `"@{u}"` and
    /// `"@{{u}}"` in assertion messages, so a counter that does not lex
    /// string literals closes a function early, and `SAMPLE`'s
    /// line-continuation literal puts prose at column 0. Indentation is
    /// reliable for the same specific reason -- `cargo fmt --check` runs
    /// over all three crates in `make lint`, so a function's closing brace
    /// sits at exactly the column its `fn` keyword started on.
    ///
    /// This matters more here than it did for `production`, because test
    /// functions nest: `scan.rs` has `mod tests { mod classifying { #[test]
    /// fn ... } }`, so the bodies being extracted are at three levels of
    /// indentation and a single fixed column would find none of them.
    ///
    /// # Why `#[test]` and not `fn`
    ///
    /// A helper inside a test module (`fn rl(...)`, `fn repo_with_worktrees`)
    /// is not a test and has no doc comment making a promise about what it
    /// asserts. Anchoring on the attribute also means `#[tokio::test]` and
    /// `#[test]\n#[ignore]` are found, since the scan looks for the
    /// attribute line and then the next `fn`.
    fn test_fns(where_: &str, src: &str) -> Vec<TestFn> {
        // Normalised before any `\n` pattern runs: a Windows checkout with
        // `core.autocrlf` has CRLF, and `invariant 5` records this hazard
        // being OBSERVED on the `platform (windows-latest)` job rather
        // than merely feared.
        let src = src.replace("\r\n", "\n");
        let lines: Vec<&str> = src.lines().collect();
        let mut out = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let t = line.trim_start();
            if t != "#[test]" && !t.starts_with("#[tokio::test") {
                continue;
            }
            // The `fn` line: the next line that declares one, so an
            // intervening `#[ignore]` or `#[should_panic]` is skipped.
            let Some(fn_at) = (i + 1..lines.len().min(i + 6)).find(|j| {
                lines[*j].trim_start().starts_with("fn ")
                    || lines[*j].trim_start().starts_with("async fn ")
            }) else {
                continue;
            };
            let fn_line = lines[fn_at];
            let indent = fn_line.len() - fn_line.trim_start().len();
            let name = fn_line
                .trim_start()
                .trim_start_matches("async ")
                .trim_start_matches("fn ")
                .split(['(', '<'])
                .next()
                .unwrap_or("<unknown>")
                .to_string();

            // The doc comment: `///` lines directly above the attribute,
            // walking UP and stopping at the first line that is not one.
            // Other attributes are walked through -- `#[ignore]` between
            // the doc and the `#[test]` does not detach the prose from the
            // test it describes.
            let mut doc_lines: Vec<&str> = Vec::new();
            for j in (0..i).rev() {
                let d = lines[j].trim_start();
                if let Some(rest) = d.strip_prefix("///") {
                    doc_lines.push(rest.trim());
                } else if d.starts_with("#[") {
                    continue;
                } else {
                    break;
                }
            }
            doc_lines.reverse();
            // Joined with a SPACE and not a newline, because the phrases
            // invariant 8 matches on are wrapped by `rustfmt`'s comment
            // width: `scan.rs:6570` reads "a timing\n/// threshold", so
            // `doc.contains("timing threshold")` is false against a
            // newline-joined doc. That is a silent gap of exactly the kind
            // #869 warns about -- a guard that looks like it covers a
            // phrase and never matches it -- and it was found by asserting
            // the match count rather than by reading the code.
            let doc = doc_lines.join(" ");

            // The body, to the closing brace at the `fn`'s own column.
            let end = item_end(&lines, fn_at, indent);
            out.push(TestFn {
                where_: where_.to_string(),
                name,
                doc,
                body: lines[fn_at..end].join("\n"),
                is_async: fn_line.trim_start().starts_with("async fn "),
            });
        }
        out
    }

    /// Every `#[test]` in every crate, with the file it came from.
    ///
    /// Derived by walking the crate tree, for the reason this module's
    /// header gives: a hand-written list cannot cover the test nobody
    /// remembered to add to it, and #868's defect was six siblings of a
    /// rule that was already enforced elsewhere.
    fn all_test_fns() -> Vec<TestFn> {
        let mut out = Vec::new();
        for (crate_name, root) in crate_roots() {
            for file in rust_files(&root) {
                let Ok(src) = std::fs::read_to_string(&file) else {
                    continue;
                };
                let rel = file.strip_prefix(&root).unwrap_or(&file).display();
                out.extend(test_fns(&format!("{crate_name}/{rel}"), &src));
            }
        }
        out
    }

    /// The body text of every named function in `src`, keyed by name.
    ///
    /// Used to follow a call one hop at a time, which is what makes
    /// invariant 7 a reachability check rather than a grep: a test calling
    /// a helper that calls `Budget::record` is in scope of the lock rule,
    /// and `observed_test_lock`'s own doc comment says so in as many words
    /// -- *"the question is whether anything it calls can store to
    /// `OBSERVED_REMAINING`"*.
    ///
    /// # What this is not
    ///
    /// It is not a call graph. Names are matched textually, so two
    /// functions called `record` in different types share an entry, and a
    /// call through a trait object or a closure variable is invisible.
    /// #869 suggests using CodeGraph's edges for this, and that is not
    /// available from inside a `cargo test` run -- the index is a
    /// developer tool in `.codegraph/`, absent on CI and in a fresh
    /// checkout, and a guard that silently becomes a no-op when its index
    /// is missing is worse than a coarse one that always runs.
    ///
    /// Coarse in the direction that is safe: matching by bare name
    /// over-approximates reachability, so the failure mode is a test told
    /// to take a cheap lock it did not strictly need. `observed_test_lock`
    /// anticipates exactly that trade -- *"a test that does not need them
    /// loses nothing by holding them"*.
    fn fn_bodies(src: &str) -> BTreeMap<String, String> {
        let src = src.replace("\r\n", "\n");
        let lines: Vec<&str> = src.lines().collect();
        let mut out: BTreeMap<String, String> = BTreeMap::new();
        for (i, line) in lines.iter().enumerate() {
            let t = line.trim_start();
            // `async` spellings are listed explicitly and are not
            // decoration: every function that reaches `OBSERVED_REMAINING`
            // WITHOUT naming it is async -- `client::fetch_viewer`,
            // `client::fetch_viewer_metered`, `client::fetch_prs_with_total`
            // and `fetch::read_metered`. Omitting them would leave invariant
            // 7 unable to follow the only indirect paths that exist, which
            // is the half of the rule `observed_test_lock`'s doc insists on.
            //
            // Longest prefix first, so `pub(crate) fn` is not matched as
            // `pub ` + garbage.
            let rest = [
                "pub(crate) async fn ",
                "pub(super) async fn ",
                "pub async fn ",
                "pub(crate) fn ",
                "pub(super) fn ",
                "pub fn ",
                "async fn ",
                "fn ",
            ]
            .iter()
            .find_map(|p| t.strip_prefix(p));
            let Some(rest) = rest else { continue };
            let name = rest
                .split(['(', '<'])
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();
            if name.is_empty() {
                continue;
            }
            let indent = line.len() - t.len();
            let end = item_end(&lines, i, indent);
            // A name declared twice (an inherent `fn` and a trait impl of
            // the same name) has its bodies CONCATENATED rather than one
            // overwriting the other, so following the name cannot miss the
            // copy that happens to be second in the file.
            out.entry(name)
                .and_modify(|b| {
                    b.push('\n');
                    b.push_str(&lines[i..end].join("\n"));
                })
                .or_insert_with(|| lines[i..end].join("\n"));
        }
        out
    }

    // ---- Invariant 7: the OBSERVED_REMAINING serialisation rule ------------

    /// Every test that can reach `OBSERVED_REMAINING` holds
    /// `observed_test_lock()`.
    ///
    /// # What it enforces
    ///
    /// A `#[test]` in `src-tauri` whose body can reach
    /// `budget::note_remaining` or `Budget::record` -- directly, or through
    /// one hop of a function in the same file -- must call
    /// `observed_test_lock()`.
    ///
    /// # The finding it would have caught (#868)
    ///
    /// `observed_test_lock`'s doc comment has said *"One lock for every
    /// TEST that touches `OBSERVED_REMAINING`, directly or through
    /// `Budget::record`"* since #843, and six tests IN THAT SAME FILE
    /// called `record` without it: three in `tests` and three in
    /// `metering`. `record` stores to the process-wide static at
    /// `budget.rs:329` and `cargo test` runs test functions on a thread
    /// pool, so those six mutated the figure underneath
    /// `a_seeded_budget_can_actually_refuse`, which reads it. MEASURED: 4
    /// failures in 6 local runs of `cargo test --lib github::stats::budget`,
    /// and one failure on `main` that blocked the v5.14.0 tag.
    ///
    /// The rule had already been propagated ACROSS a file boundary --
    /// `fetch.rs`'s `a_wave_is_refused_once_the_budget_is_under_the_reserve`
    /// takes the lock, and the doc cites that as the reason the lock is
    /// crate-visible rather than private. So a known rule, enforced once
    /// against a different file, failed to reach six siblings in its own.
    /// That is this module's founding observation (#854) recurring inside
    /// the code the audit added, which is why #869 asked for it
    /// mechanically rather than by vigilance.
    ///
    /// # Why reachability rather than a grep for `record`
    ///
    /// `observed_test_lock` states the rule in the form a future author
    /// will get wrong: *"`record` is not the only reachable path, and 'my
    /// test does not mention `note_remaining`' is not the question. The
    /// question is whether anything it calls can store to
    /// `OBSERVED_REMAINING`."* Four production functions reach the static
    /// without naming it -- `client::fetch_viewer`,
    /// `client::fetch_viewer_metered`, `client::fetch_prs_with_total` and
    /// `fetch::read_metered` -- so a grep for the two names misses any test
    /// that goes through one of them. None does today, because all four are
    /// `async` and need a live client; the guard covers them so the first
    /// one that appears fails here rather than in a release.
    ///
    /// # The async gap this guard cannot close, and what does
    ///
    /// This rule is enforceable only for SYNC tests. `observed_test_lock`
    /// returns a `std::sync::MutexGuard`, and clippy's
    /// `await_holding_lock` under `-D warnings` refuses to let one be held
    /// across an `.await` -- so an async test structurally cannot comply,
    /// and this guard cannot demand it.
    ///
    /// That gap produced three defects: #1048 (a mock fixture supplying
    /// `rateLimit.remaining`), #1050 (seeding through `record`), and #1079
    /// (an async test inheriting a STARVED figure and failing with a
    /// message about a budget it never set, which burned the v5.22.0 tag).
    ///
    /// `budget::scoped` (#1079) is the async-safe form: a thread-local
    /// override, sound because `cargo test` gives each test its own thread
    /// and every `#[tokio::test]` here uses the default `current_thread`
    /// runtime. An async test calls `scoped::enter` and is then immune to
    /// what any other test wrote -- and its own writes cannot escape.
    ///
    /// So the two mechanisms divide the space rather than compete: this
    /// guard keeps sync tests serialised, and `scoped` isolates async ones.
    /// A NEW async test needs `scoped::enter`, and nothing here can make it
    /// -- which is why the mechanism is the fix and this comment is the
    /// pointer to it.
    ///
    /// One hop, not a full closure, and the limit is stated because it is
    /// real: the hop is resolved inside the test's OWN file via
    /// [`fn_bodies`], so a test calling a helper in a sibling module that
    /// in turn calls `record` is invisible. A full transitive walk over
    /// name-matched bodies across 1,100 tests over-approximates badly --
    /// `record` and `new` are common names -- and an over-approximating
    /// guard is the ~40-false-positive mistake `check-privacy.sh:120`
    /// records. One hop covers every shape in the tree today and the
    /// reachers above by name.
    ///
    /// # What it cannot see
    ///
    /// - **Whether the lock is held for long ENOUGH.** A test taking the
    ///   guard and dropping it immediately passes. The `_g` binding idiom
    ///   (an underscore-prefixed name held to end of scope) is what the
    ///   existing tests use and what review should look for; this asserts
    ///   the lock is taken at all, which is the half that was missing six
    ///   times.
    /// - **`RestoreObserved`.** NOT asserted. The lock stops two tests
    ///   racing; the restore guard stops a seeded figure leaking into
    ///   whatever runs next, and the two were added together in #868. Only
    ///   the lock is required here, because the restore is conditional on
    ///   the test actually seeding a figure and "did this test seed one"
    ///   cannot be read off the text -- demanding it everywhere would
    ///   report the tests that only READ the static, and a guard that asks
    ///   for a line somebody then has to justify is how the ~40-false-
    ///   positive mistake starts. The lock is the half that was missing six
    ///   times.
    /// - **Async tests.** `#[tokio::test]` is skipped, and this is a limit
    ///   of the RULE, not of the scan: `observed_test_lock` hands back a
    ///   `std::sync::MutexGuard`, and clippy's `await_holding_lock` --
    ///   under the `-D warnings` that CI's `lint` job runs -- rejects
    ///   holding one across an `.await`. So an async test cannot comply.
    ///   `client.rs` has five that reach `note_remaining` through
    ///   `fetch_prs_with_total` and are latent races for that reason;
    ///   MEASURED, adding the lock to them fails the build with five
    ///   `await_holding_lock` errors. Giving the lock an async form is a
    ///   change to `budget.rs`'s test surface and wants its own issue. The
    ///   body records this at the skip.
    /// - **Other crates.** `src-mobile` and `stepup` have no `Budget`, so
    ///   the scan is scoped to `src-tauri` rather than asserting a vacuous
    ///   truth over two crates that cannot break it.
    #[test]
    fn every_test_reaching_the_observed_figure_takes_the_lock() {
        /// The names that store to `OBSERVED_REMAINING`.
        ///
        /// `note_remaining` is the setter; `record` reaches it at
        /// `budget.rs:329` and is the path all six of #868's tests took.
        /// `OBSERVED_REMAINING` itself is included because three tests
        /// store to the static directly by name.
        const STORES: &[&str] = &["note_remaining(", ".record(", "OBSERVED_REMAINING.store"];

        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut checked = 0usize;
        let mut unlocked = Vec::new();
        for file in rust_files(&manifest) {
            let Ok(src) = std::fs::read_to_string(&file) else {
                continue;
            };
            let rel = file.strip_prefix(&manifest).unwrap_or(&file).display();
            // This file is the guard itself: the names above appear here as
            // string literals and in prose. Skipped by PATH rather than by
            // comment-stripping, because a `const STORES` array is code.
            if rel.to_string().contains("invariants.rs") {
                continue;
            }
            let bodies = fn_bodies(&src);
            for t in test_fns(&format!("src-tauri/{rel}"), &src) {
                // Comment lines dropped before the search. This codebase
                // documents its rules directly above the code, so `record`
                // and `note_remaining` appear in prose far more often than
                // in a call -- and accepting a doc comment in place of the
                // code is the exact trap #869 names: three v5.14.0
                // invariants initially passed over their own defect, one of
                // them by matching a string inside a comment.
                let code: String = t
                    .body
                    .lines()
                    .filter(|l| !is_comment(l))
                    .collect::<Vec<_>>()
                    .join("\n");
                let direct = STORES.iter().any(|s| code.contains(s));
                // One hop: a helper called by this test, defined in this
                // file, that itself reaches the static.
                let indirect = !direct
                    && bodies.iter().any(|(name, body)| {
                        name != &t.name
                            && code.contains(&format!("{name}("))
                            && body
                                .lines()
                                .filter(|l| !is_comment(l))
                                .any(|l| STORES.iter().any(|s| l.contains(s)))
                    });
                if !direct && !indirect {
                    continue;
                }
                // `#[tokio::test]` is OUT OF SCOPE, and this is a limit of
                // the RULE rather than of the scan -- recorded here because
                // working around it silently is how a guard stops meaning
                // anything.
                //
                // `observed_test_lock` returns a `std::sync::MutexGuard`.
                // Clippy's `await_holding_lock` rejects holding one across
                // an `.await`, and `cargo clippy -- -D warnings` is what
                // CI's `lint` job runs, so an async test physically cannot
                // take this lock and stay green. MEASURED: adding the two
                // lines to `client.rs`'s five wiremock tests produced five
                // `await_holding_lock` errors and a failed build.
                //
                // Those five are real latent hazards, not false positives:
                // `fetch_prs_with_total` calls `note_remaining` at
                // `client.rs:1059` whenever a response carries `rateLimit`,
                // and they drive it through a mock server. They do not race
                // TODAY only because none of their mocks selects that field
                // -- which is one line away from being untrue, and is
                // exactly the shape of #868.
                //
                // Fixing it properly means giving the lock an async form (a
                // `tokio::sync::Mutex`, or a sync lock acquired around a
                // `block_in_place`), which is a change to `budget.rs`'s
                // public test surface and belongs in its own issue rather
                // than smuggled into a guard. Until then this scan covers
                // the synchronous tests -- all 14 of them, including every
                // one of #868's six -- and says plainly what it does not
                // cover.
                if t.is_async {
                    continue;
                }
                checked += 1;
                // Either spelling: `budget.rs`'s own `metering` module
                // imports it as `observed_lock`, and `fetch.rs` calls it by
                // its fully-qualified path. Matching the bare name would
                // report two correct files as defects.
                if !code.contains("observed_test_lock()") && !code.contains("observed_lock()") {
                    unlocked.push(format!("{}::{}", t.where_, t.name));
                }
            }
        }

        // Guards the guard. MEASURED at 14 today: all 13 tests in
        // `budget.rs`'s `tests` and `metering` modules, plus `fetch.rs`'s
        // `a_wave_is_refused_once_the_budget_is_under_the_reserve`. A scan
        // that found materially fewer has stopped seeing a test module,
        // which is how a derived guard dies quietly -- and the whole point
        // of #869 is that a guard passing is not evidence it can see
        // anything. The other six invariants here assert the same way, and
        // `every_recursive_delete_checks_symlinks_and_containment`'s doc
        // records this exact assertion catching a live blind spot.
        //
        // Held at 13 rather than 14 so that deleting `fetch.rs`'s wave test
        // -- a legitimate change -- does not fail this, while losing sight
        // of `budget.rs`'s module does.
        assert!(
            checked >= 13,
            "only {checked} test(s) reaching OBSERVED_REMAINING found; the scan is \
             broken, not the tests. There are 14 -- the 13 in `budget.rs`'s `tests` \
             and `metering` modules plus `fetch.rs`'s \
             `a_wave_is_refused_once_the_budget_is_under_the_reserve`."
        );
        assert!(
            unlocked.is_empty(),
            "these tests can reach the process-wide OBSERVED_REMAINING and do not take \
             `observed_test_lock()`:\n  {}\n\n\
             `cargo test` runs test functions on a thread pool and that static is \
             process-wide by design, so two tests touching it in parallel race -- and \
             the one that FAILS is whichever happened to read it, in whatever file that \
             is. `budget.rs`'s `a_seeded_budget_can_actually_refuse` failed 4 runs in 6 \
             this way, on `main`, blocking a release tag (#868).\n\n\
             `Budget::record` is not an exception: it stores to the static at \
             `budget.rs:329`, which is what all six of #868's tests missed. \
             \"My test does not mention `note_remaining`\" is not the question -- the \
             question is whether anything it calls can store to the figure. Add \
             `let _g = observed_test_lock();` and, if the test seeds a figure, \
             `let _restore = RestoreObserved::capture();`. Both are cheap, and a test \
             that does not need them loses nothing by holding them (#869).",
            unlocked.join("\n  ")
        );
    }

    // ---- Invariant 8: a wall-clock test that disclaims one -----------------

    /// A test whose doc comment disclaims a timing threshold does not
    /// assert on how long the run took.
    ///
    /// # What it enforces
    ///
    /// If a `#[test]`'s doc comment contains "wall-clock", "timing
    /// threshold" or "flake generator", then no assertion in its body may
    /// compare a RUN-SPANNING duration against a constant multiple. A
    /// run-spanning duration is the `let t = Instant::now(); <work>; let t =
    /// t.elapsed();` shape -- a stopwatch around the thing under test.
    ///
    /// # The finding it would have caught (#861)
    ///
    /// `worktrees::scan`'s `worktrees_are_classified_concurrently` carried,
    /// and still carries, this sentence: *"Asserts overlap rather than
    /// wall-clock time: a timing threshold on CI hardware is a flake
    /// generator."* Its only assertion was `whole * 2 < serial_floor`,
    /// where `whole` was a stopwatch around `classify_repo_streaming` --
    /// a wall-clock threshold, four lines under the sentence denying it.
    ///
    /// It cost two CI runs in the #835 batch on branches touching nothing
    /// near it, then failed on `main` and blocked the v5.14.0 tag. One
    /// failure had MEASURED 1.8x overlap: concurrency was working and the
    /// test rejected it anyway, which is the tell that the assertion was
    /// not merely fragile but measuring the wrong thing.
    ///
    /// The doc even named a correct model one module up --
    /// `sizing::paths_are_walked_concurrently`, which asserts `peak > 1` on
    /// a counter of simultaneously-executing workers and carries no
    /// duration at all. So the rule was written down, a working example sat
    /// in the same file, and the body ignored both.
    ///
    /// # Why the check is the STOPWATCH and not "two Durations times a
    /// constant"
    ///
    /// This is the whole difficulty of this guard and the reason it is
    /// narrow. #869 proposes flagging a comparison of "two `Instant`/
    /// `Duration` values against a constant multiple", and the fixed code
    /// is exactly that: `closest * 2 < solo`. A guard written to #869's
    /// letter fires on the FIX as loudly as on the defect, which makes it
    /// useless for telling them apart -- and a guard that cannot
    /// distinguish the defect from its repair is the cry-wolf shape
    /// `check-privacy.sh:120` records ~40 false positives from.
    ///
    /// What actually changed in #862 is which quantity is on the left:
    ///
    /// - **before**: `whole` was `Instant::now()` before
    ///   `classify_repo_streaming` and `.elapsed()` after it -- total
    ///   runtime, which a loaded CI runner inflates without the code
    ///   changing.
    /// - **after**: `closest` is `arrivals.windows(2).map(|w| w[1] - w[0])
    ///   .min()` -- the gap between two OBSERVATIONS made during the run.
    ///   Two reports cannot land closer together than the work producing
    ///   them unless that work overlapped, whatever the machine's speed.
    ///
    /// `solo` is a stopwatch too, and it stays: it is the right-hand side,
    /// the yardstick measured moments earlier on the same machine, so a
    /// slow runner moves both sides of the comparison equally. That is
    /// precisely the property the old form lacked, and it is why the check
    /// below is about the MULTIPLIED operand rather than about either
    /// operand appearing anywhere in the expression.
    ///
    /// # What it cannot see
    ///
    /// - **A test that makes the promise without the words.** The trigger
    ///   is three phrases, chosen because they are the ones this codebase
    ///   actually writes rather than an attempt to understand prose. A doc
    ///   promising overlap in other words is outside this, and #869's guard
    ///   3 -- a general prohibition-vs-body check -- is the stretch goal
    ///   that would cover it. It is deliberately NOT implemented here: #869
    ///   recommends evaluating it separately because it is likely noisy,
    ///   and this repository has already paid for one guard that cried
    ///   wolf.
    /// - **The three phrases do not all mean "disclaims".** Six tests match
    ///   today and only two are disclaimers; `health/footprint.rs` and
    ///   `health/collect.rs` write "wall-clock" to ADMIT a budget they
    ///   assert on deliberately and gate for that reason (#853). The scan
    ///   therefore cannot use the trigger alone to decide anything -- it
    ///   selects a population, and the stopwatch-multiple rule below is
    ///   what separates the defect from the four tests that are correct.
    ///   A guard keyed on "matched the phrase and compares durations" would
    ///   report both gated measurements as defects on its first run.
    /// - **A wall-clock assertion in a test that promises nothing.**
    ///   `src-mobile`'s connect-timeout tests assert on elapsed time on
    ///   purpose and say so. This guard is a consistency check between a
    ///   test's prose and its body, not a ban on timing assertions -- the
    ///   defect class #869 collects is the contradiction, not the timing.
    /// - **A stopwatch laundered through a helper.** `let t = start();` and
    ///   `let d = stop(t);` would not match the shape. Nothing in the tree
    ///   does this, and the `Instant::now()` / `.elapsed()` pair is the
    ///   only spelling in all three crates.
    #[test]
    fn no_test_asserts_wall_clock_under_a_doc_that_disclaims_it() {
        /// The phrases that make a test's doc a PROMISE about what it
        /// asserts, rather than prose that happens to mention time.
        ///
        /// All three are drawn from the two sentences already in the tree,
        /// not invented: `scan.rs:6570` and `:6988` both read "Asserts
        /// overlap rather than wall-clock time: a timing threshold on CI
        /// hardware is a flake generator." A test that writes one of these
        /// has told the next reader it carries no timing threshold, and
        /// that is the claim being held to.
        const DISCLAIMS: &[&str] = &["wall-clock", "timing threshold", "flake generator"];

        let mut checked = 0usize;
        let mut contradicted = Vec::new();
        for t in all_test_fns() {
            // The guard's own prose quotes all three phrases and the
            // defective assertion, so this file is skipped by path. Every
            // scan here does the same where it must name what it forbids.
            if t.where_.contains("invariants.rs") {
                continue;
            }
            if !DISCLAIMS.iter().any(|p| t.doc.contains(p)) {
                continue;
            }
            checked += 1;
            // Comments dropped first. The fixed test explains the old
            // defect by QUOTING `whole * 2 < serial_floor` in a comment
            // directly above the new assertion, which is the exact shape
            // #869 warns about: one v5.14.0 invariant accepted a doc
            // comment explaining a fix in place of the fix. Reading the
            // comments here would make the repaired test fail and the
            // sabotaged one fail identically -- the guard would be blind
            // in the one way that matters.
            let code: Vec<&str> = t.body.lines().filter(|l| !is_comment(l)).collect();

            // Every local binding that is a STOPWATCH: bound from
            // `Instant::now()` and read back later in the same body through
            // `NAME.elapsed()`. Both `solo` and `whole` match, and so would
            // any future name -- nothing here is keyed to the two the
            // defect happened to use.
            //
            // No minimum gap between the two lines is required, and the
            // honest reason is that it would not buy anything: `rustfmt`
            // keeps them on separate lines regardless, and a stopwatch
            // started and read with nothing in between measures zero and
            // cannot be the left side of a threshold anybody wrote on
            // purpose. Demanding a gap would add a number to tune and a way
            // for the scan to miss a real one.
            //
            // What makes this a stopwatch AROUND THE WORK rather than
            // merely a duration is the pairing itself: the value did not
            // come from an observation made during a run, it came from
            // timing a span of this test's own control flow. That is the
            // distinction invariant 8 rests on -- see its doc.
            let mut stopwatches: Vec<String> = Vec::new();
            for (i, line) in code.iter().enumerate() {
                let Some(rest) = line.trim_start().strip_prefix("let ") else {
                    continue;
                };
                if !line.contains("Instant::now()") {
                    continue;
                }
                let name = rest
                    .split([' ', ':', '='])
                    .next()
                    .unwrap_or_default()
                    .trim_start_matches("mut ")
                    .trim()
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                // Read back later in the same body, through `.elapsed()`.
                if code[i + 1..]
                    .iter()
                    .any(|l| l.contains(&format!("{name}.elapsed()")))
                {
                    stopwatches.push(name);
                }
            }

            // Whether the body ASSERTS at all. A multiple computed for a
            // `println!` in an `#[ignore]`d benchmark is a measurement
            // being reported, not a threshold being enforced, and
            // `worktrees/scan.rs`' `mod live` is full of exactly that --
            // five `Instant`/`elapsed` pairs feeding print statements with
            // no timing assertion anywhere. Reporting those would be the
            // cry-wolf failure, so an assertion is required before a
            // multiple means anything.
            let asserts: String = code
                .iter()
                .filter(|l| l.contains("assert"))
                .copied()
                .collect::<Vec<_>>()
                .join("\n");
            // The multiple is searched for over the WHOLE body rather than
            // over the assertion lines, because `rustfmt` puts the operand
            // on its own line: the real defect reads
            //
            //     assert!(
            //         whole * 2 < serial_floor,
            //
            // so a line containing `assert` and a line containing the
            // multiple are never the same line. Matching within the
            // assertion lines alone finds nothing, which is how this guard
            // would have passed over #861 while looking correct.
            let body_code = code.join("\n");
            for name in &stopwatches {
                // `NAME * <anything>`, rather than a list of multipliers.
                //
                // Enumerating them was the first version and it is the
                // list-based blind spot this module's header is about:
                // `whole * 2` was covered and `solo * count as u32` -- the
                // OTHER multiplied stopwatch in the same defect -- was not,
                // so the first run of the sabotage reported one of the two.
                // A scaled stopwatch is a threshold whatever the scale is
                // spelled as.
                //
                // `Duration` implements `Mul<u32>` and not the reverse, so
                // `NAME * x` is the only spelling that compiles; the mirror
                // form does not need matching.
                let multiple = body_code.contains(&format!("{name} * "));
                if multiple && !asserts.is_empty() {
                    contradicted.push(format!(
                        "{}::{} -- `{name}` is a stopwatch around the work and the body \
                         asserts on a multiple of it",
                        t.where_, t.name
                    ));
                }
            }
        }

        // Guards the guard, the way the other seven do. MEASURED at 6
        // today, and the identities matter more than the number because
        // two of the six are the reason this check is about a MULTIPLE and
        // not about any duration comparison:
        //
        // - `scan.rs::worktrees_are_classified_concurrently` -- #861's
        //   test, now correct.
        // - `scan.rs::paths_are_walked_concurrently` -- the model its doc
        //   cites; asserts `peak > 1` and carries no duration at all.
        // - `health/footprint.rs::a_sample_is_cheap_enough_for_a_timer` and
        //   `health/collect.rs::reading_the_gpu_is_cheap_enough_for_the_sampler`
        //   -- these say "wall-clock" to ADMIT one, not to disclaim it:
        //   both assert `elapsed < <constant>` deliberately and are gated
        //   behind `#[ignore]` plus an env var for precisely that reason
        //   (#853). They are in scope of the scan and must not be reported,
        //   which a rule phrased as "no duration comparison" would get
        //   wrong in both cases.
        // - `packages/tools.rs::a_missing_tool_is_not_looked_up_twice` --
        //   a test that WAS a wall-clock proxy and now asserts on the cache
        //   instead; its doc explains the fix, and it has no `Instant` left.
        // - `github/stats/board.rs::a_board_load_is_bounded_once_around_the_whole_thing`
        //   -- a source scan about where a timeout sits; no clock.
        //
        // A scan finding fewer has stopped reading doc comments, which
        // would make the assertion below vacuously true forever -- the
        // failure mode #869 names. This threshold found a real one: the
        // phrases are wrapped by `rustfmt`'s comment width, so a
        // newline-joined doc never matched "timing threshold". See
        // [`test_fns`].
        assert!(
            checked >= 6,
            "only {checked} test(s) found whose doc disclaims or admits a timing \
             threshold; the scan is broken, not the tests. There are six -- two in \
             `worktrees/scan.rs`, two gated live measurements in `health/`, and one \
             each in `packages/tools.rs` and `github/stats/board.rs`."
        );
        assert!(
            contradicted.is_empty(),
            "these tests promise in prose that they assert overlap rather than \
             wall-clock time, and then assert on wall-clock time:\n  {}\n\n\
             A stopwatch around the work measures the MACHINE as much as the code, so \
             the threshold fails on a loaded CI runner while the property holds. \
             `worktrees_are_classified_concurrently` asserted `whole * 2 < \
             serial_floor` under exactly this doc comment: two CI runs lost in the #835 \
             batch, then a failure on `main` that blocked the v5.14.0 tag -- one of them \
             having MEASURED 1.8x overlap, so concurrency was working and the test \
             rejected it anyway (#861).\n\n\
             Assert the overlap itself. `sizing::paths_are_walked_concurrently` counts \
             simultaneously-executing workers and asserts `peak > 1`, which is true at \
             any machine speed. Where the callback is serialised and a count cannot \
             work, compare OBSERVATIONS made during the run -- \
             `worktrees_are_classified_concurrently` now takes the gap between the two \
             closest report arrivals against a solo cost measured moments earlier on \
             the same machine, so a slow runner moves both sides equally. Do not widen \
             the threshold and do not add a retry: a widened wall-clock bound is still \
             a wall-clock bound, and #811's retry is the lesson on the other (#869).",
            contradicted.join("\n  ")
        );
    }
    /// Liveness never reads the failure and denial events (#1062).
    ///
    /// `install.rs` has recorded since #910 that `StopFailure` "does not
    /// fire on SIGKILL, so it adds error context rather than liveness",
    /// and #1062 installs the event while carrying that constraint
    /// forward. The defect it forbids is specific and tempting: a turn
    /// that died of a rate limit looks like evidence a session is over,
    /// and it is not -- the session is very much still running, and a
    /// liveness that consulted this would report every rate-limited
    /// session as dead.
    ///
    /// The separation is structural: these events live in `claude_hook_event`
    /// and liveness reads `claude_run`. This guard is what stops a later
    /// well-meaning join from erasing that, because nothing about
    /// `liveness.rs` in isolation says which table it may not touch.
    ///
    /// # Scoped to the FILE, not to a function
    ///
    /// The `guard` skill's third recorded mistake is scoping too coarsely,
    /// and this looks like a case for scoping to `derive`. It is not: the
    /// hazard is not one function reading the table, it is the DEPENDENCY
    /// existing at all -- a private helper, a new `runs_with_failures`
    /// query, or a `use super::events` at the top of the file are each the
    /// same defect arriving by another route, and a function-scoped guard
    /// would see none of them. The file is the unit that owns the
    /// constraint.
    ///
    /// PROVEN BY SABOTAGE, both directions, and the second one FOUND A
    /// DEFECT IN THIS GUARD:
    ///   - adding `let _ = "SELECT 1 FROM claude_hook_event";` to
    ///     `liveness::derive` fails this, naming the file and the rule.
    ///   - adding a module-doc paragraph to `liveness.rs` explaining why
    ///     it must never read `claude_hook_event` ALSO failed it, at first. The
    ///     guard had been written assuming `production` strips comments;
    ///     it does not, it strips `#[cfg(test)]` blocks. That is exactly
    ///     #874's recorded mistake -- a guard matching its pattern inside
    ///     a doc comment -- and it would have made this file undocumentable
    ///     on its own most important constraint. Fixed by filtering
    ///     `is_comment` lines here, after which the same prose passes and
    ///     the code sabotage still fails.
    ///   - `sessions.rs` legitimately reads BOTH tables (it assembles the
    ///     detail pane) and is not flagged, because the guard names
    ///     `liveness.rs` alone rather than sweeping the module.
    #[test]
    fn liveness_never_reads_the_failure_events() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let path = manifest.join("src/claude/liveness.rs");
        let src = std::fs::read_to_string(&path).expect("read claude/liveness.rs");

        // Comments stripped, and `production` does NOT do it -- it removes
        // `#[cfg(test)]` blocks only. That distinction is #874's recorded
        // mistake and it was caught here by sabotage rather than by
        // foresight: an earlier version of this guard claimed `production`
        // stripped comments, and adding a module-doc paragraph to
        // liveness.rs explaining why it must not read `claude_hook_event` made
        // the guard fail on the documentation of the rule it enforces.
        //
        // This codebase documents rules directly above the code they
        // govern, so these needles appear in prose far more often than in
        // a call. `is_comment` covers `//`, `///` and `//!` alike.
        //
        // CRLF is normalised first: on a Windows checkout every line would
        // otherwise carry a trailing `\r`, which changes nothing for
        // `contains` here but is the house rule for any line-oriented scan
        // and costs nothing to keep.
        let body: String = production(&src.replace("\r\n", "\n"))
            .lines()
            .filter(|l| !is_comment(l))
            .collect::<Vec<_>>()
            .join("\n");

        for needle in [
            "claude_hook_event",
            "events::",
            "StopFailure",
            "PermissionDenied",
        ] {
            assert!(
                !body.contains(needle),
                "claude/liveness.rs mentions `{needle}` in code. #1062 is \
                 explicit that the failure events must not inform whether a \
                 session is running: StopFailure does not fire on SIGKILL, \
                 so a session that hit a rate limit and is still alive \
                 would be reported as dead. Liveness reads `claude_run`; \
                 these events live in `claude_hook_event`, and the two must stay \
                 apart."
            );
        }

        // The negative direction, per the `guard` skill: prove this is not
        // merely passing because the file is empty or because the scan
        // reads nothing.
        //
        // The anchor is `fn derive` and its `runs` parameter, NOT the
        // string `claude_run` -- and that correction was itself found by
        // running this. `liveness.rs` never names the table in code at
        // all: it is handed a `&[Run]` by `sessions.rs`, which owns the
        // SQL. Asserting on the table name would have been asserting on a
        // string that only ever appears in this file's comments, which is
        // the same doc-comment confusion the needles above guard against,
        // one direction over.
        //
        // That indirection is also WHY the constraint holds so cheaply:
        // liveness cannot read a table it is never given a connection to.
        assert!(
            body.contains("fn derive"),
            "liveness.rs no longer defines `derive`, so the needles above \
             are scanning a file that no longer decides liveness -- they \
             would stay silent however the decision was made instead"
        );
        assert!(
            body.contains("runs"),
            "liveness.rs no longer takes the runs it derives from, so this \
             guard is passing for the wrong reason"
        );
    }

    /// Every crash notifier is WIRED, and reads the right count (#979).
    ///
    /// This is `every_registered_command_is_reachable_from_the_desktop`
    /// applied to a notification instead of a command, and it exists for
    /// the same reason that one does: #947's defect was a complete,
    /// correct, well-tested capability that nothing on the desktop
    /// called, and every test passed the whole time. A `notify_*` function
    /// with no caller in the poll loop is that failure exactly -- the
    /// alert would be implemented, classified, preferenced and dead.
    ///
    /// It also pins the count the notifier reads, which is the one thing
    /// a unit test of the notifier itself cannot see. `crash::Recorded`
    /// splits `crashed` (first observations) from `crashed_already_known`
    /// (the same orphans, still on disk), and `crash.rs` records that
    /// without the split an orphan "would appear to have crashed a few
    /// seconds ago, forever". A notifier reading the wrong one fires every
    /// sixty seconds until the file is removed, and nothing about the
    /// notifier in isolation says which it reads.
    ///
    /// Sabotage: delete the `notify_claude_crash` call from the poll loop
    /// in `lib.rs`, or swap `crashed_sessions` for `crashed_already_known`
    /// there, and this names the failure.
    #[test]
    fn the_claude_crash_notifier_is_wired_and_reads_first_observations() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let lib = std::fs::read_to_string(manifest.join("src/lib.rs")).expect("read lib.rs");
        let body = production(&lib);

        assert!(
            body.contains("fn notify_claude_crash"),
            "the notifier itself is gone; if #979 was reverted, remove this \
             guard deliberately rather than leaving it asserting nothing"
        );
        // Called, not merely defined. Counted, because the definition
        // line also matches the name.
        assert!(
            body.matches("notify_claude_crash").count() >= 2,
            "`notify_claude_crash` is defined and never called -- which is \
             #947's defect with a notification in place of a command: the \
             alert is implemented, classified, preferenced and dead"
        );

        // The poll loop's Claude arm, which is where it must be called
        // from: the sweep that produces the signal runs there, on the
        // 60-second timer, and a notifier anywhere else would be reading
        // a sweep somebody else ran.
        //
        // Split on the gate EXPRESSION, not on the bare field name. The
        // field is also named in two comments above the arm, and
        // `production()` strips test modules rather than comments -- so a
        // bare-name split lands on prose and this guard reported a
        // correctly-wired notifier as missing. Found by running it.
        let arm = body
            .split_once("if commands::read_ui_prefs(&app_handle).claude_integrations_enabled {")
            .map(|(_, rest)| rest)
            .expect(
                "locate the Claude arm of the poll loop by its gate expression; \
                 if that line was reworded, update this split rather than \
                 deleting the guard",
            );
        let arm = &arm[..arm.len().min(3_000)];
        // CODE only. The arm's own comments name `crashed_already_known`
        // to explain why it is not read, and a scan that could not tell
        // code from prose would report the explanation as the defect --
        // which it did, on the first run of this guard. A guard that
        // cannot distinguish the two is a guard that fails on being
        // documented.
        let code: String = arm
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let arm = code.as_str();
        assert!(
            arm.contains("notify_claude_crash"),
            "the crash notifier must be called from the poll loop's Claude \
             arm, inside the `claude_integrations_enabled` gate -- a user \
             who turned the feature off must not be interrupted by it"
        );
        assert!(
            arm.contains("crashed_sessions"),
            "the notifier must read `crashed_sessions`, which is built on \
             the same arm that increments `crashed` -- the FIRST-observation \
             count"
        );
        assert!(
            !arm.contains("crashed_already_known"),
            "the notifier must not read `crashed_already_known`: an orphan \
             left on disk is re-swept every minute, so that count would \
             announce the same dead session forever (`crash.rs`'s COALESCE \
             is what makes the split true)"
        );
        assert!(
            arm.contains("claude_crashed"),
            "the notification must be gated on its own preference as well \
             as on the feature switch, or turning it off does nothing"
        );
    }

    /// Every registered command is REACHABLE from the desktop (#947).
    ///
    /// `claude_poll_live` shipped in #927 registered in `generate_handler!`,
    /// classified in the remote surface, wired into the phone's dispatch
    /// arm, and covered by ten passing tests -- with no caller on the
    /// machine that needs it. So the hook it exists to consume wrote a
    /// file nothing read, `claude_run` stayed empty in production, and
    /// every one of those tests passed the whole time.
    ///
    /// Nothing could have caught it. The tests call the functions
    /// directly, and the remote dispatch arm makes the command genuinely
    /// reachable -- just not from the desktop. This is the module's own
    /// thesis in miniature: a property about the set of call sites, which
    /// no behavioural test at one call site can express.
    ///
    /// A command is reachable if EITHER:
    ///   - `src/api/tauri.ts` names it, so the frontend can invoke it, or
    ///   - some production Rust outside `commands.rs` calls it, which is
    ///     how a background timer reaches one.
    ///
    /// Being in the remote dispatch arm is deliberately NOT enough: that
    /// is the phone asking the desktop, and a desktop-only capability
    /// reachable only from a paired phone is exactly the defect.
    #[test]
    fn every_registered_command_is_reachable_from_the_desktop() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let lib = std::fs::read_to_string(manifest.join("src/lib.rs")).expect("read lib.rs");

        // The handler list, discovered rather than enumerated: every
        // `commands::name,` line inside `generate_handler!`.
        // Scoped to the `generate_handler!` block, not the whole file:
        // the poll loop also writes `commands::read_ui_prefs(..)`, which
        // is a CALL rather than a registration, and a whole-file scan
        // reports it as an unreachable command. The macro invocation is
        // the only place a registration can appear.
        let block = lib
            .split_once("generate_handler!")
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split_once("])"))
            .map(|(inside, _)| inside)
            .expect("locate the generate_handler! block in lib.rs");
        let names: Vec<String> = block
            .lines()
            .filter_map(|l| l.trim().strip_prefix("commands::"))
            .filter_map(|l| l.strip_suffix(','))
            .map(str::to_owned)
            .collect();
        assert!(
            names.len() > 50,
            "expected to discover the handler list; found {} entries, so the \
             `commands::name,` shape this scan depends on has changed",
            names.len()
        );

        let ts = std::fs::read_to_string(manifest.join("../src/api/tauri.ts"))
            .expect("read src/api/tauri.ts");

        // Production Rust outside `commands.rs` itself. A command calling
        // a sibling command inside that file is not a desktop entry point.
        let rust: String = rust_files(&manifest.join("src"))
            .into_iter()
            // `remote/surface.rs` is EXCLUDED, and that exclusion is the
            // whole point rather than a convenience: its dispatch arm
            // spells `commands::name(app.clone())` for every remotely
            // reachable command, so counting it would make every
            // `Class::Read` command look called and this guard would
            // pass on the exact defect it exists for. Verified by
            // sabotage: with the file included, removing the Claude live
            // pass's caller still passed.
            //
            // `commands.rs` itself: a command calling a sibling command
            // in the same file is not a desktop entry point.
            .filter(|p| {
                !p.ends_with("commands.rs")
                    && !p.ends_with("invariants.rs")
                    && !p.ends_with("remote/surface.rs")
            })
            .filter_map(|p| std::fs::read_to_string(&p).ok().map(|src| production(&src)))
            .collect::<Vec<_>>()
            .join("\n");

        // `diag_log` is the one documented exception and says so in
        // `surfaceGuard.test.ts`: it is called through the raw `call()`
        // helper rather than a named wrapper, so `tauri.ts` never spells
        // it. Named here, with its reason, rather than silently skipped.
        const CALLED_THROUGH_RAW_INVOKE: &[&str] = &["diag_log"];

        // A command whose body is a thin wrapper over a shared function
        // is reachable when that FUNCTION is called, not the command.
        // `claude_poll_live` is the case: it and the background timer
        // both call `claude_live_pass`, so the ordering rules inside it
        // exist once rather than twice (#947). Requiring the command
        // name here would push a caller into wrapping the async command
        // just to satisfy a guard, which is the wrong shape.
        //
        // Discovered from the source rather than listed: the body of a
        // one-line delegating command names the function it forwards to,
        // so a command is also reachable if some production Rust calls
        // any `fn` that `commands.rs` shows it delegating to.
        let commands_src =
            std::fs::read_to_string(manifest.join("src/commands.rs")).expect("read commands.rs");
        let delegates_to = |name: &str| -> Option<String> {
            let at = commands_src.find(&format!("pub async fn {name}("))?;
            let body = &commands_src[at..];
            let body = &body[..body.find("\n}\n").unwrap_or(body.len())];
            // `spawn_blocking(move || some_fn(..))` -- the shape the
            // extraction in #947 produced. Everything after the marker,
            // up to the first `(`, is the delegated function's name.
            let marker = "spawn_blocking(move || ";
            let after = body.split_once(marker)?.1;
            let name = after.split_once('(')?.0.trim();
            (!name.is_empty() && !name.contains(char::is_whitespace)).then(|| name.to_string())
        };

        // EMPTY, and #964 is why.
        //
        // This list held `apply_package_updates` and `open_update_pr`:
        // superseded by #626's `apply_updates_in_background` and never
        // unregistered, so they stayed remotely dispatchable at
        // `Destructive` and `Write` with nothing on the desktop calling
        // them. The entry's own comment said "when #964 lands, these
        // lines come out with it", and the assertion below is what made
        // that more than a note -- an exemption for a command that is no
        // longer registered fails, so the fix could not land half-done.
        //
        // #964 unregistered both and dropped their rows from
        // `remote/surface.rs`, its `dispatch`, and
        // `src-mobile/src/surface.rs`. The helpers they wrapped
        // (`open_update_pr_inner`, `packages::apply::run`) are the live
        // code and stay.
        //
        // Kept as an empty list rather than deleted along with the loop
        // below: the next superseded command wants exactly this
        // structure, and the pair of assertions -- one for unreachable
        // commands, one for exemptions that have stopped being needed --
        // is the mechanism, not the entries.
        const KNOWN_UNREACHABLE: &[&str] = &[];

        let mut unreachable = Vec::new();
        let mut stale_exemptions = Vec::new();
        for name in &names {
            if CALLED_THROUGH_RAW_INVOKE.contains(&name.as_str()) {
                continue;
            }
            if ts.contains(name.as_str()) {
                continue;
            }
            // `commands::name(` from another module, or a re-export used
            // as a path. Either is a real desktop caller.
            if rust.contains(&format!("commands::{name}(")) {
                continue;
            }
            // The delegation case described above.
            if let Some(target) = delegates_to(name) {
                if !target.is_empty() && rust.contains(&format!("commands::{target}(")) {
                    continue;
                }
            }
            if KNOWN_UNREACHABLE.contains(&name.as_str()) {
                continue;
            }
            unreachable.push(name.clone());
        }

        // The exemption list must not outlive what it excuses. A command
        // named here that HAS become reachable means #964 landed and the
        // entry should go, so this fails rather than quietly passing --
        // the failure mode a hand-written list otherwise has (#844).
        for name in KNOWN_UNREACHABLE {
            let reachable = ts.contains(name) || rust.contains(&format!("commands::{name}("));
            if reachable || !names.iter().any(|n| n == name) {
                stale_exemptions.push((*name).to_string());
            }
        }
        assert!(
            stale_exemptions.is_empty(),
            "KNOWN_UNREACHABLE names {} that no longer needs excusing:\n  {}\n\n\
             It is now called, or no longer registered. Remove it from the list.",
            if stale_exemptions.len() == 1 {
                "a command"
            } else {
                "commands"
            },
            stale_exemptions.join("\n  ")
        );

        assert!(
            unreachable.is_empty(),
            "{} registered command(s) have no desktop caller:\n  {}\n\n\
             A command in `generate_handler!` with no `tauri.ts` wrapper and no \
             production Rust caller can only be invoked by a paired phone. If it is a \
             background pass, call it from the timer in `lib.rs` the way the Claude \
             live pass is (#947). If the frontend should drive it, add the wrapper. If \
             it is superseded, unregister it -- leaving it registered keeps it \
             remotely dispatchable while nothing on the desktop uses it.\n\n\
             `claude_poll_live` shipped in exactly this state and its ten tests all \
             passed while `claude_run` stayed empty in production.",
            unreachable.len(),
            unreachable.join("\n  ")
        );
    }

    /// No test mock anywhere may supply `rateLimit.remaining` (#1048).
    ///
    /// # The defect this is the generalisation of
    ///
    /// `client.rs` already carries this rule, as
    /// `no_mock_here_arms_the_process_wide_budget_race` (#875), and its
    /// doc comment states the hazard exactly: a mock response carrying
    /// `rateLimit.remaining` makes `map_rate_limit` return `Some`, which
    /// calls `budget::note_remaining`, which stores to the PROCESS-WIDE
    /// `OBSERVED_REMAINING`. Async tests structurally cannot take
    /// `observed_test_lock()` -- it returns a `std::sync::MutexGuard` and
    /// clippy's `await_holding_lock` under `-D warnings` refuses to let
    /// one be held across an `.await` -- so such a mock arms a race that
    /// surfaces as a flake in `budget.rs`, in another file entirely.
    ///
    /// That guard scanned ONE file: `include_str!("client.rs")`. #1044
    /// then added async mock-server tests to `board.rs`, whose two fixture
    /// bodies each supplied `"remaining": 4000` -- the precise arming
    /// condition, in the one file the guard could not see.
    ///
    /// It was not hypothetical. It failed `platform (windows-latest)` on
    /// the v5.20.0 tag with
    /// `the_observed_figure_recovers_after_the_window_rolls_over` panicking
    /// at its FIRST assertion ("under the reserve, so refused"), which
    /// reads the figure right after writing 80. The release gate refused
    /// the tag and published nothing. Measured directly, by probing the
    /// static at the end of the board load: `Some(4000)`.
    ///
    /// # Why a tree-wide scan rather than a second copy
    ///
    /// A per-file guard protects the file it names and nothing else, and
    /// the next mock-server test in a third file would re-arm this in
    /// exactly the same way. This walks every Rust file in both crates, so
    /// a new file is covered without anyone remembering to add it -- the
    /// property this module's own header argues for.
    #[test]
    fn no_test_mock_arms_the_process_wide_budget_race() {
        let mut offenders: Vec<String> = Vec::new();
        for (_, root) in crate_roots() {
            for file in rust_files(&root) {
                let Ok(src) = std::fs::read_to_string(&file) else {
                    continue;
                };
                let rel = file
                    .strip_prefix(&root)
                    .unwrap_or(&file)
                    .display()
                    .to_string();
                // This file states the rule in prose and in its own
                // assertion message; a guard that reports itself reports
                // nothing useful. Skipped by PATH, never by comment
                // matching, for the reason #874's sabotage proof found:
                // accepting a comment in place of code is the trap these
                // source-reading guards fall into.
                if rel.contains("invariants.rs") {
                    continue;
                }
                // Line endings normalised before any byte pattern runs:
                // a Windows checkout with `core.autocrlf` has CRLF, and a
                // `\n`-anchored split would find no test module and scan
                // nothing -- a silent pass on the one platform where this
                // defect actually fired.
                let src = src.replace("\r\n", "\n");
                // The TEST module only. A `rateLimit` selection in a real
                // document is the feature working as intended.
                let Some((_, tests)) = src.split_once("\nmod tests {") else {
                    continue;
                };
                let lines: Vec<&str> = tests.lines().collect();
                let _ = &lines;
                // Scoped to the TEST FUNCTION, not the file (#1050).
                //
                // A file-level "does this module contain `#[tokio::test]`"
                // check was the first shape of this guard, and it is too
                // coarse the moment one file holds both kinds: adding an
                // async test to `fetch.rs` made its three SAFE sync mocks --
                // which hold `observed_test_lock` and restore the static --
                // look like offenders. The hazard belongs to the individual
                // test that cannot take the lock, so that is what is asked
                // about.
                for t in test_fns(&format!("src-tauri/{rel}"), &src) {
                    if !t.is_async {
                        continue;
                    }
                    let body: Vec<&str> = t.body.lines().collect();
                    for (n, line) in body.iter().enumerate() {
                        let trimmed = line.trim_start();
                        if trimmed.starts_with("///") || trimmed.starts_with("//") {
                            continue;
                        }
                        // `remaining` is the load-bearing field: it is what
                        // `map_rate_limit` needs to return `Some`. A mock may
                        // carry `cost` or `resetAt` without arming anything.
                        if !(line.contains("remaining") && line.contains(':')) {
                            continue;
                        }
                        let lo = n.saturating_sub(4);
                        if body[lo..(n + 2).min(body.len())]
                            .iter()
                            .any(|l| l.contains("rateLimit"))
                        {
                            offenders.push(format!("{rel}: {}", line.trim()));
                        }
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "a test mock supplies `rateLimit.remaining`, which writes the \
             process-wide OBSERVED_REMAINING and arms the cross-test race \
             that async tests cannot lock against (#1048, and #875 before \
             it). Drop the field from the mock -- no test needs it -- or \
             adopt an async form of `observed_test_lock` and delete this \
             guard deliberately. Offending lines:\n  {}",
            offenders.join("\n  ")
        );
    }

    /// Every INSTALLED hook event has a row in the record-size table.
    ///
    /// # The rule, and why prose could not carry it
    ///
    /// `hook.rs`'s no-lock concurrency claim rests on each record fitting
    /// one small `write(2)`, and
    /// `hook::tests::every_events_worst_case_record_stays_small` is what
    /// holds the line -- but only for the events it lists. Its own doc
    /// says so out loud: *"if a future event carries BOTH a subagent
    /// identity and a capped message, this test will not have covered it
    /// -- so that event must add its own row to the table below."*
    ///
    /// That is a rule stated in prose, next to a table, asking the next
    /// person to remember. Epic #1060 has six sub-issues each adding an
    /// event, written by different people at different times; the
    /// `guard` skill's whole argument is that this is the shape that
    /// gets missed. So it is checked instead.
    ///
    /// The failure it prevents is not a test failure. An event installed
    /// with no measured worst case can emit a line over 512 bytes, and
    /// the symptom is a TORN LINE in a user's handoff file -- a record
    /// that parses as nothing, in a file nobody looks at, on somebody
    /// else's machine.
    ///
    /// # Derived, not enumerated
    ///
    /// Both sides are read out of the source at runtime: the installed
    /// list from `install.rs`'s `EVENTS`, the measured list from the
    /// case table in `hook.rs`. Neither is restated here, so adding an
    /// event to either file is covered without anyone editing this
    /// guard. `EVENTS` is parsed from the production half only --
    /// `install.rs`'s test module also contains an `EXPANDED` fixture
    /// naming events that are deliberately NOT installed, and a scan
    /// that read both would demand rows for events nothing emits.
    ///
    /// PROVEN BY SABOTAGE, both directions. Adding `"PostCompact"` to
    /// `EVENTS` without a table row fails here naming `PostCompact`;
    /// renaming the `SubagentStart` row out of the table fails naming
    /// `SubagentStart`. With the tree as it stands it is silent, which is
    /// the other half of the proof.
    ///
    /// # It shipped broken once, on Windows only
    ///
    /// Recorded because the lesson is worth more than the fix. The first
    /// version matched rustfmt's literal indentation for a multi-line
    /// row, `(\n                "Name",`. Every sabotage passed, the
    /// negative direction was silent, and the guard was green on macOS --
    /// then `platform (windows-latest)` failed it naming three events
    /// that were plainly in the table, because a CRLF checkout makes
    /// every `\n` a `\r\n` and the pattern matched nothing.
    ///
    /// `src-tauri/CLAUDE.md` states that rule in so many words, and it
    /// was still missed, because sabotaging a guard proves it reacts to
    /// the DEFECT and says nothing about the platform it runs on. The
    /// failure was reproduced locally by converting `hook.rs` to CRLF,
    /// which fails the old matcher with the same three event names and
    /// passes the current one -- so the fix is tested rather than
    /// assumed.
    ///
    /// Hence stripping ALL whitespace rather than normalising `\r\n`:
    /// it removes the line-ending dependency and the dependency on
    /// rustfmt's one-line-versus-four choice at the same time, so a later
    /// `cargo fmt` cannot resurrect this in a new costume.
    #[test]
    fn every_installed_hook_event_has_a_measured_worst_case_record() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let install = std::fs::read_to_string(manifest.join("src/claude/install.rs"))
            .expect("read claude/install.rs");
        let hook = std::fs::read_to_string(manifest.join("src/claude/hook.rs"))
            .expect("read claude/hook.rs");

        // Production only: the test module's `EXPANDED` fixture names
        // events the epic proposes but does not install, and they have no
        // business being required here.
        let installed_src = production(&install);
        let decl = installed_src
            .split_once("pub const EVENTS: &[&str] = &[")
            .map(|(_, rest)| rest)
            .expect(
                "locate `EVENTS` in claude/install.rs; if it was reshaped, \
                 update this guard rather than deleting it",
            );
        let decl = decl
            .split_once("];")
            .map(|(head, _)| head)
            .expect("`EVENTS` has no closing bracket");
        let installed: Vec<String> = decl
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with("//"))
            .flat_map(|l| l.split(','))
            .filter_map(|t| {
                let t = t.trim().trim_end_matches(',').trim();
                t.strip_prefix('"')?.strip_suffix('"').map(str::to_owned)
            })
            .collect();
        assert!(
            installed.len() >= 2,
            "parsed {installed:?} out of `EVENTS`, which cannot be right -- \
             the declaration's shape changed and this guard is now reading \
             nothing"
        );

        // The measured set: the `(event, payload)` rows of the size
        // table. Matched on the row shape rather than on a list, so a row
        // added in any order is seen.
        let table = hook
            .split_once("fn every_events_worst_case_record_stays_small()")
            .map(|(_, rest)| rest)
            .expect(
                "locate the record-size table in claude/hook.rs; if it was \
                 renamed, update this guard rather than deleting it",
            );
        let table = &table[..table.len().min(4_000)];

        // WHITESPACE-INSENSITIVE, and that is the fix for a real
        // Windows-only failure rather than tidiness.
        //
        // The first version matched the literal `(\n                "Name",`
        // -- rustfmt's own indentation for a multi-line row. It passed on
        // macOS and failed on windows-latest naming three events, because
        // a CRLF checkout makes every `\n` a `\r\n` and the pattern
        // matched nothing. Exactly the trap `src-tauri/CLAUDE.md` states:
        // "Normalise \r\n before any \n-anchored byte pattern, or it
        // matches nothing on a CRLF checkout and passes silently on
        // Windows only." Here it did not pass silently -- it failed
        // loudly, on the opposite platform -- which is the better of the
        // two outcomes and still a defect.
        //
        // Stripping ALL whitespace removes the dependency on line endings
        // AND on rustfmt's choice of one line versus four, so a row
        // reformatted by a later `cargo fmt` cannot make this guard report
        // a missing measurement that is right there.
        let squashed: String = table.chars().filter(|c| !c.is_whitespace()).collect();

        let missing: Vec<&String> = installed
            .iter()
            .filter(|e| !squashed.contains(&format!("(\"{e}\",")))
            .collect();

        assert!(
            missing.is_empty(),
            "these hook events are INSTALLED but have no row in \
             `every_events_worst_case_record_stays_small`, so nothing \
             measures whether the line they write fits the single-write \
             regime the no-lock append design was measured in. The symptom \
             of getting this wrong is a torn line in a user's handoff file, \
             not a test failure. Add a row carrying the event's real \
             payload fields at their worst case: {missing:?}"
        );
    }

    // ---- Invariant 9: a sync command that blocks the runtime ---------------

    /// No synchronous `#[tauri::command]` shells out or reads a whole
    /// file.
    ///
    /// # What it enforces
    ///
    /// A `#[tauri::command]` that is not `async fn`, and whose body names
    /// a subprocess spawn or a whole-file read, must hand that work to
    /// `spawn_blocking`. A plain `fn` command runs ON the async runtime's
    /// worker and blocks it, so the whole UI freezes -- not just the view
    /// that asked -- and on the remote surface it stalls the phone's HTTP
    /// listener for every other request.
    ///
    /// # What it replaces
    ///
    /// `commands::tests::docker_commands_never_block_the_async_runtime`,
    /// which asserted the same rule -- in the same words, *"a plain `fn`
    /// stalls the runtime and freezes the entire UI"* -- over an allowlist
    /// of FOUR hardcoded names. #1090's finding is that the siblings of
    /// those four were among the offenders: `docker_dangling_volumes` and
    /// `docker_running_containers` shell out and were both plain `fn`,
    /// one name away from a rule that already covered them in spirit.
    ///
    /// The proof that this was live rather than theoretical is an
    /// asymmetry: `remote/surface.rs` wrapped those two in `blocking()`
    /// for the phone, citing #496 as *"this bug in the webview"*, while
    /// the desktop called the same two functions synchronously. One path
    /// knew; the other did not; nothing checked.
    ///
    /// # Why subprocesses and whole-file reads, and NOT SQLite
    ///
    /// This is the part that decides whether the guard survives. A rule
    /// of "reaches the filesystem or SQLite" flags 18 commands here, 15
    /// of which are single-row settings reads -- `get_notify_prefs`,
    /// `get_ui_prefs`, `get_cleanup_prefs`, `get_poll_interval` and their
    /// setters. Those are one indexed lookup against a local database
    /// file, they are what the app does on every window focus, and a
    /// guard demanding `spawn_blocking` around each of them would be
    /// weakened or deleted within a week -- which is the outcome
    /// `check-privacy.sh:120` records ~40 false positives buying.
    ///
    /// So the trigger is the UNBOUNDED work, which is what actually
    /// froze the UI in every incident this rule has a number for:
    ///
    /// - **a subprocess**, because its cost is another program's. #496
    ///   was five seconds of `docker`; `assessed_worktrees` was `git
    ///   rev-parse HEAD` once per worktree over ~295 of them, on a
    ///   five-second refresh cadence.
    /// - **a whole-file read**, because nothing bounds the file.
    ///   `read_claude_md` was `read_to_string` of an arbitrary path,
    ///   dispatched inline on the remote surface.
    ///
    /// `std::fs::metadata` is deliberately NOT in the list.
    /// `claude_reveal_path` stats one path to tell "deleted" from "cannot
    /// tell", which is a single syscall, and flagging it would be the
    /// false positive that costs this guard its credibility for no defect
    /// caught.
    ///
    /// # What it cannot see
    ///
    /// **One hop.** The scan reads the command's OWN body and does not
    /// follow calls. That is a measured choice, not laziness: following
    /// bare names one level -- the technique `fn_bodies` uses for
    /// invariant 7, whose doc already warns it "is not a call graph" --
    /// reports `list_paired_devices` as an offender, because its
    /// `devices::list(&conn)` (a SQL query) matches some other `list` in
    /// the tree that reads a file. Six flags, of which two were real and
    /// four were name collisions. A guard that is right one time in three
    /// does not survive contact with anyone in a hurry, so this checks
    /// what it can check exactly.
    ///
    /// The cost is real and worth stating: a command that moves its
    /// `Command::new` into a private helper passes this. What catches
    /// that is the same thing that caught these -- somebody noticing the
    /// UI freeze -- and the guard at least ensures the direct spelling,
    /// which is what every offender in #1090 actually used, cannot come
    /// back.
    ///
    /// **A command in another crate.** `src-mobile` and the stepup crate
    /// register no `#[tauri::command]`, so walking all three roots costs
    /// nothing and covers them the moment one does.
    ///
    /// # Proven by sabotage, both directions
    ///
    /// **It fires on the real defect.** Reverting `read_claude_md` to a
    /// `pub fn` with its bare `read_to_string` -- the shape it shipped in
    /// -- fails here naming the file, the command and the pattern. Adding
    /// a brand-new `pub fn` command that runs `std::process::Command`
    /// fails the same way, which is the case the four-name allowlist
    /// could not see at all.
    ///
    /// **It stays silent on the trivial one.** With the tree as it
    /// stands, all 15 single-row settings commands -- `get_notify_prefs`,
    /// `get_ui_prefs`, `get_cleanup_prefs`, `get_poll_interval`,
    /// `get_worktree_dirs`, `get_cached`, `get_cached_reviewing`,
    /// `cleanup_log`, five setters, `mark_assessed` and `clear_assessed`
    /// -- pass, and that was checked rather than assumed.
    ///
    /// The two halves were then checked against each other, one command
    /// at a time: giving each of those 15 a real `read_to_string` in its
    /// body makes this fail naming that command, all 15 of 15. So the
    /// silence is about what they DO and not about which file they live
    /// in or what they are called -- which is the failure mode the guard
    /// skill records as "passing for the wrong reason".
    ///
    /// **And it was proven on a CRLF checkout**, which sabotage alone does
    /// not cover: sabotaging a guard shows it reacts to the defect and
    /// says nothing about the platform it runs on. `every_installed_hook_
    /// event_has_a_measured_worst_case_record` records this repository
    /// shipping exactly that -- every sabotage passing, the negative
    /// direction silent, green on macOS, and `platform (windows-latest)`
    /// failing because a CRLF checkout made every `\n` a `\r\n`.
    ///
    /// So it was reproduced rather than assumed: converting `commands.rs`
    /// to CRLF and injecting a sync `Command::new` command fails here
    /// naming it, with no new false positive. The `replace("\r\n", "\n")`
    /// below is what makes that true, and it is tested.
    #[test]
    fn no_sync_command_reaches_a_subprocess_or_a_whole_file() {
        /// The spellings that mean "another program" or "a file of
        /// unknown size".
        ///
        /// Every entry is a shape that appears in this tree, not a
        /// speculative list. `fs::read(` carries its paren so it cannot
        /// match `fs::read_dir` or `fs::read_link` by prefix -- those are
        /// listed separately and on purpose, because a directory listing
        /// over a large tree is the same unbounded shape.
        const UNBOUNDED: &[&str] = &[
            "Command::new",
            "read_to_string",
            "read_to_end",
            "File::open",
            "fs::read(",
            "fs::read_dir",
            "read_dir(",
        ];

        let mut checked = 0usize;
        let mut offenders: Vec<String> = Vec::new();
        for (crate_name, root) in crate_roots() {
            for file in rust_files(&root) {
                let Ok(src) = std::fs::read_to_string(&file) else {
                    continue;
                };
                // This file's own prose names every pattern above. Skipped
                // BY PATH rather than by comment-matching, which is the
                // rule the module header states and #874 paid for.
                if file.ends_with("invariants.rs") {
                    continue;
                }
                let rel = file.strip_prefix(&root).unwrap_or(&file).display();
                let where_ = format!("{crate_name}/{rel}");
                // Normalised before any `\n`-anchored split: a CRLF
                // checkout makes `lines()` keep a trailing `\r`, and the
                // attribute comparison below would then match nothing and
                // pass silently on Windows only. Invariant 5 records this
                // being OBSERVED rather than feared.
                let src = src.replace("\r\n", "\n");
                let lines: Vec<&str> = src.lines().collect();
                for (i, line) in lines.iter().enumerate() {
                    if line.trim() != "#[tauri::command]" {
                        continue;
                    }
                    // The `fn` line: the next declaration, so an
                    // intervening attribute or doc comment is skipped.
                    // `#[tauri::command]` sits both above and below the
                    // doc comment in this file, which is why the search
                    // runs forward from the attribute rather than
                    // assuming adjacency.
                    let Some(fn_at) = (i + 1..lines.len().min(i + 12)).find(|j| {
                        let t = lines[*j].trim_start();
                        t.starts_with("fn ")
                            || t.starts_with("async fn ")
                            || t.starts_with("pub fn ")
                            || t.starts_with("pub async fn ")
                            || t.starts_with("pub(crate) fn ")
                            || t.starts_with("pub(crate) async fn ")
                    }) else {
                        continue;
                    };
                    let fn_line = lines[fn_at];
                    checked += 1;
                    if fn_line.contains("async fn ") {
                        continue;
                    }
                    let indent = fn_line.len() - fn_line.trim_start().len();
                    let end = item_end(&lines, fn_at, indent);
                    // Comments dropped, for the reason the module header
                    // gives at length: this codebase states its rules in
                    // prose directly above the code they govern, so every
                    // pattern above appears far more often in a doc
                    // comment than in a call. Without this the guard would
                    // fire on the very sentences that explain it.
                    let code: String = lines[fn_at..end]
                        .iter()
                        .filter(|l| !is_comment(l))
                        .copied()
                        .collect::<Vec<_>>()
                        .join("\n");
                    // Already on the blocking pool: a sync command MAY do
                    // this work, it just may not do it on the runtime.
                    // That is the whole remedy the rule asks for, so
                    // naming it is compliance rather than an exemption.
                    if code.contains("spawn_blocking") {
                        continue;
                    }
                    let name = fn_line
                        .trim_start()
                        .trim_start_matches("pub(crate) ")
                        .trim_start_matches("pub ")
                        .trim_start_matches("fn ")
                        .split(['(', '<'])
                        .next()
                        .unwrap_or("<unknown>");
                    for pat in UNBOUNDED {
                        if code.contains(pat) {
                            offenders.push(format!("{where_}: {name} reaches `{pat}`"));
                        }
                    }
                }
            }
        }

        // The self-guard every scan here carries. If the attribute
        // spelling or the `fn` shapes above ever stop matching, this test
        // goes green while seeing nothing -- which is worse than no guard,
        // because it reports safety it is not checking.
        //
        // 126 is what it sees today: the 116 registered in `lib.rs` plus
        // the `#[tauri::command]`s in test fixtures, which are scanned
        // too. That is deliberate rather than an oversight -- a fixture
        // command that blocks is not a shipped defect, but it is also not
        // worth an exception that could hide a real one, and none of them
        // does. The floor is set well below the figure so a handful of
        // commands being deleted does not fail this, while the shape of
        // the scan breaking still does.
        assert!(
            checked >= 90,
            "found only {checked} `#[tauri::command]` functions, which \
             cannot be right -- the scan's shape no longer matches the \
             source and this guard is now reading nothing"
        );

        assert!(
            offenders.is_empty(),
            "these `#[tauri::command]` functions are a plain `fn` and \
             reach a subprocess or an unbounded file read directly in \
             their own body. A sync command runs ON the async runtime's \
             worker and blocks it, so the whole UI freezes -- not just \
             the view that asked -- and on the remote surface it stalls \
             the phone's listener for every other request (#496, #1090). \
             Make it `pub async fn` and move the work into \
             `tauri::async_runtime::spawn_blocking`, the way \
             `docker_builds` and `assessed_worktrees` do. A single-row \
             settings read is deliberately NOT in scope here; if this \
             fired on one, the pattern list is wrong rather than the \
             command: {offenders:#?}"
        );
    }
}

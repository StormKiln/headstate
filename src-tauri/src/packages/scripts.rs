//! The targets a `Makefile` or `justfile` offers, and the scripts a
//! `package.json` offers.
//!
//! Shared by the toolchain and rot producers, so "does `make lint`
//! exist" is answered by one parser. Nothing here runs anything.
//!
//! # Absent is not unreadable
//!
//! A repository with no `Makefile` has no targets, and that is an ordinary
//! answer. A repository whose `Makefile` exists and could not be read has
//! an UNKNOWN number of targets, and a producer that received an empty
//! list would call every documented target rot. So the return type has
//! three states, not two: [`Manifest::Absent`], [`Manifest::Unreadable`]
//! with the io error, and [`Manifest::Present`] with what was read.

use std::path::Path;

/// What a manifest read produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Manifest<T> {
    /// No such file. Nothing to read is not a failure.
    Absent,
    /// The file exists and could not be read or parsed, with why. A
    /// producer reports this as Unknown, never as an empty list.
    Unreadable(String),
    Present(T),
}

/// One make or just target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    /// The manifest's file name, `Makefile` or `justfile`, so a finding
    /// can cite `Makefile:12`.
    pub file: String,
    /// 1-based.
    pub line: usize,
}

/// GNU make's own search order.
const MAKEFILES: &[&str] = &["GNUmakefile", "makefile", "Makefile"];

/// The makefile GNU make would pick under `dir`, by its on-disk name.
///
/// Taken from the directory LISTING, not by probing each candidate with
/// `is_file()`. On a case-insensitive filesystem the probe for
/// `makefile` is answered by `Makefile`, so a repository with the usual
/// `Makefile` was reported as `makefile` on macOS and as `Makefile` on
/// Linux -- the same file, two names, and a test that pinned the name
/// failed on exactly one platform. The listing returns the name as it is
/// spelled on disk. A listing failure is `Err`: the directory holds an
/// unknown number of makefiles, which is not "none".
fn makefile_name(dir: &Path) -> Result<Option<&'static str>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut present: Vec<&'static str> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        if let Some(known) = MAKEFILES.iter().find(|m| name == **m) {
            if entry.path().is_file() {
                present.push(known);
            }
        }
    }
    Ok(MAKEFILES.iter().copied().find(|m| present.contains(m)))
}

/// The targets under `dir`, from the first makefile GNU make would pick
/// plus a `justfile` when one exists.
///
/// Both manifests present and one unreadable is `Unreadable`: the reader
/// cannot tell which of the documented targets would have been in the
/// file it could not read, so nothing is claimed.
pub fn targets(dir: &Path) -> Manifest<Vec<Target>> {
    let mut out = Vec::new();
    let mut any = false;

    let makefile = match makefile_name(dir) {
        Ok(name) => name,
        Err(e) => return Manifest::Unreadable(e),
    };
    if let Some(name) = makefile {
        any = true;
        match std::fs::read_to_string(dir.join(name)) {
            Ok(text) => out.extend(make_targets(&text, name)),
            Err(e) => return Manifest::Unreadable(format!("{name}: {e}")),
        }
    }
    let just = dir.join("justfile");
    if just.is_file() {
        any = true;
        match std::fs::read_to_string(&just) {
            Ok(text) => out.extend(just_recipes(&text)),
            Err(e) => return Manifest::Unreadable(format!("justfile: {e}")),
        }
    }
    if any {
        Manifest::Present(out)
    } else {
        Manifest::Absent
    }
}

/// Target lines: `^[A-Za-z0-9_.-]+:` at column 0, not `:=` assignments,
/// and not the dotted special targets (`.PHONY`, `.DEFAULT`, `.SUFFIXES`
/// and the rest of GNU make's all-caps set). A dotted target that is not
/// all caps, such as `.venv:`, is a real target and kept.
fn make_targets(text: &str, file: &str) -> Vec<Target> {
    let mut out = Vec::new();
    for (i, line) in text.replace("\r\n", "\n").lines().enumerate() {
        let Some(name) = target_name(line) else {
            continue;
        };
        if name.starts_with('.') && name[1..].chars().all(|c| c.is_ascii_uppercase()) {
            continue;
        }
        out.push(Target {
            name: name.to_string(),
            file: file.to_string(),
            line: i + 1,
        });
    }
    out
}

/// The name before the `:` on a target line, when the line is one.
fn target_name(line: &str) -> Option<&str> {
    if line.starts_with([' ', '\t', '#']) {
        return None;
    }
    let colon = line.find(':')?;
    let name = &line[..colon];
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        return None;
    }
    // `FOO:=bar` and `FOO::=bar` are assignments.
    let after = &line[colon..];
    if after.starts_with(":=") || after.starts_with("::=") {
        return None;
    }
    Some(name)
}

/// just recipes: `name` or `name arg…` then `:` at column 0, optionally
/// after `@`; `:=` lines are variables and `[attr]` lines are not recipes.
fn just_recipes(text: &str) -> Vec<Target> {
    let mut out = Vec::new();
    for (i, line) in text.replace("\r\n", "\n").lines().enumerate() {
        if line.starts_with([' ', '\t', '#', '[']) {
            continue;
        }
        let line = line.strip_prefix('@').unwrap_or(line);
        let Some(colon) = line.find(':') else {
            continue;
        };
        if line[colon..].starts_with(":=") {
            continue;
        }
        let head = line[..colon].trim();
        let Some(name) = head.split_whitespace().next() else {
            continue;
        };
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        {
            continue;
        }
        out.push(Target {
            name: name.to_string(),
            file: "justfile".to_string(),
            line: i + 1,
        });
    }
    out
}

/// The script names in `dir/package.json`, in the file's order.
///
/// A file that will not parse is `Unreadable` with serde's error, for the
/// same reason as a permission wall: the number of scripts is unknown.
pub fn scripts(dir: &Path) -> Manifest<Vec<String>> {
    match script_bodies(dir) {
        Manifest::Present(list) => Manifest::Present(list.into_iter().map(|s| s.name).collect()),
        Manifest::Unreadable(e) => Manifest::Unreadable(e),
        Manifest::Absent => Manifest::Absent,
    }
}

/// One `package.json` script: its name and the command line it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub name: String,
    /// The command line, verbatim. A value that is not a string (npm
    /// will not run one) is empty, so it maps to nothing.
    pub body: String,
}

/// The scripts in `dir/package.json` with their bodies, in the file's
/// order.
///
/// A sibling of [`scripts`], not a change to it: the rot producer wants
/// names only, and the toolchain reads a body only when a script's name
/// maps to no verb (#1341). Three-state for the same reason.
pub fn script_bodies(dir: &Path) -> Manifest<Vec<Script>> {
    let path = dir.join("package.json");
    if !path.is_file() {
        return Manifest::Absent;
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return Manifest::Unreadable(format!("package.json: {e}")),
    };
    let json: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return Manifest::Unreadable(format!("package.json: {e}")),
    };
    Manifest::Present(
        json.get("scripts")
            .and_then(|s| s.as_object())
            .map(|o| {
                o.iter()
                    .map(|(name, body)| Script {
                        name: name.clone(),
                        body: body.as_str().unwrap_or_default().to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// The dependency tables npm resolves a bare import against, in the
/// order it resolves them. `peerDependencies` is included because a
/// CLAUDE.md naming a peer names something the project really does
/// depend on; a peer that is not also installed is npm's problem, not
/// a rot finding.
const DEPENDENCY_TABLES: &[&str] = &[
    "dependencies",
    "devDependencies",
    "optionalDependencies",
    "peerDependencies",
];

/// The package names `dir/package.json` declares, across every
/// dependency table.
///
/// Exists because a CLAUDE.md naming `chart.js` names a DEPENDENCY, and
/// the rot check had no way to tell that from a file called `chart.js`
/// and called it missing (#1300). The manifest is the signal: a token
/// that is a declared dependency is a package reference, not a path.
///
/// Three-state for the same reason [`scripts`] is: a `package.json` that
/// exists and will not parse declares an UNKNOWN set of packages, and a
/// caller handed an empty list would call every named package rot.
pub fn dependencies(dir: &Path) -> Manifest<Vec<String>> {
    let path = dir.join("package.json");
    if !path.is_file() {
        return Manifest::Absent;
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return Manifest::Unreadable(format!("package.json: {e}")),
    };
    let json: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return Manifest::Unreadable(format!("package.json: {e}")),
    };
    let mut out: Vec<String> = Vec::new();
    for table in DEPENDENCY_TABLES {
        let Some(o) = json.get(table).and_then(|t| t.as_object()) else {
            continue;
        };
        for name in o.keys() {
            if !out.iter().any(|n| n == name) {
                out.push(name.clone());
            }
        }
    }
    Manifest::Present(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const MAKEFILE: &str = "\
.PHONY: dev build test lint \\
\tfmt
.DEFAULT_GOAL := dev
VERSION := 1.0
CFLAGS:=-O2

dev:
\tyarn dev

# a comment: not a target
test: test-rust test-ui

test-rust:
\tcargo test

.venv:
\tpython -m venv .venv
lint.sh:
\techo
";

    #[test]
    fn make_targets_are_read_with_their_lines_and_phony_excluded() {
        let got = make_targets(MAKEFILE, "Makefile");
        let names: Vec<(&str, usize)> = got.iter().map(|t| (t.name.as_str(), t.line)).collect();
        assert_eq!(
            names,
            vec![
                ("dev", 7),
                ("test", 11),
                ("test-rust", 13),
                (".venv", 16),
                ("lint.sh", 18)
            ]
        );
        assert!(got.iter().all(|t| t.file == "Makefile"));
    }

    #[test]
    fn just_recipes_are_read_and_variables_are_not() {
        let just = "set shell := [\"bash\"]\nversion := \"1\"\n[private]\n@build target=\"x\":\n  cargo build\ntest:\n  cargo test\n";
        let got = just_recipes(just);
        let names: Vec<(&str, usize)> = got.iter().map(|t| (t.name.as_str(), t.line)).collect();
        assert_eq!(names, vec![("build", 4), ("test", 6)]);
    }

    /// Absent is not unreadable, and neither is an empty list.
    #[test]
    fn an_absent_manifest_is_absent_not_empty() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(targets(t.path()), Manifest::Absent);
        assert_eq!(scripts(t.path()), Manifest::Absent);
    }

    #[test]
    fn a_makefile_and_a_justfile_combine() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("Makefile"), "lint:\n\techo\n").unwrap();
        fs::write(t.path().join("justfile"), "fmt:\n  cargo fmt\n").unwrap();
        match targets(t.path()) {
            Manifest::Present(got) => {
                let names: Vec<&str> = got.iter().map(|t| t.name.as_str()).collect();
                assert_eq!(names, vec!["lint", "fmt"]);
                assert_eq!(got[1].file, "justfile");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn gnumakefile_wins_over_makefile() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("Makefile"), "wrong:\n\techo\n").unwrap();
        fs::write(t.path().join("GNUmakefile"), "right:\n\techo\n").unwrap();
        match targets(t.path()) {
            Manifest::Present(got) => assert_eq!(got[0].name, "right"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn package_json_scripts_are_read_in_order() {
        let t = tempfile::tempdir().unwrap();
        fs::write(
            t.path().join("package.json"),
            r#"{"name":"x","scripts":{"test":"vitest","lint":"eslint ."}}"#,
        )
        .unwrap();
        assert_eq!(
            scripts(t.path()),
            Manifest::Present(vec!["test".into(), "lint".into()])
        );
    }

    /// #1341: the bodies come with the names, in the file's order, and a
    /// value that is not a string is an empty body.
    #[test]
    fn package_json_script_bodies_are_read_in_order() {
        let t = tempfile::tempdir().unwrap();
        fs::write(
            t.path().join("package.json"),
            r#"{"scripts":{"verify":"eslint . && vitest run","odd":7}}"#,
        )
        .unwrap();
        assert_eq!(
            script_bodies(t.path()),
            Manifest::Present(vec![
                Script {
                    name: "verify".into(),
                    body: "eslint . && vitest run".into()
                },
                Script {
                    name: "odd".into(),
                    body: String::new()
                },
            ])
        );
        assert_eq!(script_bodies(&t.path().join("absent")), Manifest::Absent);
    }

    #[test]
    fn a_package_json_without_scripts_is_present_and_empty() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("package.json"), r#"{"name":"x"}"#).unwrap();
        assert_eq!(scripts(t.path()), Manifest::Present(vec![]));
    }

    /// A manifest that will not parse has an unknown number of scripts.
    #[test]
    fn a_malformed_package_json_is_unreadable_not_empty() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("package.json"), "{ not json").unwrap();
        match scripts(t.path()) {
            Manifest::Unreadable(why) => assert!(why.starts_with("package.json: "), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    /// The name a target cites is the one on disk. On a case-insensitive
    /// filesystem a probe for `makefile` finds `Makefile`, and CI on macOS
    /// reported the usual spelling under the unusual name.
    #[test]
    fn the_makefile_is_named_as_it_is_spelled_on_disk() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("Makefile"), "lint:\n\techo\n").unwrap();
        match targets(t.path()) {
            Manifest::Present(list) => {
                assert_eq!(list.len(), 1);
                assert_eq!(list[0].file, "Makefile");
            }
            other => panic!("{other:?}"),
        }
    }

    /// A permission wall is `Unreadable` with the io error, never an
    /// empty list. `chmod 000` is not honoured on Windows, and as root the
    /// gate drops `cap_dac_override` to make it bite.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_makefile_is_unreadable_not_empty() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let mk = t.path().join("Makefile");
        fs::write(&mk, "lint:\n\techo\n").unwrap();
        fs::set_permissions(&mk, fs::Permissions::from_mode(0o000)).unwrap();

        let got = targets(t.path());

        fs::set_permissions(&mk, fs::Permissions::from_mode(0o644)).unwrap();
        match got {
            Manifest::Unreadable(why) => assert!(why.starts_with("Makefile: "), "{why}"),
            other => panic!("an unreadable manifest must not be an empty list: {other:?}"),
        }
    }

    /// The real shape from #1300: `chart.js` is a dependency, not a file.
    #[test]
    fn dependencies_are_read_from_every_table() {
        let t = tempfile::tempdir().unwrap();
        fs::write(
            t.path().join("package.json"),
            r#"{"dependencies":{"chart.js":"^4.5.1"},"devDependencies":{"vitest":"~1"},"optionalDependencies":{"fsevents":"*"},"peerDependencies":{"react":"^18"}}"#,
        )
        .unwrap();
        match dependencies(t.path()) {
            Manifest::Present(got) => {
                assert_eq!(got, vec!["chart.js", "vitest", "fsevents", "react"]);
            }
            other => panic!("{other:?}"),
        }
    }

    /// Absent is not "declares nothing", and a manifest with no
    /// dependency table really does declare nothing.
    #[test]
    fn an_absent_manifest_declares_nothing_knowably() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(dependencies(t.path()), Manifest::Absent);
        fs::write(t.path().join("package.json"), r#"{"name":"x"}"#).unwrap();
        assert_eq!(dependencies(t.path()), Manifest::Present(vec![]));
    }

    /// A manifest that will not parse declares an unknown set.
    #[test]
    fn a_malformed_package_json_has_unknown_dependencies() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("package.json"), "{ not json").unwrap();
        match dependencies(t.path()) {
            Manifest::Unreadable(why) => assert!(why.starts_with("package.json: "), "{why}"),
            other => panic!("{other:?}"),
        }
    }
}

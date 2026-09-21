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

/// The targets under `dir`, from the first makefile GNU make would pick
/// plus a `justfile` when one exists.
///
/// Both manifests present and one unreadable is `Unreadable`: the reader
/// cannot tell which of the documented targets would have been in the
/// file it could not read, so nothing is claimed.
pub fn targets(dir: &Path) -> Manifest<Vec<Target>> {
    let mut out = Vec::new();
    let mut any = false;

    if let Some(name) = MAKEFILES.iter().find(|n| dir.join(n).is_file()) {
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
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default(),
    )
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
}

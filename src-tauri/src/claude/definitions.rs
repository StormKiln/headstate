//! The skills, subagents and slash commands on this machine (#1129).
//!
//! `plugins.rs` reports which plugins ship a `skills/`, `agents/` or
//! `commands/` directory and nothing about what is inside them:
//! `read_contribution` matches on directory NAMES and sets four
//! booleans. So a user could not answer "what subagents do I have, and
//! which have I ever used?" -- and every hand-written definition in
//! `~/.claude/` was invisible, because it belongs to no plugin at all.
//!
//! # Usage counts are NOT joined here, and the rule for adding them
//!
//! A `Definition` carries what is on disk and nothing about use: there
//! is no call count in this module, and an earlier version of this
//! header wrongly described one as already shipped (#1207).
//!
//! Stated as a requirement rather than a description, for whoever
//! joins the counts: `plugins.rs`'s "absent is not zero" argument
//! applies here unchanged. A definition with no recorded calls may
//! have never been used, or may simply predate the scan -- and a false
//! zero argues for deleting something the user relies on. So the field
//! must be `Option<u64>`, rendering `None` as "never observed" and
//! never as "0 calls". A plain `u64` reintroduces exactly the defect
//! `plugins.rs:89-105` exists to prevent.
//!
//! # What could not be read is reported
//!
//! A directory behind a permission wall hides an unknown number of
//! definitions. `claudemd::Scan` keeps its unreadable list for exactly
//! this reason, and the same rule holds here: an inventory that silently
//! omitted a subtree would say "you have three skills" about a machine
//! with thirty.
//!
//! Read-only. Nothing here creates, edits, enables or deletes a
//! definition.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Which kind of definition this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Skill,
    Agent,
    Command,
}

/// One definition found on disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Definition {
    pub kind: Kind,
    /// The `name:` from frontmatter, or the filename when it carries
    /// none.
    ///
    /// The filename fallback is NOT a guess dressed as data: a skill
    /// directory IS addressed by its directory name, so that is the
    /// real name rather than an invention. `named_in_frontmatter`
    /// records which it was, for a reader who needs to know.
    pub name: String,
    pub named_in_frontmatter: bool,
    /// The `description:` from frontmatter, when present.
    pub description: Option<String>,
    /// Absolute path, so the UI can reveal it.
    pub path: String,
}

/// Everything found, and everything that could not be read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Definitions {
    pub definitions: Vec<Definition>,
    /// Directories that exist and could not be listed, with why.
    ///
    /// A permission wall here hides an unknown number of definitions, so
    /// it travels as a message rather than a boolean -- the same shape
    /// `claudemd::Scan::unreadable_dirs` uses and for the same reason.
    pub unreadable: Vec<String>,
}

/// Parse the `name` and `description` out of YAML frontmatter.
///
/// Deliberately NOT a YAML parser. The frontmatter this reads is a
/// handful of `key: value` lines, and pulling in a parser to read two of
/// them would be a dependency for a shape we already know. A line this
/// does not understand is SKIPPED rather than failing the file: a
/// definition with an exotic frontmatter key is still a definition, and
/// refusing to list it would hide something that exists.
///
/// Returns `(name, description)`, either of which may be absent.
fn frontmatter(text: &str) -> (Option<String>, Option<String>) {
    let mut lines = text.lines();
    // Frontmatter opens with `---` on the first line, or there is none.
    if lines.next().map(str::trim) != Some("---") {
        return (None, None);
    }
    let (mut name, mut description) = (None, None);
    for line in lines {
        let t = line.trim();
        if t == "---" {
            break;
        }
        if let Some(v) = t.strip_prefix("name:") {
            name = Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
        } else if let Some(v) = t.strip_prefix("description:") {
            description = Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }
    (name, description)
}

/// Read one definition file.
fn read_one(kind: Kind, path: &Path, fallback: &str) -> Option<Definition> {
    let text = std::fs::read_to_string(path).ok()?;
    let (name, description) = frontmatter(&text);
    Some(Definition {
        kind,
        named_in_frontmatter: name.is_some(),
        name: name.unwrap_or_else(|| fallback.to_string()),
        description,
        path: path.to_string_lossy().to_string(),
    })
}

/// Every skill, agent and command under a `.claude` directory.
///
/// `root` is a PARAMETER rather than resolved here, so this is testable
/// without touching `$HOME` -- process-global state that would race
/// every other test in the binary, which is the reason
/// `claudemd::expand_home_in` gives for the same choice.
pub fn scan_in(root: &Path) -> Definitions {
    let mut out = Definitions::default();

    // Skills are DIRECTORIES holding a SKILL.md; agents and commands are
    // plain `.md` files. Handled separately rather than by one walk,
    // because conflating them would list a skill's directory as a
    // command.
    let skills = root.join("skills");
    if skills.is_dir() {
        match std::fs::read_dir(&skills) {
            Ok(entries) => {
                for e in entries.flatten() {
                    let dir = e.path();
                    if !dir.is_dir() {
                        continue;
                    }
                    let file = dir.join("SKILL.md");
                    if !file.is_file() {
                        continue;
                    }
                    let fallback = e.file_name().to_string_lossy().into_owned();
                    if let Some(d) = read_one(Kind::Skill, &file, &fallback) {
                        out.definitions.push(d);
                    } else {
                        out.unreadable
                            .push(format!("{}: could not be read", file.display()));
                    }
                }
            }
            Err(e) => out.unreadable.push(format!("{}: {e}", skills.display())),
        }
    }

    for (kind, sub) in [(Kind::Agent, "agents"), (Kind::Command, "commands")] {
        let dir = root.join(sub);
        if !dir.is_dir() {
            continue;
        }
        collect_markdown(kind, &dir, &mut out);
    }

    out.definitions.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Every `.md` under a directory, recursively.
///
/// Recursive because commands nest: `~/.claude/commands/git/sync.md` is
/// `/git:sync`. A flat read would miss every namespaced command, which
/// on a configured machine is most of them.
fn collect_markdown(kind: Kind, dir: &Path, out: &mut Definitions) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            out.unreadable.push(format!("{}: {e}", dir.display()));
            return;
        }
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_markdown(kind, &p, out);
            continue;
        }
        if p.extension().and_then(|x| x.to_str()) != Some("md") {
            continue;
        }
        let fallback = p
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Some(d) = read_one(kind, &p, &fallback) {
            out.definitions.push(d);
        } else {
            out.unreadable
                .push(format!("{}: could not be read", p.display()));
        }
    }
}

/// `~/.claude`, when a home directory is known.
pub fn user_root() -> Option<PathBuf> {
    crate::auth::home_dir().map(|h| h.join(".claude"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn a_skill_is_found_by_its_frontmatter_name() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("skills").join("deploy").join("SKILL.md"),
            "---\nname: shipping\ndescription: how we ship\n---\n\nbody",
        );

        let found = scan_in(root);
        let d = &found.definitions[0];
        assert_eq!(d.kind, Kind::Skill);
        assert_eq!(d.name, "shipping");
        assert!(d.named_in_frontmatter);
        assert_eq!(d.description.as_deref(), Some("how we ship"));
    }

    /// A skill directory IS addressed by its name, so the fallback is
    /// the real name rather than an invention -- but which it was still
    /// has to be recoverable.
    #[test]
    fn a_skill_without_frontmatter_falls_back_to_its_directory_name() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("skills").join("deploy").join("SKILL.md"),
            "no frontmatter here",
        );

        let found = scan_in(root);
        assert_eq!(found.definitions[0].name, "deploy");
        assert!(
            !found.definitions[0].named_in_frontmatter,
            "and a reader can tell it was a fallback"
        );
    }

    /// Commands nest: `commands/git/sync.md` is `/git:sync`. A flat read
    /// would miss every namespaced command, which on a configured
    /// machine is most of them.
    #[test]
    fn nested_commands_are_found() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("commands").join("git").join("sync.md"),
            "---\nname: sync\n---\n",
        );

        let found = scan_in(root);
        assert_eq!(found.definitions.len(), 1);
        assert_eq!(found.definitions[0].kind, Kind::Command);
    }

    /// A skill's directory must not be listed as a command, and an
    /// agent file must not be listed as a skill.
    #[test]
    fn the_three_kinds_stay_distinct() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("skills").join("a").join("SKILL.md"),
            "---\nname: a\n---\n",
        );
        write(&root.join("agents").join("b.md"), "---\nname: b\n---\n");
        write(&root.join("commands").join("c.md"), "---\nname: c\n---\n");

        let found = scan_in(root);
        let kind = |n: &str| found.definitions.iter().find(|d| d.name == n).unwrap().kind;
        assert_eq!(kind("a"), Kind::Skill);
        assert_eq!(kind("b"), Kind::Agent);
        assert_eq!(kind("c"), Kind::Command);
    }

    /// An exotic frontmatter key must not fail the file. A definition
    /// with a key this does not parse is still a definition, and
    /// refusing to list it would hide something that exists.
    #[test]
    fn an_unrecognised_frontmatter_key_does_not_hide_the_definition() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("agents").join("x.md"),
            "---\nname: x\nmodel: opus\nallowed-tools: [a, b]\n---\n",
        );

        let found = scan_in(root);
        assert_eq!(found.definitions.len(), 1);
        assert_eq!(found.definitions[0].name, "x");
    }

    /// An ABSENT directory is not a problem. A machine with no agents
    /// has no `agents/`, and reporting that would make the honest signal
    /// worthless.
    #[test]
    fn absent_directories_are_not_reported_as_unreadable() {
        let tmp = tempfile::tempdir().unwrap();
        let found = scan_in(tmp.path());
        assert!(found.definitions.is_empty());
        assert!(found.unreadable.is_empty());
    }
}

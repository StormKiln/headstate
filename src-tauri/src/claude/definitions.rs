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
//! # Three scopes, and no winner between them (#1215)
//!
//! `~/.claude` is one of several places a definition lives. A repository
//! carries its own `.claude/`, and every installed plugin ships its own
//! `skills/`, `agents/` and `commands/`. Scanning only the user's root
//! made ~38 repositories' worth of project definitions invisible, which
//! is the same shape as the absent-is-not-zero argument above: "you
//! have four skills" about a machine with forty.
//!
//! So [`scan_scopes`] calls [`scan_in`] once per root and stamps each
//! result with the [`Source`] it came from.
//!
//! ## Why a collision is SHOWN rather than resolved
//!
//! A project skill and a user skill with the same name are not two
//! independent entries -- one shadows the other, and which one wins is a
//! rule that belongs to Claude Code. `settings.rs` declined to
//! re-implement that kind of rule for the same reason, in the comment on
//! its `KEYS` list: "re-implementing Claude Code's whole merge algorithm
//! would be a second source of truth for someone else's behaviour, and
//! wrong the moment it changes."
//!
//! Deduping by name -- first wins, or project-beats-user -- is exactly
//! that second source of truth, and it is worse here than in
//! `settings.rs` would have been: a dropped definition leaves no trace
//! at all, so a wrong precedence rule does not render a wrong winner, it
//! renders a definition the user can see on disk as simply absent.
//!
//! This module has not MEASURED Claude Code's shadowing rule, so it
//! asserts none. Both definitions are listed, and a [`Collision`] names
//! every source that claimed the name so the user can resolve it
//! themselves against the tool that actually decides.
//!
//! ## Unreadable stays per scope
//!
//! The "what could not be read is reported" rule above does not survive
//! a multi-root scan by itself: flattening every root's `unreadable`
//! into one list loses WHICH scope was walled off, and a project whose
//! `.claude/` is unreadable hides an unknown number of definitions that
//! a successful user scan would otherwise paper over. So a refusal
//! travels with its [`Source`], the way `settings::ScopeRefusal` carries
//! its `Origin`.
//!
//! Read-only. Nothing here creates, edits, enables or deletes a
//! definition.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Which kind of definition this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Skill,
    Agent,
    Command,
}

/// Which scope a definition came from.
///
/// Carried on every [`Definition`] and every [`ScopeRefusal`] rather
/// than implied by the path, because the path alone cannot be read back
/// into a scope: a plugin installs under `~/.claude/plugins/...`, which
/// is INSIDE the user root, and a project checked out under the home
/// directory is indistinguishable from either by prefix. The scanner
/// knows which root it walked; the UI should not have to guess.
///
/// Deliberately NOT ordered. `settings::Origin` derives `Ord` because
/// its three scopes have a precedence Claude Code documents and this
/// repo could check. Definitions have no such measured order here --
/// see the module header -- and deriving one would hand every caller a
/// comparison that looks like an answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "camelCase")]
pub enum Source {
    /// `~/.claude`.
    User,
    /// A repository's `<repo>/.claude`.
    #[serde(rename_all = "camelCase")]
    Project {
        /// The repository root, so the UI can name WHICH project. Across
        /// ~38 repositories "a project skill" is not an answer.
        path: String,
    },
    /// An installed plugin's own directory.
    #[serde(rename_all = "camelCase")]
    Plugin {
        /// The plugin's name as `plugins.rs` parsed it out of
        /// `installed_plugins.json`.
        name: String,
        /// Its install path.
        path: String,
    },
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
    /// Which scope this was found in (#1215).
    ///
    /// Not an `Option`: every definition is found by walking some root,
    /// so there is always an answer, and a `None` here would be a scope
    /// we forgot to stamp rather than one that does not exist.
    pub source: Source,
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
fn read_one(kind: Kind, path: &Path, fallback: &str, source: &Source) -> Option<Definition> {
    let text = std::fs::read_to_string(path).ok()?;
    let (name, description) = frontmatter(&text);
    Some(Definition {
        kind,
        named_in_frontmatter: name.is_some(),
        name: name.unwrap_or_else(|| fallback.to_string()),
        description,
        path: path.to_string_lossy().to_string(),
        source: source.clone(),
    })
}

/// Every skill, agent and command under a `.claude` directory.
///
/// `root` is a PARAMETER rather than resolved here, so this is testable
/// without touching `$HOME` -- process-global state that would race
/// every other test in the binary, which is the reason
/// `claudemd::expand_home_in` gives for the same choice.
///
/// `source` is what the caller is walking this root AS. The function
/// cannot infer it -- a plugin root sits inside the user root, so a
/// prefix test would call every plugin definition a user one.
pub fn scan_in(root: &Path, source: &Source) -> Definitions {
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
                    if let Some(d) = read_one(Kind::Skill, &file, &fallback, source) {
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
        collect_markdown(kind, &dir, source, &mut out);
    }

    out.definitions.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Every `.md` under a directory, recursively.
///
/// Recursive because commands nest: `~/.claude/commands/git/sync.md` is
/// `/git:sync`. A flat read would miss every namespaced command, which
/// on a configured machine is most of them.
fn collect_markdown(kind: Kind, dir: &Path, source: &Source, out: &mut Definitions) {
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
            collect_markdown(kind, &p, source, out);
            continue;
        }
        if p.extension().and_then(|x| x.to_str()) != Some("md") {
            continue;
        }
        let fallback = p
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Some(d) = read_one(kind, &p, &fallback, source) {
            out.definitions.push(d);
        } else {
            out.unreadable
                .push(format!("{}: could not be read", p.display()));
        }
    }
}

/// One scope that exists and could not be listed, and which scope.
///
/// `Definitions::unreadable` is a flat `Vec<String>` because it only
/// ever described one root. Across many roots the string alone loses the
/// thing a reader needs: a message naming `/a/b/.claude/agents` does not
/// say whether that was the user's root, a plugin, or which of ~38
/// projects. Modelled on `settings::ScopeRefusal`, which carries its
/// `Origin` for the same reason.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeRefusal {
    pub source: Source,
    /// The message `scan_in` produced, which names the path and the
    /// OS error.
    pub detail: String,
}

/// Two or more definitions of the same kind claiming one name.
///
/// This is a REPORT, not a resolution. See the module header: which of
/// these Claude Code actually loads is a rule this module has not
/// measured, so it names the claimants and asserts no winner. The
/// `members` are indices into [`Inventory::definitions`] rather than
/// copies, so the UI renders one list and marks rows in it -- a copy
/// would let a collision and the list it describes drift apart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Collision {
    pub kind: Kind,
    pub name: String,
    /// Indices into [`Inventory::definitions`], in that list's order.
    pub members: Vec<usize>,
}

/// Every scope's definitions, the collisions between them, and what
/// could not be read (#1215).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    /// Every definition found, from every scope. NOTHING is deduped --
    /// a name claimed twice appears twice, and [`Inventory::collisions`]
    /// is how a reader finds out.
    pub definitions: Vec<Definition>,
    pub collisions: Vec<Collision>,
    /// Per scope, so a walled-off project cannot hide inside a
    /// successful user scan.
    pub unreadable: Vec<ScopeRefusal>,
}

impl Inventory {
    /// Whether this definition's `(kind, name)` is claimed by more than
    /// one scope.
    pub fn is_colliding(&self, index: usize) -> bool {
        self.collisions.iter().any(|c| c.members.contains(&index))
    }
}

/// Scan several roots and report the collisions between them.
///
/// `roots` is `(source, path)` pairs, and it is a PARAMETER for the
/// reason `scan_in`'s single root is: resolving the home directory, the
/// repository list and the plugin inventory in here would make this
/// untestable without process-global state.
///
/// A root that does not exist is simply skipped by `scan_in`, which is
/// the right answer -- most repositories have no `.claude/`, and
/// reporting that would drown the honest signal, the argument
/// `absent_directories_are_not_reported_as_unreadable` already makes.
pub fn scan_scopes(roots: &[(Source, PathBuf)]) -> Inventory {
    let mut out = Inventory::default();
    for (source, root) in roots {
        let found = scan_in(root, source);
        out.definitions.extend(found.definitions);
        for detail in found.unreadable {
            out.unreadable.push(ScopeRefusal {
                source: source.clone(),
                detail,
            });
        }
    }

    // Sorted by name so the list reads the way the single-root scan did,
    // which also puts a colliding pair adjacent.
    out.definitions
        .sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.path.cmp(&b.path)));

    out.collisions = collisions_in(&out.definitions);
    out
}

/// Group definitions by `(kind, name)` and keep the groups with more
/// than one member.
///
/// Keyed on the KIND as well as the name deliberately: a skill and a
/// command may both be called `review` without either shadowing the
/// other, and flagging that pair would be a false alarm that teaches the
/// reader to ignore the real ones.
///
/// Two entries at the SAME path are not a collision -- that is one file
/// reached twice, which happens when a caller passes overlapping roots
/// (a repository nested inside another's scan). Naming the same file as
/// its own rival would be a collision the user cannot resolve because
/// there is nothing to resolve.
fn collisions_in(definitions: &[Definition]) -> Vec<Collision> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<(Kind, &str), Vec<usize>> = BTreeMap::new();
    for (i, d) in definitions.iter().enumerate() {
        groups.entry((d.kind, d.name.as_str())).or_default().push(i);
    }
    groups
        .into_iter()
        .filter_map(|((kind, name), members)| {
            let distinct: Vec<usize> = {
                let mut seen: Vec<&str> = Vec::new();
                members
                    .into_iter()
                    .filter(|i| {
                        let p = definitions[*i].path.as_str();
                        if seen.contains(&p) {
                            false
                        } else {
                            seen.push(p);
                            true
                        }
                    })
                    .collect()
            };
            (distinct.len() > 1).then(|| Collision {
                kind,
                name: name.to_string(),
                members: distinct,
            })
        })
        .collect()
}

/// `~/.claude`, when a home directory is known.
pub fn user_root() -> Option<PathBuf> {
    crate::auth::home_dir().map(|h| h.join(".claude"))
}

/// Every directory under `dirs` that has a `.claude/`, to depth 2.
///
/// # Why this is not `worktrees::scan_dirs_fast_reporting`
///
/// That walk is the one the repo picker uses and it is the right answer
/// to a different question: it runs `git worktree list` per repository
/// and takes ~800ms for 37 repos on this machine. A definitions scan
/// does not need a repository -- it needs a `.claude/` -- and a
/// directory that has one is worth scanning whether or not git agrees it
/// is a checkout. So this is `read_dir` and a `symlink_metadata`, with
/// no subprocess at all, which is what keeps the multi-root scan cheap
/// enough to stay a single command.
///
/// Depth 2 to match the repo picker's own `collect_inner`, so the set of
/// projects this finds is the set the rest of the app already shows
/// rather than a second, differently-shaped list.
///
/// `symlink_metadata` rather than `is_dir()` for the reason
/// `worktrees::scan::collect_inner` gives: `is_dir()` returns a plain
/// `false` for a path it could not stat, so a directory behind a
/// permission wall would be indistinguishable from a file -- and this
/// module's whole contract is that such a directory gets REPORTED. The
/// refusals come back beside the roots for exactly that.
pub fn project_roots(dirs: &[String]) -> (Vec<PathBuf>, Vec<String>) {
    let mut found = Vec::new();
    let mut unreadable = Vec::new();
    for d in dirs {
        walk_projects(Path::new(d), 0, &mut found, &mut unreadable);
    }
    found.sort();
    found.dedup();
    (found, unreadable)
}

fn walk_projects(dir: &Path, depth: usize, found: &mut Vec<PathBuf>, unreadable: &mut Vec<String>) {
    if depth > 2 {
        return;
    }
    match std::fs::symlink_metadata(dir) {
        Ok(md) if md.is_dir() => {}
        // Not a directory, and nothing was hidden: a file or a symlink,
        // whose target is reachable by its real path anyway.
        Ok(_) => return,
        Err(e) => {
            unreadable.push(format!("{}: {e}", dir.display()));
            return;
        }
    }
    if dir.join(".claude").is_dir() {
        found.push(dir.to_path_buf());
        // Still descends: a monorepo can hold a `.claude/` at the top
        // AND in a package beneath it, and stopping here would hide the
        // inner one -- a silent omission, which is the failure this
        // module exists to avoid.
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            unreadable.push(format!("{}: {e}", dir.display()));
            return;
        }
    };
    for e in entries.flatten() {
        let p = e.path();
        // `.claude` itself is the thing we look FOR, not a project to
        // look inside; and the dotted directories beneath a checkout
        // (`.git`, `.worktrees`) are not projects either.
        if p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'))
        {
            continue;
        }
        walk_projects(&p, depth + 1, found, unreadable);
    }
}

/// The roots to scan, given a user root, the repositories to look in and
/// the installed plugins.
///
/// Assembled here rather than in the command so the SET of roots has a
/// test that does not need a home directory, a database or an
/// `installed_plugins.json`.
///
/// `repos` are repository roots; each contributes `<repo>/.claude`.
/// `plugins` are `(name, install_path)` as `plugins.rs` parsed them --
/// a plugin's definitions live directly under its install path, not
/// under a `.claude` inside it, which is what `read_contribution`
/// already matches on.
pub fn roots(
    user: Option<PathBuf>,
    repos: &[PathBuf],
    plugins: &[(String, String)],
) -> Vec<(Source, PathBuf)> {
    let mut out = Vec::new();
    if let Some(u) = user {
        out.push((Source::User, u));
    }
    for repo in repos {
        out.push((
            Source::Project {
                path: repo.to_string_lossy().to_string(),
            },
            repo.join(".claude"),
        ));
    }
    for (name, path) in plugins {
        out.push((
            Source::Plugin {
                name: name.clone(),
                path: path.clone(),
            },
            PathBuf::from(path),
        ));
    }
    out
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

        let found = scan_in(root, &Source::User);
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

        let found = scan_in(root, &Source::User);
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

        let found = scan_in(root, &Source::User);
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

        let found = scan_in(root, &Source::User);
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

        let found = scan_in(root, &Source::User);
        assert_eq!(found.definitions.len(), 1);
        assert_eq!(found.definitions[0].name, "x");
    }

    /// An ABSENT directory is not a problem. A machine with no agents
    /// has no `agents/`, and reporting that would make the honest signal
    /// worthless.
    #[test]
    fn absent_directories_are_not_reported_as_unreadable() {
        let tmp = tempfile::tempdir().unwrap();
        let found = scan_in(tmp.path(), &Source::User);
        assert!(found.definitions.is_empty());
        assert!(found.unreadable.is_empty());
    }

    // ---- #1215: three scopes ------------------------------------------

    fn project(path: &str) -> Source {
        Source::Project {
            path: path.to_string(),
        }
    }

    /// THE load-bearing test of #1215.
    ///
    /// A project skill and a user skill with the same name are not one
    /// entry. Which Claude Code actually loads is a rule this module has
    /// not measured, so deduping -- by either precedence -- would delete
    /// a definition the user can see on disk. Both survive, and the
    /// collision names both sources.
    #[test]
    fn a_name_claimed_by_two_scopes_collides_rather_than_deduping() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home").join(".claude");
        let repo = tmp.path().join("repo");
        write(
            &home.join("skills").join("review").join("SKILL.md"),
            "---\nname: review\ndescription: the user's\n---\n",
        );
        write(
            &repo
                .join(".claude")
                .join("skills")
                .join("review")
                .join("SKILL.md"),
            "---\nname: review\ndescription: the project's\n---\n",
        );

        let inv = scan_scopes(&roots(Some(home), std::slice::from_ref(&repo), &[]));

        assert_eq!(
            inv.definitions.len(),
            2,
            "neither definition may be dropped: both exist on disk"
        );
        assert_eq!(inv.collisions.len(), 1, "and the clash is reported");
        let c = &inv.collisions[0];
        assert_eq!(c.name, "review");
        assert_eq!(c.kind, Kind::Skill);
        assert_eq!(c.members.len(), 2);

        // Each colliding member names WHERE it came from, which is the
        // whole point: the user resolves this themselves, and cannot
        // without knowing which file is which.
        let sources: Vec<&Source> = c
            .members
            .iter()
            .map(|i| &inv.definitions[*i].source)
            .collect();
        assert!(sources.contains(&&Source::User));
        assert!(sources
            .iter()
            .any(|s| matches!(s, Source::Project { path } if path == &repo.to_string_lossy())));

        // And no winner is asserted anywhere. There is no field that
        // could carry one -- this assertion is the test that keeps it
        // that way.
        assert!(inv.is_colliding(c.members[0]));
        assert!(inv.is_colliding(c.members[1]));
    }

    /// The same name in one scope twice is still a collision -- but a
    /// skill and a command sharing a name is NOT, because neither
    /// shadows the other and a false alarm teaches the reader to ignore
    /// the real ones.
    #[test]
    fn different_kinds_sharing_a_name_do_not_collide() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".claude");
        write(
            &home.join("skills").join("review").join("SKILL.md"),
            "---\nname: review\n---\n",
        );
        write(
            &home.join("commands").join("review.md"),
            "---\nname: review\n---\n",
        );

        let inv = scan_scopes(&roots(Some(home), &[], &[]));
        assert_eq!(inv.definitions.len(), 2);
        assert!(
            inv.collisions.is_empty(),
            "a skill and a command may share a name without shadowing"
        );
    }

    /// Every definition says which scope it came from. Without this the
    /// list is a pile of names and the user cannot act on any of them.
    #[test]
    fn every_definition_reports_the_scope_it_came_from() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home").join(".claude");
        let repo = tmp.path().join("work").join("headstate");
        let plug = tmp
            .path()
            .join("home")
            .join(".claude")
            .join("plugins")
            .join("cache")
            .join("superpowers");
        write(&home.join("agents").join("u.md"), "---\nname: u\n---\n");
        write(
            &repo.join(".claude").join("agents").join("p.md"),
            "---\nname: p\n---\n",
        );
        write(&plug.join("agents").join("g.md"), "---\nname: g\n---\n");

        let inv = scan_scopes(&roots(
            Some(home),
            std::slice::from_ref(&repo),
            &[(
                "superpowers".to_string(),
                plug.to_string_lossy().to_string(),
            )],
        ));

        let src = |n: &str| {
            inv.definitions
                .iter()
                .find(|d| d.name == n)
                .unwrap_or_else(|| panic!("{n} was not found at all"))
                .source
                .clone()
        };
        assert_eq!(src("u"), Source::User);
        assert_eq!(
            src("p"),
            Source::Project {
                path: repo.to_string_lossy().to_string()
            },
            "and it names WHICH project -- across ~38 repos 'a project' is not an answer"
        );
        assert_eq!(
            src("g"),
            Source::Plugin {
                name: "superpowers".to_string(),
                path: plug.to_string_lossy().to_string()
            }
        );
        // A plugin lives INSIDE the user root, so a prefix test on the
        // path would have called `g` a user definition. The stamp is
        // what makes this answerable.
        assert!(plug.starts_with(tmp.path().join("home").join(".claude")));
    }

    /// The module's existing "what could not be read is reported"
    /// guarantee, carried across a multi-root scan. A project behind a
    /// permission wall hides an unknown number of definitions, and a
    /// successful user scan beside it must not paper over that.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_scope_is_reported_and_named() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home").join(".claude");
        let repo = tmp.path().join("repo");
        write(
            &home.join("agents").join("fine.md"),
            "---\nname: fine\n---\n",
        );
        let walled = repo.join(".claude").join("agents");
        std::fs::create_dir_all(&walled).unwrap();
        write(&walled.join("hidden.md"), "---\nname: hidden\n---\n");
        std::fs::set_permissions(&walled, std::fs::Permissions::from_mode(0o000)).unwrap();

        let inv = scan_scopes(&roots(Some(home), std::slice::from_ref(&repo), &[]));

        // Restored before any assertion, so a failure cannot leave an
        // unreadable directory behind for `tempfile` to trip over.
        std::fs::set_permissions(&walled, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(
            inv.definitions.len(),
            1,
            "the readable scope still reports its definition"
        );
        assert_eq!(
            inv.unreadable.len(),
            1,
            "and the walled-off scope is NOT swallowed by the successful one"
        );
        assert_eq!(
            inv.unreadable[0].source,
            Source::Project {
                path: repo.to_string_lossy().to_string()
            },
            "carrying WHICH scope: a message naming a path does not say \
             whether that was the user's root, a plugin, or one of ~38 projects"
        );
        assert!(inv.unreadable[0].detail.contains("agents"));
    }

    /// One file reached twice -- overlapping roots -- is not a
    /// collision. There is nothing for the user to resolve.
    #[test]
    fn the_same_file_reached_twice_is_not_a_collision() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(".claude");
        write(&root.join("agents").join("a.md"), "---\nname: a\n---\n");

        let inv = scan_scopes(&[
            (Source::User, root.clone()),
            (project("/somewhere"), root.clone()),
        ]);
        assert_eq!(inv.definitions.len(), 2, "both walks found it");
        assert!(
            inv.collisions.is_empty(),
            "but it is ONE file, and naming it as its own rival is not actionable"
        );
    }

    /// `project_roots` finds a `.claude` without running git, which is
    /// what keeps ~38 repositories affordable in one command.
    #[test]
    fn project_roots_finds_dot_claude_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("code").join("alpha");
        let b = tmp.path().join("code").join("beta");
        std::fs::create_dir_all(a.join(".claude").join("agents")).unwrap();
        std::fs::create_dir_all(b.join("src")).unwrap();

        let (found, unreadable) = project_roots(&[tmp.path().to_string_lossy().to_string()]);
        assert_eq!(found, vec![a], "beta has no .claude and is not a scope");
        assert!(unreadable.is_empty());
    }

    /// `Collision::members` index into the FINAL, sorted list. Computing
    /// them before the sort -- or sorting again afterwards -- would leave
    /// every index pointing at the wrong row, and the UI marks rows by
    /// exactly these numbers. A silent off-by-one here mislabels which
    /// definition is in conflict, which is worse than not reporting it.
    #[test]
    fn collision_members_index_the_sorted_list() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home").join(".claude");
        let repo = tmp.path().join("repo");
        // `zeta` is written FIRST and in the user scope, so an
        // unsorted list would put it at index 0 -- the sort moves it
        // last, and the collision indices must follow.
        write(
            &home.join("agents").join("zeta.md"),
            "---\nname: zeta\n---\n",
        );
        write(
            &home.join("agents").join("alpha.md"),
            "---\nname: alpha\n---\n",
        );
        write(
            &repo.join(".claude").join("agents").join("alpha.md"),
            "---\nname: alpha\n---\n",
        );

        let inv = scan_scopes(&roots(Some(home), std::slice::from_ref(&repo), &[]));

        assert_eq!(
            inv.definitions
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "alpha", "zeta"],
        );
        let c = &inv.collisions[0];
        assert_eq!(c.name, "alpha");
        for m in &c.members {
            assert_eq!(
                inv.definitions[*m].name, "alpha",
                "member {m} points at the wrong row: the indices did not follow the sort"
            );
        }
    }

    /// A definition in NO scope at all is the shape #1215 exists to
    /// fix: before this, a project skill simply did not appear.
    #[test]
    fn a_project_skill_is_visible_at_all() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        write(
            &repo
                .join(".claude")
                .join("skills")
                .join("deploy")
                .join("SKILL.md"),
            "---\nname: deploy\n---\n",
        );
        // No user root: this is exactly the case the old single-root
        // scan reported as empty.
        let inv = scan_scopes(&roots(None, &[repo], &[]));
        assert_eq!(inv.definitions.len(), 1);
        assert_eq!(inv.definitions[0].name, "deploy");
    }
}

//! The repository's `.claude/rules/*.md`: what they say and which
//! directories their `paths:` frontmatter scopes them to.
//!
//! Claude Code loads every `.md` file under `.claude/rules/`
//! (subdirectories included). A rule with no `paths:` loads at launch
//! like the root CLAUDE.md; one with `paths:` loads when a session reads
//! a matching file, like a nested CLAUDE.md. Shared by the toolchain
//! ("names"), transcripts ("already written") and gaps ("covered")
//! producers (#1340). The placement producer probes only whether the
//! directory exists and keeps its own probe.
//!
//! # Unreadable is not "no rules"
//!
//! A `.claude/rules` that does not exist (or is not a directory) is no
//! rules, an ordinary answer. One that exists and cannot be listed, a
//! subdirectory that cannot be listed, or a rule that cannot be read is
//! recorded in [`Rules::unreadable`] with the io error, and a caller
//! stating a negative that depends on the rules must say Unknown.
//!
//! # The glob matching is deliberately small
//!
//! A pattern is split at `/`. `**` as a whole segment matches zero or
//! more segments; `*` inside a segment matches any run of characters
//! within that segment; one level of `{a,b}` is expanded before matching.
//! NOT supported, and matched literally: `?`, `[...]` classes, nested
//! braces, `!` negation and escapes. A leading `./` or `/` is dropped.
//!
//! A directory is in a pattern's reach when a file directly in it could
//! match: the pattern less its last segment (all of it when the last is
//! `**`) matches the directory's segments. The file name is not checked,
//! so `crates/foo/**/*.rs` reaches `crates/foo` whatever it holds.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Subdirectory depth under `.claude/rules` read before stopping, so a
/// symlink cycle cannot loop.
const MAX_DEPTH: usize = 8;

/// One rule file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub path: PathBuf,
    pub text: String,
    /// The `paths:` patterns, verbatim less quotes. Empty: unconditional.
    pub paths: Vec<String>,
}

/// Every rule read, and what could not be.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rules {
    pub files: Vec<Rule>,
    /// `path (io error)` for every directory or file that exists and
    /// could not be read.
    pub unreadable: Vec<String>,
}

impl Rule {
    /// Whether a path-scoped rule gives `rel_dir` (relative to the root,
    /// `/`-separated, empty for the root) a place of its own: some
    /// pattern reaches it and its first segment is neither `**` nor a
    /// bare `*`. `**/*.ts` reaches everywhere and scopes nothing, as the
    /// root file covers nothing below it by itself. An unconditional rule
    /// scopes nothing.
    pub fn scopes(&self, rel_dir: &str) -> bool {
        self.patterns()
            .iter()
            .any(|p| p.first().is_some_and(|s| s != "**" && s != "*") && reaches(p, rel_dir))
    }

    /// Whether a session working in `rel_dir` would load this rule:
    /// unconditional, or some pattern reaches it.
    pub fn applies_to(&self, rel_dir: &str) -> bool {
        self.paths.is_empty() || self.patterns().iter().any(|p| reaches(p, rel_dir))
    }

    fn patterns(&self) -> Vec<Vec<String>> {
        self.paths
            .iter()
            .flat_map(|p| expand_braces(p))
            .map(|p| {
                let p = p.trim_start_matches("./").trim_start_matches('/');
                p.split('/')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .collect()
    }
}

/// Every rule under `repo/.claude/rules`, in path order.
pub fn read(repo: &Path) -> Rules {
    let mut out = Rules::default();
    walk(&repo.join(".claude").join("rules"), 0, true, &mut out);
    out.files.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn walk(dir: &Path, depth: usize, top: bool, out: &mut Rules) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if top && matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            return
        }
        Err(e) => {
            out.unreadable
                .push(format!("{} ({e})", dir.to_string_lossy()));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                out.unreadable
                    .push(format!("{} ({e})", dir.to_string_lossy()));
                continue;
            }
        };
        let path = entry.path();
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                out.unreadable
                    .push(format!("{} ({e})", path.to_string_lossy()));
                continue;
            }
        };
        if meta.is_dir() {
            if depth < MAX_DEPTH {
                walk(&path, depth + 1, false, out);
            }
        } else if path.extension().is_some_and(|x| x == "md") {
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    let paths = paths_of(&text);
                    out.files.push(Rule { path, text, paths });
                }
                Err(e) => out
                    .unreadable
                    .push(format!("{} ({e})", path.to_string_lossy())),
            }
        }
    }
}

/// The `paths:` values of a rule's frontmatter: a block list
/// (`- "src/**"` lines), a flow list (`[a, b]`), or one inline value,
/// which may be comma-separated. No closed frontmatter is no `paths:`.
fn paths_of(text: &str) -> Vec<String> {
    let text = text.replace("\r\n", "\n");
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let lines: Vec<&str> = text.lines().collect();
    if lines.first().map(|l| l.trim_end()) != Some("---") {
        return Vec::new();
    }
    let Some(close) = lines[1..].iter().position(|l| l.trim_end() == "---") else {
        return Vec::new();
    };
    let front = &lines[1..=close];
    let mut out = Vec::new();
    let mut i = 0;
    while i < front.len() {
        let Some(value) = front[i].strip_prefix("paths:") else {
            i += 1;
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            for item in &front[i + 1..] {
                match item.trim_start().strip_prefix("- ") {
                    Some(v) if item.starts_with([' ', '\t', '-']) => out.push(unquote(v)),
                    _ if item.trim().is_empty() => {}
                    _ => break,
                }
            }
        } else {
            let value = value
                .strip_prefix('[')
                .and_then(|v| v.strip_suffix(']'))
                .unwrap_or(value);
            out.extend(split_top_level(value).into_iter().map(unquote));
        }
        i += 1;
    }
    out.retain(|p| !p.is_empty());
    out
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').trim_matches('\'').to_string()
}

/// `s` split at commas outside braces, so `src/*.{ts,tsx}, lib/**` is two.
fn split_top_level(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// One level of `{a,b}` expanded: the first brace pair, then again on
/// each result, so `{a,b}/*.{c,d}` is four. A pair nested in another is
/// not supported and is left as written.
fn expand_braces(p: &str) -> Vec<String> {
    let (Some(open), Some(close)) = (p.find('{'), p.find('}')) else {
        return vec![p.to_string()];
    };
    if close < open || p[open + 1..close].contains('{') {
        return vec![p.to_string()];
    }
    p[open + 1..close]
        .split(',')
        .flat_map(|alt| expand_braces(&format!("{}{alt}{}", &p[..open], &p[close + 1..])))
        .collect()
}

/// Whether a file directly in `rel_dir` could match `pattern`.
fn reaches(pattern: &[String], rel_dir: &str) -> bool {
    let dir: Vec<&str> = rel_dir.split('/').filter(|s| !s.is_empty()).collect();
    let pat: Vec<&str> = match pattern.last().map(String::as_str) {
        Some("**") => pattern.iter().map(String::as_str).collect(),
        Some(_) => pattern[..pattern.len() - 1]
            .iter()
            .map(String::as_str)
            .collect(),
        None => return false,
    };
    segments_match(&pat, &dir)
}

fn segments_match(pat: &[&str], path: &[&str]) -> bool {
    match pat.first() {
        None => path.is_empty(),
        Some(&"**") => (0..=path.len()).any(|k| segments_match(&pat[1..], &path[k..])),
        Some(p) => {
            path.first().is_some_and(|s| segment_match(p, s))
                && segments_match(&pat[1..], &path[1..])
        }
    }
}

/// `*` matches any run within one segment; everything else is literal.
fn segment_match(pat: &str, s: &str) -> bool {
    let parts: Vec<&str> = pat.split('*').collect();
    if parts.len() == 1 {
        return pat == s;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    // Both checks together guarantee `s` is long enough for the slice.
    if !s.starts_with(first) || !s[first.len()..].ends_with(last) {
        return false;
    }
    let mut rest = &s[first.len()..s.len() - last.len()];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn rule(paths: &[&str]) -> Rule {
        Rule {
            path: PathBuf::from("r.md"),
            text: String::new(),
            paths: paths.iter().map(|p| p.to_string()).collect(),
        }
    }

    /// The three frontmatter shapes, quotes stripped, and no closed
    /// frontmatter is no `paths:`.
    #[test]
    fn paths_are_read_from_each_frontmatter_shape() {
        assert_eq!(
            paths_of("---\npaths:\n  - \"src/api/**/*.ts\"\n  - 'lib/**'\n---\n# API\n"),
            vec!["src/api/**/*.ts", "lib/**"]
        );
        assert_eq!(
            paths_of("---\r\ndescription: x\r\npaths: [\"a/**\", b/*.md]\r\n---\r\n"),
            vec!["a/**", "b/*.md"]
        );
        assert_eq!(
            paths_of("---\npaths: src/*.{ts,tsx}, docs/**\n---\n"),
            vec!["src/*.{ts,tsx}", "docs/**"]
        );
        assert_eq!(
            paths_of("# no frontmatter\npaths: a/**\n"),
            Vec::<String>::new()
        );
        assert_eq!(paths_of("---\npaths: a/**\n"), Vec::<String>::new());
        assert_eq!(paths_of("---\ndescription: x\n---\n"), Vec::<String>::new());
    }

    /// A pattern reaches a directory a file directly in it could match,
    /// and scopes it only from a literal first segment.
    #[test]
    fn a_pattern_scopes_the_directories_it_reaches() {
        let r = rule(&["crates/foo/**/*.rs"]);
        assert!(r.scopes("crates/foo"));
        assert!(r.scopes("crates/foo/src"));
        assert!(!r.scopes("crates"));
        assert!(!r.scopes("crates/bar"));

        assert!(rule(&["crates/foo/**"]).scopes("crates/foo"));
        assert!(rule(&["crates/**"]).scopes("crates/foo"));
        assert!(rule(&["./packages/*/src/*.ts"]).scopes("packages/web/src"));
        assert!(!rule(&["packages/*/src/*.ts"]).scopes("packages/web"));
        assert!(rule(&["pkg-*/**"]).scopes("pkg-web"));
        assert!(rule(&["{apps,libs}/web/**"]).scopes("libs/web"));
        assert!(rule(&["docs/*.md"]).scopes("docs"));

        // Reaches everywhere, scopes nothing; but it applies.
        assert!(!rule(&["*/src/**"]).scopes("web/src"));
        let everywhere = rule(&["**/*.ts"]);
        assert!(!everywhere.scopes("crates/foo"));
        assert!(everywhere.applies_to("crates/foo"));
        // Unconditional: applies everywhere, scopes nothing.
        let always = rule(&[]);
        assert!(!always.scopes("docs"));
        assert!(always.applies_to("docs"));
        assert!(!rule(&["docs/**"]).applies_to("src"));

        // Not supported, and matched literally.
        assert!(!rule(&["doc?/**"]).scopes("docs"));
        assert!(!rule(&["[d]ocs/**"]).scopes("docs"));
    }

    /// Absent is no rules; subdirectories are read; a non-`.md` file is
    /// not a rule.
    #[test]
    fn rules_are_read_recursively_and_absent_is_empty() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(read(t.path()), Rules::default());

        let dir = t.path().join(".claude").join("rules");
        fs::create_dir_all(dir.join("area")).unwrap();
        fs::write(dir.join("testing.md"), "Run `make test`.\n").unwrap();
        fs::write(
            dir.join("area").join("api.md"),
            "---\npaths: api/**\n---\nx\n",
        )
        .unwrap();
        fs::write(dir.join("notes.txt"), "not a rule").unwrap();
        let got = read(t.path());
        assert!(got.unreadable.is_empty(), "{got:?}");
        let names: Vec<_> = got
            .files
            .iter()
            .map(|r| r.path.strip_prefix(&dir).unwrap().to_path_buf())
            .collect();
        assert_eq!(
            names,
            vec![
                PathBuf::from("area").join("api.md"),
                PathBuf::from("testing.md")
            ]
        );
        assert_eq!(got.files[0].paths, vec!["api/**"]);
    }

    /// A rules directory that exists and cannot be listed is recorded
    /// with the io error, never an empty list. Under root's DAC override
    /// the bit does nothing; the gate drops it for this reason.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_rules_directory_is_recorded_not_empty() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join(".claude").join("rules");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.md"), "x\n").unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).unwrap();
        let got = read(t.path());
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(got.files.is_empty());
        assert_eq!(got.unreadable.len(), 1, "{got:?}");
        assert!(got.unreadable[0].contains(".claude"), "{got:?}");
    }
}

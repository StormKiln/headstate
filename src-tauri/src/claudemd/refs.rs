//! Reference extraction from CLAUDE.md prose: the things a file names
//! that a producer can check against the repository.
//!
//! Landed with the advice model so the rot, skills and toolchain
//! producers read one extractor rather than three. Nothing here resolves
//! anything; it classifies what the text SAYS, and the producers decide
//! what each kind resolves against (the table in the design document).
//!
//! # What is, and is not, a reference
//!
//! Only inline code spans outside fences are read, plus `#NNNN` issue
//! numbers in prose. Prose never names a path on purpose often enough to
//! be worth the false positives, and a fenced block is somebody's syntax.
//!
//! Bare words are never symbols. Measured on this repository, `main` hits
//! 100 files and `false` 355, so a bare word proves nothing; only
//! `A::b`, `name()` and `SCREAMING_CASE` (with at least one underscore,
//! so `ALL` and `README` do not qualify) are symbols. `-D warnings`,
//! A `/slash-command` is a skill invocation, not a path. `PATH_TOKEN`
//! matches `/stacked-prs` happily and the resolver joined it onto an
//! anchor, where a leading `/` discards the anchor and probes the real
//! filesystem root -- so it was reported missing from the repository it
//! was never looked for in (#1300). A leading `/` followed by a
//! kebab-case word is the invocation shape and nothing else, so it is
//! classified as a skill and resolved against the definitions inventory,
//! which already knows every skill name in every scope.
//!
//! `\r\n` and `format!("{}/…")` are not paths: a path is a single token of
//! `[A-Za-z0-9_./-]` that contains a `/`, ends in a known extension, or is
//! a known extensionless manifest name. `~/` paths are outside the
//! repository and are not extracted; `imports.rs` already resolves the
//! `@~/` form.

use super::text;
use regex::Regex;
use std::sync::LazyLock;

/// One reference, with the line it was found on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    /// 1-based.
    pub line: usize,
    /// The span text, verbatim.
    pub raw: String,
    pub kind: RefKind,
}

/// What kind of thing a span names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefKind {
    /// A file or directory path.
    Path { path: String },
    /// A path with a line number, `claude/cli.rs:128`.
    PathLine { path: String, line: u32 },
    /// `make <target>`.
    MakeTarget { name: String },
    /// `yarn <script>` or `npm run <script>`.
    Script { runner: Runner, name: String },
    /// Any `cargo …` invocation. Counted, never resolved.
    Cargo,
    /// A skill, named as `` `X` `` skill, as a `## Skills` list item,
    /// or as a `/slash-command` invocation.
    Skill { name: String },
    /// A qualified symbol: the last segment is what a grep resolves.
    Symbol { last: String },
    /// `#NNNN`. Counted, never resolved: `claudemd` never talks to GitHub.
    Issue { number: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runner {
    Yarn,
    Npm,
}

static PATH_TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_./-]+$").unwrap());
static PATH_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z0-9_./-]+):(\d+)$").unwrap());
static MAKE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^make ([A-Za-z0-9_-]+)$").unwrap());
// `yarn run x` names the script `x`, so the optional `run` is skipped
// rather than reported as a script called "run".
static YARN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^yarn (?:run )?(\S+)").unwrap());
static NPM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^npm run (\S+)").unwrap());
static QUALIFIED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)+(?:\(\))?$").unwrap()
});
static CALL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*)\(\)$").unwrap());
static SCREAMING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+$").unwrap());
static ISSUE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#(\d+)$").unwrap());
// `/stacked-prs`, and the `/plugin:skill` form a plugin's command takes.
// Lower-case kebab only: `/Users/me/notes.md` and `/usr/bin` carry an
// upper-case segment or a second slash and are not invocations.
static SLASH_COMMAND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/(?:[a-z0-9]+(?:-[a-z0-9]+)*:)?[a-z0-9]+(?:-[a-z0-9]+)*$").unwrap()
});

/// File extensions a span can carry to count as a path without a `/`.
const EXTENSIONS: &[&str] = &[
    "md", "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "json", "toml", "yaml", "yml", "py", "sh",
    "css", "html", "swift", "kt", "java", "go", "rb", "lock", "txt", "xml", "plist", "sql", "mk",
    "cfg", "ini", "env", "csv", "svg", "png",
];

/// Extensionless names that are paths on their own.
const MANIFESTS: &[&str] = &[
    "Makefile",
    "GNUmakefile",
    "makefile",
    "justfile",
    "Dockerfile",
];

/// Whether a single token is a path.
fn is_path(token: &str) -> bool {
    if !PATH_TOKEN.is_match(token) || token == "." || token == ".." {
        return false;
    }
    if token.contains('/') {
        return true;
    }
    if MANIFESTS.contains(&token) {
        return true;
    }
    let last = token.rsplit('/').next().unwrap_or(token);
    // The extension after the LAST dot, and a name of at least one
    // character before it, so `.md` alone is not a path and `v1.0` is not
    // a file named `v1` with extension `0`.
    match last.rsplit_once('.') {
        Some((name, ext)) => !name.is_empty() && EXTENSIONS.contains(&ext),
        None => false,
    }
}

/// Classify one span's text. `None` when it names nothing checkable.
fn classify(text: &str) -> Option<RefKind> {
    if let Some(c) = MAKE.captures(text) {
        return Some(RefKind::MakeTarget {
            name: c[1].to_string(),
        });
    }
    if let Some(c) = YARN.captures(text) {
        return Some(RefKind::Script {
            runner: Runner::Yarn,
            name: c[1].to_string(),
        });
    }
    if let Some(c) = NPM.captures(text) {
        return Some(RefKind::Script {
            runner: Runner::Npm,
            name: c[1].to_string(),
        });
    }
    if text.starts_with("cargo ") {
        return Some(RefKind::Cargo);
    }
    if let Some(c) = ISSUE.captures(text) {
        return c[1].parse().ok().map(|number| RefKind::Issue { number });
    }
    // Before the path arms: `PATH_TOKEN` matches a slash command, and
    // the resolver would probe the filesystem root for it (#1300).
    if SLASH_COMMAND.is_match(text) {
        return Some(RefKind::Skill {
            name: text.trim_start_matches('/').to_string(),
        });
    }
    if let Some(c) = PATH_LINE.captures(text) {
        if is_path(&c[1]) {
            if let Ok(line) = c[2].parse() {
                return Some(RefKind::PathLine {
                    path: c[1].to_string(),
                    line,
                });
            }
        }
    }
    if is_path(text) {
        return Some(RefKind::Path {
            path: text.to_string(),
        });
    }
    if QUALIFIED.is_match(text) {
        let last = text
            .trim_end_matches("()")
            .rsplit("::")
            .next()
            .unwrap_or(text)
            .to_string();
        return Some(RefKind::Symbol { last });
    }
    if let Some(c) = CALL.captures(text) {
        return Some(RefKind::Symbol {
            last: c[1].to_string(),
        });
    }
    if SCREAMING.is_match(text) {
        return Some(RefKind::Symbol {
            last: text.to_string(),
        });
    }
    None
}

/// Every reference in a file's text, in document order.
pub fn extract(text: &str) -> Vec<Ref> {
    let normalised = text.replace("\r\n", "\n");
    let lines: Vec<&str> = normalised.split('\n').collect();

    // Lines that are list items under a `## Skills` heading (any level):
    // the first span on such a line is a skill name.
    let mut skill_list_lines = std::collections::BTreeSet::new();
    for s in text::sections(&normalised) {
        if s.heading
            .as_deref()
            .is_some_and(|h| h.eq_ignore_ascii_case("skills"))
        {
            for (i, l) in s.text.split('\n').enumerate() {
                let t = l.trim_start();
                if t.starts_with("- ") || t.starts_with("* ") {
                    skill_list_lines.insert(s.line + 1 + i);
                }
            }
        }
    }
    let mut first_span_on: std::collections::BTreeSet<usize> = Default::default();
    // `(line, column, ref)`, so the two passes below can be merged back
    // into document order.
    let mut found: Vec<(usize, usize, Ref)> = Vec::new();

    for span in text::spans(&normalised) {
        let line_text = lines.get(span.line - 1).copied().unwrap_or("");
        let first_on_line = first_span_on.insert(span.line);
        let at = line_text.find(&format!("`{}`", span.text));

        // `` `X` `` skill: the word "skill" follows the closing backtick.
        let followed_by_skill = at
            .map(|at| line_text[at + span.text.len() + 2..].trim_start())
            .is_some_and(|rest| {
                rest.starts_with("skill")
                    && !rest[5..].starts_with(|c: char| c.is_ascii_alphanumeric())
            });
        let in_skill_list = first_on_line && skill_list_lines.contains(&span.line);
        let col = at.unwrap_or(0);
        if followed_by_skill || in_skill_list {
            found.push((
                span.line,
                col,
                Ref {
                    line: span.line,
                    raw: span.text.clone(),
                    kind: RefKind::Skill {
                        name: span.text.clone(),
                    },
                },
            ));
            continue;
        }

        if let Some(kind) = classify(&span.text) {
            found.push((
                span.line,
                col,
                Ref {
                    line: span.line,
                    raw: span.text,
                    kind,
                },
            ));
        }
    }

    // `#NNNN` in prose, outside fences. Trailing punctuation and a
    // wrapping parenthesis are stripped, so "(#1044)" and "#1044." count.
    // A backticked `#NNNN` keeps its backticks after the trim, so it does
    // not match here and is counted once, by the span pass.
    for (n, l) in text::prose_lines(&normalised) {
        for token in l.split_whitespace() {
            let t = token
                .trim_start_matches('(')
                .trim_end_matches(['.', ',', ';', ':', ')']);
            if let Some(c) = ISSUE.captures(t) {
                if let Ok(number) = c[1].parse() {
                    found.push((
                        n,
                        l.find(t).unwrap_or(0),
                        Ref {
                            line: n,
                            raw: t.to_string(),
                            kind: RefKind::Issue { number },
                        },
                    ));
                }
            }
        }
    }

    found.sort_by_key(|(line, col, _)| (*line, *col));
    found.into_iter().map(|(_, _, r)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<RefKind> {
        extract(text).into_iter().map(|r| r.kind).collect()
    }

    #[test]
    fn a_path_needs_a_slash_or_a_known_extension() {
        assert_eq!(
            kinds("see `src/lib/target.ts` and `CLAUDE.md` and `Makefile`\n"),
            vec![
                RefKind::Path {
                    path: "src/lib/target.ts".into()
                },
                RefKind::Path {
                    path: "CLAUDE.md".into()
                },
                RefKind::Path {
                    path: "Makefile".into()
                },
            ]
        );
    }

    /// The counter-examples the design names: flags, escapes and macro
    /// calls are not paths, and bare words are not symbols.
    #[test]
    fn flags_escapes_macros_and_bare_words_are_nothing() {
        let text = "under `-D warnings` normalise `\\r\\n` never `format!(\"{}/…\")` `main` `false` `ALL`\n";
        assert!(kinds(text).is_empty(), "{:?}", extract(text));
    }

    /// `src-tauri/Cargo.toml` is a path, not a cargo command; `cargo fmt`
    /// is the command.
    #[test]
    fn a_cargo_manifest_is_a_path_and_a_cargo_command_is_cargo() {
        assert_eq!(
            kinds("`src-tauri/Cargo.toml` then `cargo fmt`\n"),
            vec![
                RefKind::Path {
                    path: "src-tauri/Cargo.toml".into()
                },
                RefKind::Cargo,
            ]
        );
    }

    #[test]
    fn a_path_with_a_line_is_a_path_line() {
        assert_eq!(
            kinds("stated at `claude/cli.rs:128`\n"),
            vec![RefKind::PathLine {
                path: "claude/cli.rs".into(),
                line: 128
            }]
        );
    }

    #[test]
    fn make_targets_and_scripts_are_named() {
        assert_eq!(
            kinds("`make lint`, not `yarn lint`; `npm run build`; `make` alone\n"),
            vec![
                RefKind::MakeTarget {
                    name: "lint".into()
                },
                RefKind::Script {
                    runner: Runner::Yarn,
                    name: "lint".into()
                },
                RefKind::Script {
                    runner: Runner::Npm,
                    name: "build".into()
                },
            ]
        );
    }

    /// `yarn run x` and `yarn x` name the same script.
    #[test]
    fn yarn_run_names_the_script_after_run() {
        assert_eq!(
            kinds("`yarn run build` then `yarn vitest run`\n"),
            vec![
                RefKind::Script {
                    runner: Runner::Yarn,
                    name: "build".into()
                },
                RefKind::Script {
                    runner: Runner::Yarn,
                    name: "vitest".into()
                },
            ]
        );
    }

    #[test]
    fn qualified_symbols_calls_and_screaming_case_are_symbols() {
        assert_eq!(
            kinds(
                "`Terms::splice`, `rank()`, `OBSERVED_REMAINING`, `packages::markdown::render()`\n"
            ),
            vec![
                RefKind::Symbol {
                    last: "splice".into()
                },
                RefKind::Symbol {
                    last: "rank".into()
                },
                RefKind::Symbol {
                    last: "OBSERVED_REMAINING".into()
                },
                RefKind::Symbol {
                    last: "render".into()
                },
            ]
        );
    }

    #[test]
    fn a_skill_is_named_by_the_word_or_by_the_list() {
        let text = "\
Use the `verify` skill before pushing. Skillful `prose` is not one.

## Skills

- `release` — tag-driven releases
- `guard` — adding an invariant
";
        assert_eq!(
            kinds(text),
            vec![
                RefKind::Skill {
                    name: "verify".into()
                },
                RefKind::Skill {
                    name: "release".into()
                },
                RefKind::Skill {
                    name: "guard".into()
                },
            ]
        );
    }

    /// A slash command is a skill invocation, not a path (#1300).
    /// `PATH_TOKEN` matches `/stacked-prs`, and the resolver probed the
    /// real filesystem root for it.
    #[test]
    fn a_slash_command_is_a_skill_not_a_path() {
        assert_eq!(
            kinds("Run `/stacked-prs`, then `/figma-sync` and `/pr:review`.\n"),
            vec![
                RefKind::Skill {
                    name: "stacked-prs".into()
                },
                RefKind::Skill {
                    name: "figma-sync".into()
                },
                RefKind::Skill {
                    name: "pr:review".into()
                },
            ]
        );
    }

    /// An absolute path is not an invocation: a second slash, a dot or
    /// an upper-case segment all rule it out, so `/etc/hosts` stays a
    /// path and `/Users/me` stays outside the invocation shape.
    #[test]
    fn an_absolute_path_is_not_a_slash_command() {
        assert_eq!(
            kinds("`/etc/hosts` `/README.md` `/Users/me/notes.md`\n"),
            vec![
                RefKind::Path {
                    path: "/etc/hosts".into()
                },
                RefKind::Path {
                    path: "/README.md".into()
                },
                RefKind::Path {
                    path: "/Users/me/notes.md".into()
                },
            ]
        );
    }

    #[test]
    fn issue_numbers_are_counted_in_prose_and_spans_once_each() {
        let refs =
            extract("shipped in #846 (see #1044). Also `#972`.\n```\n#999 not counted\n```\n");
        let numbers: Vec<u64> = refs
            .iter()
            .filter_map(|r| match r.kind {
                RefKind::Issue { number } => Some(number),
                _ => None,
            })
            .collect();
        assert_eq!(numbers, vec![846, 1044, 972]);
    }

    #[test]
    fn a_fenced_block_is_not_read() {
        assert!(kinds("```\n`src/a.ts` and make lint\n```\n").is_empty());
    }

    #[test]
    fn lines_are_one_based() {
        let refs = extract("first\nsecond `src/x.rs`\n");
        assert_eq!(refs[0].line, 2);
    }
}

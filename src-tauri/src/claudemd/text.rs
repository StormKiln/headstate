//! Shared parsing of CLAUDE.md prose: sections, inline code spans and
//! fenced blocks, all fence-aware.
//!
//! Three advice producers read CLAUDE.md text and two of the same
//! mistakes would otherwise be made three times: a `#` inside a fenced
//! block is not a heading, and a backtick inside one is not a span. One
//! walker here, in the shape of `imports::parse_imports` (a fence toggles
//! on ```` ``` ```` and `~~~`), and every producer reads through it.
//!
//! # Setext headings are deliberately unsupported
//!
//! A setext heading is a line of text followed by `===` or `---`. Every
//! `SKILL.md` opens with YAML frontmatter -- a `---` line, keys, a `---`
//! line -- and a naive setext detector turns the last key of that block
//! into an h2. Only column-0 ATX headings (`#` to `######`, followed by a
//! space) count. An indented `#` is a code block or a list continuation
//! in CommonMark, and is not a heading here either.
//!
//! # Block-level HTML comments are invisible here, as they are to Claude
//!
//! Claude Code strips block-level HTML comments from a CLAUDE.md before
//! injecting it (changelog 2.1.72; the memory docs: "Block-level HTML
//! comments … are stripped before the content is injected … Comments
//! inside code blocks are preserved"). A rule written inside a comment
//! is therefore a rule Claude never reads, and a producer that counted
//! it would report an instruction that does not exist. So the walker
//! classifies a comment's lines as [`Kind::Comment`], and `sections`,
//! `spans` and `prose_lines` all skip them. [`strip_block_html_comments`]
//! is the same rule applied to the text as a whole, for the token
//! estimate.
//!
//! Block-level means: a line whose first non-blank characters are `<!--`
//! opens the comment, and it runs to the first line that ends (bar
//! whitespace) with `-->`. A comment that opens and closes on one line
//! with text after the `-->` is inline, and is kept: it is injected. A
//! `<!--` inside a fence is code, and is kept.
//!
//! # Line numbers are 1-based
//!
//! So a producer can print `path:line` and an editor opens the right
//! line. `Section::line` is the heading's line; the preamble before the
//! first heading is a section at line 1 with no heading.

use std::path::Path;

/// One section: a heading and the text under it, up to the next heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The heading text after its `#`s, trimmed. `None` for the text
    /// before the first heading.
    pub heading: Option<String>,
    /// 1 to 6 for a heading; 0 for the preamble.
    pub level: u8,
    /// The heading's line, 1-based; 1 for the preamble.
    pub line: usize,
    /// The lines under the heading, joined with `\n`, without the heading
    /// line itself.
    pub text: String,
}

/// One inline code span outside a fence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub line: usize,
    /// The text between the backticks, with one surrounding space
    /// stripped from each side when both are present, as CommonMark
    /// renders it.
    pub text: String,
}

/// One fenced block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fence {
    /// The opening fence's line, 1-based.
    pub line: usize,
    /// The info string after the fence marker, trimmed. Empty when none.
    pub info: String,
    /// The lines between the fences, joined with `\n`.
    pub body: String,
}

/// What one line is, once fences are accounted for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Prose,
    /// The line that opens a fence, with its info string.
    Open(String),
    Body,
    Close,
    /// A line of a block-level HTML comment, which Claude Code strips
    /// before injection. Never inside a fence.
    Comment,
}

/// Every line, numbered and classified.
///
/// A fence opens on a line whose first non-blank characters are three or
/// more backticks or tildes, and closes on a line of the same character
/// at least as long with nothing else on it. Requiring the same character
/// is the difference between a ```` ``` ```` inside a `~~~` block being
/// body and being a close. `parse_imports` reads through this walker for
/// that reason.
fn walk(text: &str) -> Vec<(usize, &str, Kind)> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let mut out = Vec::new();
    // The open fence's character and length, while inside one.
    let mut open: Option<(char, usize)> = None;
    // Inside a block-level HTML comment that has not yet closed.
    let mut in_comment = false;

    for (i, line) in text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let n = i + 1;
        let trimmed = line.trim_start();
        if in_comment {
            if closes_comment(line) {
                in_comment = false;
            }
            out.push((n, line, Kind::Comment));
            continue;
        }
        let marker = fence_marker(trimmed);
        match (open, marker) {
            (None, Some((ch, len))) => {
                let info = trimmed[len..].trim().to_string();
                open = Some((ch, len));
                out.push((n, line, Kind::Open(info)));
            }
            (Some((ch, len)), Some((mch, mlen)))
                if mch == ch && mlen >= len && trimmed[mlen..].trim().is_empty() =>
            {
                open = None;
                out.push((n, line, Kind::Close));
            }
            (Some(_), _) => out.push((n, line, Kind::Body)),
            (None, None) => {
                if let Some(closed) = opens_comment(trimmed) {
                    in_comment = !closed;
                    out.push((n, line, Kind::Comment));
                } else {
                    out.push((n, line, Kind::Prose));
                }
            }
        }
    }
    out
}

/// Whether a prose line opens a block-level HTML comment, and if so
/// whether it also closes it. `None` for a line that is not a comment
/// opener, including an inline `<!-- x --> text`, which is injected.
fn opens_comment(trimmed: &str) -> Option<bool> {
    let rest = trimmed.strip_prefix("<!--")?;
    match rest.find("-->") {
        // Closes on this line: block-level only when nothing follows.
        Some(at) => rest[at + 3..].trim().is_empty().then_some(true),
        None => Some(false),
    }
}

/// Whether a line inside a comment closes it: `-->` with nothing but
/// whitespace after.
fn closes_comment(line: &str) -> bool {
    line.trim_end().ends_with("-->")
}

/// The text with block-level HTML comments removed: what Claude Code
/// injects, and therefore what `tokens::estimate` must count.
///
/// Lines are rejoined with `\n` (a CRLF file comes back LF), and the
/// trailing newline is kept when the input had one. Comments inside
/// fences are kept, as Claude Code keeps them.
pub fn strip_block_html_comments(text: &str) -> String {
    let mut out: Vec<&str> = walk(text)
        .into_iter()
        .filter(|(_, _, kind)| *kind != Kind::Comment)
        .map(|(_, line, _)| line)
        .collect();
    if text.ends_with('\n') {
        out.push("");
    }
    out.join("\n")
}

/// The fence character and run length at the start of a trimmed line,
/// when there is one of at least three.
fn fence_marker(trimmed: &str) -> Option<(char, usize)> {
    let ch = trimmed.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let len = trimmed.chars().take_while(|c| *c == ch).count();
    (len >= 3).then_some((ch, len))
}

/// A column-0 ATX heading's level and text, when the line is one.
fn heading(line: &str) -> Option<(u8, &str)> {
    if !line.starts_with('#') {
        return None;
    }
    let level = line.chars().take_while(|c| *c == '#').count();
    if level > 6 {
        return None;
    }
    let rest = &line[level..];
    if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
        // `#hashtag` is not a heading.
        return None;
    }
    Some((level as u8, rest.trim()))
}

/// The sections of a file, split at column-0 ATX headings outside fences.
///
/// The text before the first heading is a section with no heading when it
/// has any non-blank line; a file that opens with a heading has no
/// preamble section.
pub fn sections(text: &str) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    let mut current: Option<(Option<String>, u8, usize, Vec<&str>)> = None;

    for (n, line, kind) in walk(text) {
        if kind == Kind::Prose {
            if let Some((level, title)) = heading(line) {
                if let Some((h, l, at, lines)) = current.take() {
                    push_section(&mut out, h, l, at, &lines);
                }
                current = Some((Some(title.to_string()), level, n, Vec::new()));
                continue;
            }
        }
        // Not injected, so not part of any section's text.
        if kind == Kind::Comment {
            continue;
        }
        match &mut current {
            Some((_, _, _, lines)) => lines.push(line),
            None => current = Some((None, 0, 1, vec![line])),
        }
    }
    if let Some((h, l, at, lines)) = current {
        push_section(&mut out, h, l, at, &lines);
    }
    out
}

fn push_section(
    out: &mut Vec<Section>,
    heading: Option<String>,
    level: u8,
    line: usize,
    lines: &[&str],
) {
    // A preamble of nothing but blank lines is not a section.
    if heading.is_none() && lines.iter().all(|l| l.trim().is_empty()) {
        return;
    }
    out.push(Section {
        heading,
        level,
        line,
        text: lines.join("\n"),
    });
}

/// Every inline code span outside a fence, in document order.
///
/// A span opens at a run of N backticks and closes at the next run of
/// exactly N on the same line; a run that never closes is literal text.
/// Spans do not cross lines here, which loses a CommonMark span wrapped
/// by an editor and gains not having to reason about a backtick that
/// opens in one paragraph and closes three below.
pub fn spans(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    for (n, line, kind) in walk(text) {
        if kind != Kind::Prose {
            continue;
        }
        for s in spans_in_line(line) {
            out.push(Span { line: n, text: s });
        }
    }
    out
}

fn spans_in_line(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    span_bounds(&chars)
        .into_iter()
        .map(|(_, start, end, _)| {
            let inner: String = chars[start..end].iter().collect();
            // One space stripped from each side when both are present,
            // so `` ` `X` ` `` yields `` `X` ``.
            if inner.len() >= 2 && inner.starts_with(' ') && inner.ends_with(' ') {
                inner[1..inner.len() - 1].to_string()
            } else {
                inner
            }
        })
        .collect()
}

/// Every span on one line, as char indices: the first backtick of the
/// opening run, the inner text's start and end, and one past the closing
/// run. One algorithm for `spans` and [`blank_spans`], so the two cannot
/// disagree about where a span is.
fn span_bounds(chars: &[char]) -> Vec<(usize, usize, usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '`' {
            i += 1;
            continue;
        }
        let open_len = chars[i..].iter().take_while(|c| **c == '`').count();
        let start = i + open_len;
        // The closing run: exactly `open_len` backticks.
        let mut j = start;
        let mut close: Option<usize> = None;
        while j < chars.len() {
            if chars[j] == '`' {
                let run = chars[j..].iter().take_while(|c| **c == '`').count();
                if run == open_len {
                    close = Some(j);
                    break;
                }
                j += run;
            } else {
                j += 1;
            }
        }
        match close {
            Some(end) => {
                out.push((i, start, end, end + open_len));
                i = end + open_len;
            }
            None => i = start,
        }
    }
    out
}

/// One line with every inline code span, backticks included, replaced by
/// spaces of the same length. Positions are unchanged, and nothing that
/// was inside a span can match a pattern afterwards. For
/// `imports::parse_imports`, which must skip spans as Claude Code does.
pub fn blank_spans(line: &str) -> String {
    let mut chars: Vec<char> = line.chars().collect();
    for (open, _, _, close) in span_bounds(&chars) {
        chars[open..close].fill(' ');
    }
    chars.into_iter().collect()
}

/// Every fenced block, with its info string and body.
pub fn fences(text: &str) -> Vec<Fence> {
    let mut out = Vec::new();
    let mut current: Option<(usize, String, Vec<&str>)> = None;
    for (n, line, kind) in walk(text) {
        match kind {
            Kind::Open(info) => current = Some((n, info, Vec::new())),
            Kind::Body => {
                if let Some((_, _, body)) = &mut current {
                    body.push(line);
                }
            }
            Kind::Close => {
                if let Some((line, info, body)) = current.take() {
                    out.push(Fence {
                        line,
                        info,
                        body: body.join("\n"),
                    });
                }
            }
            Kind::Prose | Kind::Comment => {}
        }
    }
    // An unclosed fence runs to the end of the file, as CommonMark
    // renders it.
    if let Some((line, info, body)) = current {
        out.push(Fence {
            line,
            info,
            body: body.join("\n"),
        });
    }
    out
}

/// Every line outside a fence, with its 1-based number.
///
/// For a producer that reads prose tokens rather than spans -- `#NNNN`
/// issue numbers -- and must not read them out of a fenced block.
pub fn prose_lines(text: &str) -> Vec<(usize, &str)> {
    walk(text)
        .into_iter()
        .filter(|(_, _, kind)| *kind == Kind::Prose)
        .map(|(n, line, _)| (n, line))
        .collect()
}

/// The headings that name a directory, with their lines.
///
/// Owned by the gaps producer (missing subdirectory CLAUDE.md), which
/// SUPPRESSES a finding on a hit: a parent file with a `## crates`
/// section already has a place for that directory's rules. The
/// placement producer (content in the wrong file) judges whether such a
/// section belongs in the parent, and reads through this same helper
/// rather than matching headings a second way.
///
/// A heading names `dir` when one of its whitespace-separated words,
/// with the backticks and punctuation prose wraps it in stripped and a
/// trailing `/` dropped, is exactly `dir`. `## packages`,
/// ``## The `packages/` directory`` and `## packages:` all name
/// `packages`; `## packaging` and `## packages/octocat-a` do not. `dir`
/// is a path relative to the file's own directory, so
/// `crates/headstate-stepup` matches a heading that writes the whole
/// path and not one that writes `crates` -- naming an ancestor is the
/// caller's separate question.
pub fn sections_naming(text: &str, dir: &str) -> Vec<(String, usize)> {
    let dir = dir.trim_end_matches('/');
    sections(text)
        .into_iter()
        .filter_map(|s| {
            let heading = s.heading?;
            heading
                .split_whitespace()
                .any(|w| strip_word(w) == dir)
                .then_some((heading, s.line))
        })
        .collect()
}

/// The path-shaped words in prose that name a directory or something
/// under it, with their lines.
///
/// The gaps producer's other half: a hit DOWNGRADES rather than
/// suppresses, because a parent file that writes `docs/mobile-*.md` has
/// referred to the directory without giving it a section. Prose lines
/// only, so a path inside a fenced command is not a reference, and
/// `Path::starts_with` for the prefix test, so `docs-old/x` does not
/// name `docs`. A leading `./` is dropped first.
pub fn paths_naming(text: &str, dir: &str) -> Vec<(String, usize)> {
    let dir = Path::new(dir.trim_end_matches('/'));
    let mut out = Vec::new();
    for (n, line) in prose_lines(text) {
        for word in line.split_whitespace() {
            let word = strip_word(word);
            let word = word.strip_prefix("./").unwrap_or(word);
            if !word.is_empty() && Path::new(word).starts_with(dir) {
                out.push((word.to_string(), n));
            }
        }
    }
    out
}

/// A word with the backticks and punctuation prose wraps it in stripped,
/// and a trailing `/` dropped. A leading `.` is kept: `.github` is a
/// name, not punctuation.
fn strip_word(word: &str) -> &str {
    word.trim_start_matches(['`', '*', '_', '(', '"', '\'', '['])
        .trim_end_matches(['`', '*', '_', ')', ',', '.', ':', ';', '"', '\'', ']'])
        .trim_end_matches('/')
}

#[cfg(test)]
mod naming_tests {
    use super::*;

    const ROOT: &str = "\
# Headstate

Anything narrower lives in that directory's own `CLAUDE.md`.
See `docs/mobile-pairing-walkthrough.md` and (`./scripts/release.sh`).

## packages

Shared by every member.

## The `crates/` directory:

```bash
cat docs/README.md
```
";

    /// A heading names a directory by an exact word, whatever wraps it.
    #[test]
    fn a_heading_naming_the_directory_is_found_with_its_line() {
        assert_eq!(
            sections_naming(ROOT, "packages"),
            vec![("packages".to_string(), 6)]
        );
        assert_eq!(
            sections_naming(ROOT, "crates/"),
            vec![("The `crates/` directory:".to_string(), 10)]
        );
    }

    /// `## packages` names `packages`, not `packages/octocat-a`, and not
    /// `packaging`: an ancestor or a prefix is the caller's question.
    #[test]
    fn a_heading_names_only_the_exact_directory() {
        assert!(sections_naming(ROOT, "packages/octocat-a").is_empty());
        assert!(sections_naming(ROOT, "packag").is_empty());
        assert!(sections_naming("## packaging\n", "packages").is_empty());
    }

    /// A path in prose references the directory it starts in, with the
    /// line; one in a fence does not, and a prefix of the name is not a
    /// component match.
    #[test]
    fn a_path_reference_is_found_outside_fences_by_component() {
        assert_eq!(
            paths_naming(ROOT, "docs"),
            vec![("docs/mobile-pairing-walkthrough.md".to_string(), 4)]
        );
        assert_eq!(
            paths_naming(ROOT, "scripts"),
            vec![("scripts/release.sh".to_string(), 4)],
            "a leading ./ and wrapping parentheses are stripped"
        );
        assert!(
            paths_naming("see docs-old/x.md\n", "docs").is_empty(),
            "`docs-old` is not under `docs`"
        );
        assert!(paths_naming("`.github/workflows/ci.yml`\n", ".github/workflows").len() == 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The prose walker skips fence lines and their bodies, and keeps
    /// the numbering of the lines it returns.
    #[test]
    fn prose_lines_skip_fences_and_keep_numbers() {
        let got = prose_lines("a\n```\nb\n```\nc\n");
        assert_eq!(got, vec![(1, "a"), (5, "c")]);
    }

    /// A block-level HTML comment is not injected (changelog 2.1.72), so
    /// no reader here sees it: not as prose, not as a span, not in a
    /// section. An inline comment and one inside a fence are kept, as
    /// Claude Code keeps them.
    #[test]
    fn block_level_html_comments_are_invisible_to_every_reader() {
        let text = "\
# Rules
<!-- maintainer note: NEVER read this -->
kept `span`
<!--
a multi-line note with `hidden span`
-->
inline <!-- x --> stays `inline span`
```
<!-- code comment stays -->
```
";
        assert_eq!(
            prose_lines(text),
            vec![
                (1, "# Rules"),
                (3, "kept `span`"),
                (7, "inline <!-- x --> stays `inline span`")
            ]
        );
        let found: Vec<String> = spans(text).into_iter().map(|s| s.text).collect();
        assert_eq!(found, ["span", "inline span"]);
        let s = sections(text);
        assert_eq!(s.len(), 1);
        assert!(!s[0].text.contains("NEVER"), "{}", s[0].text);
        assert!(!s[0].text.contains("hidden"), "{}", s[0].text);
        assert!(s[0].text.contains("inline <!-- x -->"), "{}", s[0].text);
        // The fence keeps its comment.
        assert_eq!(fences(text)[0].body, "<!-- code comment stays -->");
    }

    /// The stripped text is what the token estimate counts: the comment's
    /// characters are gone, the rest is verbatim, and the trailing newline
    /// survives.
    #[test]
    fn strip_block_html_comments_removes_only_the_comment_lines() {
        let text =
            "a\n<!-- gone -->\nb <!-- kept --> c\n<!--\ngone\n-->\n```\n<!-- kept -->\n```\n";
        assert_eq!(
            strip_block_html_comments(text),
            "a\nb <!-- kept --> c\n```\n<!-- kept -->\n```\n"
        );
        assert_eq!(strip_block_html_comments("no comment"), "no comment");
        assert_eq!(strip_block_html_comments(""), "");
        // A comment left open runs to the end of the file, like an
        // unclosed fence.
        assert_eq!(strip_block_html_comments("a\n<!-- open\nb\n"), "a\n");
    }

    const DOC: &str = "\
intro line
`intro span`

# Title

Prose with `one` and `two::three()` spans.

## Verifying

```bash
# not a heading
make lint
```

    # indented, not a heading either

~~~
```
# inside tildes
```
~~~

### Deep
last
";

    #[test]
    fn splits_at_column_zero_atx_headings_only() {
        let s = sections(DOC);
        let heads: Vec<(Option<&str>, u8, usize)> = s
            .iter()
            .map(|x| (x.heading.as_deref(), x.level, x.line))
            .collect();
        assert_eq!(
            heads,
            vec![
                (None, 0, 1),
                (Some("Title"), 1, 4),
                (Some("Verifying"), 2, 8),
                (Some("Deep"), 3, 23),
            ]
        );
        assert_eq!(s[3].text, "last");
    }

    /// The failure modes the module docs pin: `#` inside a fence is not a
    /// heading, and an indented `#` is not either.
    #[test]
    fn a_hash_inside_a_fence_or_indented_is_not_a_heading() {
        let s = sections(DOC);
        assert!(
            !s.iter()
                .any(|x| x.heading.as_deref() == Some("not a heading")),
            "{s:?}"
        );
        assert!(
            !s.iter()
                .any(|x| x.heading.as_deref() == Some("indented, not a heading either")),
            "{s:?}"
        );
        assert!(
            !s.iter()
                .any(|x| x.heading.as_deref() == Some("inside tildes")),
            "a backtick fence inside a tilde fence does not close it: {s:?}"
        );
        // And the text under `## Verifying` keeps the whole fence.
        assert!(s[2].text.contains("# not a heading"));
    }

    /// YAML frontmatter must not become a heading, which is why setext
    /// is unsupported.
    #[test]
    fn frontmatter_is_not_a_setext_heading() {
        let skill = "---\nname: verify\ndescription: the gate\n---\n\n# Verify\nbody\n";
        let s = sections(skill);
        assert_eq!(s.len(), 2, "{s:?}");
        assert_eq!(s[0].heading, None);
        assert_eq!(s[1].heading.as_deref(), Some("Verify"));
    }

    #[test]
    fn a_hashtag_without_a_space_is_not_a_heading() {
        let s = sections("#1044 is an issue\n# Real\n");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].heading, None);
        assert_eq!(s[1].heading.as_deref(), Some("Real"));
    }

    #[test]
    fn a_file_opening_with_a_heading_has_no_preamble() {
        let s = sections("# Only\ntext\n");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].line, 1);
    }

    #[test]
    fn spans_are_found_outside_fences_only() {
        let got = spans(DOC);
        let texts: Vec<(usize, &str)> = got.iter().map(|s| (s.line, s.text.as_str())).collect();
        assert_eq!(
            texts,
            vec![(2, "intro span"), (6, "one"), (6, "two::three()")]
        );
    }

    /// Blanking keeps every position and removes every span, backticks
    /// and all, whatever the run length.
    #[test]
    fn blank_spans_keeps_positions_and_removes_whole_spans() {
        assert_eq!(blank_spans("a `b` c"), "a     c");
        assert_eq!(blank_spans("``x`y`` z"), "        z");
        assert_eq!(blank_spans("no span"), "no span");
        // An unclosed backtick is literal, and stays.
        assert_eq!(blank_spans("a ` b"), "a ` b");
    }

    #[test]
    fn a_double_backtick_span_holds_a_single_backtick() {
        let got = spans("the `` `X` `` skill and ``a``\n");
        let texts: Vec<&str> = got.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, vec!["`X`", "a"]);
    }

    #[test]
    fn an_unclosed_backtick_is_literal() {
        assert!(spans("a lone ` here\n").is_empty());
    }

    #[test]
    fn fences_carry_their_info_string_and_body() {
        let got = fences(DOC);
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0].line, 10);
        assert_eq!(got[0].info, "bash");
        assert_eq!(got[0].body, "# not a heading\nmake lint");
        assert_eq!(got[1].info, "");
        assert_eq!(got[1].body, "```\n# inside tildes\n```");
    }

    #[test]
    fn an_unclosed_fence_runs_to_the_end() {
        let got = fences("```rust\nfn x() {}\n");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].body, "fn x() {}");
    }

    /// CRLF is normalised: a Windows checkout must parse identically.
    #[test]
    fn crlf_lines_parse_the_same() {
        let unix = sections(DOC);
        let dos = sections(&DOC.replace('\n', "\r\n"));
        assert_eq!(unix, dos);
    }
}

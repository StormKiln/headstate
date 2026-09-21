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
//! # Line numbers are 1-based
//!
//! So a producer can print `path:line` and an editor opens the right
//! line. `Section::line` is the heading's line; the preamble before the
//! first heading is a section at line 1 with no heading.

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
}

/// Every line, numbered and classified.
///
/// A fence opens on a line whose first non-blank characters are three or
/// more backticks or tildes, and closes on a line of the same character
/// at least as long with nothing else on it. Requiring the same character
/// is what `parse_imports`'s toggle does not do, and is the difference
/// between a ```` ``` ```` inside a `~~~` block being body and being a
/// close.
fn walk(text: &str) -> Vec<(usize, &str, Kind)> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let mut out = Vec::new();
    // The open fence's character and length, while inside one.
    let mut open: Option<(char, usize)> = None;

    for (i, line) in text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let n = i + 1;
        let trimmed = line.trim_start();
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
            (None, None) => out.push((n, line, Kind::Prose)),
        }
    }
    out
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
    let mut out = Vec::new();
    let chars: Vec<char> = line.chars().collect();
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
                let inner: String = chars[start..end].iter().collect();
                // One space stripped from each side when both are
                // present, so `` ` `X` ` `` yields `` `X` ``.
                let inner = if inner.len() >= 2 && inner.starts_with(' ') && inner.ends_with(' ') {
                    inner[1..inner.len() - 1].to_string()
                } else {
                    inner
                };
                out.push(inner);
                i = end + open_len;
            }
            None => i = start,
        }
    }
    out
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
            Kind::Prose => {}
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

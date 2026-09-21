/// An ESTIMATE of how many tokens a string costs.
///
/// Not a tokeniser. This is characters ÷ 4, the standard rough figure for
/// English prose, and it is wrong for the code blocks and file paths that
/// CLAUDE.md files are full of -- both tokenise denser than prose.
///
/// # Count what is injected, not what is on disk
///
/// Claude Code strips block-level HTML comments from a CLAUDE.md before
/// injecting it (changelog 2.1.72; memory docs, "Block-level HTML
/// comments … are stripped before the content is injected"). A file
/// whose maintainer notes live in comments costs the model nothing for
/// them, so `claudemd::read_file_reporting` and the import resolver pass
/// this function `text::strip_block_html_comments(&text)`, never the raw
/// file. `ClaudeFile::bytes` stays the on-disk size: that is a different
/// question, and the one the file browser answers. Pinned by
/// `html_comments_do_not_count_toward_the_estimate` in `claudemd/mod.rs`.
///
/// The strip is applied to imported files as well, on the reading that
/// the strip runs over the assembled memory content. The docs state the
/// rule for CLAUDE.md files and say nothing about imports; if an
/// imported file turns out to be injected verbatim, its comments are
/// undercounted here. An estimate either way, and labelled so.
///
/// The alternative was a real BPE tokeniser, which means a new crate and
/// its embedded vocabulary. That is a reasonable trade to make later; the
/// requirement now is that the UI never presents this as a measurement.
/// Everything downstream labels it an estimate, because a number called
/// "tokens" that is actually chars/4 is the kind of confidently-wrong
/// figure this codebase refuses to ship.
pub fn estimate(text: &str) -> u64 {
    // `chars`, not `len`: a multi-byte character is one character to a
    // tokeniser, and byte length would inflate every non-ASCII file.
    (text.chars().count() as u64).div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_roughly_four_characters_per_token() {
        assert_eq!(estimate("12345678"), 2);
        assert_eq!(estimate(""), 0);
    }

    /// Rounds UP, so a short file never reports zero tokens while
    /// holding content.
    #[test]
    fn a_non_empty_file_never_estimates_zero() {
        assert_eq!(estimate("a"), 1);
        assert_eq!(estimate("abc"), 1);
    }

    /// Counted in CHARACTERS, not bytes. Byte length would inflate every
    /// non-ASCII file by up to 4x.
    #[test]
    fn multibyte_characters_count_once() {
        // Four characters, twelve bytes.
        assert_eq!(estimate("日本語だ"), 1);
    }
}

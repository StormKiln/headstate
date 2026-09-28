/// A one-line, plain-text summary of a comment body, for the collapsed
/// title row of a comment nobody has expanded yet.
///
/// The title row is the ONLY thing distinguishing two comments by the same
/// author on the same day, so it has to carry actual content -- but it also
/// has to survive whatever markdown the body opens with.
///
/// `body.slice(0, n)` is what this exists instead of. A comment that opens
/// with a heading previews as "###", one that opens with a fenced block
/// previews as "```", and one that opens with a blank line previews as
/// nothing at all -- three common shapes that all produce a row saying
/// nothing.
export function commentPreview(body: string, max = 80): string {
  // A body with no characters at all has nothing to label: the row shows
  // no preview rather than a placeholder standing in for nothing.
  if (body.trim().length === 0) return "";

  // The first line with real content, not the first line. Bodies routinely
  // open with a blank line, a heading, or a fence, none of which say
  // anything about what the comment IS.
  //
  // HTML comments go first, across lines (#1458): bots open their bodies
  // with a marker like `<!-- ai-review -->`, which renders as nothing and
  // previewed as itself. An unterminated `<!--` hides the rest of the
  // body when rendered, so it hides it here too.
  const line = body
    .replace(/<!--[\s\S]*?(?:-->|$)/g, "")
    .split("\n")
    .map((l) => stripMarkdown(stripTags(l)))
    .find((l) => l.length > 0);

  // The body HAD content, but none of it is text a reader would see -- a
  // marker, an empty fence, a bare tag. Say so, rather than render a title
  // row that looks like the preview failed to load.
  if (line === undefined) return NO_TEXT;
  // Ellipsis only when something was actually cut. Appending it
  // unconditionally implies every preview is truncated, which makes a
  // short complete comment look like it continues.
  return line.length > max ? `${line.slice(0, max).trimEnd()}…` : line;
}

/// The title of a comment whose body has content but no visible text.
const NO_TEXT = "(no text)";

/// HTML elements GitHub renders in a comment body. A tag is stripped only
/// when its name is one of these, so `<summary>CI report</summary>` reads
/// "CI report" while `Option<T>` and `Vec<String>` -- prose about code,
/// not markup -- survive intact.
const HTML_TAGS = new Set([
  "a", "b", "blockquote", "br", "code", "dd", "del", "details", "div", "dl",
  "dt", "em", "h1", "h2", "h3", "h4", "h5", "h6", "hr", "i", "img", "ins",
  "kbd", "li", "ol", "p", "picture", "pre", "q", "s", "samp", "source", "span",
  "strike", "strong", "sub", "summary", "sup", "table", "tbody", "td", "tfoot",
  "th", "thead", "tr", "tt", "ul", "var",
]);

/// Tags that separate words when rendered. Every other tag is inline and
/// joins its neighbours: `<b>CI</b>: failed` reads "CI: failed", not
/// "CI : failed".
const BREAKING_TAGS = new Set(["br", "div", "hr", "li", "p", "td", "th", "tr"]);

/// Removes HTML tags, keeping the text between them.
function stripTags(line: string): string {
  return line.replace(/<\/?([A-Za-z][A-Za-z0-9]*)\b[^>]*>/g, (tag, name: string) => {
    const lower = name.toLowerCase();
    if (!HTML_TAGS.has(lower)) return tag;
    return BREAKING_TAGS.has(lower) ? " " : "";
  });
}

/// Renders inline markdown as the text a person reads.
///
/// The preview sits in a title row as PLAIN text -- it is deliberately not
/// passed through the markdown renderer, because a `**bold**` fragment or a
/// half-open link in a truncated line would inject formatting into a row
/// that has to stay one line tall.
function stripMarkdown(line: string): string {
  return (
    line
      // Fences and block quotes carry no content of their own.
      .replace(/^\s*(?:```+|~~~+).*$/, "")
      .replace(/^\s*>+\s?/, "")
      // Leading markers: heading hashes, list bullets, numbered items,
      // and task boxes. The TEXT after them is the useful part.
      .replace(/^\s*#{1,6}\s+/, "")
      .replace(/^\s*[-*+]\s+(?:\[[ xX]\]\s+)?/, "")
      .replace(/^\s*\d+[.)]\s+/, "")
      // Images before links: an image is `![alt](url)` and the link rule
      // would otherwise leave a stray `!` behind.
      .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
      .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
      // Emphasis and code spans, keeping the text inside.
      .replace(/(\*\*\*|___)(.+?)\1/g, "$2")
      .replace(/(\*\*|__)(.+?)\1/g, "$2")
      .replace(/(\*|_)(.+?)\1/g, "$2")
      .replace(/`+([^`]+)`+/g, "$1")
      // Collapse runs of whitespace so a line padded for alignment in the
      // source does not render as a gap in the title row.
      .replace(/\s+/g, " ")
      .trim()
  );
}

import type { PrComment } from "@/types/pr";

/// Folding repeated comments under the newest of their kind (#1581).
///
/// A long-running pull request collects one coverage report, one AI
/// review, one preview-deploy note per CI round. Each is a new comment,
/// and each supersedes the last; dozens of them bury the comments a
/// reviewer opened the page to read.
///
/// # The rule, and why it is this conservative
///
/// Two comments are the SAME KIND when they come from the same account
/// and carry the same signature:
///
/// - **From a bot** (GitHub says the author is a `Bot`): the signature is
///   the first hidden HTML comment in the body (`<!-- coverage-report -->`,
///   which many bots embed to find their own comment again), or, failing
///   that, the first non-empty line -- usually the heading. Either is
///   NORMALISED: URLs, dates, times, commit hashes and numbers are masked,
///   because those are exactly what change between rounds ("Coverage
///   81.2%" and "Coverage 81.4%" are one kind). The body as a whole is
///   never compared: review text and tables change every round.
/// - **From anyone else**: only a hidden HTML marker counts, and it must
///   match EXACTLY, unmasked. A tool posting under a person's token
///   embeds one; a person typing does not. A human comment without one
///   is never folded, whatever it says -- "LGTM" twice is two decisions.
///
/// Masking can only merge comments that were already from the same
/// account with the same heading shape, so the worst case is folding two
/// of one bot's reports that differed only in their numbers, which is the
/// case this exists for. It never folds across accounts.
///
/// # What is shown
///
/// The NEWEST of each kind stays in its own chronological place; the
/// older copies leave the list and sit under it, collapsed. Everything
/// else keeps its order. The input is assumed oldest-first, which is what
/// the query returns (`last:` pages keep chronological order).
///
/// # Partial is not nothing
///
/// Folding runs on whatever the fetch returned. When that was not every
/// comment, the older copies it found are a floor -- more of them may sit
/// beyond the fetch -- and `truncated` makes the label say "at least".

/// One entry in the rendered list: a comment, and the older copies of the
/// same kind that it supersedes, oldest first. `superseded` is empty for
/// a comment with no repeats.
export interface CommentEntry {
  comment: PrComment;
  superseded: PrComment[];
}

/// `\r\n` first: every pattern below that looks for a line end is
/// `\n`-anchored, and a Windows-authored body would otherwise keep its
/// `\r` in the signature and never match its own next round.
function unixLines(body: string): string {
  return body.replace(/\r\n?/g, "\n");
}

/// The first hidden HTML comment's text, whitespace-collapsed, or null.
function marker(body: string): string | null {
  const m = /<!--([\s\S]*?)-->/.exec(body);
  if (m === null) return null;
  const text = m[1].replace(/\s+/g, " ").trim();
  return text === "" ? null : text;
}

/// The first line with any text in it, trimmed, or null.
function firstLine(body: string): string | null {
  for (const line of body.split("\n")) {
    const t = line.trim();
    if (t !== "") return t;
  }
  return null;
}

/// Masks what changes between rounds of the same bot comment. Order
/// matters: a URL contains digits and hashes, and a date contains
/// numbers, so the wider patterns go first.
export function normaliseSignature(text: string): string {
  return (
    text
      // URLs, bare or inside markdown links and images.
      .replace(/https?:\/\/[^\s)\]>"']+/g, "<url>")
      // ISO dates and times, with or without a time zone.
      .replace(/\d{4}-\d{2}-\d{2}(?:[T ]\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:Z|[+-]\d{2}:?\d{2})?)?/g, "<date>")
      .replace(/\b\d{1,2}:\d{2}(?::\d{2})?\b/g, "<time>")
      // Commit hashes: 7 to 40 hex characters with at least one digit
      // and one letter, so an ordinary word like "added" or a plain
      // number is not taken for one.
      .replace(/\b(?=[0-9a-f]*\d)(?=[0-9a-f]*[a-f])[0-9a-f]{7,40}\b/gi, "<hash>")
      // Numbers, with signs, decimals, separators and percentages.
      .replace(/[+-]?\d[\d,]*(?:\.\d+)?%?/g, "<n>")
      .replace(/\s+/g, " ")
      .trim()
  );
}

/// The kind key for one comment, or null when it is never folded.
///
/// Exported for the tests, which pin the rule rather than only its
/// effect on a list.
export function commentKind(c: PrComment): string | null {
  const body = unixLines(c.body);
  const hidden = marker(body);
  if (!c.author_is_bot) {
    // Not a bot, or GitHub did not say: an exact marker or nothing.
    return hidden === null ? null : `h\u0000${c.author}\u0000${hidden}`;
  }
  const sig = hidden ?? firstLine(body);
  if (sig === null) return null;
  const norm = normaliseSignature(sig);
  return norm === "" ? null : `b\u0000${c.author}\u0000${hidden === null ? "line" : "marker"}\u0000${norm}`;
}

/// Folds each kind's older copies under its newest, keeping order.
export function foldSuperseded(comments: readonly PrComment[]): CommentEntry[] {
  const kinds = comments.map(commentKind);
  // The index of the newest comment of each kind.
  const newest = new Map<string, number>();
  kinds.forEach((k, i) => {
    if (k !== null) newest.set(k, i);
  });
  const older = new Map<string, PrComment[]>();
  const out: CommentEntry[] = [];
  comments.forEach((c, i) => {
    const k = kinds[i];
    if (k === null) {
      out.push({ comment: c, superseded: [] });
      return;
    }
    if (newest.get(k) !== i) {
      const list = older.get(k) ?? [];
      list.push(c);
      older.set(k, list);
      return;
    }
    // Every older copy precedes the newest, so its list is complete here.
    out.push({ comment: c, superseded: older.get(k) ?? [] });
  });
  return out;
}

/// The control's label. `truncated` when the fetch did not return every
/// comment: the older copies it found are then a floor.
export function supersededLabel(count: number, truncated: boolean): string {
  return truncated ? `Superseded (at least ${count} older)` : `Superseded (${count} older)`;
}

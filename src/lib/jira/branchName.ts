/// The branch "Start work" proposes for a ticket (#2045).
///
/// A proposal only: the dialog shows it in an editable field, and the
/// desktop -- not this -- decides whether git accepts the name and whether
/// detection (#2035) will find the key in it.

export const DEFAULT_BRANCH_TEMPLATE = "{key}-{summary}";

/// How many characters of the summary go into the name. Long enough to say
/// what the work is, short enough to stay readable in a branch list.
const SUMMARY_MAX = 48;

/// The summary as a branch-safe slug: lower case, ASCII letters and digits,
/// words joined by `-`, cut at a word boundary.
export function slugify(summary: string, max = SUMMARY_MAX): string {
  const words = summary
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter(Boolean);
  let out = "";
  for (const w of words) {
    const next = out ? `${out}-${w}` : w;
    if (next.length > max) {
      // A single word longer than the limit is cut rather than dropped.
      if (!out) out = w.slice(0, max);
      break;
    }
    out = next;
  }
  return out;
}

/// The template filled in. `{key}` is the key in lower case, the branch
/// convention detection reads in any case; `{KEY}` keeps Jira's case.
/// Separators left dangling by an empty summary are trimmed.
export function branchName(template: string, key: string, summary: string): string {
  const t = template.trim() || DEFAULT_BRANCH_TEMPLATE;
  const filled = t
    .replaceAll("{KEY}", key)
    .replaceAll("{key}", key.toLowerCase())
    .replaceAll("{summary}", slugify(summary));
  return filled
    .replace(/[-_.]{2,}/g, (m) => m[0])
    .replace(/[-_./]+$/, "")
    .replace(/^[-_.]+/, "")
    .replace(/\/[-_.]+/g, "/");
}

/// Whether a template can name a branch that links to its ticket.
export function templateHasKey(template: string): boolean {
  const t = template.trim() || DEFAULT_BRANCH_TEMPLATE;
  return t.includes("{key}") || t.includes("{KEY}");
}

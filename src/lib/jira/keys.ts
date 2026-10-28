/// Finding Jira keys in what Headstate already holds (#2035, epic #2031).
///
/// The text is local -- a pull request's title and head branch, a
/// worktree's or local branch's name -- so detection costs no request.
/// Jira supplies only the dictionary: a candidate counts as a key only when
/// its prefix is one of the user's real projects. That is what rejects the
/// look-alikes (`UTF-8`, `SHA-256`, `GPT-4`) without asking anyone to
/// configure anything.
///
/// This is the only Jira-key pattern in the frontend. Everything that needs
/// a key -- the ticket chip, the jump box, the Jira page -- reads it from
/// here rather than re-parsing.

export type KeySource = "title" | "branch";

export interface FoundKey {
  key: string;
  /// Where it was found, shown so a surprising match can be explained.
  source: KeySource;
}

/// Upper case only: a title is prose, and "see pr-2" is not a ticket.
/// Underscores belong to the key here (`A_B-3` is a legal project key).
const TITLE = /(?<![A-Za-z0-9_])([A-Z][A-Z0-9_]{1,9})-([1-9][0-9]{0,8})(?![A-Za-z0-9])/g;

/// Any case, because branches are conventionally lower case
/// (`feature/abc-1-fix-login`). `_` is a separator in a branch name
/// (`bugfix_abc-1`), so it may not appear inside the key -- otherwise the
/// pattern would read `FIX_ABC-1` out of `bugfix_abc-1`.
const BRANCH = /(?<![A-Za-z0-9])([A-Za-z][A-Za-z0-9]{1,9})-([1-9][0-9]{0,8})(?![A-Za-z0-9])/g;

/// The keys in `text` whose project is in `projects`, in order, once each.
export function keysIn(text: string, source: KeySource, projects: ReadonlySet<string>): FoundKey[] {
  const found: FoundKey[] = [];
  for (const m of text.matchAll(source === "title" ? TITLE : BRANCH)) {
    const project = m[1].toUpperCase();
    if (!projects.has(project)) continue;
    const key = `${project}-${m[2]}`;
    if (!found.some((f) => f.key === key)) found.push({ key, source });
  }
  return found;
}

/// A pull request's keys: the title's first, then any only the branch has.
export function keysForPr(
  pr: { title: string; head_ref?: string | null },
  projects: ReadonlySet<string>,
): FoundKey[] {
  const found = keysIn(pr.title, "title", projects);
  for (const k of pr.head_ref ? keysIn(pr.head_ref, "branch", projects) : []) {
    if (!found.some((f) => f.key === k.key)) found.push(k);
  }
  return found;
}

/// The keys in a branch name -- a worktree's or a local branch's.
export function keysForBranch(branch: string | null | undefined, projects: ReadonlySet<string>): FoundKey[] {
  return branch ? keysIn(branch, "branch", projects) : [];
}

/// What a person typed into a jump box -- `abc-1`, ` ABC-1 `, or a browse
/// URL ending in one -- as a key, or null. No project check: an unknown
/// prefix is reported as such by the caller, not silently dropped.
export function parseTypedKey(input: string): string | null {
  const m = /(?:^|\/browse\/)\s*([A-Za-z][A-Za-z0-9_]{1,9})-([1-9][0-9]{0,8})\s*(?:[?#].*)?$/.exec(input.trim());
  return m ? `${m[1].toUpperCase()}-${m[2]}` : null;
}

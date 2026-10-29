/// "No ticket" -- the user's open pull requests that name no Jira key
/// (#2046), and the title that would name one.
///
/// Whether a pull request names a key is the desktop's call
/// (`src-tauri/src/jira/keys.rs`); this never parses a title for keys.
/// What it adds is the difference between "the index looked at this pull
/// request, as it is now, and found no key" and "the index has not looked
/// at it yet" -- a new pull request, or one renamed since the last read.
/// Only the first is "no ticket". The second is pending, and renders as
/// nothing rather than as a claim.

import type { LinkIndex } from "@/types/jira";

/// What the index knows about one pull request's tickets.
/// - `has`: looked at as it is now, and it names a key.
/// - `none`: looked at as it is now, and it names none.
/// - `pending`: not looked at as it is now. Never shown as "no ticket".
export type TicketState = "has" | "none" | "pending";

export interface NoTicketIndex {
  /// Each authored pull request the index read, by URL, as it read it.
  checked: ReadonlyMap<string, { title: string; head_ref: string }>;
  /// Authored pull requests with a key in the latest read.
  ticketed: ReadonlySet<string>;
}

/// The part of a pull request or merge request this needs.
export interface TicketRow {
  url: string;
  title: string;
  head_ref: string;
}

export function noTicketIndex(index: Pick<LinkIndex, "links" | "checked">): NoTicketIndex {
  const checked = new Map<string, { title: string; head_ref: string }>();
  for (const c of index.checked ?? []) checked.set(c.url, { title: c.title, head_ref: c.head_ref });
  const ticketed = new Set<string>();
  for (const l of index.links) {
    // A link that is not current is from an earlier read of the pull
    // request -- a key since removed from its title, say.
    if (l.current && l.target.kind === "pr" && l.target.role === "authored") ticketed.add(l.target.url);
  }
  return { checked, ticketed };
}

export function ticketState(ix: NoTicketIndex, pr: TicketRow): TicketState {
  const seen = ix.checked.get(pr.url);
  // Read under another title or branch: what it found is about a pull
  // request that is no longer the one on screen.
  if (!seen || seen.title !== pr.title || seen.head_ref !== pr.head_ref) return "pending";
  return ix.ticketed.has(pr.url) ? "has" : "none";
}

/// The pull requests known to name no ticket, and how many could not be
/// decided yet -- so the filter can say "and N not checked yet" rather than
/// present a partial list as the whole.
export function filterNoTicket<T extends TicketRow>(prs: readonly T[], ix: NoTicketIndex): { shown: T[]; pending: number } {
  const shown: T[] = [];
  let pending = 0;
  for (const pr of prs) {
    const s = ticketState(ix, pr);
    if (s === "none") shown.push(pr);
    else if (s === "pending") pending += 1;
  }
  return { shown, pending };
}

/// A GitLab draft marker. GitLab reads it only at the START of a title, so
/// a key goes after it: `ABC-1: Draft: x` would mark the MR ready.
const DRAFT_PREFIX = /^\s*(?:draft:|\[draft\]|\(draft\))\s*/i;

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/// `title` with `key` in front -- `ABC-1: fix login` -- or `null` when the
/// title already names `key`, so linking twice changes nothing. Matched as
/// the desktop matches a title key: upper case, as a whole token.
export function titleWithKey(title: string, key: string): string | null {
  if (new RegExp(`(?:^|[^A-Za-z0-9_])${escape(key)}(?:$|[^A-Za-z0-9])`).test(title)) return null;
  const draft = DRAFT_PREFIX.exec(title);
  if (draft) return `${draft[0].trim()} ${key}: ${title.slice(draft[0].length)}`;
  return `${key}: ${title.trimStart()}`;
}

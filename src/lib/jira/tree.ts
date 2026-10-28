/// The My Jira issues tree, as pure functions (#2037, #2038).
///
/// Kept apart from the components so the grouping rules -- which are where
/// "could not see" can quietly turn into "has none" -- are tested directly.

import type { IssueState } from "@/api/jira";
import type { PrIdentity } from "@/types/identity";
import type { JiraIssue, JiraStatusCategory } from "@/types/jira";
import { keysForBranch, keysForPr, type KeySource } from "./keys";

export interface LinkedPr {
  identity: PrIdentity;
  title: string;
  url: string;
  is_draft: boolean;
  source: KeySource;
}

export interface LinkedWorktree {
  repo: string;
  path: string;
  branch: string;
}

export interface Links {
  prs: LinkedPr[];
  worktrees: LinkedWorktree[];
}

type PrLike = PrIdentity & { title: string; url: string; is_draft: boolean; head_ref: string };

/// Every key named by these pull requests and worktrees, with what names it.
export function linkIndex(
  prs: readonly PrLike[],
  worktrees: readonly LinkedWorktree[],
  projects: ReadonlySet<string>,
): Map<string, Links> {
  const index = new Map<string, Links>();
  const at = (key: string) => {
    let links = index.get(key);
    if (!links) index.set(key, (links = { prs: [], worktrees: [] }));
    return links;
  };
  for (const pr of prs) {
    for (const f of keysForPr(pr, projects)) {
      at(f.key).prs.push({
        identity: { source: pr.source, repo: pr.repo, number: pr.number },
        title: pr.title,
        url: pr.url,
        is_draft: pr.is_draft,
        source: f.source,
      });
    }
  }
  for (const wt of worktrees) {
    for (const f of keysForBranch(wt.branch, projects)) at(f.key).worktrees.push(wt);
  }
  return index;
}

/// A lookup into the per-key cache. `undefined` is "not answered yet".
export type Lookup = (key: string) => IssueState | "failed" | undefined;

/// Deeper than any Jira hierarchy in practice; the bound is a cycle guard.
const MAX_DEPTH = 5;

const found = (s: ReturnType<Lookup>): JiraIssue | null =>
  s !== undefined && s !== "failed" && s.state === "found" ? s.issue : null;

/// Every ancestor of `keys` the cache can reach: each answered parent, and
/// the first unanswered one on each chain. Asking for these, render after
/// render, walks the hierarchy up one level per answer.
export function ancestorKeys(keys: readonly string[], lookup: Lookup): string[] {
  const out = new Set<string>();
  for (const key of keys) {
    let cur = found(lookup(key));
    const seen = new Set([key]);
    for (let d = 0; cur?.parent_key && d < MAX_DEPTH; d++) {
      const p = cur.parent_key;
      if (seen.has(p)) break;
      seen.add(p);
      out.add(p);
      cur = found(lookup(p));
    }
  }
  return [...out];
}

/// Group ids that are not issue keys.
export const NO_EPIC = "no-epic";
/// Issues whose own answer has not arrived, or did not: their parent is
/// unknown, so they cannot honestly be filed under "No epic".
export const UNRESOLVED = "unresolved";

/// The epic an issue belongs under: the nearest ancestor that is an epic
/// (hierarchy level 1+), the issue itself when it IS one, `NO_EPIC` when
/// it has no parent, or `UNRESOLVED` when its own answer is missing.
///
/// A parent we could not read still names the group -- its header says
/// "cannot view" -- rather than dropping the child into `NO_EPIC`. "Could
/// not see the parent" is not "has no parent".
export function epicOf(key: string, lookup: Lookup): string {
  const start = found(lookup(key));
  if (!start) return UNRESOLVED;
  if ((start.issue_type.hierarchy_level ?? 0) >= 1) return start.key;
  let cur = start;
  for (let d = 0; d < MAX_DEPTH; d++) {
    const p = cur.parent_key;
    if (!p) return cur === start ? NO_EPIC : cur.key;
    const parent = found(lookup(p));
    if (!parent) return p;
    const level = parent.issue_type.hierarchy_level;
    // An unknown level is taken as the epic: on a site that does not
    // report levels, the immediate parent is the grouping people expect.
    if (level === null || level >= 1) return parent.key;
    cur = parent;
  }
  return cur.key;
}

/// The initiative above an epic, when the site has one and we could read it.
export function initiativeOf(epic: string, lookup: Lookup): string | null {
  if (epic === NO_EPIC || epic === UNRESOLVED) return null;
  const e = found(lookup(epic));
  if (!e?.parent_key) return null;
  const s = lookup(e.parent_key);
  const p = found(s);
  // Unread: still named, as for an epic. Read and not above an epic: not
  // an initiative, so the epic stands alone.
  if (!p) return e.parent_key;
  return (p.issue_type.hierarchy_level ?? 2) >= 2 ? p.key : null;
}

export interface EpicGroup {
  epic: string;
  issues: string[];
}

export interface InitiativeGroup {
  /// `null`: epics with no initiative above them.
  initiative: string | null;
  epics: EpicGroup[];
}

/// `keys` grouped initiative → epic → issue. Order follows `keys`, with the
/// special groups last. An epic that is itself in `keys` heads its own
/// group and is not listed again inside it.
export function buildTree(keys: readonly string[], lookup: Lookup): InitiativeGroup[] {
  const epics = new Map<string, string[]>();
  for (const key of keys) {
    const epic = epicOf(key, lookup);
    const list = epics.get(epic) ?? [];
    if (epic !== key) list.push(key);
    epics.set(epic, list);
  }
  const rank = (e: string) => (e === UNRESOLVED ? 2 : e === NO_EPIC ? 1 : 0);
  const ordered = [...epics.entries()].sort((a, b) => rank(a[0]) - rank(b[0]));
  const groups = new Map<string | null, EpicGroup[]>();
  for (const [epic, issues] of ordered) {
    const initiative = initiativeOf(epic, lookup);
    const list = groups.get(initiative) ?? [];
    list.push({ epic, issues });
    groups.set(initiative, list);
  }
  return [...groups.entries()]
    .sort((a, b) => (a[0] === null ? 1 : 0) - (b[0] === null ? 1 : 0))
    .map(([initiative, epics]) => ({ initiative, epics }));
}

/// Children of an epic by status category, counting only answers we have.
/// `unknown` covers both an unreported category and an unanswered child, so
/// the counts always sum to `issues.length` and none is invented.
export function rollUp(issues: readonly string[], lookup: Lookup): Record<JiraStatusCategory, number> {
  const counts: Record<JiraStatusCategory, number> = { new: 0, in_progress: 0, done: 0, unknown: 0 };
  for (const key of issues) counts[found(lookup(key))?.status.category ?? "unknown"]++;
  return counts;
}

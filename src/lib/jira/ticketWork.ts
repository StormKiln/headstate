/// What Go to ticket lists for one key (#2040), as pure functions.
///
/// Two inputs: the link index Headstate already holds (instant), and the
/// desktop's lookup (`jira_find`), which adds pull requests from review
/// lists and from searching, local branches, and says what it searched.
/// The words for that search live here too, so "nothing found" is worded
/// once and tested: never wider than what was asked (#1050).

import type { PrIdentity } from "@/types/identity";
import type { JiraLink, KeyLookup, KeySearch, PrState } from "@/types/jira";
import type { Links } from "./tree";

export type WorkState = "open" | "draft" | "merged" | "closed" | "no longer open";

export interface WorkPr {
  kind: "pr";
  title: string;
  /// `repo#7` on GitHub, `group/app!7` on GitLab.
  ref: string;
  gitlab: boolean;
  url: string;
  state: WorkState;
  branch: string;
  /// Set when Headstate can open it itself: it is in one of the user's
  /// lists. A pull request found only by searching opens on its host.
  identity: PrIdentity | null;
}

export interface WorkWorktree {
  kind: "worktree";
  repo: string;
  branch: string;
  path: string;
}

export interface WorkBranch {
  kind: "branch";
  repoPath: string;
  branch: string;
}

export type WorkRow = WorkPr | WorkWorktree | WorkBranch;

const KNOWN: Record<PrState, WorkState> = { open: "open", merged: "merged", closed: "closed" };

function prState(state: PrState | null, current: boolean, draft: boolean): WorkState {
  // A searched pull request carries its state; a listed one is open while
  // it is in the list, and "no longer open" -- merged or closed, unknown
  // which -- once it has left.
  const s = state ? KNOWN[state] : current ? "open" : "no longer open";
  return s === "open" && draft ? "draft" : s;
}

/// Every pull request, worktree and branch that names the key: the index
/// first, then whatever the lookup adds, each listed once.
export function ticketWork(local: Links | undefined, found: KeyLookup | undefined): WorkRow[] {
  const rows: WorkRow[] = [];
  const seen = new Set<string>();
  for (const pr of local?.prs ?? []) {
    seen.add(pr.url);
    const gitlab = pr.identity.source?.provider === "gitlab";
    rows.push({
      kind: "pr",
      title: pr.title,
      ref: `${pr.identity.repo}${gitlab ? "!" : "#"}${pr.identity.number}`,
      gitlab,
      url: pr.url,
      state: prState(null, pr.current, pr.is_draft),
      branch: pr.branch,
      identity: pr.identity,
    });
  }
  for (const l of found?.links ?? []) rows.push(...fromLink(l, seen));
  for (const w of local?.worktrees ?? []) {
    if (seen.has(`wt\n${w.path}`)) continue;
    seen.add(`wt\n${w.path}`);
    rows.push({ kind: "worktree", repo: w.repo, branch: w.branch, path: w.path });
  }
  // Pull requests first, then worktrees, then branches: what can be opened
  // before what can only be read. Stable within each.
  const order = { pr: 0, worktree: 1, branch: 2 } as const;
  return rows.map((r, i) => [r, i] as const).sort((a, b) => order[a[0].kind] - order[b[0].kind] || a[1] - b[1]).map(([r]) => r);
}

function fromLink(l: JiraLink, seen: Set<string>): WorkRow[] {
  const t = l.target;
  if (t.kind === "pr") {
    if (seen.has(t.url)) return [];
    seen.add(t.url);
    const gitlab = t.source.provider === "gitlab";
    return [
      {
        kind: "pr",
        title: t.title,
        ref: `${t.repo}${gitlab ? "!" : "#"}${t.number}`,
        gitlab,
        url: t.url,
        state: prState(t.state, l.current, t.is_draft),
        branch: t.head_ref,
        identity: t.role === "searched" ? null : { source: t.source, repo: t.repo, number: t.number },
      },
    ];
  }
  if (t.kind === "worktree") {
    if (seen.has(`wt\n${t.path}`)) return [];
    seen.add(`wt\n${t.path}`);
    return [{ kind: "worktree", repo: t.repo, branch: t.branch, path: t.path }];
  }
  const id = `br\n${t.repo_path}\n${t.branch}`;
  if (seen.has(id)) return [];
  seen.add(id);
  return [{ kind: "branch", repoPath: t.repo_path, branch: t.branch }];
}

/// "a, b and c", or "a, b, c and 4 more".
export function listed(items: readonly string[], max = 3): string {
  if (items.length <= 1) return items[0] ?? "";
  if (items.length > max) return `${items.slice(0, max).join(", ")} and ${items.length - max} more`;
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

export type Provider = "GitHub" | "GitLab";

/// One provider's search, in words. `shown` is how many pull requests the
/// search added to what the user already had; `mine` how many they had.
///
/// - not asked says why, and never reads as "none" (#1050);
/// - "nothing found" names exactly the owners or projects searched, and
///   says how many more of the user's were not;
/// - more matched than read is "N matched; R read", or "at least N" when
///   the provider did not count.
export function searchWords(
  provider: Provider,
  s: KeySearch,
  key: string,
  { shown, mine, time }: { shown: number; mine: number; time: (iso: string) => string },
): string {
  if (s.state === "failed") return `Searching ${provider} failed: ${s.error}`;
  if (s.state === "not_asked") return `${provider} not searched: ${s.reason}.`;
  const what = provider === "GitHub" ? "owner" : "project";
  const where = `${provider === "GitHub" ? "repositories owned by " : ""}${listed(s.scope)}`;
  const skipped = s.unsearched.length
    ? ` ${s.unsearched.length} more ${what}${s.unsearched.length === 1 ? "" : "s"} of yours not searched.`
    : "";
  const when = time(s.at);
  if (s.truncated) {
    const count = s.total > s.read ? `${s.total} matched and only the first ${s.read} were read` : `at least ${s.total} matched`;
    return `Searched ${provider} ${when} in ${where}: ${count}.${skipped}`;
  }
  if (shown === 0) {
    const noun = provider === "GitHub" ? "pull requests" : "merge requests";
    const none = mine === 0 ? `No ${noun} name ${key}` : `Nothing else names ${key}`;
    return `${none} in ${where} on ${provider} (searched ${when}).${skipped}`;
  }
  return `Searched ${provider} ${when} in ${where}.${skipped}`;
}

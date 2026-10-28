/// The desktop's link index, shaped for the screens that read it (#2035).
///
/// The desktop finds the keys (`src-tauri/src/jira/keys.rs`) and keeps
/// them; this only files each link two ways -- by the row that names it,
/// for chips, and by key, for My Jira issues.

import type { JiraLink } from "@/types/jira";
import type { FoundKey } from "./keys";
import type { Links } from "./tree";

/// The row a chip sits on.
export type RowRef = { prUrl: string } | { worktreePath: string } | { repoPath: string; branch: string };

export const rowId = (r: RowRef): string =>
  "prUrl" in r ? `pr\n${r.prUrl}` : "worktreePath" in r ? `wt\n${r.worktreePath}` : `br\n${r.repoPath}\n${r.branch}`;

export interface LinkMaps {
  /// Keys each row names, title keys first.
  byRow: ReadonlyMap<string, FoundKey[]>;
  /// What names each key: the user's own pull requests and worktrees.
  byKey: ReadonlyMap<string, Links>;
}

export function linkMaps(links: readonly JiraLink[]): LinkMaps {
  const byRow = new Map<string, FoundKey[]>();
  const byKey = new Map<string, Links>();
  const at = (key: string) => {
    let l = byKey.get(key);
    if (!l) byKey.set(key, (l = { prs: [], worktrees: [] }));
    return l;
  };
  for (const link of links) {
    const t = link.target;
    const row = rowId(
      t.kind === "pr" ? { prUrl: t.url } : t.kind === "worktree" ? { worktreePath: t.path } : { repoPath: t.repo_path, branch: t.branch },
    );
    const found = byRow.get(row) ?? [];
    if (!found.some((f) => f.key === link.key)) found.push({ key: link.key, source: link.found_in });
    byRow.set(row, found);

    // My Jira issues lists the user's own work: their pull requests and
    // worktrees. Review requests still get chips, above.
    if (t.kind === "pr" && t.role === "authored") {
      at(link.key).prs.push({
        identity: { source: t.source, repo: t.repo, number: t.number },
        title: t.title,
        url: t.url,
        is_draft: t.is_draft,
        source: link.found_in,
        current: link.current,
      });
    } else if (t.kind === "worktree") {
      at(link.key).worktrees.push({ repo: t.repo, path: t.path, branch: t.branch });
    }
  }
  for (const found of byRow.values()) found.sort((a, b) => (a.source === b.source ? 0 : a.source === "title" ? -1 : 1));
  return { byRow, byKey };
}

import { useMemo } from "react";
import { useQueries, useQueryClient } from "@tanstack/react-query";
import { issueQuery, useJiraAssigned, type IssueState } from "@/api/jira";
import { useWorktrees } from "@/api/hooks";
import { ancestorKeys, linkIndex, type LinkedWorktree, type Lookup } from "@/lib/jira/tree";
import type { PrIdentity } from "@/types/identity";
import type { JiraIssue } from "@/types/jira";
import { useJira } from "./jiraState";

export type BoardPr = PrIdentity & { title: string; url: string; is_draft: boolean; head_ref: string };

const EMPTY: never[] = [];

/// Everything My Jira issues renders, for its sidebar and its main panel
/// alike (#2038). Both call this; the per-key cache makes the second call
/// free.
export function useJiraBoard(prs: readonly BoardPr[]) {
  const jira = useJira();
  const on = jira !== null;
  const worktrees = useWorktrees(on);
  const assigned = useJiraAssigned(on);

  const wts = useMemo<LinkedWorktree[]>(
    () =>
      (worktrees.data ?? []).flatMap((r) =>
        r.worktrees.filter((w) => w.branch).map((w) => ({ repo: r.name, path: w.path, branch: w.branch })),
      ),
    [worktrees.data],
  );
  const links = useMemo(() => (jira ? linkIndex(prs, wts, jira.projects) : new Map()), [jira, prs, wts]);
  const linked = useMemo(() => [...links.keys()], [links]);
  const assignedIssues = assigned.data?.issues ?? EMPTY;
  const assignedKeys = useMemo(() => assignedIssues.map((i: JiraIssue) => i.key), [assignedIssues]);
  const assignedMap = useMemo(() => new Map(assignedIssues.map((i: JiraIssue) => [i.key, i])), [assignedIssues]);

  // Parents are found by asking: a story's epic is only known once the
  // story has answered. Derived from the cache on every render rather than
  // held in state, so each answer that names a new parent adds it to the
  // next render's queries, one level at a time.
  const qc = useQueryClient();
  const fromAssigned = (key: string): IssueState | undefined => {
    const a = assignedMap.get(key);
    return a ? { state: "found", issue: a } : undefined;
  };
  const cached: Lookup = (key) => {
    const st = qc.getQueryState<IssueState>(["jira-issue", key]);
    if (st?.status === "error") return "failed";
    return st?.data ?? fromAssigned(key);
  };
  const ancestors = on ? ancestorKeys([...linked, ...assignedKeys], cached) : EMPTY;
  const queried = [...new Set([...linked, ...ancestors])];
  const results = useQueries({ queries: queried.map((k) => ({ ...issueQuery(k), enabled: on })) });

  const lookup: Lookup = (key) => {
    const i = queried.indexOf(key);
    const r = i >= 0 ? results[i] : undefined;
    if (r?.isError) return "failed";
    return r?.data ?? fromAssigned(key);
  };

  return {
    jira,
    links,
    linked,
    assigned,
    assignedKeys,
    worktreesLoading: worktrees.isPending && on,
    lookup,
  };
}

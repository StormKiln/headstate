import { useMemo } from "react";
import { useQueries, useQueryClient } from "@tanstack/react-query";
import { issueQuery, useFollowJiraLinks, useJiraAssigned, type IssueState } from "@/api/jira";
import { useWorktrees } from "@/api/hooks";
import { ancestorKeys, type Links, type Lookup } from "@/lib/jira/tree";
import type { JiraIssue } from "@/types/jira";
import { useJira } from "./jiraState";

const EMPTY: never[] = [];
const EMPTY_LINKS: ReadonlyMap<string, Links> = new Map();

/// Everything My Jira issues renders, for its sidebar and its main panel
/// alike (#2038). Both call this; the per-key cache makes the second call
/// free.
///
/// What names each key comes from the desktop's index. The worktree scan
/// is still started here: the desktop indexes worktrees as it scans them,
/// so this page keeps that index current.
export function useJiraBoard() {
  const jira = useJira();
  const on = jira !== null;
  const worktrees = useWorktrees(on);
  useFollowJiraLinks(on, worktrees.dataUpdatedAt);
  const assigned = useJiraAssigned(on);

  const links: ReadonlyMap<string, Links> = jira?.links.byKey ?? EMPTY_LINKS;
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

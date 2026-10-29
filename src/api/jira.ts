import { useEffect } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { jiraAssigned, jiraConnection, jiraFind, jiraIssues, jiraLinks, jiraProjects } from "./tauri";
import type { JiraIssue } from "../types/jira";

/// --- Jira (epic #2031) --------------------------------------------
///
/// Kept out of `hooks.ts`: every Jira read is gated on the capability
/// switch, and a disabled query here must not issue anything. `enabled`
/// is the switch itself, so turning Jira off stops the calls rather than
/// asking the desktop to refuse them. The query functions reference the
/// API lazily for the same reason: a disabled query touches nothing.

/// Whether `acli` is installed and signed in. `pollMs` is the caller's:
/// the open Settings panel polls briskly so a sign-in in a terminal shows
/// up, while the app-wide provider only needs to notice eventually.
export function useJiraConnection(enabled: boolean, pollMs = 30_000) {
  return useQuery({
    queryKey: ["jira-connection"],
    queryFn: () => jiraConnection(),
    enabled,
    refetchInterval: enabled ? pollMs : false,
    staleTime: 10_000,
  });
}

/// The projects the signed-in user can see. Changes on human time.
export function useJiraProjects(enabled: boolean) {
  return useQuery({
    queryKey: ["jira-projects"],
    queryFn: () => jiraProjects(),
    enabled,
    staleTime: 10 * 60_000,
  });
}

/// One key's answer. Three states, because they render differently:
/// `found`; `missing` -- asked, and Jira returned nothing (no such issue,
/// or not yours to see; Jira does not say which); and `unasked` -- the
/// desktop's call budget ran out before reaching it, which is "not checked
/// yet", never "does not exist".
export type IssueState =
  | { state: "found"; issue: JiraIssue }
  | { state: "missing" }
  | { state: "unasked" };

type Waiter = { resolve: (s: IssueState) => void; reject: (e: unknown) => void };

/// How long to gather keys before asking. A list renders its rows in one
/// pass, so this turns a page of chips into one `acli` search.
const GATHER_MS = 15;
/// Keys per desktop call. The desktop batches and budgets further.
const CHUNK = 200;

let waiting = new Map<string, Waiter[]>();
let timer: ReturnType<typeof setTimeout> | null = null;

async function flush() {
  timer = null;
  const batch = waiting;
  waiting = new Map();
  const keys = [...batch.keys()];
  for (let i = 0; i < keys.length; i += CHUNK) {
    const chunk = keys.slice(i, i + CHUNK);
    try {
      const reply = await jiraIssues(chunk);
      const found = new Map(reply.issues.map((issue) => [issue.key, issue]));
      const missing = new Set(reply.missing);
      for (const key of chunk) {
        const issue = found.get(key);
        const state: IssueState = issue
          ? { state: "found", issue }
          : missing.has(key)
            ? { state: "missing" }
            : // Neither returned nor reported missing: the desktop did not
              // reach it. Never "missing" -- that would claim Jira answered.
              { state: "unasked" };
        for (const w of batch.get(key) ?? []) w.resolve(state);
      }
    } catch (e) {
      for (const key of chunk) for (const w of batch.get(key) ?? []) w.reject(e);
    }
  }
}

/// Ask for one key, batched with every other key asked within `GATHER_MS`.
export function loadIssue(key: string): Promise<IssueState> {
  return new Promise((resolve, reject) => {
    const list = waiting.get(key);
    if (list) list.push({ resolve, reject });
    else waiting.set(key, [{ resolve, reject }]);
    timer ??= setTimeout(() => void flush(), GATHER_MS);
  });
}

/// The per-key query, shared by `useJiraIssue` and the Jira page's
/// `useQueries`, so both read and fill the same cache entry.
export function issueQuery(key: string) {
  return {
    queryKey: ["jira-issue", key],
    queryFn: () => loadIssue(key),
    staleTime: 2 * 60_000,
    // An unasked key, or one whose parent is not read yet, is asked again
    // soon; an answered one on the usual cadence.
    refetchInterval: (q: { state: { data?: IssueState } }) => {
      const d = q.state.data;
      const pending = d?.state === "unasked" || (d?.state === "found" && !d.issue.parent_checked);
      return pending ? 30_000 : 5 * 60_000;
    },
  };
}

/// One key's Jira state, cached per key so the same ticket on a PR row, a
/// worktree and the Jira page is asked for once.
export function useJiraIssue(key: string, enabled = true) {
  return useQuery({ ...issueQuery(key), enabled });
}

/// Open issues assigned to the signed-in user (#2038). One search, read
/// only while the Jira page is open.
export function useJiraAssigned(enabled: boolean) {
  return useQuery({
    queryKey: ["jira-assigned"],
    queryFn: () => jiraAssigned(),
    enabled,
    staleTime: 2 * 60_000,
    refetchInterval: enabled ? 5 * 60_000 : false,
  });
}

/// Where each key in the user's projects is named, from the desktop's index
/// (#2035). The desktop rebuilds it only when what feeds it changed, so a
/// refetch is a cheap read. Polled as a backstop; `useFollowJiraLinks`
/// refetches as soon as a list it reads from does.
export function useJiraLinks(enabled: boolean) {
  return useQuery({
    queryKey: ["jira-links"],
    queryFn: () => jiraLinks(),
    enabled,
    staleTime: 10_000,
    refetchInterval: enabled ? 60_000 : false,
  });
}

/// Refetch the link index when data it is built from has just changed:
/// pass the `dataUpdatedAt` of the pull request, worktree or branch query
/// that fed it. The desktop indexes as it stores, so the read that follows
/// sees the new rows.
export function useFollowJiraLinks(enabled: boolean, updatedAt: number) {
  const qc = useQueryClient();
  useEffect(() => {
    if (enabled && updatedAt > 0) void qc.invalidateQueries({ queryKey: ["jira-links"] });
  }, [qc, enabled, updatedAt]);
}

/// Every pull request that names `key`, for the issue panel (#2041). The
/// desktop answers from its database and searches GitHub only when that
/// holds no pull request for the key, or `more` asks for a search.
export function useJiraFind(key: string, enabled: boolean, more: boolean) {
  return useQuery({
    queryKey: ["jira-find", key, more],
    queryFn: () => jiraFind(key, more),
    enabled: enabled && key !== "",
    staleTime: 60_000,
  });
}

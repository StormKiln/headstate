import { useEffect } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { jiraAcliVersion, jiraAssigned, jiraConnection, jiraFind, jiraIssues, jiraLinks, jiraProjects, jiraProjectStatuses } from "./tauri";
import type { IssueBatch, JiraError, JiraIssue } from "../types/jira";
import { NOT_ASKED } from "../lib/notAsked";

/// --- Jira (epic #2031) --------------------------------------------
///
/// Kept out of `hooks.ts`: every Jira read is gated on the capability
/// switch, and a disabled query here must not issue anything. `enabled`
/// is the switch itself, so turning Jira off stops the calls rather than
/// asking the desktop to refuse them. The query functions reference the
/// API lazily for the same reason: a disabled query touches nothing.

/// --- Why a read did not happen (#2033) ------------------------------

const STATES = new Set<JiraError["state"]>([
  "not_asked",
  "not_installed",
  "not_logged_in",
  "rate_limited",
  "timed_out",
  "failed",
]);

/// A rejected Jira command, carrying the desktop's typed reason. Thrown by
/// every query function here, so `String(q.error)` still reads as a
/// sentence and `failureOf(q.error)` gives the state back.
export class JiraFailure extends Error {
  readonly reason: JiraError;
  constructor(reason: JiraError) {
    super(describeJiraError(reason));
    this.name = "JiraFailure";
    this.reason = reason;
  }
}

/// The typed reason inside any rejection. On the desktop a Jira command
/// rejects with the `JiraError` object itself; through the phone's
/// transport a rejection arrives as text, where only "not asked" survives
/// (by its marker) and anything else is `failed` with that text verbatim.
export function failureOf(e: unknown): JiraError {
  if (e instanceof JiraFailure) return e.reason;
  if (typeof e === "object" && e !== null && "state" in e && STATES.has((e as JiraError).state)) {
    return e as JiraError;
  }
  const text = e instanceof Error ? e.message : String(e);
  if (text.startsWith(NOT_ASKED)) return { state: "not_asked" };
  return { state: "failed", detail: text };
}

/// Run a Jira command, rejecting with a `JiraFailure` rather than the raw
/// value the transport produced.
async function typed<T>(run: () => Promise<T>): Promise<T> {
  try {
    return await run();
  } catch (e) {
    throw new JiraFailure(failureOf(e));
  }
}

/// One sentence per state. "Not asked" says Headstate did not ask; it never
/// reads as "Jira did not answer" (#1050).
export function describeJiraError(e: JiraError): string {
  switch (e.state) {
    case "not_asked":
      return "Jira is switched off in Settings, so it was not asked.";
    case "not_installed":
      return e.searched.length > 0
        ? `acli is not installed: looked in ${e.searched.join(", ")}. Set HEADSTATE_ACLI to its path if it is elsewhere.`
        : "acli is not installed.";
    case "not_logged_in":
      return "acli is not signed in to Jira. Run `acli jira auth login --web` in a terminal.";
    case "rate_limited":
      return e.retry_after_secs === null
        ? "Jira is limiting how often it is asked, and did not say for how long. Headstate will ask again later."
        : `Jira is limiting how often it is asked. Headstate will ask again in ${e.retry_after_secs}s.`;
    case "timed_out":
      return `acli did not answer within ${e.secs}s.`;
    case "failed":
      return e.detail;
  }
}

/// How long to wait after a rate limit when Jira did not say.
const RATE_LIMIT_DEFAULT_MS = 60_000;

/// The wait a rate limit asks for, or `null` when `e` is not one.
export function rateLimitWaitMs(e: JiraError | null | undefined): number | null {
  if (e?.state !== "rate_limited") return null;
  return e.retry_after_secs === null ? RATE_LIMIT_DEFAULT_MS : e.retry_after_secs * 1000;
}

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
    queryFn: () => typed(jiraProjects),
    enabled,
    staleTime: 10 * 60_000,
  });
}

/// The installed `acli`'s version. Asked only while Jira is on and `acli`
/// is installed: off, nothing spawns (#2033).
export function useJiraAcliVersion(enabled: boolean) {
  return useQuery({
    queryKey: ["jira-acli-version"],
    queryFn: () => typed(jiraAcliVersion),
    enabled,
    staleTime: 10 * 60_000,
  });
}

/// The statuses seen in `project`'s recent issues, for choosing an
/// expectation. Asked only when that project's editor is open.
export function useJiraProjectStatuses(project: string, enabled: boolean) {
  return useQuery({
    queryKey: ["jira-project-statuses", project],
    queryFn: () => typed(() => jiraProjectStatuses(project)),
    enabled,
    staleTime: 10 * 60_000,
  });
}

/// One key's answer. Three states, because they render differently:
/// `found`; `missing` -- asked, and Jira returned nothing (no such issue,
/// or not yours to see; Jira does not say which); and `unasked` -- the
/// desktop's call budget ran out before reaching it, which is "not checked
/// yet", never "does not exist". `why` says what stopped the desktop when
/// it was stopped rather than out of budget: a rate limit, a timeout, a
/// lost sign-in. Absent or `null`: the call budget ran out first.
export type IssueState =
  | { state: "found"; issue: JiraIssue }
  | { state: "missing" }
  | { state: "unasked"; why?: JiraError | null };

type Waiter = { resolve: (s: IssueState) => void; reject: (e: unknown) => void };

/// How long to gather keys before asking. A list renders its rows in one
/// pass, so this turns a page of chips into one `acli` search.
const GATHER_MS = 15;
/// Keys per desktop call. The desktop batches and budgets further.
const CHUNK = 200;

let waiting = new Map<string, Waiter[]>();
let timer: ReturnType<typeof setTimeout> | null = null;

/// While Jira is rate limiting, no key is sent at all: every ask before
/// this moment is answered `unasked` with the limit as its reason, so a
/// page of chips refetching cannot spend a limit already exhausted.
let coolOff: { until: number; why: JiraError } | null = null;

async function flush() {
  timer = null;
  const batch = waiting;
  waiting = new Map();
  const keys = [...batch.keys()];
  for (let i = 0; i < keys.length; i += CHUNK) {
    const chunk = keys.slice(i, i + CHUNK);
    if (coolOff && Date.now() < coolOff.until) {
      const why = coolOff.why;
      for (const key of chunk) for (const w of batch.get(key) ?? []) w.resolve({ state: "unasked", why });
      continue;
    }
    try {
      const reply: IssueBatch = await typed(() => jiraIssues(chunk));
      const wait = rateLimitWaitMs(reply.error);
      coolOff = wait !== null && reply.error ? { until: Date.now() + wait, why: reply.error } : null;
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
              { state: "unasked", why: reply.error };
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
    // soon; an answered one on the usual cadence. One left unasked by a
    // rate limit waits at least as long as Jira asked.
    refetchInterval: (q: { state: { data?: IssueState } }) => {
      const d = q.state.data;
      const limited = d?.state === "unasked" ? rateLimitWaitMs(d.why) : null;
      if (limited !== null) return Math.max(30_000, limited);
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
    queryFn: () => typed(jiraAssigned),
    enabled,
    staleTime: 2 * 60_000,
    // Never sooner than a rate limit asked for.
    refetchInterval: (q: { state: { data?: IssueBatch } }) =>
      enabled ? Math.max(5 * 60_000, rateLimitWaitMs(q.state.data?.error) ?? 0) : false,
  });
}

/// Where each key in the user's projects is named, from the desktop's index
/// (#2035). The desktop rebuilds it only when what feeds it changed, so a
/// refetch is a cheap read. Polled as a backstop; `useFollowJiraLinks`
/// refetches as soon as a list it reads from does.
export function useJiraLinks(enabled: boolean) {
  return useQuery({
    queryKey: ["jira-links"],
    queryFn: () => typed(jiraLinks),
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
    queryFn: () => typed(() => jiraFind(key, more)),
    enabled: enabled && key !== "",
    staleTime: 60_000,
  });
}

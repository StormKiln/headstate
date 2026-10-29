import { useEffect } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  jiraAcliVersion,
  jiraAssigned,
  jiraConnection,
  jiraFind,
  jiraIssues,
  jiraLinks,
  jiraProjects,
  jiraProjectStatuses,
  jiraTransition,
} from "./tauri";
import type { IssueBatch, JiraError, JiraIssue, TransitionResult } from "../types/jira";
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

/// The marker the phone's companion puts before a Jira command's typed
/// reason (#2047). JSON follows: `{"reason": JiraError, "message": text}`.
/// Must match `JIRA_REASON` in `src-mobile/src/client.rs`;
/// `mirroredConstants.test.ts` asserts it.
export const JIRA_REASON = "headstate:jira-reason ";

/// Whether `e` is a `JiraError` this build knows.
function isJiraError(e: unknown): e is JiraError {
  return typeof e === "object" && e !== null && "state" in e && STATES.has((e as JiraError).state);
}

/// What follows `JIRA_REASON`: the typed reason when it holds a state this
/// build knows, else `failed` with the desktop's own sentence. `null` when
/// the text is not marked or what follows is not the expected JSON.
function markedReason(text: string): JiraError | null {
  if (!text.startsWith(JIRA_REASON)) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(text.slice(JIRA_REASON.length));
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null) return null;
  const { reason, message } = parsed as { reason?: unknown; message?: unknown };
  if (isJiraError(reason)) return reason;
  // A state added on a newer desktop: its sentence still reads.
  return typeof message === "string" ? { state: "failed", detail: message } : null;
}

/// The typed reason inside any rejection. On the desktop a Jira command
/// rejects with the `JiraError` object itself. Through the phone's
/// transport a rejection arrives as text: the reason as JSON behind
/// `JIRA_REASON` from a desktop that sends it, or, from one that does not,
/// the prose alone, where only "not asked" survives (by its marker) and
/// anything else is `failed` with that text verbatim.
export function failureOf(e: unknown): JiraError {
  if (e instanceof JiraFailure) return e.reason;
  if (isJiraError(e)) return e;
  const text = e instanceof Error ? e.message : String(e);
  const marked = markedReason(text);
  if (marked) return marked;
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

/// The last issue each key was found as. A key not asked because of a
/// rate limit is answered with this, marked stale, rather than turning an
/// answer already on screen into "not checked yet" (#2036).
const lastFound = new Map<string, JiraIssue>();

/// What a key was not asked about for `why` reads as: its last answer,
/// stale, when there is one.
function notAsked(key: string, why: JiraError | null): IssueState {
  const last = lastFound.get(key);
  return last ? { state: "found", issue: { ...last, stale: true } } : { state: "unasked", why };
}

/// One key's state from a desktop reply.
function stateIn(key: string, reply: IssueBatch, found: Map<string, JiraIssue>, missing: Set<string>): IssueState {
  const issue = found.get(key);
  if (issue) {
    lastFound.set(key, issue);
    return { state: "found", issue };
  }
  if (missing.has(key)) return { state: "missing" };
  // Neither returned nor reported missing: the desktop did not reach it.
  // Never "missing" -- that would claim Jira answered.
  return notAsked(key, reply.error);
}

/// Remember a reply's rate limit, if it carried one.
function noteLimit(reply: IssueBatch) {
  const wait = rateLimitWaitMs(reply.error);
  coolOff = wait !== null && reply.error ? { until: Date.now() + wait, why: reply.error } : null;
}

async function flush() {
  timer = null;
  const batch = waiting;
  waiting = new Map();
  const keys = [...batch.keys()];
  for (let i = 0; i < keys.length; i += CHUNK) {
    const chunk = keys.slice(i, i + CHUNK);
    if (coolOff && Date.now() < coolOff.until) {
      const why = coolOff.why;
      for (const key of chunk) for (const w of batch.get(key) ?? []) w.resolve(notAsked(key, why));
      continue;
    }
    try {
      const reply: IssueBatch = await typed(() => jiraIssues(chunk));
      noteLimit(reply);
      const found = new Map(reply.issues.map((issue) => [issue.key, issue]));
      const missing = new Set(reply.missing);
      for (const key of chunk) {
        const state = stateIn(key, reply, found, missing);
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

/// Read one key past every cache -- the desktop's minute and this side's
/// `staleTime` -- for an issue being opened (#2036). Not while Jira is
/// rate limiting: then the last answer, marked stale, is what there is.
export async function loadIssueFresh(key: string): Promise<IssueState> {
  if (coolOff && Date.now() < coolOff.until) return notAsked(key, coolOff.why);
  const reply = await typed(() => jiraIssues([key], true));
  noteLimit(reply);
  return stateIn(key, reply, new Map(reply.issues.map((i) => [i.key, i])), new Set(reply.missing));
}

/// The issue being opened, read fresh each time it is opened. A found
/// answer also fills the per-key cache, so its chips agree at once.
export function useJiraIssueFresh(key: string, enabled: boolean) {
  const qc = useQueryClient();
  return useQuery({
    queryKey: ["jira-issue-fresh", key],
    queryFn: async () => {
      const state = await loadIssueFresh(key);
      if (state.state === "found" && !state.issue.stale) qc.setQueryData(["jira-issue", key], state);
      return state;
    },
    enabled: enabled && key !== "",
    staleTime: 0,
    gcTime: 0,
    refetchOnMount: "always",
    refetchOnWindowFocus: false,
  });
}

/// Move a Jira issue to another status (#2044), the one Jira write.
///
/// Call `mutate({ key, status })` only after the user has confirmed the
/// argv `transitionArgv` builds. Rejects (with a `JiraFailure`) only when
/// the desktop sent nothing it could report on -- Jira off, no `acli`, a
/// status it refused -- or the call never came back. Anything else,
/// refusals included, resolves with a `TransitionResult`; read it through
/// `transitionOutcome`, never by `requested`.
///
/// On success the issue as Jira reported it after the move replaces what
/// every Jira surface holds for it, so a chip, the sidebar and the panel
/// agree at once; the assigned list is read again, since a move to Done
/// takes an issue off it.
export function useJiraTransition() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ key, status }: { key: string; status: string }) => typed(() => jiraTransition(key, status)),
    onSuccess: (r: TransitionResult) => {
      if (r.after) {
        const state: IssueState = { state: "found", issue: r.after };
        lastFound.set(r.key, r.after);
        qc.setQueryData(["jira-issue", r.key], state);
        qc.setQueryData(["jira-issue-fresh", r.key], state);
      }
      void qc.invalidateQueries({ queryKey: ["jira-assigned"] });
    },
  });
}

/// A query's answer with a failed refetch taken into account: data already
/// loaded is kept, marked stale, rather than replaced by the failure
/// (#2036). `"failed"` only when there is no answer at all.
export function keptOrFailed(
  data: IssueState | undefined,
  isError: boolean,
): IssueState | "failed" | undefined {
  if (data === undefined) return isError ? "failed" : undefined;
  if (isError && data.state === "found") return { state: "found", issue: { ...data.issue, stale: true } };
  return data;
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

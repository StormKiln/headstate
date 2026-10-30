/// Where a pull request and its ticket disagree about the work (#2042).
///
/// Decided by default by Jira's status CATEGORY, which every workflow has,
/// so it needs no configuration: "In Testing" and "Code Review" are both in
/// progress, whatever a project calls them. Settings › Jira can set, per
/// project, what a ticket should be while its pull request is open and once
/// it merged (#2034): a category, matched by category, or one named status,
/// matched by name. Unset, the defaults below are exactly what these rules
/// did before that setting existed.
///
/// `driftFor` holds the rules an OPEN pull request decides. The rest --
/// merged (R2), some merged (R3), closed unmerged (R5) -- need to know what
/// became of a pull request, which only a search reports (#2041): one that
/// merely left the user's list may have merged, been closed, or still be
/// open with the user no longer on it. So `afterMerge` is fed the lookup
/// and told how much of it is known. A lookup that did not ask everywhere
/// it could, or a pull request whose fate is unknown, makes "merged, ticket
/// not moved" a qualified statement, never a warning.
///
/// "Everywhere" is the lookup's own scope (`jira/lookup.rs`): GitHub and
/// GitLab searched by TITLE across the owners and projects in the user's
/// lists. A teammate's pull request naming the key only in its branch, in
/// a repository the user has no list for, is not seen. Complete means
/// complete over that scope.

import type {
  JiraIssue,
  JiraLink,
  JiraStatus,
  KeyLookup,
  KeySearch,
  StatusExpectation,
  StatusExpectations,
} from "@/types/jira";
import type { KeySource } from "./keys";

export interface Drift {
  /// Stable, for tests and for a filter to count by.
  rule: "open-but-not-started" | "done-but-open" | "open-not-expected";
  message: string;
}

/// Expectations by project key, as `UiPrefs.jira_status_expectations`.
export type ExpectationMap = Readonly<Record<string, StatusExpectations>>;

export const DEFAULT_ON_OPEN: StatusExpectation = { kind: "category", category: "in_progress" };
export const DEFAULT_ON_MERGED: StatusExpectation = { kind: "category", category: "done" };

const CATEGORY_LABEL = { new: "To Do", in_progress: "In Progress", done: "Done" } as const;

/// How an expectation reads in a sentence and in Settings.
export function expectationLabel(e: StatusExpectation): string {
  return e.kind === "category" ? `any ${CATEGORY_LABEL[e.category]} status` : e.name;
}

function isDefault(e: StatusExpectation, d: StatusExpectation): boolean {
  return e.kind === "category" && d.kind === "category" && e.category === d.category;
}

/// Whether `status` meets `e`: true, false, or null when it cannot be
/// told. A category expectation cannot judge an unreported category, and a
/// named one cannot judge a status whose name did not come back -- absent
/// is not a mismatch.
function meets(status: JiraStatus, e: StatusExpectation): boolean | null {
  if (e.kind === "category") return status.category === "unknown" ? null : status.category === e.category;
  return status.name === "" ? null : status.name === e.name;
}

/// The project's expectations with the defaults filled in.
function expectationsFor(issue: JiraIssue, map?: ExpectationMap): { on_open: StatusExpectation; on_merged: StatusExpectation } {
  const set = map?.[issue.project_key];
  return { on_open: set?.on_open ?? DEFAULT_ON_OPEN, on_merged: set?.on_merged ?? DEFAULT_ON_MERGED };
}

/// The disagreement between an open pull request and the issue it names,
/// or null. `source` is where the key was found; any linked key counts,
/// but a draft has not been put up for review and is never "open" here.
export function driftFor(
  issue: JiraIssue,
  pr: { draft: boolean; source: KeySource },
  expectations?: ExpectationMap,
): Drift | null {
  if (pr.draft) return null;
  const expected = expectationsFor(issue, expectations).on_open;
  if (meets(issue.status, expected) !== false) return null;
  switch (issue.status.category) {
    case "new":
      return {
        rule: "open-but-not-started",
        message: `This pull request is open, but ${issue.key} is still ${issue.status.name || "To Do"}.`,
      };
    case "done":
      return {
        rule: "done-but-open",
        message: `${issue.key} is ${issue.status.name || "Done"}, but this pull request is still open.`,
      };
    // Only reachable with a non-default expectation: in progress (or an
    // unreported category, by name) is not what this project asked for.
    case "in_progress":
    case "unknown":
      return {
        rule: "open-not-expected",
        message: `This pull request is open, but ${issue.key} is ${issue.status.name || "in progress"}, not ${expectationLabel(expected)}.`,
      };
  }
}

/// What became of one pull request that names the ticket, as far as it is
/// known. `unknown` is one of the user's own that left its list with no
/// search to say whether it merged, was closed, or is still open without
/// them on it -- never read as any of those.
export type PrFate = "open" | "merged" | "closed" | "unknown";

export interface KnownPr {
  url: string;
  fate: PrFate;
  draft: boolean;
  /// Only for `merged`, when the search reported it.
  merged_at: string | null;
  /// Where the key was found. Only a title or branch link drives a rule.
  source: KeySource;
}

type PrTarget = Extract<JiraLink["target"], { kind: "pr" }>;

/// The pull requests a lookup knows for its key, one per URL. One from the
/// user's lists is open while listed; once it has left, it is what a search
/// said of it, else `unknown`. A searched one whose state did not come back
/// is `unknown` too.
export function knownPrs(links: readonly JiraLink[]): KnownPr[] {
  const searched = new Map<string, PrTarget>();
  for (const l of links) {
    if (l.target.kind === "pr" && l.target.role === "searched") searched.set(l.target.url, l.target);
  }
  const out = new Map<string, KnownPr>();
  for (const l of links) {
    const t = l.target;
    if (t.kind !== "pr" || !LINKED.has(l.found_in)) continue;
    const listed = t.role !== "searched" && l.current;
    const s = t.role === "searched" ? t : searched.get(t.url);
    // A listed pull request is open whatever an older search said.
    const fate: PrFate = listed ? "open" : (s?.state ?? "unknown");
    const prior = out.get(t.url);
    // Authored and reviewing can both hold one pull request: open wins.
    if (prior && (prior.fate === "open" || !listed)) continue;
    out.set(t.url, {
      url: t.url,
      fate,
      draft: listed ? t.is_draft : (s?.is_draft ?? t.is_draft),
      merged_at: fate === "merged" ? (s?.merged_at ?? null) : null,
      source: l.found_in,
    });
  }
  return [...out.values()];
}

/// The sources that link a pull request to a ticket. A mention elsewhere
/// -- a body, a comment -- would not drive a rule (#2042).
const LINKED: ReadonlySet<KeySource> = new Set<KeySource>(["title", "branch"]);

/// Whether a lookup asked everywhere it could: each half asked, none cut
/// short, none leaving candidates out. A half not asked -- even because the
/// database already knew -- or one that failed is not complete.
export function lookupComplete(l: Pick<KeyLookup, "search" | "gitlab">): boolean {
  const whole = (s: KeySearch) => s.state === "asked" && !s.truncated && s.unsearched.length === 0;
  return whole(l.search) && (l.gitlab === null || whole(l.gitlab));
}

/// What the pull requests that are no longer open say about the ticket
/// (#2042: R2, R3, R5). `level` is how it may be shown:
/// - `fact`: every pull request is accounted for. A warning.
/// - `qualified`: true of what is known, but the lookup was partial or a
///   pull request's fate is unknown. Says so, and is never a warning.
/// - `waiting`: merged within the grace period, so an automation that
///   moves the ticket on merge has not had its chance yet.
/// - `progress`: some merged, some open (R3). A count, not a warning.
export interface Verdict {
  rule: "merged-not-done" | "closed-not-merged" | "some-merged";
  level: "fact" | "qualified" | "waiting" | "progress";
  message: string;
}

export const DEFAULT_MERGE_GRACE_MINS = 15;

/// The rule table for pull requests that are no longer open. `complete` is
/// `lookupComplete` of the lookup `prs` came from. A status whose category
/// did not come back decides nothing. Merged alongside closed-unmerged is
/// R2: the work landed, and an abandoned attempt does not change that.
export function afterMerge(
  issue: JiraIssue,
  prs: readonly KnownPr[],
  complete: boolean,
  opts: { expectations?: ExpectationMap; now: Date; graceMins?: number },
): Verdict | null {
  const live = prs.filter((p) => LINKED.has(p.source));
  const count = (f: PrFate) => live.filter((p) => p.fate === f).length;
  const [open, merged, closed, unknown] = [count("open"), count("merged"), count("closed"), count("unknown")];
  const key = issue.key;
  if (merged > 0 && open > 0) {
    const floor = complete && unknown === 0 ? "" : "at least ";
    return {
      rule: "some-merged",
      level: "progress",
      message: `${merged} of ${floor}${live.length} pull requests for ${key} merged.`,
    };
  }
  if (open > 0 || issue.status.category === "unknown") return null;
  const status = issue.status.name;
  // Why a true statement is not the whole truth, if it is not.
  const caveat = !complete
    ? `other pull requests for ${key} were not checked`
    : unknown > 0
      ? `${unknown} other pull request${unknown === 1 ? "" : "s"} for ${key} could not be checked`
      : null;
  if (merged > 0) {
    const expected = expectationsFor(issue, opts.expectations).on_merged;
    if (meets(issue.status, expected) !== false) return null;
    const plain = isDefault(expected, DEFAULT_ON_MERGED);
    const where = plain
      ? `still ${status || "open"}`
      : `${status || "in an unreported status"}, not ${expectationLabel(expected)}`;
    const qualified = (why: string): Verdict => ({
      rule: "merged-not-done",
      level: "qualified",
      message: `A pull request for ${key} has merged and the ticket is ${where}; ${why}.`,
    });
    if (caveat) return qualified(caveat);
    const times = live.filter((p) => p.fate === "merged").map((p) => (p.merged_at ? Date.parse(p.merged_at) : NaN));
    // Not reported: the wait cannot be timed, so it is not known to be over.
    if (times.some(Number.isNaN)) return qualified("when it merged was not reported");
    const since = opts.now.getTime() - Math.max(...times);
    if (since < (opts.graceMins ?? DEFAULT_MERGE_GRACE_MINS) * 60_000) {
      const mins = Math.max(0, Math.floor(since / 60_000));
      const ago = mins === 0 ? "less than a minute" : `${mins} minute${mins === 1 ? "" : "s"}`;
      return {
        rule: "merged-not-done",
        level: "waiting",
        message: `A pull request for ${key} merged ${ago} ago; the ticket is ${where} for now.`,
      };
    }
    return {
      rule: "merged-not-done",
      level: "fact",
      message: plain
        ? `A pull request for ${key} has merged, but the ticket is still ${status || "open"}.`
        : `A pull request for ${key} has merged, but the ticket is ${where}.`,
    };
  }
  if (closed > 0 && issue.status.category === "in_progress") {
    const base = `A pull request for ${key} was closed without merging, and the ticket is still ${status || "in progress"}`;
    return caveat
      ? { rule: "closed-not-merged", level: "qualified", message: `${base}; ${caveat}.` }
      : { rule: "closed-not-merged", level: "fact", message: `${base}.` };
  }
  return null;
}

/// The move that would settle a drift, as far as Headstate can name one
/// (#2042, #2045). A NAMED expectation is a status to move to. A CATEGORY
/// is not: "any In Progress status" is many statuses, and acli cannot list
/// which ones the workflow allows from here, so the reader chooses
/// (`choose`) -- a status name is never made up from a category.
export type Fix = { kind: "move"; status: string } | { kind: "choose"; expected: string };

function toward(e: StatusExpectation): Fix {
  return e.kind === "status" ? { kind: "move", status: e.name } : { kind: "choose", expected: expectationLabel(e) };
}

/// The fix an open pull request's drift offers: only R1 -- open, ticket
/// still To Do -- toward the project's on-open expectation. R4 (done but
/// open) is not fixed by moving the ticket, and the rest are not raised as
/// a plain fact about a status to move to.
export function fixForDrift(issue: JiraIssue, drift: Drift, expectations?: ExpectationMap): Fix | null {
  return drift.rule === "open-but-not-started" ? toward(expectationsFor(issue, expectations).on_open) : null;
}

/// The fix a verdict offers: only R2 raised as FACT -- every pull request
/// accounted for, merged, past the grace period -- toward the on-merged
/// expectation. A qualified, waiting or progress verdict is not a
/// statement to act on, so it offers nothing.
export function fixForVerdict(issue: JiraIssue, verdict: Verdict, expectations?: ExpectationMap): Fix | null {
  return verdict.rule === "merged-not-done" && verdict.level === "fact"
    ? toward(expectationsFor(issue, expectations).on_merged)
    : null;
}

/// Start work's optional move (#2045): toward the on-open expectation, or
/// nothing when the ticket already meets it -- or when its status did not
/// come back, so whether it does cannot be told.
export function startFix(issue: JiraIssue, expectations?: ExpectationMap): Fix | null {
  const e = expectationsFor(issue, expectations).on_open;
  return meets(issue.status, e) === false ? toward(e) : null;
}

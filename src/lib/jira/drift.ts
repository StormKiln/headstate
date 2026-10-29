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
/// `driftFor` holds the rules an OPEN pull request decides. "Merged, but
/// the ticket did not move" needs to know a pull request MERGED, which only
/// a GitHub search reports (#2041) -- one that merely left the user's list
/// may have been closed -- so it is `mergedButNotDone`, fed the lookup.

import type { JiraIssue, JiraStatus, StatusExpectation, StatusExpectations } from "@/types/jira";
import type { KeySource } from "./keys";

export interface Drift {
  /// Stable, for tests and for a filter to count by.
  rule: "open-but-not-started" | "done-but-open" | "open-not-expected" | "merged-not-done";
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

/// A pull request for the ticket merged and none is still open, yet the
/// ticket is not where it should be once merged (#2042, R2) -- by default,
/// not done. `merged` and `open` count what the lookup KNOWS; a status that
/// cannot be judged decides nothing.
export function mergedButNotDone(
  issue: JiraIssue,
  known: { merged: number; open: number },
  expectations?: ExpectationMap,
): Drift | null {
  if (known.merged === 0 || known.open > 0) return null;
  const expected = expectationsFor(issue, expectations).on_merged;
  if (meets(issue.status, expected) !== false) return null;
  return {
    rule: "merged-not-done",
    message: isDefault(expected, DEFAULT_ON_MERGED)
      ? `A pull request for ${issue.key} has merged, but the ticket is still ${issue.status.name || "open"}.`
      : `A pull request for ${issue.key} has merged, but the ticket is ${issue.status.name || "in an unreported status"}, not ${expectationLabel(expected)}.`,
  };
}

/// Where a pull request and its ticket disagree about the work (#2042).
///
/// Decided by Jira's status CATEGORY, which every workflow has, so it
/// needs no configuration: "In Testing" and "Code Review" are both in
/// progress, whatever a project calls them.
///
/// `driftFor` holds the rules an OPEN pull request decides. "Merged, but
/// the ticket did not move" needs to know a pull request MERGED, which only
/// a GitHub search reports (#2041) -- one that merely left the user's list
/// may have been closed -- so it is `mergedButNotDone`, fed the lookup.

import type { JiraIssue } from "@/types/jira";
import type { KeySource } from "./keys";

export interface Drift {
  /// Stable, for tests and for a filter to count by.
  rule: "open-but-not-started" | "done-but-open" | "merged-not-done";
  message: string;
}

/// The disagreement between an open pull request and the issue it names,
/// or null. `source` is where the key was found; any linked key counts,
/// but a draft has not been put up for review and is never "open" here.
export function driftFor(issue: JiraIssue, pr: { draft: boolean; source: KeySource }): Drift | null {
  if (pr.draft) return null;
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
    // In progress agrees with an open pull request. Unknown decides
    // nothing: a status whose category did not come back is not To Do.
    case "in_progress":
    case "unknown":
      return null;
  }
}

/// A pull request for the ticket merged and none is still open, yet the
/// ticket is not done (#2042, R2). `merged` and `open` count what the
/// lookup KNOWS; an unknown category decides nothing.
export function mergedButNotDone(issue: JiraIssue, known: { merged: number; open: number }): Drift | null {
  if (known.merged === 0 || known.open > 0) return null;
  if (issue.status.category !== "new" && issue.status.category !== "in_progress") return null;
  return {
    rule: "merged-not-done",
    message: `A pull request for ${issue.key} has merged, but the ticket is still ${issue.status.name || "open"}.`,
  };
}

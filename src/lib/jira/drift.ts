/// Where a pull request and its ticket disagree about the work (#2042).
///
/// Decided by Jira's status CATEGORY, which every workflow has, so it
/// needs no configuration: "In Testing" and "Code Review" are both in
/// progress, whatever a project calls them.
///
/// Only the rules an OPEN pull request can decide live here. "Merged, but
/// the ticket did not move" needs every pull request for the ticket,
/// merged ones included, and Headstate reads open ones only -- asserting
/// it from that would be a guess. It waits on the reverse lookup (#2041).

import type { JiraIssue } from "@/types/jira";
import type { KeySource } from "./keys";

export interface Drift {
  /// Stable, for tests and for a filter to count by.
  rule: "open-but-not-started" | "done-but-open";
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

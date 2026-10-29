import { describe, expect, it } from "vitest";
import type { JiraIssue, JiraStatusCategory } from "@/types/jira";
import { driftFor, mergedButNotDone } from "./drift";

const issue = (category: JiraStatusCategory, name: string): JiraIssue => ({
  key: "ABC-1",
  summary: "s",
  issue_type: { name: "Story", hierarchy_level: 0 },
  status: { name, category },
  assignee: null,
  parent_key: null,
  parent_checked: true,
  project_key: "ABC",
  url: null,
  updated: null,
});

describe("driftFor", () => {
  it.each<[JiraStatusCategory, string, string | null]>([
    ["new", "Backlog", "open-but-not-started"],
    ["in_progress", "In Testing", null],
    ["done", "Closed", "done-but-open"],
    // Absent is not zero: an unreported category is never To Do.
    ["unknown", "Odd", null],
  ])("%s (%s) → %s", (category, name, rule) => {
    expect(driftFor(issue(category, name), { draft: false, source: "title" })?.rule ?? null).toBe(rule);
  });

  it("names the project's own status in the message", () => {
    expect(driftFor(issue("new", "Backlog"), { draft: false, source: "branch" })?.message).toBe(
      "This pull request is open, but ABC-1 is still Backlog.",
    );
  });

  it("never flags a draft", () => {
    expect(driftFor(issue("new", "To Do"), { draft: true, source: "title" })).toBeNull();
    expect(driftFor(issue("done", "Done"), { draft: true, source: "title" })).toBeNull();
  });
});

describe("mergedButNotDone", () => {
  it("flags a ticket left behind by its merged pull request", () => {
    expect(mergedButNotDone(issue("in_progress", "In Review"), { merged: 1, open: 0 })?.message).toBe(
      "A pull request for ABC-1 has merged, but the ticket is still In Review.",
    );
  });

  it.each<[JiraStatusCategory, { merged: number; open: number }]>([
    ["done", { merged: 1, open: 0 }],
    // Absent is not zero.
    ["unknown", { merged: 1, open: 0 }],
    // Work continues in another pull request.
    ["in_progress", { merged: 1, open: 1 }],
    ["new", { merged: 0, open: 0 }],
  ])("does not flag %s with %j", (category, known) => {
    expect(mergedButNotDone(issue(category, "x"), known)).toBeNull();
  });
});

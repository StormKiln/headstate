import { describe, expect, it } from "vitest";
import type { JiraIssue, JiraStatusCategory } from "@/types/jira";
import { driftFor } from "./drift";

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

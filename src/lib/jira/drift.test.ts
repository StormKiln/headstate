import { describe, expect, it } from "vitest";
import type { JiraIssue, JiraStatusCategory } from "@/types/jira";
import { driftFor, mergedButNotDone, type ExpectationMap } from "./drift";

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

/// Per-project expectations (#2034). The tests above pass no map and so
/// pin the defaults: a user who sets nothing sees exactly what they did.
describe("status expectations", () => {
  const pr = { draft: false, source: "title" as const };
  const named = (name: string): ExpectationMap => ({ ABC: { on_open: { kind: "status", name } } });

  it("matches a named status by name, whatever its category", () => {
    expect(driftFor(issue("in_progress", "In Review"), pr, named("In Review"))).toBeNull();
    expect(driftFor(issue("in_progress", "In Progress"), pr, named("In Review"))).toEqual({
      rule: "open-not-expected",
      message: "This pull request is open, but ABC-1 is In Progress, not In Review.",
    });
    expect(driftFor(issue("new", "To Do"), pr, named("In Review"))?.rule).toBe("open-but-not-started");
  });

  it("matches a category by category", () => {
    const notYet: ExpectationMap = { ABC: { on_open: { kind: "category", category: "new" } } };
    expect(driftFor(issue("new", "Backlog"), pr, notYet)).toBeNull();
    expect(driftFor(issue("in_progress", "Doing"), pr, notYet)?.message).toBe(
      "This pull request is open, but ABC-1 is Doing, not any To Do status.",
    );
  });

  it("applies only to the project it names", () => {
    expect(driftFor(issue("in_progress", "In Progress"), pr, { XY: { on_open: { kind: "status", name: "QA" } } })).toBeNull();
  });

  /// Absent is not a mismatch.
  it("decides nothing when the status it needs did not come back", () => {
    expect(driftFor(issue("in_progress", ""), pr, named("In Review"))).toBeNull();
    expect(mergedButNotDone(issue("unknown", "Odd"), { merged: 1, open: 0 }, {})).toBeNull();
  });

  it("compares a merged ticket against its own expectation", () => {
    const released: ExpectationMap = { ABC: { on_merged: { kind: "status", name: "Released" } } };
    expect(mergedButNotDone(issue("done", "Released"), { merged: 1, open: 0 }, released)).toBeNull();
    expect(mergedButNotDone(issue("done", "Done"), { merged: 1, open: 0 }, released)?.message).toBe(
      "A pull request for ABC-1 has merged, but the ticket is Done, not Released.",
    );
    // An unset side keeps its default.
    expect(driftFor(issue("in_progress", "Doing"), pr, released)).toBeNull();
  });
});

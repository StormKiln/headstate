import { describe, expect, it } from "vitest";
import type { IssueState } from "@/api/jira";
import type { JiraIssue } from "@/types/jira";
import { NO_EPIC, UNRESOLVED, ancestorKeys, buildTree, epicOf, initiativeOf, rollUp, type Lookup } from "./tree";

const issue = (key: string, level: number | null, parent: string | null = null, category: JiraIssue["status"]["category"] = "new"): IssueState => ({
  state: "found",
  issue: {
    key,
    summary: key,
    issue_type: { name: "x", hierarchy_level: level },
    status: { name: "s", category },
    assignee: null,
    parent_key: parent,
    parent_checked: true,
    project_key: key.split("-")[0],
    url: null,
    updated: null,
  },
});

const table = (entries: Record<string, IssueState | "failed">): Lookup => (k) => entries[k];

describe("epicOf", () => {
  const lookup = table({
    "ABC-1": issue("ABC-1", 0, "ABC-2"),
    "ABC-2": issue("ABC-2", 1, "ABC-9"),
    "ABC-3": issue("ABC-3", -1, "ABC-1"),
    "ABC-4": issue("ABC-4", 0),
    "ABC-5": issue("ABC-5", 0, "ABC-6"),
    "ABC-6": { state: "missing" },
    "ABC-7": issue("ABC-7", 0, "ABC-8"),
    "ABC-9": issue("ABC-9", 2),
  });

  it("files a story under its epic and a sub-task under its story's epic", () => {
    expect(epicOf("ABC-1", lookup)).toBe("ABC-2");
    expect(epicOf("ABC-3", lookup)).toBe("ABC-2");
  });

  it("files an epic under itself", () => {
    expect(epicOf("ABC-2", lookup)).toBe("ABC-2");
  });

  it("does not file an issue whose parent is not read yet under No epic", () => {
    const unread = issue("ABC-4", 0);
    if (unread.state === "found") unread.issue.parent_checked = false;
    expect(epicOf("ABC-4", table({ "ABC-4": unread }))).toBe(UNRESOLVED);
  });

  it("uses NO_EPIC only for an issue that has no parent", () => {
    expect(epicOf("ABC-4", lookup)).toBe(NO_EPIC);
  });

  /// "Could not see the parent" is not "has no parent".
  it("keeps an unreadable or unanswered parent as the group", () => {
    expect(epicOf("ABC-5", lookup)).toBe("ABC-6");
    expect(epicOf("ABC-7", lookup)).toBe("ABC-8");
  });

  it("files an issue whose own answer is missing as unresolved, not as no epic", () => {
    expect(epicOf("XY-1", lookup)).toBe(UNRESOLVED);
    expect(epicOf("XY-2", table({ "XY-2": { state: "unasked" } }))).toBe(UNRESOLVED);
    expect(epicOf("XY-3", table({ "XY-3": "failed" }))).toBe(UNRESOLVED);
  });

  it("finds the initiative above an epic, and none above a plain epic", () => {
    expect(initiativeOf("ABC-2", lookup)).toBe("ABC-9");
    expect(initiativeOf(NO_EPIC, lookup)).toBe(null);
    expect(initiativeOf("ABC-6", lookup)).toBe(null);
  });

  it("survives a parent cycle", () => {
    const cyc = table({ "A-1": issue("A-1", 0, "A-2"), "A-2": issue("A-2", 0, "A-1") });
    expect(() => epicOf("A-1", cyc)).not.toThrow();
    expect(ancestorKeys(["A-1"], cyc)).toEqual(["A-2"]);
  });
});

describe("ancestorKeys", () => {
  it("returns every reachable ancestor and the first unanswered one", () => {
    const lookup = table({ "ABC-1": issue("ABC-1", -1, "ABC-2"), "ABC-2": issue("ABC-2", 0, "ABC-3") });
    expect(ancestorKeys(["ABC-1"], lookup)).toEqual(["ABC-2", "ABC-3"]);
    expect(ancestorKeys(["XY-1"], lookup)).toEqual([]);
  });
});

describe("buildTree", () => {
  it("groups initiative → epic → issue, with the special groups last", () => {
    const lookup = table({
      "ABC-1": issue("ABC-1", 0, "ABC-2"),
      "ABC-2": issue("ABC-2", 1, "ABC-9"),
      "ABC-9": issue("ABC-9", 2),
      "ABC-4": issue("ABC-4", 0),
      "XY-1": issue("XY-1", 0, "XY-5"),
      "XY-5": issue("XY-5", 1),
    });
    expect(buildTree(["ABC-4", "UNKNOWN-1", "ABC-1", "XY-1", "ABC-2"], lookup)).toEqual([
      { initiative: "ABC-9", epics: [{ epic: "ABC-2", issues: ["ABC-1"] }] },
      {
        initiative: null,
        epics: [
          { epic: "XY-5", issues: ["XY-1"] },
          { epic: NO_EPIC, issues: ["ABC-4"] },
          { epic: UNRESOLVED, issues: ["UNKNOWN-1"] },
        ],
      },
    ]);
  });
});

describe("rollUp", () => {
  it("counts only answers it has, and the counts sum to the children", () => {
    const lookup = table({ "A-1": issue("A-1", 0, null, "done"), "A-2": issue("A-2", 0, null, "in_progress") });
    expect(rollUp(["A-1", "A-2", "A-3"], lookup)).toEqual({ new: 0, in_progress: 1, done: 1, unknown: 1 });
  });
});

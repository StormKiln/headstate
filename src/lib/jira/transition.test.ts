import { describe, expect, it } from "vitest";
import type { JiraIssue, TransitionResult } from "@/types/jira";
import { candidateStatuses, isStatusName, transitionArgv, transitionOutcome } from "./transition";

const issue = (status: string): JiraIssue => ({
  key: "ABC-1",
  summary: "s",
  issue_type: { name: "Story", hierarchy_level: 0 },
  status: { name: status, category: "in_progress" },
  assignee: null,
  parent_key: null,
  parent_checked: true,
  project_key: "ABC",
  url: null,
  updated: null,
});

const result = (over: Partial<TransitionResult>): TransitionResult => ({
  key: "ABC-1",
  requested: "In Review",
  reply: { state: "accepted", message: "Work item ABC-1 has been successfully transitioned to In Review" },
  after: issue("In Review"),
  reread_error: null,
  ...over,
});

describe("transitionArgv", () => {
  /// The same literal `transition_argv_is_the_literal_the_preview_shows`
  /// asserts in `cli.rs`: the preview is what runs.
  it("is exactly what the desktop runs", () => {
    expect(transitionArgv("ABC-1", "In Review")).toEqual([
      "jira",
      "workitem",
      "transition",
      "--key",
      "ABC-1",
      "--status",
      "In Review",
      "--yes",
      "--json",
    ]);
  });

  it("accepts only what the desktop will send", () => {
    expect(isStatusName("In Review")).toBe(true);
    expect(isStatusName("À faire")).toBe(true);
    for (const bad of ["", " ", "-x", "Done ", "a\tb", "x".repeat(256)]) expect(isStatusName(bad)).toBe(false);
  });
});

describe("candidateStatuses", () => {
  it("leaves out the status it is in, whatever its case", () => {
    const seen = [
      { name: "To Do", category: "new" as const },
      { name: "In Progress", category: "in_progress" as const },
      { name: "", category: "unknown" as const },
    ];
    expect(candidateStatuses(seen, "to do").map((s) => s.name)).toEqual(["In Progress"]);
    expect(candidateStatuses(seen, null).map((s) => s.name)).toEqual(["To Do", "In Progress"]);
  });
});

describe("transitionOutcome", () => {
  it("names the status Jira reports, not the one asked for", () => {
    expect(transitionOutcome(result({}))).toEqual({
      tone: "ok",
      text: "Jira now reports ABC-1 as In Review.",
      openInJira: false,
    });
    const elsewhere = transitionOutcome(result({ after: issue("In Testing") }));
    expect(elsewhere.tone).toBe("warn");
    expect(elsewhere.text).toBe("Jira accepted the move to In Review, but now reports ABC-1 as In Testing.");
  });

  it("never confirms a move it could not read back", () => {
    const o = transitionOutcome(
      result({ after: null, reread_error: { state: "timed_out", secs: 20 } }),
    );
    expect(o.tone).toBe("warn");
    expect(o.text).toContain("not confirmed");
    expect(o.text).not.toMatch(/now reports/);
    expect(o.openInJira).toBe(true);
  });

  it("shows Jira's reason for a refusal and offers Jira", () => {
    const o = transitionOutcome(
      result({
        requested: "Done",
        reply: { state: "refused", reason: "No allowed transitions found for given status" },
        after: issue("To Do"),
      }),
    );
    expect(o).toEqual({
      tone: "fail",
      text: "Jira refused to move ABC-1 to Done: No allowed transitions found for given status. Jira reports it as To Do.",
      openInJira: true,
    });
  });

  it("says when it cannot tell, and where Jira has it", () => {
    const o = transitionOutcome(
      result({ reply: { state: "unknown", error: { state: "timed_out", secs: 20 } }, after: issue("In Review") }),
    );
    expect(o.text).toBe("Could not tell whether Jira moved ABC-1: acli did not answer within 20s. Jira now reports it as In Review.");
    expect(o.tone).toBe("warn");
  });
});

import { describe, expect, it } from "vitest";
import { HELP_TOPICS } from "./topics";
import { DEFAULT_MERGE_GRACE_MINS } from "@/lib/jira/drift";
import { TRANSITION_PROGRAM, transitionArgv } from "@/lib/jira/transition";

/// The Jira help quotes commands and a default that live in code (#2049).
/// Help text is not rendered by any test that would notice them drifting,
/// so these pin each quotation to its source.
describe("the Jira help topic", () => {
  const body = HELP_TOPICS.jira.body;

  it("quotes the transition command the confirmation shows", () => {
    // `transitionArgv` asserts the same literal as Rust's `transition_args`,
    // so this ties the help to what actually runs. The status is quoted in
    // prose because it has a space, as a shell would need.
    const argv = transitionArgv("ABC-1", "In Review").map((a) => (a.includes(" ") ? `"${a}"` : a));
    expect(body).toContain([TRANSITION_PROGRAM, ...argv].join(" "));
  });

  it("states the grace period's real default", () => {
    expect(body).toContain(`${DEFAULT_MERGE_GRACE_MINS} minutes by default`);
  });

  it("gives the sign-in command the Settings remedy gives", () => {
    expect(body).toContain("acli jira auth login --web");
  });
});

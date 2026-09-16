import { describe, expect, it } from "vitest";
import type { ClaudeAgentTypes } from "@/types/pr";
import { statedSpawns, subagentDisagreement } from "./subagentDisagreement";

const types = (over: Partial<ClaudeAgentTypes> = {}): ClaudeAgentTypes => ({
  stated: [],
  untyped: 0,
  // Defaults to zero because that is what makes the fixture neutral:
  // every case below states the numbers it is about, and a non-zero
  // default would make the pre-hook case -- the common one -- the one no
  // test set up deliberately.
  inferred_children: 0,
  ...over,
});

describe("counting what the hook observed", () => {
  it("sums the stated types", () => {
    expect(
      statedSpawns(
        types({
          stated: [
            ["general-purpose", 2],
            ["code-reviewer", 1],
          ],
        }),
      ),
    ).toBe(3);
  });

  /// An untyped spawn is still a spawn. The hook fired and named no
  /// type, which is something we SAW -- dropping it would make the
  /// disagreement check blind to exactly the sessions whose payload
  /// shape moved.
  it("counts untyped spawns, which are spawns we saw", () => {
    expect(statedSpawns(types({ untyped: 2 }))).toBe(2);
    expect(statedSpawns(types({ stated: [["Explore", 1]], untyped: 2 }))).toBe(3);
  });

  it("is zero when the hook observed nothing", () => {
    expect(statedSpawns(types())).toBe(0);
  });
});

/// The asymmetry is the whole design, so it is asserted in both
/// directions rather than in the one that fires.
describe("the subagent disagreement", () => {
  it("fires when the hook saw spawns the directory rule did not find", () => {
    const note = subagentDisagreement(
      types({ stated: [["general-purpose", 3]], inferred_children: 0 }),
    );
    expect(note).toBeTruthy();
    expect(note).toMatch(/3 subagent starts/);
    expect(note).toMatch(/working directory/i);
  });

  it("says 'start' rather than 'starts' for one", () => {
    const note = subagentDisagreement(types({ stated: [["Explore", 1]] }));
    expect(note).toMatch(/1 subagent start\b/);
    expect(note).not.toMatch(/starts/);
  });

  /// An untyped spawn with no inferred children is still the
  /// disagreement: the claim in conflict is "this session spawned
  /// subagents at all", and the type is not part of it.
  it("fires on untyped spawns too", () => {
    expect(subagentDisagreement(types({ untyped: 1 }))).toBeTruthy();
  });

  /// **The direction that must stay silent.** This is every session that
  /// predates the hook install -- the entire corpus on the day this
  /// ships. A note here would flag all of it as contradictory, and a
  /// guard that fires everywhere is one that gets turned off.
  it("stays silent for a pre-hook session: inferred children, no hook spawns", () => {
    expect(subagentDisagreement(types({ inferred_children: 4 }))).toBe(null);
  });

  it("stays silent when both sources found subagents", () => {
    expect(
      subagentDisagreement(types({ stated: [["general-purpose", 2]], inferred_children: 2 })),
    ).toBe(null);
  });

  /// The counts need not MATCH for the sources to agree. `stated` counts
  /// spawn events and `inferred_children` counts child sessions, so a
  /// session that spawned five and left three traceable worktrees is not
  /// a contradiction -- only zero against non-zero is.
  it("stays silent when the two counts merely differ", () => {
    expect(
      subagentDisagreement(types({ stated: [["general-purpose", 5]], inferred_children: 3 })),
    ).toBe(null);
  });

  it("stays silent when neither source found anything", () => {
    expect(subagentDisagreement(types())).toBe(null);
  });

  /// `null` is "neither source has anything to say", which is an absence
  /// and not a disagreement.
  it("stays silent when there is no record at all", () => {
    expect(subagentDisagreement(null)).toBe(null);
  });
});

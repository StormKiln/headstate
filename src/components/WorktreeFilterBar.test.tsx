import { describe, expect, it } from "vitest";
import { WORKTREE_FACETS } from "./WorktreeFilterBar";
import { safetyReason } from "@/lib/worktrees";
import type { Safety } from "@/types/pr";

/// Every verdict `Safety` can carry, as the type declares them.
///
/// Written out rather than derived: the point is to fail when the union
/// gains a member, and a list derived from the union could not.
const ALL_KINDS = [
  "safe",
  "main_checkout",
  "dirty",
  "unpushed",
  "never_pushed",
  "merged_upstream_deleted",
  "merged_no_upstream",
  "detached_merged",
  "merged_as_pr",
  "unmerged",
  "locked",
  "prunable",
  "orphaned",
  "empty",
  "in_progress",
  "unknown",
] as const;

describe("the worktree facet list", () => {
  it("offers only verdicts that Safety actually carries", () => {
    // A facet for a verdict that does not exist is a button that can
    // only ever empty the list.
    for (const f of WORKTREE_FACETS) {
      expect(ALL_KINDS, f.kind).toContain(f.kind);
    }
  });

  it("never offers the main checkout as a facet", () => {
    // It is the repository, never a removal candidate, and
    // `matchesWorktreeFilters` exempts it from every filter -- so the
    // button could not narrow anything.
    expect(WORKTREE_FACETS.map((f) => f.kind)).not.toContain("main_checkout");
  });

  it("names each facet distinctly", () => {
    // Two buttons reading the same word filter to different rows, and
    // nothing on screen would say which is which.
    const labels = WORKTREE_FACETS.map((f) => f.label);
    expect(new Set(labels).size).toBe(labels.length);
  });

  it("lists each kind at most once", () => {
    const kinds = WORKTREE_FACETS.map((f) => f.kind);
    expect(new Set(kinds).size).toBe(kinds.length);
  });

  it("agrees with what the row says about the same verdict", () => {
    // A facet named one thing filtering to rows described as another is
    // how a user concludes the filter is broken. Checked loosely -- the
    // row's sentence is prose and the facet is a word -- but a facet
    // whose word appears nowhere in the verdict's own description is
    // worth flagging.
    const checked: [string, Safety][] = [
      ["safe", { kind: "safe" }],
      ["dirty", { kind: "dirty", detail: 2 }],
      ["unpushed", { kind: "unpushed", detail: 1 }],
      ["never_pushed", { kind: "never_pushed" }],
      ["merged_no_upstream", { kind: "merged_no_upstream" }],
      ["unmerged", { kind: "unmerged" }],
      ["merged_as_pr", { kind: "merged_as_pr", detail: 7 }],
    ];
    for (const [kind, safety] of checked) {
      const facet = WORKTREE_FACETS.find((f) => f.kind === kind);
      expect(facet, kind).toBeTruthy();
      const word = facet!.label.split(" ")[0].toLowerCase();
      expect(safetyReason(safety).toLowerCase(), kind).toContain(word);
    }
  });
});

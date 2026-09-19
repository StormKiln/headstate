import { describe, expect, it } from "vitest";
import { CLEANUP_GROUPS } from "./cleanupGroups";
import cleanupSource from "../../src-tauri/src/cleanup.rs?raw";

/// `pending` against what `propose` actually implements (#1141).
///
/// The label claims the automatic pass does nothing for a category. It
/// has to stop claiming that the moment it stops being true -- and a
/// flag maintained by hand in another language is exactly the kind that
/// drifts. Read from the Rust rather than duplicated here.
describe("the pending flags", () => {
  /// Which categories `propose` has a branch for.
  ///
  /// Parsed from `if prefs.<key> {` at the top level of `propose`,
  /// which is how every category is gated. Asserted non-empty below, so
  /// a spelling change cannot make this pass vacuously.
  const implemented = new Set(
    [...cleanupSource.matchAll(/^\s{4}if prefs\.(\w+) \{$/gm)].map((m) => m[1]),
  );

  it("found the proposer's own gates", () => {
    // Without this the regex silently matching nothing would make every
    // assertion below trivially true.
    expect(implemented.size).toBeGreaterThanOrEqual(4);
  });

  it("marks a category pending exactly when propose has no branch for it", () => {
    for (const g of CLEANUP_GROUPS) {
      const works = implemented.has(g.key);
      expect(
        g.pending !== true,
        works
          ? `${g.key} is implemented in propose but still labelled pending`
          : `${g.key} is labelled as working but propose has no branch for it`,
      ).toBe(works);
    }
  });

  it("has worktrees and branches working, and Docker still pending", () => {
    // The specific transition #1141 makes, pinned so a revert of the
    // Rust half without the label half fails here rather than shipping
    // a setting that silently does nothing.
    const byKey = new Map(CLEANUP_GROUPS.map((g) => [g.key, g]));
    expect(byKey.get("worktrees")?.pending).toBeUndefined();
    expect(byKey.get("branches")?.pending).toBeUndefined();
    expect(byKey.get("docker")?.pending).toBe(true);
  });
});

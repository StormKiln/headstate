import { describe, expect, it } from "vitest";
import { reclaimable } from "./worktrees";
import type { Worktree } from "../types/pr";

const wt = (over: Partial<Worktree>): Worktree => ({
  path: "/code/proj-a",
  branch: "feature",
  head: "abc",
  size_bytes: 1024,
  safety: { kind: "safe" },
  is_main: false,
  merged_at: null,
  upstream: null,
  last_commit: null,
  ...over,
});

/// The join #1181 is about: merge state and disk size, together.
describe("reclaimable", () => {
  it("is zero for an empty list", () => {
    expect(reclaimable([])).toEqual({ bytes: 0, unmeasured: 0 });
  });

  it("sums the sizes of the rows it is given", () => {
    const r = reclaimable([wt({ size_bytes: 10 }), wt({ size_bytes: 32 })]);
    expect(r.bytes).toBe(42);
    expect(r.unmeasured).toBe(0);
  });

  it("counts an unmeasured row rather than treating it as zero", () => {
    // THE honesty rule (#846, #1044). A worktree whose size could not
    // be read contributes nothing to the total, and without this count
    // the caller would present a floor as a total.
    const r = reclaimable([wt({ size_bytes: 100 }), wt({ size_bytes: null })]);
    expect(r.bytes).toBe(100);
    expect(r.unmeasured).toBe(1);
  });

  it("treats undefined the same as null", () => {
    // The field is optional in transit; a row that never carried a size
    // is as unmeasured as one whose measurement failed.
    const r = reclaimable([wt({ size_bytes: undefined as unknown as null })]);
    expect(r.bytes).toBe(0);
    expect(r.unmeasured).toBe(1);
  });

  it("does not re-derive the gate, so the caller's exclusions hold", () => {
    // It takes the ALREADY-GATED list. The page's gate is not only
    // safety -- it also excludes trees a Claude session is working in --
    // and a figure offering space the Remove button then refuses would
    // send the user round a loop (#770).
    //
    // Passing a row the page would never include proves this sums what
    // it is given rather than second-guessing it.
    const r = reclaimable([wt({ safety: { kind: "dirty", detail: 3 }, size_bytes: 7 })]);
    expect(r.bytes).toBe(7);
  });
});

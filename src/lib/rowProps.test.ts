import { describe, expect, it } from "vitest";
import { sameRowProps } from "./rowProps";

/// #1582: the row memo's comparator. The page builds each row as a fresh
/// spread every render, so the worktree must compare by its fields and
/// everything else by identity.
describe("sameRowProps", () => {
  const safety = { kind: "unmerged" };
  const onRemove = () => {};
  const props = () => ({ wt: { path: "/code/a", safety, size_bytes: 1 }, checked: false, onRemove });

  it("treats a re-spread worktree with the same fields as unchanged", () => {
    expect(sameRowProps(props(), props())).toBe(true);
  });

  it("sees a changed field on the worktree", () => {
    const b = props();
    b.wt.size_bytes = 2;
    expect(sameRowProps(props(), b)).toBe(false);
  });

  it("sees a verdict that re-arrived as a new object", () => {
    const b = props();
    b.wt.safety = { kind: "unmerged" };
    expect(sameRowProps(props(), b)).toBe(false);
  });

  it("sees a field added to the worktree", () => {
    const b = { ...props(), wt: { ...props().wt, sizeUnmeasurable: true } };
    expect(sameRowProps(props(), b)).toBe(false);
  });

  it("compares every other prop by identity", () => {
    expect(sameRowProps(props(), { ...props(), checked: true })).toBe(false);
    expect(sameRowProps(props(), { ...props(), onRemove: () => {} })).toBe(false);
  });
});

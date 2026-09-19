import { describe, expect, it } from "vitest";
import { matchesWorktreeFilters, worktreeFiltersActive } from "./worktrees";
import type { Worktree } from "../types/pr";

const wt = (over: Partial<Worktree> = {}): Worktree => ({
  path: "/code/proj/.worktrees/feature-a",
  branch: "feature/a",
  head: "abc",
  size_bytes: 1024,
  safety: { kind: "safe" },
  is_main: false,
  merged_at: null,
  upstream: null,
  last_commit: null,
  ...over,
});

const none = () => false;
const all = () => true;

describe("matchesWorktreeFilters", () => {
  it("keeps everything when no filter is set", () => {
    expect(matchesWorktreeFilters(wt(), {}, none)).toBe(true);
  });

  it("treats an EMPTY facet set as everything, not as nothing", () => {
    // What a user gets by unticking the last box. An empty list there
    // would read as "this repository has no worktrees" -- the confident
    // wrong answer #846 keeps having to remove.
    expect(matchesWorktreeFilters(wt(), { safety: [] }, none)).toBe(true);
  });

  it("narrows to the chosen verdicts", () => {
    expect(matchesWorktreeFilters(wt({ safety: { kind: "safe" } }), { safety: ["safe"] }, none)).toBe(true);
    expect(
      matchesWorktreeFilters(wt({ safety: { kind: "dirty", detail: 3 } }), { safety: ["safe"] }, none),
    ).toBe(false);
  });

  it("matches a verdict by KIND, whatever its payload", () => {
    // A user filtering for "dirty" wants all of them, not one file
    // count -- the verdicts carry payloads and a facet is a category.
    for (const detail of [1, 7, 400]) {
      expect(
        matchesWorktreeFilters(wt({ safety: { kind: "dirty", detail } }), { safety: ["dirty"] }, none),
      ).toBe(true);
    }
  });

  it("accepts any of several chosen verdicts", () => {
    const f = { safety: ["dirty", "unpushed"] };
    expect(matchesWorktreeFilters(wt({ safety: { kind: "unpushed", detail: 2 } }), f, none)).toBe(true);
    expect(matchesWorktreeFilters(wt({ safety: { kind: "safe" } }), f, none)).toBe(false);
  });

  it("never filters out the main checkout", () => {
    // It is the repository, not a removal candidate, and every count on
    // the page is stated relative to it. Hiding it would make the page
    // describe a repository it is not showing.
    const main = wt({ is_main: true, safety: { kind: "main_checkout" } });
    expect(matchesWorktreeFilters(main, { safety: ["dirty"] }, none)).toBe(true);
    expect(matchesWorktreeFilters(main, { worktreeQuery: "zzzz" }, none)).toBe(true);
    expect(matchesWorktreeFilters(main, { occupiedOnly: true }, none)).toBe(true);
  });

  it("matches the query against the path", () => {
    expect(matchesWorktreeFilters(wt(), { worktreeQuery: "feature-a" }, none)).toBe(true);
    expect(matchesWorktreeFilters(wt(), { worktreeQuery: "nothing" }, none)).toBe(false);
  });

  it("matches the query against the branch too", () => {
    // A user knows one or the other -- the directory they made it under
    // or the branch they have open -- and which is not predictable.
    expect(
      matchesWorktreeFilters(wt({ path: "/x/y", branch: "fix/login" }), { worktreeQuery: "login" }, none),
    ).toBe(true);
  });

  it("ignores case and surrounding whitespace in the query", () => {
    expect(matchesWorktreeFilters(wt(), { worktreeQuery: "  FEATURE-A " }, none)).toBe(true);
  });

  it("treats a whitespace-only query as no query", () => {
    // Otherwise clearing a search box by selecting-all and typing a
    // space empties the page.
    expect(matchesWorktreeFilters(wt(), { worktreeQuery: "   " }, none)).toBe(true);
  });

  it("narrows to occupied worktrees when asked", () => {
    expect(matchesWorktreeFilters(wt(), { occupiedOnly: true }, all)).toBe(true);
    expect(matchesWorktreeFilters(wt(), { occupiedOnly: true }, none)).toBe(false);
  });

  it("applies every filter together", () => {
    const w = wt({ safety: { kind: "dirty", detail: 2 }, path: "/code/proj/alpha" });
    expect(
      matchesWorktreeFilters(w, { safety: ["dirty"], worktreeQuery: "alpha", occupiedOnly: true }, all),
    ).toBe(true);
    // Any one of them failing is enough to exclude it.
    expect(
      matchesWorktreeFilters(w, { safety: ["dirty"], worktreeQuery: "alpha", occupiedOnly: true }, none),
    ).toBe(false);
    expect(
      matchesWorktreeFilters(w, { safety: ["safe"], worktreeQuery: "alpha", occupiedOnly: true }, all),
    ).toBe(false);
  });
});

describe("worktreeFiltersActive", () => {
  it("is false when nothing is set", () => {
    expect(worktreeFiltersActive({})).toBe(false);
    expect(worktreeFiltersActive({ safety: [], worktreeQuery: "", occupiedOnly: false })).toBe(false);
  });

  it("is false for a whitespace-only query", () => {
    // It narrows nothing, so claiming the list is filtered would put a
    // caveat on a complete list.
    expect(worktreeFiltersActive({ worktreeQuery: "  " })).toBe(false);
  });

  it("is true for any active filter", () => {
    expect(worktreeFiltersActive({ safety: ["safe"] })).toBe(true);
    expect(worktreeFiltersActive({ worktreeQuery: "x" })).toBe(true);
    expect(worktreeFiltersActive({ occupiedOnly: true })).toBe(true);
  });
});

import { describe, expect, it } from "vitest";

import { matches, segments } from "./findOverData";

const text = (s: ReturnType<typeof segments>) => s.map((x) => x.text).join("");
const hits = (s: ReturnType<typeof segments>) => s.filter((x) => x.hit).map((x) => x.text);

describe("segments", () => {
  /// The property that matters most: highlighting must never change
  /// what the row SAYS. A bug that dropped or duplicated a run would
  /// render a path the user does not have.
  it("always reassembles to the original value", () => {
    for (const [value, q] of [
      ["feat/1200-find", "1200"],
      ["MAIN", "main"],
      ["aaaa", "aa"],
      ["nothing here", "zzz"],
      ["edge", ""],
      ["", "x"],
    ] as const) {
      expect(text(segments(value, q))).toBe(value);
    }
  });

  it("matches case-insensitively and renders the ORIGINAL casing", () => {
    const s = segments("feat/MAIN-branch", "main");
    // Not "main": downcasing here would rewrite the user's branch name.
    expect(hits(s)).toEqual(["MAIN"]);
  });

  it("finds every occurrence, not just the first", () => {
    expect(hits(segments("src/src/lib", "src"))).toEqual(["src", "src"]);
  });

  /// Overlapping matches advance past the whole match rather than by
  /// one character, so `aa` in `aaaa` is two hits and not three.
  it("does not overlap matches", () => {
    expect(hits(segments("aaaa", "aa"))).toEqual(["aa", "aa"]);
    expect(text(segments("aaaa", "aa"))).toBe("aaaa");
  });

  it("treats the query as literal text, never a pattern", () => {
    // A regex built from the query would make these mean something else
    // -- or, for the unbalanced paren, throw.
    expect(hits(segments("a.c", "."))).toEqual(["."]);
    expect(hits(segments("abc", "."))).toEqual([]);
    expect(hits(segments("a*b", "*"))).toEqual(["*"]);
    expect(() => segments("f(x)", "(")).not.toThrow();
    expect(hits(segments("f(x)", "("))).toEqual(["("]);
  });

  /// An empty query is "no search", not "everything matches".
  it("returns one unhighlighted run for an empty or blank query", () => {
    expect(segments("anything", "")).toEqual([{ text: "anything", hit: false }]);
    expect(segments("anything", "   ")).toEqual([{ text: "anything", hit: false }]);
  });

  it("returns nothing for an empty value rather than an empty segment", () => {
    expect(segments("", "q")).toEqual([]);
  });

  it("trims the query the same way the filter does", () => {
    expect(hits(segments("main", "  main  "))).toEqual(["main"]);
  });
});

describe("matches", () => {
  it("agrees with segments about what is a hit", () => {
    for (const [value, q] of [
      ["feat/main", "MAIN"],
      ["feat/main", "zzz"],
      ["feat/main", ""],
      ["feat/main", "  main "],
    ] as const) {
      // The two must never disagree: a highlighted row that the count
      // did not count is the defect the module header describes.
      expect(matches(value, q)).toBe(hits(segments(value, q)).length > 0);
    }
  });

  it("is false for an absent field rather than throwing", () => {
    // `git_branch` and `opening_prompt` are genuinely nullable.
    expect(matches(null, "q")).toBe(false);
    expect(matches(undefined, "q")).toBe(false);
  });

  it("is false for an empty query, so no search highlights nothing", () => {
    expect(matches("anything", "")).toBe(false);
  });
});

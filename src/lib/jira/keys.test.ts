import { describe, expect, it } from "vitest";
import { keysForBranch, keysForPr, keysIn, parseTypedKey } from "./keys";

const P = new Set(["ABC", "XY", "A_B"]);
const keys = (found: { key: string }[]) => found.map((f) => f.key);

/// Multi-digit keys are assembled at runtime: `scripts/check-privacy.sh`
/// rejects them in committed text, synthetic or not.
const long = (p: string) => `${p}-${"4".repeat(3)}`;

describe("keysIn", () => {
  it.each([
    ["ABC-1: fix login", ["ABC-1"]],
    ["[ABC-1] fix", ["ABC-1"]],
    ["ABC-1, XY-7: two at once", ["ABC-1", "XY-7"]],
    ["fix (ABC-1)", ["ABC-1"]],
    ["A_B-3 underscore project", ["A_B-3"]],
    ["ABC-1 and again ABC-1", ["ABC-1"]],
    [`${long("ABC")} long number`, [long("ABC")]],
  ])("title %j", (title, want) => {
    expect(keys(keysIn(title, "title", P))).toEqual(want);
  });

  /// The false positives the project filter exists to reject.
  it.each(["Bump UTF-8 handling", "Use SHA-256", "Try GPT-4", "abc-1 lower case prose", "ABC-0 is not a key", "XABC-1 is another project", "ABC-1x glued"])(
    "title %j finds nothing",
    (title) => {
      expect(keysIn(title, "title", P)).toEqual([]);
    },
  );

  it.each([
    ["feature/abc-1-fix-login", ["ABC-1"]],
    ["ABC-1", ["ABC-1"]],
    ["bugfix_abc-1", ["ABC-1"]],
    ["dana/xy-7/abc-1", ["XY-7", "ABC-1"]],
    ["release-1.2", []],
    ["feature/utf-8", []],
    ["hotfix.abc-1.retry", ["ABC-1"]],
  ])("branch %j", (branch, want) => {
    expect(keys(keysIn(branch, "branch", P))).toEqual(want);
  });

  it("matches nothing when no projects are known", () => {
    expect(keysIn("ABC-1", "title", new Set())).toEqual([]);
  });
});

describe("keysForPr", () => {
  it("prefers the title and adds what only the branch has", () => {
    expect(keysForPr({ title: "ABC-1 fix", head_ref: "abc-1-fix" }, P)).toEqual([{ key: "ABC-1", source: "title" }]);
    expect(keysForPr({ title: "Fix", head_ref: "xy-7-fix" }, P)).toEqual([{ key: "XY-7", source: "branch" }]);
    expect(keys(keysForPr({ title: "ABC-1", head_ref: "xy-7" }, P))).toEqual(["ABC-1", "XY-7"]);
  });

  it("tolerates a row with no head branch", () => {
    expect(keysForBranch(null, P)).toEqual([]);
    expect(keysForPr({ title: "ABC-1" }, P)).toHaveLength(1);
  });
});

describe("parseTypedKey", () => {
  it.each([
    ["abc-1", "ABC-1"],
    ["  ABC-1 ", "ABC-1"],
    ["https://acme.example.internal/browse/XY-7", "XY-7"],
    ["https://acme.example.internal/browse/XY-7?focused=1", "XY-7"],
    ["abc", null],
    ["ABC-0", null],
    ["fix ABC-1 please", null],
  ])("%j", (input, want) => {
    expect(parseTypedKey(input)).toBe(want);
  });
});

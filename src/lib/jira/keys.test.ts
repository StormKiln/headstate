import { describe, expect, it } from "vitest";
import { parseTypedKey } from "./keys";

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

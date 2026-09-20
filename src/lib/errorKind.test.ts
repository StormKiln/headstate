import { describe, expect, it } from "vitest";

import { ERROR_KINDS, isCommandError, type CommandError, type ErrorKind } from "./errorKind";

describe("isCommandError", () => {
  it("accepts the shape the remote wire now sends", () => {
    const wire: unknown = JSON.parse(
      '{"kind":"not-asked","message":"headstate:not-asked not authenticated"}',
    );
    expect(isCommandError(wire)).toBe(true);
    if (isCommandError(wire)) {
      // The narrowing is the point: this does not compile without it.
      const kind: ErrorKind = wire.kind;
      const err: CommandError = wire;
      expect(kind).toBe("not-asked");
      expect(err.message).toContain("not authenticated");
    }
  });

  /// A kind outside the vocabulary means the two sides have DRIFTED, and
  /// guessing at that point is how a false classification reaches the
  /// screen. The caller falls back to reading the message, which is
  /// today's behaviour -- so rejecting is strictly safer than accepting.
  it("rejects a kind outside the asserted vocabulary", () => {
    expect(isCommandError({ kind: "throttled", message: "slow down" })).toBe(false);
  });

  it("rejects anything that is not the wire shape", () => {
    for (const v of [null, undefined, "boom", 7, [], {}, { message: "no kind" }, { kind: "other" }]) {
      expect(isCommandError(v), `accepted ${JSON.stringify(v) ?? "undefined"}`).toBe(false);
    }
  });

  it("accepts every kind in the vocabulary", () => {
    // Derived from ERROR_KINDS rather than listed, so a kind added to
    // both sides is covered here without anyone remembering to add it.
    for (const kind of ERROR_KINDS) {
      expect(isCommandError({ kind, message: "x" }), kind).toBe(true);
    }
  });
});

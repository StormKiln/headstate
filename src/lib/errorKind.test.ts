import { describe, expect, it } from "vitest";

import {
  ERROR_KINDS,
  commandError,
  isCommandError,
  type CommandError,
  type ErrorKind,
} from "./errorKind";
import { AUTH_EXPIRED, NOT_ASKED } from "./notAsked";

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

/// `commandError` is the client-side twin of Rust's
/// `CommandError::classify` (#1230): the ONE place this side reads the
/// marker, so no consumer has to test prose.
///
/// These cases moved here from `notAsked.test.ts` along with the code
/// they cover. The marker constant's agreement with Rust stays there,
/// because that is the assertion that reads Rust source.
describe("commandError", () => {
  it("recognises a rejection that carries the marker", () => {
    expect(commandError(new Error(`${NOT_ASKED} not authenticated`)).kind).toBe("not-asked");
  });

  /// The half that matters: a real failure must keep its retry. A
  /// timeout classified as `not-asked` would render `NotAskedNotice`,
  /// which withholds the retry -- so this direction being wrong takes
  /// away the remedy for a failure retrying would actually fix.
  it("does not claim a GitHub failure was never asked", () => {
    expect(commandError(new Error("request timed out after 60s")).kind).toBe("other");
    expect(commandError(new Error("401 Bad credentials")).kind).toBe("other");
  });

  /// The refused-token marker, and the property that makes it a type
  /// rather than the old regex relocated (#1230).
  ///
  /// `"401 Bad credentials"` above is unmarked prose, and it stays
  /// `"other"` -- which is the whole point. The deleted regex matched
  /// exactly that string and would have claimed it; this classifier
  /// only recognises what Rust marked, and Rust marks it from
  /// `octocrab::Error::GitHub`'s `status_code`. No sentence decides
  /// anything on this side any more.
  it("recognises a refused token only by its marker", () => {
    expect(commandError(new Error(`${AUTH_EXPIRED} GitHub rejected the token`)).kind).toBe(
      "expired-token",
    );
    // Unmarked prose that would have matched the old regex.
    expect(commandError(new Error("unauthorized")).kind).toBe("other");
    expect(commandError(new Error("GitHub request failed: GitHub")).kind).toBe("other");
    // And the prefix rule, as with `NOT_ASKED`.
    expect(commandError(`quoting ${AUTH_EXPIRED} in passing`).kind).toBe("other");
  });

  /// The two markers must not collide. `AUTH_EXPIRED` and `NOT_ASKED`
  /// are both `headstate:`-prefixed, and a prefix test on the shorter
  /// one would swallow the other if either were ever reworded into a
  /// prefix of its sibling.
  it("keeps the two markers distinct", () => {
    expect(AUTH_EXPIRED.startsWith(NOT_ASKED)).toBe(false);
    expect(NOT_ASKED.startsWith(AUTH_EXPIRED)).toBe(false);
    expect(commandError(`${NOT_ASKED} x`).kind).toBe("not-asked");
    expect(commandError(`${AUTH_EXPIRED} x`).kind).toBe("expired-token");
  });

  /// The marker is a PREFIX, matched as one -- the same rule
  /// `error_kind.rs`' own tests pin on the Rust side. Prose that merely
  /// quotes the marker later on is not a declined request.
  it("matches the marker as a prefix, not anywhere in the message", () => {
    expect(commandError(`the command replied with ${NOT_ASKED}`).kind).toBe("other");
  });

  /// The marker must never reach the screen: `cancelled.ts` exists
  /// because one did, and a user saw `headstate:cancelled` in a toast.
  it("strips the marker before the text is shown", () => {
    const err = commandError(new Error(`${NOT_ASKED} not authenticated: run \`gh auth login\``));
    expect(err.message).toBe("not authenticated: run `gh auth login`");
    expect(err.message).not.toContain(NOT_ASKED);
  });

  /// No classified message may carry the marker, whatever the kind.
  /// Asserted over both arms rather than the one that strips, so a
  /// future kind cannot quietly start leaking it.
  it("never leaves the marker in the message it returns", () => {
    for (const raw of [
      `${NOT_ASKED} not authenticated`,
      NOT_ASKED,
      "request timed out",
      "",
    ]) {
      expect(commandError(raw).message, raw).not.toContain(NOT_ASKED);
    }
  });

  /// A message with no marker passes through unchanged, so a caller can
  /// classify unconditionally -- which is what lets `QueryError` render
  /// `err.message` on every arm.
  it("leaves an unmarked message alone", () => {
    expect(commandError(new Error("request timed out")).message).toBe("request timed out");
    expect(commandError("request timed out").message).toBe("request timed out");
  });

  /// Tauri rejects a command's `Err(String)` as a bare string, but a
  /// transport-level failure rejects with an `Error`. Both reach this,
  /// and neither may render as "[object Object]".
  it("reads both shapes a rejected command throws", () => {
    expect(commandError("boom").message).toBe("boom");
    expect(commandError(new Error("boom")).message).toBe("boom");
    expect(commandError(undefined).message).toBe("undefined");
  });

  /// An already-typed rejection is taken at its word rather than having
  /// its prose re-read. Nothing on today's transports produces one, but
  /// honouring the shape is what lets a producer deliver a kind this
  /// side could not derive from prose at all.
  it("passes through a rejection that already carries a kind", () => {
    const typed: CommandError = { kind: "not-asked", message: "already classified" };
    expect(commandError(typed)).toBe(typed);
  });

  /// Every kind it returns is one the vocabulary asserted against Rust
  /// contains -- so a classification can never be a kind the Rust side
  /// has no variant for.
  it("only ever returns a kind in the asserted vocabulary", () => {
    for (const raw of [`${NOT_ASKED} x`, "boom", "", "401"]) {
      expect(ERROR_KINDS as readonly string[], raw).toContain(commandError(raw).kind);
      expect(isCommandError(commandError(raw)), raw).toBe(true);
    }
  });
});

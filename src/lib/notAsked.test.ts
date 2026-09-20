import { describe, expect, it } from "vitest";
// `?raw` rather than `node:fs`, for `mirroredConstants.test.ts:3`'s
// reason: the project carries no `@types/node`.
import commandsRs from "../../src-tauri/src/commands.rs?raw";
import { NOT_ASKED } from "./notAsked";

/// What is left here after #1230 is the cross-language agreement.
///
/// The BEHAVIOUR that used to live in this file -- recognising the
/// marker, stripping it before display -- moved to `commandError` in
/// `errorKind.test.ts`, because it moved in the source: every consumer
/// now reads a typed `kind` instead of re-testing the prose.
///
/// This assertion does not move and must not. It is the one that reads
/// the RUST source, and it is the reason the marker can be relied on at
/// all: a changed Rust constant with no TS counterpart is precisely the
/// drift `cancelled.ts` records, where every test on both sides passed
/// while the user saw marker syntax in a toast.
describe("the not-asked marker", () => {
  /// Reads the RUST source, so changing either side alone fails. A test
  /// comparing this module's constant to its own literal would pass at
  /// any value, which is the failure `cancelled.ts` documents.
  it("matches the Rust constant", () => {
    const m = commandsRs.match(/pub const NOT_ASKED: &str = "([^"]*)";/);
    expect(m, "commands.rs must define NOT_ASKED").toBeTruthy();
    expect(m![1]).toBe(NOT_ASKED);
  });

  /// Guards the guard: the regex above must actually have found a
  /// marker-shaped value. A pattern that silently matched something
  /// empty would make the assertion vacuous in the one direction it
  /// exists for.
  it("reads a plausible marker out of commands.rs", () => {
    expect(NOT_ASKED.length).toBeGreaterThan(0);
    expect(NOT_ASKED).toContain(":");
  });
});

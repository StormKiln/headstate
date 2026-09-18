import { describe, expect, it } from "vitest";
// `?raw` rather than `node:fs`, for `mirroredConstants.test.ts:3`'s
// reason: the project carries no `@types/node`.
import commandsRs from "../../src-tauri/src/commands.rs?raw";
import { NOT_ASKED, isNotAsked, notAskedMessage } from "./notAsked";

describe("the not-asked marker", () => {
  /// Reads the RUST source, so changing either side alone fails. A test
  /// comparing this module's constant to its own literal would pass at
  /// any value, which is the failure `cancelled.ts` documents.
  it("matches the Rust constant", () => {
    const m = commandsRs.match(/pub const NOT_ASKED: &str = "([^"]*)";/);
    expect(m, "commands.rs must define NOT_ASKED").toBeTruthy();
    expect(m![1]).toBe(NOT_ASKED);
  });

  it("recognises a rejection that carries it", () => {
    expect(isNotAsked(new Error(`${NOT_ASKED} not authenticated`))).toBe(true);
  });

  /// The half that matters: a real failure must keep its retry.
  it("does not claim a GitHub failure was never asked", () => {
    expect(isNotAsked(new Error("request timed out after 60s"))).toBe(false);
    expect(isNotAsked(new Error("401 Bad credentials"))).toBe(false);
  });

  it("strips the marker before the text is shown", () => {
    expect(notAskedMessage(new Error(`${NOT_ASKED} not authenticated: run \`gh auth login\``))).toBe(
      "not authenticated: run `gh auth login`",
    );
  });

  /// A message with no marker passes through unchanged, so a caller can
  /// use this unconditionally.
  it("leaves an unmarked message alone", () => {
    expect(notAskedMessage(new Error("request timed out"))).toBe("request timed out");
  });
});

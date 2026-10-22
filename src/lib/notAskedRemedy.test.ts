import { describe, expect, it } from "vitest";
import { AUTH_EXPIRED, NOT_ASKED, NOT_AUTHENTICATED } from "./notAsked";
import { notAskedRemedy, sourceBanner } from "./notAskedRemedy";

describe("notAskedRemedy (#2004)", () => {
  it("sends a missing credential to sign-in, never to a retry", () => {
    expect(notAskedRemedy(NOT_AUTHENTICATED)).toBe("sign-in");
  });

  it("offers nothing for a source that is not enabled", () => {
    expect(notAskedRemedy(": fetching is not enabled for this source")).toBe("none");
  });

  it("lets every admission refusal retry", () => {
    for (const reason of [
      "operation attempt allowance is spent",
      "provider cooldown is active",
      "read deadline elapsed before dispatch",
      "advisory consumer share is spent for this cycle",
    ]) {
      expect(notAskedRemedy(reason), reason).toBe("retry");
    }
  });
});

describe("sourceBanner (#2005)", () => {
  it("reads a marked rejection as not checked, with the marker stripped", () => {
    const line = sourceBanner(`${NOT_ASKED} provider cooldown is active`, false);
    expect(line).toEqual({ message: "not checked: provider cooldown is active", notAsked: "retry", remedy: "retry" });
  });

  it("takes the caller's verdict for an already-stripped message", () => {
    expect(sourceBanner(NOT_AUTHENTICATED, true)).toEqual({
      message: `not checked: ${NOT_AUTHENTICATED}`,
      notAsked: "sign-in",
      remedy: "sign-in",
    });
  });

  it("keeps a real failure a failure", () => {
    expect(sourceBanner("network down", false)).toEqual({ message: "network down", notAsked: null, remedy: "retry" });
    expect(sourceBanner(undefined, false)).toEqual({ message: "Could not refresh GitHub", notAsked: null, remedy: "retry" });
  });
});

/// #2008: a SENT request GitHub refused the token for is a failure whose
/// retry cannot work, so its line offers sign-in.
describe("sourceBanner, refused token (#2008)", () => {
  const REFUSED = "GitHub rejected the token: Bad credentials";

  it("reads the marker: a failure, sign-in, marker stripped", () => {
    expect(sourceBanner(`${AUTH_EXPIRED} ${REFUSED}`, false)).toEqual({ message: REFUSED, notAsked: null, remedy: "sign-in" });
  });

  it("takes the caller's verdict for an already-stripped message", () => {
    expect(sourceBanner(REFUSED, false, true)).toEqual({ message: REFUSED, notAsked: null, remedy: "sign-in" });
  });

  /// The wrong answer, not the missing one: a 403 rate or secondary limit
  /// clears after the reset, and sending its reader to re-authenticate
  /// would withhold the retry that works.
  it("keeps the retry for a rate limit", () => {
    const limited = "GitHub rate limit reached — polling will resume automatically (retry in 60 seconds)";
    expect(sourceBanner(limited, false).remedy).toBe("retry");
  });
});

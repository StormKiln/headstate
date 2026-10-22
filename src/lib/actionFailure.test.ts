import { describe, expect, it } from "vitest";
import { AUTH_EXPIRED } from "./notAsked";
import { actionFailure, signInRemedy } from "./notAskedRemedy";

describe("actionFailure (#2013)", () => {
  const refused = `${AUTH_EXPIRED} GitHub rejected the token: Bad credentials`;

  it("sends a write GitHub refused for the token to sign-in, marker stripped", () => {
    const desktop = actionFailure(refused, false);
    expect(desktop).toBe(`GitHub rejected the token: Bad credentials. ${signInRemedy(false)}`);
    expect(desktop).not.toContain(AUTH_EXPIRED);
    expect(actionFailure(refused, true)).toBe(
      `GitHub rejected the token: Bad credentials. ${signInRemedy(true)}`,
    );
  });

  it("leaves a 403 rate limit, and GitHub's other refusals, exactly as they read", () => {
    for (const message of [
      "GitHub request failed: You have exceeded a secondary rate limit",
      "Base branch was modified. Review and try the merge again.",
    ]) {
      expect(actionFailure(message, false)).toBe(message);
    }
    expect(actionFailure(undefined, false)).toBeUndefined();
  });
});

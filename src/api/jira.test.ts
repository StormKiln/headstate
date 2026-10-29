import { afterEach, describe, expect, it, vi } from "vitest";

import type { IssueBatch } from "@/types/jira";
import { NOT_ASKED } from "@/lib/notAsked";

const issues = vi.hoisted(() => vi.fn<(keys: string[]) => Promise<IssueBatch>>());
vi.mock("./tauri", () => ({ jiraIssues: issues }));

import { describeJiraError, failureOf, JiraFailure, loadIssue, rateLimitWaitMs } from "./jira";

const batch = (over: Partial<IssueBatch>): IssueBatch => ({
  issues: [],
  missing: [],
  unasked: [],
  error: null,
  truncated: false,
  ...over,
});

afterEach(() => {
  issues.mockReset();
  vi.restoreAllMocks();
});

describe("Jira failures (#2033)", () => {
  it("reads the desktop's typed rejection, and the phone's text one", () => {
    expect(failureOf({ state: "timed_out", secs: 20 })).toEqual({ state: "timed_out", secs: 20 });
    expect(failureOf(new JiraFailure({ state: "not_logged_in" }))).toEqual({ state: "not_logged_in" });
    // Text alone (a desktop older than #2047, through the phone): only
    // "not asked" survives, by its marker. `jira.remote.test.ts` has the rest.
    expect(failureOf(new Error(`${NOT_ASKED} Jira is switched off, so it was not asked`))).toEqual({
      state: "not_asked",
    });
    expect(failureOf("acli is not installed")).toEqual({ state: "failed", detail: "acli is not installed" });
    expect(failureOf({ state: "something else" })).toEqual({ state: "failed", detail: "[object Object]" });
  });

  /// #1050: "not asked" never reads as Jira not answering.
  it("says not asked as not asked", () => {
    const text = describeJiraError({ state: "not_asked" });
    expect(text).toMatch(/was not asked/);
    expect(text).not.toMatch(/did not answer|no answer|none|nothing found/i);
  });

  it("says when a rate limit will be retried, and admits when it does not know", () => {
    expect(describeJiraError({ state: "rate_limited", retry_after_secs: 9 })).toMatch(/ask again in 9s/);
    expect(describeJiraError({ state: "rate_limited", retry_after_secs: null })).toMatch(/did not say for how long/);
    expect(rateLimitWaitMs({ state: "rate_limited", retry_after_secs: 9 })).toBe(9_000);
    expect(rateLimitWaitMs({ state: "rate_limited", retry_after_secs: null })).toBe(60_000);
    expect(rateLimitWaitMs({ state: "timed_out", secs: 1 })).toBeNull();
    expect(rateLimitWaitMs(null)).toBeNull();
  });

  it("names where acli was looked for", () => {
    expect(describeJiraError({ state: "not_installed", searched: ["/a", "/b"] })).toMatch(/looked in \/a, \/b/);
  });

  /// After a rate limit, no key is sent until the wait is over: the asks
  /// are answered unasked, with the limit as the reason.
  it("holds every ask for as long as a rate limit asked", async () => {
    const limit = { state: "rate_limited", retry_after_secs: 10 } as const;
    const now = vi.spyOn(Date, "now").mockReturnValue(1_000);
    issues.mockResolvedValue(batch({ unasked: ["ABC-1"], error: limit }));
    expect(await loadIssue("ABC-1")).toEqual({ state: "unasked", why: limit });
    expect(issues).toHaveBeenCalledTimes(1);

    now.mockReturnValue(10_999);
    expect(await loadIssue("ABC-2")).toEqual({ state: "unasked", why: limit });
    expect(issues).toHaveBeenCalledTimes(1);

    now.mockReturnValue(11_000);
    issues.mockResolvedValue(batch({ missing: ["ABC-2"] }));
    expect(await loadIssue("ABC-2")).toEqual({ state: "missing" });
    expect(issues).toHaveBeenCalledTimes(2);
  });

  it("rejects a read that could not be asked with its typed reason", async () => {
    issues.mockRejectedValue({ state: "not_installed", searched: ["/x"] });
    const e = await loadIssue("ABC-3").catch((err: unknown) => err);
    expect(e).toBeInstanceOf(JiraFailure);
    expect(failureOf(e)).toEqual({ state: "not_installed", searched: ["/x"] });
  });
});

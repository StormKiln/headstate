import { beforeEach, describe, expect, it, vi } from "vitest";

import type { JiraError } from "@/types/jira";
import { NOT_ASKED } from "@/lib/notAsked";

const tauri = vi.hoisted(() => ({
  invoke: vi.fn<(cmd: string, args?: unknown) => Promise<unknown>>(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { remote } from "./remote";
import { describeJiraError, failureOf, JIRA_REASON } from "./jira";

/// A Jira command rejected over the phone's relay (#2047), as the
/// companion's `remote_call` rejects: with a string. A current desktop
/// sends the typed reason, which the companion puts behind `JIRA_REASON`
/// with the desktop's sentence; an older one sends the sentence alone.
const marked = (reason: unknown, message: string) => `${JIRA_REASON}${JSON.stringify({ reason, message })}`;

/// What `failureOf` makes of a `jira_issues` call the relay rejected with `text`.
async function overThePhone(text: string): Promise<JiraError> {
  tauri.invoke.mockRejectedValueOnce(text);
  try {
    await remote.call("jira_issues", { keys: ["ABC-1"], fresh: false });
  } catch (e) {
    return failureOf(e);
  }
  throw new Error("the call should have rejected");
}

beforeEach(() => tauri.invoke.mockReset());

describe("a Jira rejection through the phone's transport", () => {
  const REASONS: JiraError[] = [
    { state: "not_asked" },
    { state: "not_installed", searched: ["/opt/acli", "/usr/local/bin/acli"] },
    { state: "not_logged_in" },
    { state: "rate_limited", retry_after_secs: 30 },
    { state: "rate_limited", retry_after_secs: null },
    { state: "timed_out", secs: 20 },
    { state: "failed", detail: "acli exited 2" },
  ];

  it.each(REASONS)("keeps its typed reason: $state", async (reason) => {
    expect(await overThePhone(marked(reason, "the desktop's sentence"))).toEqual(reason);
    // It went over the relay, not the companion's own commands.
    expect(tauri.invoke.mock.calls[0][0]).toBe("remote_call");
  });

  /// A rate limit reaching the phone as a rate limit is what lets the
  /// loader hold off for as long as Jira asked, instead of retrying a
  /// "failure" at once.
  it("reads a rate limit as one, not as a failure with its text", async () => {
    const reason = await overThePhone(marked({ state: "rate_limited", retry_after_secs: 30 }, "x"));
    expect(describeJiraError(reason)).toBe("Jira is limiting how often it is asked. Headstate will ask again in 30s.");
  });

  /// An older desktop sends the sentence alone: "not asked" by its marker,
  /// anything else a failure with that sentence, exactly as before.
  it("degrades to the old reading from a desktop that sends text alone", async () => {
    expect(await overThePhone(`${NOT_ASKED} Jira is switched off, so it was not asked`)).toEqual({ state: "not_asked" });
    expect(await overThePhone("acli did not answer within 20s")).toEqual({
      state: "failed",
      detail: "acli did not answer within 20s",
    });
  });

  /// A state added on a newer desktop is never shown as JSON: its sentence reads.
  it("reads a state it does not know as a failure with the desktop's sentence", async () => {
    expect(await overThePhone(marked({ state: "on_fire" }, "Jira is on fire"))).toEqual({
      state: "failed",
      detail: "Jira is on fire",
    });
  });

  it("reads a marker with no JSON after it as a failure with that text", async () => {
    const text = `${JIRA_REASON}not json`;
    expect(await overThePhone(text)).toEqual({ state: "failed", detail: text });
  });
});

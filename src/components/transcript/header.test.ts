import { describe, expect, it } from "vitest";
import type { ClaudeWaiting, Liveness } from "../../types/pr";
import {
  abbreviatePath,
  compactCount,
  elapsedView,
  livenessView,
  subagentLiveness,
  waitingView,
} from "./header";

/// The sentence `liveness.rs` gives a row whose folder holds a live
/// process that published only a `.key` (#1315): `unknown`, naming the
/// pid and the folder. The header must take it as it is.
const KEY_ONLY: Liveness = {
  state: "unknown",
  why:
    "1 Claude Code session that did not record which session it is could be this one " +
    "(pid 4242, running in /work/app), so it cannot be called stopped",
};
const RUNNING: Liveness = { state: "running", pid: 4242, status: null };
const DEAD: Liveness = { state: "dead", why: "pid 4242 is no longer running" };
const clock = () => "09:41";

describe("livenessView (#1485)", () => {
  it("renders each liveness state distinctly", () => {
    const views = [
      livenessView(RUNNING, false),
      livenessView({ ...RUNNING, status: "idle" }, false),
      livenessView(RUNNING, true),
      livenessView(DEAD, false),
      livenessView(KEY_ONLY, false),
    ];
    expect(views.map((v) => v.label)).toEqual([
      "Running",
      "Running · idle",
      "Running, no transcript yet",
      "Stopped",
      "Could not tell whether it is running",
    ]);
    expect(new Set(views.map((v) => v.label)).size).toBe(views.length);
  });

  it("never renders unknown as stopped", () => {
    const v = livenessView({ state: "unknown", why: "could not read the registry" }, false);
    expect(v.tone).toBe("unknown");
    expect(v.label).not.toMatch(/stopped|not running/i);
  });

  it("a key-only session reads as possibly running, naming the pid and the folder", () => {
    const v = livenessView(KEY_ONLY, false);
    expect(v.tone).toBe("unknown");
    expect(v.label).not.toMatch(/stopped/i);
    expect(v.detail).toContain("pid 4242");
    expect(v.detail).toContain("/work/app");
  });

  it("a missing transcript is 'not yet' only for a running session", () => {
    expect(livenessView(RUNNING, true).label).toBe("Running, no transcript yet");
    expect(livenessView(DEAD, true).label).toBe("Stopped");
  });
});

describe("waitingView (#1485)", () => {
  const cases: [ClaudeWaiting, string][] = [
    [{ state: "now", kind: "idle_prompt", at: "2026-09-26T09:41:00Z" }, "Waiting for your input"],
    [
      { state: "now", kind: "permission_prompt", at: "2026-09-26T09:41:00Z" },
      "Waiting for your permission",
    ],
    [{ state: "now", kind: "elicitation", at: "2026-09-26T09:41:00Z" }, "Waiting: elicitation"],
    [
      { state: "last-seen", kind: "idle_prompt", at: "2026-09-26T09:41:00Z", why: "exited" },
      "Last seen waiting for your input at 09:41",
    ],
    [{ state: "no", reason: "superseded" }, "Not waiting for you"],
    [{ state: "no", reason: "never-observed" }, "Whether it is waiting is not recorded"],
  ];

  it.each(cases)("words %j as %s", (w, label) => {
    expect(waitingView(w, clock).label).toBe(label);
  });

  it("keeps 'not recorded' apart from 'not waiting'", () => {
    const unrecorded = waitingView({ state: "no", reason: "never-observed" }, clock);
    const moved = waitingView({ state: "no", reason: "not-a-prompt" }, clock);
    expect(unrecorded.tone).toBe("not-recorded");
    expect(moved.tone).toBe("not-waiting");
    expect(unrecorded.label).not.toBe(moved.label);
  });

  it("only the present-tense arm is 'now'", () => {
    const tones = cases.map(([w]) => waitingView(w, clock).tone);
    expect(tones.filter((t) => t === "now")).toHaveLength(3);
  });
});

describe("subagentLiveness (#1481, #1485)", () => {
  it("a running parent and an unfinished subagent is running", () => {
    expect(subagentLiveness(RUNNING, { status: "async_launched" })).toEqual({
      state: "running",
      pid: 4242,
      status: null,
    });
  });

  it("a completed subagent is stopped whatever its parent", () => {
    for (const parent of [RUNNING, DEAD, KEY_ONLY]) {
      expect(subagentLiveness(parent, { status: "completed" }).state).toBe("dead");
    }
  });

  it("an unfinished subagent of a stopped or unknown parent says so", () => {
    expect(subagentLiveness(DEAD, { status: null })).toMatchObject({ state: "dead" });
    const u = subagentLiveness(KEY_ONLY, { status: null });
    expect(u.state).toBe("unknown");
    expect(u.state === "unknown" && u.why).toContain("pid 4242");
  });
});

describe("abbreviatePath", () => {
  it("shortens the home directory and keeps the last two parts", () => {
    expect(abbreviatePath("/Users/someone/code/org/app")).toBe("~/…/org/app");
    expect(abbreviatePath("/home/someone/app")).toBe("~/app");
    expect(abbreviatePath("/opt/work/org/app")).toBe("…/org/app");
    expect(abbreviatePath("/srv/app")).toBe("/srv/app");
    expect(abbreviatePath("C:\\Users\\someone\\code\\org\\app")).toBe("~\\…\\org\\app");
  });
});

describe("elapsedView", () => {
  const start = "2026-09-26T08:00:00Z";
  it("a running session counts to now, from first seen", () => {
    expect(elapsedView(start, null, true, Date.parse("2026-09-26T10:05:00Z"))).toBe(
      "2 h 5 min since first seen",
    );
  });
  it("a stopped one counts to its last activity", () => {
    expect(elapsedView(start, "2026-09-26T08:30:00Z", false, 0)).toBe(
      "30 min from first seen to last activity",
    );
  });
  it("is absent rather than invented", () => {
    expect(elapsedView(start, null, false, 0)).toBeNull();
    expect(elapsedView("not a time", null, true, 1)).toBeNull();
    expect(elapsedView(start, null, true, 0)).toBeNull();
  });
});

describe("compactCount", () => {
  it("abbreviates large counts", () => {
    expect(compactCount(1234)).toBe((1234).toLocaleString());
    expect(compactCount(45_300)).toBe("45.3k");
    expect(compactCount(2_100_000)).toBe("2.1M");
  });
});

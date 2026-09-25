import { describe, expect, it } from "vitest";
import type { PrDetail, ReviewGates, ReviewThread } from "../types/pr";
import { LAST_PUSH_WONT_COUNT, LAST_PUSH_UNKNOWN, gateVerdict } from "./reviewGates";

const pr = (over: Partial<PrDetail> = {}): PrDetail =>
  ({
    author: "author",
    review_threads: [],
    review_threads_total: 0,
    unresolved_threads: 0,
    ...over,
  }) as PrDetail;

const thread = (resolved: boolean, outdated: boolean): ReviewThread => ({
  id: `RT_${Math.random()}`,
  is_resolved: resolved,
  is_outdated: outdated,
  path: "a.ts",
  line: outdated ? null : 1,
  viewer_can_reply: false,
  viewer_can_resolve: false,
  viewer_can_unresolve: false,
  comments: [],
  comment_count: 0,
});

const read = (lastPush: boolean, resolution: boolean): ReviewGates["rules"] => ({
  state: "read",
  require_last_push_approval: lastPush,
  required_review_thread_resolution: resolution,
});

describe("gateVerdict: last-push approval (#1451)", () => {
  it("warns that the viewer's approval won't count when the viewer pushed last", () => {
    const g: ReviewGates = { rules: read(true, false), last_pusher: { state: "known", login: "me" } };
    expect(gateVerdict(g, pr(), "me", false).approveWontCount).toBe(LAST_PUSH_WONT_COUNT);
  });

  it("says nothing when someone else pushed last", () => {
    const g: ReviewGates = { rules: read(true, false), last_pusher: { state: "known", login: "them" } };
    const v = gateVerdict(g, pr(), "me", false);
    expect(v.approveWontCount).toBeNull();
    expect(v.approveCaveat).toBeNull();
  });

  it("says nothing when no readable rule requires it", () => {
    const g: ReviewGates = { rules: read(false, false), last_pusher: { state: "not_needed" } };
    expect(gateVerdict(g, pr(), "me", false)).toEqual({
      approveWontCount: null,
      approveCaveat: null,
      mergeBlocked: null,
    });
  });

  /// Qualify, do not assert: the pusher could not be confirmed.
  it("qualifies rather than warns when the pusher is unknown or was not asked", () => {
    for (const last_pusher of [
      { state: "unknown", reason: "x" },
      { state: "declined", reason: "x" },
    ] as const) {
      const v = gateVerdict({ rules: read(true, false), last_pusher }, pr(), "me", false);
      expect(v.approveWontCount).toBeNull();
      expect(v.approveCaveat).toBe(LAST_PUSH_UNKNOWN);
    }
  });

  /// An unknown viewer is "we could not ask", never "it is you".
  it("never warns on an unknown viewer", () => {
    const g: ReviewGates = { rules: read(true, false), last_pusher: { state: "known", login: "me" } };
    expect(gateVerdict(g, pr(), undefined, false).approveWontCount).toBeNull();
  });

  /// GitHub refuses self-approval anyway, and ReviewBox already says so.
  it("adds nothing on the viewer's own pull request", () => {
    const g: ReviewGates = { rules: read(true, false), last_pusher: { state: "known", login: "me" } };
    const v = gateVerdict(g, pr({ author: "me" }), "me", false);
    expect(v.approveWontCount).toBeNull();
    expect(v.approveCaveat).toBeNull();
  });
});

describe("gateVerdict: rules not read render nothing new", () => {
  const bad: ReviewGates["rules"][] = [
    { state: "unreadable", reason: "404" },
    { state: "declined", reason: "budget" },
  ];
  it.each(bad)("$state", (rules) => {
    const g: ReviewGates = { rules, last_pusher: { state: "not_needed" } };
    const busy = pr({ review_threads: [thread(false, false)], review_threads_total: 1 });
    expect(gateVerdict(g, busy, "me", false)).toEqual({
      approveWontCount: null,
      approveCaveat: null,
      mergeBlocked: null,
    });
  });

  it("pending (no answer yet)", () => {
    expect(gateVerdict(undefined, pr(), "me", false).mergeBlocked).toBeNull();
  });
});

describe("gateVerdict: conversation resolution (#1454)", () => {
  const g: ReviewGates = { rules: read(false, true), last_pusher: { state: "not_needed" } };

  it("blocks merge on unresolved threads", () => {
    const p = pr({ review_threads: [thread(false, false), thread(true, false)], review_threads_total: 2 });
    expect(gateVerdict(g, p, "me", false).mergeBlocked).toBe(
      "1 conversation must be resolved first",
    );
  });

  /// Outdated unresolved threads still block, and are called out so the
  /// gate's number does not silently contradict the header's.
  it("counts outdated unresolved threads and says how many", () => {
    const p = pr({
      review_threads: [thread(false, false), thread(false, true), thread(false, true)],
      review_threads_total: 3,
      unresolved_threads: 1,
    });
    expect(gateVerdict(g, p, "me", false).mergeBlocked).toBe(
      "3 conversations must be resolved first (2 outdated)",
    );
  });

  it("says nothing when every thread is resolved", () => {
    const p = pr({ review_threads: [thread(true, true)], review_threads_total: 1 });
    expect(gateVerdict(g, p, "me", false).mergeBlocked).toBeNull();
  });

  /// A truncated list makes the count a floor.
  it("qualifies a count from a truncated thread list", () => {
    const p = pr({ review_threads: [thread(false, false)], review_threads_total: 150 });
    expect(gateVerdict(g, p, "me", false).mergeBlocked).toBe(
      "at least 1 conversation must be resolved first",
    );
  });

  /// The seeded placeholder has no threads; "0" would be invented.
  it("says nothing on the placeholder", () => {
    expect(gateVerdict(g, pr({ unresolved_threads: 4 }), "me", true).mergeBlocked).toBeNull();
  });
});

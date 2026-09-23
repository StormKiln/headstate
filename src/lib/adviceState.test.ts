import { describe, expect, it } from "vitest";
import { adviceState, needsRefresh, type AdviceQuery } from "./adviceState";
import type { ClaudeMdAdviceFreshness, ClaudeMdAdviceResult } from "@/types/pr";

const result = (freshness: ClaudeMdAdviceFreshness, tag = "x"): ClaudeMdAdviceResult => ({
  report: {
    repo: "/code/app",
    findings: [],
    checks: [{ check: "imports", run: { state: "ran", findings: 0 } }],
    brief: tag,
  },
  freshness,
  computedAt: "2026-01-01T00:00:00Z",
  build: "7.4.0",
});

const q = (over: Partial<AdviceQuery> = {}): AdviceQuery => ({
  data: undefined,
  isError: false,
  error: undefined,
  isFetching: false,
  enabled: true,
  ...over,
});

const OFF = q({ enabled: false });

describe("needsRefresh", () => {
  /// The ONE state a refresh is fired for unprompted: the backend has
  /// said a tracked input changed, so a better report exists.
  it("is true only for a stale cached report", () => {
    expect(needsRefresh(result({ state: "cached", stale: true }))).toBe(true);
    expect(needsRefresh(result({ state: "cached", stale: false }))).toBe(false);
    expect(needsRefresh(result({ state: "fresh", recomputed: true }))).toBe(false);
    expect(needsRefresh(result({ state: "fresh", recomputed: false }))).toBe(false);
  });

  /// `"unverified"` is the case that looks like it wants a refresh and
  /// must not get one automatically. An input that could not be READ
  /// will not read on a second run, so auto-firing there would spend a
  /// whole-body read of every session under the repository on every
  /// visit and learn nothing -- the cost #1290 exists to avoid.
  it("is false for unverified, whether or not the producers ran", () => {
    expect(needsRefresh(result({ state: "unverified", reason: "x: denied", recomputed: true }))).toBe(
      false,
    );
    expect(
      needsRefresh(result({ state: "unverified", reason: "x: denied", recomputed: false })),
    ).toBe(false);
  });

  it("is false with no result at all", () => {
    expect(needsRefresh(undefined)).toBe(false);
  });
});

describe("adviceState", () => {
  /// "Never asked" survives the move to a tab (#1290). It is reached only
  /// when NEITHER query is switched on, and it is not the same claim as
  /// "asked and came back with nothing".
  it("is idle only when no query is enabled", () => {
    expect(adviceState(OFF, OFF)).toEqual({ kind: "idle" });
  });

  /// And the moment the cached query is on, the absence of a report is
  /// BUILDING. This is the distinction the old `enabled` comment drew,
  /// carried over: a tab the user has not visited while a fetch is in
  /// flight is building, not empty.
  it("is building as soon as a query is enabled with no answer", () => {
    expect(adviceState(q(), OFF)).toEqual({ kind: "building" });
    expect(adviceState(q({ isFetching: true }), OFF)).toEqual({ kind: "building" });
  });

  it("shows a cached report that is not being refreshed", () => {
    const cached = result({ state: "cached", stale: false });
    expect(adviceState(q({ data: cached }), OFF)).toEqual({
      kind: "report",
      result: cached,
      refreshing: false,
    });
  });

  /// THE composed state. Neither half is invented: the backend observed
  /// `stale`, and this client observed its own fresh request in flight.
  it("composes from-cache-while-refreshing from the two queries", () => {
    const cached = result({ state: "cached", stale: true });
    expect(adviceState(q({ data: cached }), q({ isFetching: true }))).toEqual({
      kind: "report",
      result: cached,
      refreshing: true,
    });
  });

  /// A DISABLED fresh query is not a refresh in flight, whatever it
  /// reports for `isFetching`. Relying on TanStack reporting `false` for
  /// a disabled query would make an honest claim depend on a library
  /// detail.
  it("does not claim a refresh from a disabled fresh query", () => {
    const cached = result({ state: "cached", stale: true });
    const state = adviceState(q({ data: cached }), q({ enabled: false, isFetching: true }));
    expect(state).toEqual({ kind: "report", result: cached, refreshing: false });
  });

  /// The fresh answer is strictly newer, so it replaces the cached one
  /// the moment it lands -- and `refreshing` goes false with it.
  it("prefers the fresh report once it has landed", () => {
    const cached = result({ state: "cached", stale: true }, "old");
    const fresh = result({ state: "fresh", recomputed: true }, "new");
    expect(adviceState(q({ data: cached }), q({ data: fresh }))).toEqual({
      kind: "report",
      result: fresh,
      refreshing: false,
    });
  });

  /// A rejected first read is a failure with nothing underneath (#846),
  /// never an empty report.
  it("is a failure when the cached call was rejected with nothing served", () => {
    expect(adviceState(q({ isError: true, error: "boom" }), OFF)).toEqual({
      kind: "failed",
      error: "boom",
      stale: undefined,
    });
  });

  /// A rejected REFRESH keeps what was already served. Withdrawing a
  /// real answer because the attempt to better it failed would turn one
  /// failure into two -- but it is still reported as a failure rather
  /// than quietly re-labelled "from cache".
  it("reports a rejected refresh while keeping the served report", () => {
    const cached = result({ state: "cached", stale: true });
    expect(adviceState(q({ data: cached }), q({ isError: true, error: "panicked" }))).toEqual({
      kind: "failed",
      error: "panicked",
      stale: cached,
    });
  });

  /// A manual Refresh pressed during the first build can fail before the
  /// cached call has answered. That is a real failure with nothing
  /// underneath it, not a report and not a skeleton.
  it("is a bare failure when the fresh call fails before anything was served", () => {
    expect(adviceState(q(), q({ isError: true, error: "panicked" }))).toEqual({
      kind: "failed",
      error: "panicked",
      stale: undefined,
    });
  });

  /// Every state this surface must distinguish is reachable, and no two
  /// of them are the same value. The list is the issue's own, so a state
  /// quietly collapsed into another fails here.
  it("keeps all five states distinct", () => {
    const cached = result({ state: "cached", stale: true });
    const fresh = result({ state: "fresh", recomputed: true });
    const kinds = [
      adviceState(OFF, OFF),
      adviceState(q(), OFF),
      adviceState(q({ data: cached }), q({ isFetching: true })),
      adviceState(q({ data: fresh }), OFF),
      adviceState(q({ isError: true, error: "e" }), OFF),
    ];
    expect(kinds.map((k) => k.kind)).toEqual([
      "idle",
      "building",
      "report",
      "report",
      "failed",
    ]);
    // And the two reports differ in the fact that distinguishes them.
    expect(kinds[2]).toMatchObject({ refreshing: true });
    expect(kinds[3]).toMatchObject({ refreshing: false });
  });
});

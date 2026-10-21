import { describe, expect, it, vi } from "vitest";
import { ALL_VIEWS } from "../store/filters";

const setNeeds = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock("./tauri", async (orig) => ({
  ...(await orig<Record<string, unknown>>()),
  setViewNeedsGithub: setNeeds,
}));

const { useViewCadence } = await import("./hooks");
const { renderHook } = await import("@testing-library/react");

/// The views that render polled pull-request data (#1936). Spelled out
/// here rather than read from the hook, so the test checks the decision
/// instead of echoing it.
const SHOWS_GITHUB: ReadonlySet<string> = new Set(["my-prs", "to-review"]);

describe("useViewCadence", () => {
  // Every `View`, not a sample: the defect was a one-entry denylist that
  // gave every other view -- including ones added later -- the focused
  // GitHub cadence.
  it.each(ALL_VIEWS.map((v) => [v]))("asks for the fast cadence only on a PR view: %s", (view) => {
    setNeeds.mockClear();
    renderHook(() => useViewCadence(view));
    expect(setNeeds).toHaveBeenCalledTimes(1);
    expect(setNeeds).toHaveBeenCalledWith(SHOWS_GITHUB.has(view), view === "to-review");
  });

  it("names no view in the allowlist that does not exist", () => {
    for (const v of SHOWS_GITHUB) expect(ALL_VIEWS as readonly string[]).toContain(v);
  });

  // Cadence is an optimisation; failing to set it must never break the
  // page -- but the failure is logged, not swallowed.
  it("survives and logs a host without the command", async () => {
    setNeeds.mockClear();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    setNeeds.mockImplementationOnce(() => Promise.reject(new Error("no such command")));
    expect(() => renderHook(() => useViewCadence("my-prs"))).not.toThrow();
    await vi.waitFor(() => expect(warn).toHaveBeenCalled());
    warn.mockRestore();
  });
});

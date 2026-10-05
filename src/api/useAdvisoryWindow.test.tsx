import { act, cleanup, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { useAdvisoryWindow } from "./useAdvisoryWindow";
afterEach(() => { cleanup(); vi.useRealTimers(); });
const keys = Array.from({ length: 12 }, (_, i) => String(i).padStart(2, "0"));
const priority = new Set(keys.slice(0, 6));
function setup() {
  vi.useFakeTimers();
  const qc = new QueryClient();
  const states = new Map<string, { lastAdmittedAt?: number; hasContinuation?: boolean; resumeBoostSpent?: boolean; ineligible?: boolean }>();
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
  const view = renderHook(() => useAdvisoryWindow(keys, priority, true, new Set(keys), key => states.get(key)), { wrapper });
  return { states, view };
}
async function tick() { await act(async () => { await vi.advanceTimersByTimeAsync(30_000); }); }
it("keeps declined tail debt until an actual admission instead of treating selection as service", async () => {
  const { view, states } = setup();
  expect(view.result.current.selected.filter(key => !priority.has(key))).toEqual(["06", "07"]);
  await tick();
  expect(view.result.current.selected.filter(key => !priority.has(key))).toEqual(["06", "07"]);
  states.set("06", { lastAdmittedAt: 1 });
  await tick();
  expect(view.result.current.selected.filter(key => !priority.has(key))).toEqual(["07", "08"]);
});
it("allows only one unspent continuation boost while retaining an ordinary oldest tail slot", async () => {
  const { view, states } = setup();
  states.set("10", { lastAdmittedAt: 1, hasContinuation: true, resumeBoostSpent: false });
  states.set("11", { lastAdmittedAt: 2, hasContinuation: true, resumeBoostSpent: false });
  await tick();
  expect(view.result.current.selected.filter(key => !priority.has(key))).toEqual(["10", "06"]);
  states.set("10", { lastAdmittedAt: 3, hasContinuation: true, resumeBoostSpent: true });
  await tick();
  expect(view.result.current.selected.filter(key => !priority.has(key))).toEqual(["11", "06"]);
  states.set("11", { lastAdmittedAt: 4, hasContinuation: true, resumeBoostSpent: true });
  await tick();
  expect(view.result.current.selected.filter(key => !priority.has(key))).toEqual(["06", "07"]);
});
it("skips locally ineligible debt without increasing the finite eight-row window", async () => {
  const { view, states } = setup();
  states.set("06", { ineligible: true });
  states.set("07", { ineligible: true });
  await tick();
  expect(view.result.current.selected.filter(key => !priority.has(key))).toEqual(["08", "09"]);
  expect(view.result.current.selected).toHaveLength(8);
});

it("replacement identities inherit only their vacated slot until the next window", async () => {
  vi.useFakeTimers();
  const qc = new QueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
  const view = renderHook(({ population, preferred }) => useAdvisoryWindow(population, preferred, true), {
    wrapper, initialProps: { population: keys, preferred: priority },
  });
  const changed = [...keys.slice(1), "12"];
  const moved = new Set(["01", "02", "03", "04", "06", "07"]);
  view.rerender({ population: changed, preferred: moved });
  expect(view.result.current.selected).toEqual(["12", "01", "02", "03", "04", "05", "06", "07"]);
  expect([...view.result.current.preferred]).toEqual(["12", "01", "02", "03", "04", "05"]);
  expect(view.result.current.boosted.has("12")).toBe(false);
  await tick();
  expect([...view.result.current.preferred].sort()).toEqual([...moved].sort());
  expect(view.result.current.selected).toHaveLength(8);
});

import { act, renderHook, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
const events = vi.hoisted(() => ({ callback: undefined as undefined | ((event: { payload: [number, number] }) => void) }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (_name, callback) => { events.callback = callback; return () => {}; }),
}));
import { useUpdateAllProgress } from "./hooks";

it("resets stale cancelled progress for each run and retains terminal counts until completion", async () => {
  const { result, rerender } = renderHook(({ runId }) => useUpdateAllProgress(runId), { initialProps: { runId: 1 } });
  await waitFor(() => expect(events.callback).toBeDefined());
  act(() => events.callback!({ payload: [2, 5] }));
  expect(result.current).toEqual({ done: 2, total: 5 });
  rerender({ runId: 2 });
  expect(result.current).toBeNull();
  act(() => events.callback!({ payload: [0, 3] }));
  expect(result.current).toEqual({ done: 0, total: 3 });
  act(() => events.callback!({ payload: [3, 3] }));
  expect(result.current).toEqual({ done: 3, total: 3 });
  rerender({ runId: 3 });
  expect(result.current).toBeNull();
});

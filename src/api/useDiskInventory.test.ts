import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DiskHistory, DiskStatus } from "../types/disk";

/// #1965 removed the generation ref this hook kept beside `live`, whose
/// read in the effect cleanup was the lint warning. These pin what the two
/// together guaranteed, so dropping one cannot change what is on screen:
/// an answer from a run that has been superseded or switched off never
/// lands, and a started scan re-polls.

type Deferred<T> = { promise: Promise<T>; resolve: (value: T) => void };
function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}

const api = vi.hoisted(() => ({
  statuses: [] as Deferred<DiskStatus>[],
  started: null as DiskStatus | null,
}));
vi.mock("./diskInventory", () => ({
  diskStatus: () => {
    const next = deferred<DiskStatus>();
    api.statuses.push(next);
    return next.promise;
  },
  diskHistory: () => Promise.resolve<DiskHistory>({ observations: [], unreadable: 0 }),
  diskSettings: () => Promise.resolve({ external_roots: [], additional_locations: false }),
  saveDiskSettings: () => Promise.resolve(),
  startDiskScan: () => Promise.resolve(api.started),
  cancelDiskScan: () => Promise.resolve(),
}));

const { useDiskInventory } = await import("./useDiskInventory");

const status = (visited: number, running = false): DiskStatus => ({
  run_id: running ? "run" : null, running, visited, current_path: null, observations: [], error: null,
});

afterEach(() => {
  api.statuses = [];
  api.started = null;
});

describe("useDiskInventory", () => {
  it("ignores the superseded run's answer once a scan has started a new one", async () => {
    const { result } = renderHook(() => useDiskInventory(true));
    await act(async () => {});
    expect(api.statuses).toHaveLength(1);

    api.started = status(5, true);
    await act(async () => { await result.current.start(); });
    // `refresh` is a dependency: starting a scan re-runs the poll.
    expect(api.statuses).toHaveLength(2);
    const shown = result.current.status;

    // The first run's answer arrives late, and is older than what is shown.
    await act(async () => { api.statuses[0].resolve(status(1)); });
    expect(result.current.status).toBe(shown);

    await act(async () => { api.statuses[1].resolve(status(9)); });
    expect(result.current.status?.visited).toBe(9);
  });

  it("ignores an answer that lands after it was switched off", async () => {
    const { result, rerender } = renderHook(({ on }) => useDiskInventory(on), { initialProps: { on: true } });
    await act(async () => {});
    rerender({ on: false });
    await act(async () => { api.statuses[0].resolve(status(3, true)); });
    expect(result.current.status?.visited).not.toBe(3);
    // And a running answer from the dead run schedules no further poll.
    expect(api.statuses).toHaveLength(1);
  });
});

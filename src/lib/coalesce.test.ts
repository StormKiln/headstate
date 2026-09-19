import { describe, expect, it, vi } from "vitest";
import { createCoalescer, type Scheduler } from "./coalesce";

/// A scheduler the test drives by hand, so what is asserted is the
/// BATCHING rather than a delay.
function manual() {
  let pending: (() => void) | null = null;
  const schedule: Scheduler = (flush) => {
    pending = flush;
    return () => {
      pending = null;
    };
  };
  return {
    schedule,
    tick() {
      const f = pending;
      pending = null;
      f?.();
    },
    armed: () => pending !== null,
  };
}

/// #1150: ~295 worktrees meant ~295 full re-renders during the initial
/// burst, each rebuilding every derived row and re-sorting the list.
describe("the event coalescer", () => {
  it("commits a burst as one batch, not one commit per event", () => {
    const m = manual();
    const commit = vi.fn();
    const c = createCoalescer<number>(commit, m.schedule);

    for (const n of [1, 2, 3]) c.push(n);
    expect(commit).not.toHaveBeenCalled();

    m.tick();
    expect(commit).toHaveBeenCalledTimes(1);
    expect(commit).toHaveBeenCalledWith([1, 2, 3]);
  });

  /// Armed on the FIRST event of a burst. Re-arming per event would
  /// push the flush ever further out under a sustained stream, and the
  /// list would never settle.
  it("does not re-arm on every event", () => {
    const m = manual();
    const schedule = vi.fn(m.schedule);
    const c = createCoalescer<number>(() => {}, schedule);

    c.push(1);
    c.push(2);
    c.push(3);
    expect(schedule).toHaveBeenCalledTimes(1);
  });

  it("arms again for the next burst", () => {
    const m = manual();
    const commit = vi.fn();
    const c = createCoalescer<number>(commit, m.schedule);

    c.push(1);
    m.tick();
    c.push(2);
    m.tick();
    expect(commit).toHaveBeenCalledTimes(2);
    expect(commit).toHaveBeenLastCalledWith([2]);
  });

  /// An event arriving DURING a commit must land in the next batch
  /// rather than being dropped or counted twice.
  it("keeps an event that arrives during the commit", () => {
    const m = manual();
    const seen: number[][] = [];
    const c: ReturnType<typeof createCoalescer<number>> = createCoalescer<number>((batch) => {
      seen.push(batch);
      if (batch[0] === 1) c.push(2);
    }, m.schedule);

    c.push(1);
    m.tick();
    expect(seen).toEqual([[1]]);
    m.tick();
    expect(seen).toEqual([[1], [2]]);
  });

  /// An empty flush commits nothing: a scheduler that fires with an
  /// empty buffer must not trigger a render.
  it("commits nothing when the buffer is empty", () => {
    const m = manual();
    const commit = vi.fn();
    createCoalescer<number>(commit, m.schedule);
    m.tick();
    expect(commit).not.toHaveBeenCalled();
  });

  /// `stop` cancels a pending flush, so an unmounted page does not
  /// commit into a component that is gone.
  it("stops cleanly", () => {
    const m = manual();
    const commit = vi.fn();
    const c = createCoalescer<number>(commit, m.schedule);

    c.push(1);
    expect(m.armed()).toBe(true);
    c.stop();
    expect(m.armed()).toBe(false);
    m.tick();
    expect(commit).not.toHaveBeenCalled();
  });

  it("reports what is waiting", () => {
    const m = manual();
    const c = createCoalescer<number>(() => {}, m.schedule);
    c.push(1);
    c.push(2);
    expect(c.pending()).toBe(2);
    m.tick();
    expect(c.pending()).toBe(0);
  });
});

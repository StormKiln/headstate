import { describe, expect, it, vi } from "vitest";
import { createLimiter, withDeadline } from "./limiter";

/// A promise the test settles by hand, so "still in flight" is a state
/// the test holds rather than a race it hopes to catch.
function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("createLimiter", () => {
  it("runs at most N at once and starts the next as one settles", async () => {
    const limit = createLimiter(2);
    const started: string[] = [];
    const calls = ["a", "b", "c"].map((k) => {
      const d = deferred<string>();
      const p = limit.run(k, () => {
        started.push(k);
        return d.promise;
      });
      return { d, p };
    });
    expect(started).toEqual(["a", "b"]);
    calls[0].d.resolve("A");
    await expect(calls[0].p).resolves.toBe("A");
    await flush();
    expect(started).toEqual(["a", "b", "c"]);
    calls[1].d.resolve("B");
    calls[2].d.resolve("C");
    await expect(Promise.all([calls[1].p, calls[2].p])).resolves.toEqual(["B", "C"]);
  });

  /// A rejection frees the slot just as a success does, or every failed
  /// call would shrink the pool for the life of the page.
  it("frees the slot on a rejection, and on a synchronous throw", async () => {
    const limit = createLimiter(1);
    const failed = limit.run("a", () => Promise.reject(new Error("no")));
    const threw = limit.run("b", () => {
      throw new Error("sync");
    });
    const ok = limit.run("c", () => Promise.resolve("C"));
    await expect(failed).rejects.toThrow("no");
    await expect(threw).rejects.toThrow("sync");
    await expect(ok).resolves.toBe("C");
  });

  it("promote moves a queued key ahead of the others", async () => {
    const limit = createLimiter(1);
    const started: string[] = [];
    const gate = deferred<void>();
    const first = limit.run("first", () => {
      started.push("first");
      return gate.promise;
    });
    const rest = ["b", "c", "d"].map((k) =>
      limit.run(k, () => {
        started.push(k);
        return Promise.resolve();
      }),
    );
    limit.promote("d");
    // Promoting the running job, or a key nobody queued, changes nothing.
    limit.promote("first");
    limit.promote("nope");
    gate.resolve();
    await first;
    await Promise.all(rest);
    expect(started).toEqual(["first", "d", "b", "c"]);
  });

  it("is a pass-through when unbounded: every task starts synchronously", () => {
    const limit = createLimiter(Number.POSITIVE_INFINITY);
    const started: number[] = [];
    for (let i = 0; i < 50; i++) {
      void limit.run(String(i), () => {
        started.push(i);
        return new Promise<void>(() => {});
      });
    }
    expect(started).toHaveLength(50);
  });
});

describe("withDeadline", () => {
  it("rejects a promise that never settles once the deadline passes", async () => {
    vi.useFakeTimers();
    try {
      const p = withDeadline(new Promise<never>(() => {}), 1000, "too slow");
      const seen = p.catch((e: unknown) => e);
      await vi.advanceTimersByTimeAsync(999);
      let settled = false;
      void p.then(
        () => (settled = true),
        () => (settled = true),
      );
      await Promise.resolve();
      expect(settled).toBe(false);
      await vi.advanceTimersByTimeAsync(1);
      expect(((await seen) as Error).message).toBe("too slow");
    } finally {
      vi.useRealTimers();
    }
  });

  it("passes a settlement through unchanged, rejection included", async () => {
    await expect(withDeadline(Promise.resolve(7), 1000, "x")).resolves.toBe(7);
    await expect(withDeadline(Promise.reject("desktop said no"), 1000, "x")).rejects.toBe(
      "desktop said no",
    );
  });
});

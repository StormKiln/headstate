import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MAX_LINES, countLines, grammarFor, oversize } from "./highlightLangs";

/// `highlight.ts` keeps its worker in module state, so each test gets a
/// fresh copy of the module: a worker left alive by one test must not
/// answer for the next.
async function fresh() {
  vi.resetModules();
  return import("./highlight");
}

/// A worker that never answers: a grammar stuck on hostile input.
class HungWorker extends EventTarget {
  static made = 0;
  static terminated = 0;
  constructor() {
    super();
    HungWorker.made++;
  }
  postMessage() {}
  terminate() {
    HungWorker.terminated++;
  }
}

beforeEach(() => {
  HungWorker.made = 0;
  HungWorker.terminated = 0;
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("highlight: bounded by time", () => {
  // Prism's JS grammar takes seconds on a long unbroken identifier, well
  // under the size caps. Only killing the worker bounds that.
  it("kills a job at the deadline, and the next job gets a fresh worker", async () => {
    const { DEADLINE_MS, highlight } = await fresh();
    vi.useFakeTimers();
    vi.stubGlobal("Worker", HungWorker);

    const first = highlight("x".repeat(20_000), "javascript");
    await vi.advanceTimersByTimeAsync(DEADLINE_MS - 1);
    expect(HungWorker.terminated).toBe(0);
    await vi.advanceTimersByTimeAsync(1);
    await expect(first).resolves.toEqual({ kind: "too-slow" });
    expect(HungWorker.terminated).toBe(1);

    const second = highlight("y", "javascript");
    await vi.advanceTimersByTimeAsync(DEADLINE_MS);
    await expect(second).resolves.toEqual({ kind: "too-slow" });
    expect(HungWorker.made).toBe(2);
  });

  it("skips a job whose block went away before its turn", async () => {
    const { DEADLINE_MS, highlight } = await fresh();
    vi.useFakeTimers();
    vi.stubGlobal("Worker", HungWorker);
    const ahead = highlight("a", "json");
    const abort = new AbortController();
    const behind = highlight("b", "json", abort.signal);
    abort.abort();
    await vi.advanceTimersByTimeAsync(DEADLINE_MS);
    await expect(ahead).resolves.toEqual({ kind: "too-slow" });
    await expect(behind).resolves.toEqual({ kind: "skipped" });
    // Only the first job ever reached a worker.
    expect(HungWorker.made).toBe(1);
  });

  it("reports failure, not a hang, when the worker cannot start", async () => {
    const { highlight } = await fresh();
    class Broken extends EventTarget {
      postMessage() {
        queueMicrotask(() => this.dispatchEvent(new Event("error")));
      }
      terminate() {}
    }
    vi.stubGlobal("Worker", Broken);
    await expect(highlight("{}", "json")).resolves.toEqual({ kind: "failed" });
  });

  // The grammars never run on the main thread. With no worker there is
  // no highlighting at all, rather than a fallback that could hang it.
  it("never highlights on the main thread when there is no worker", async () => {
    const { highlight } = await fresh();
    vi.stubGlobal("Worker", undefined);
    await expect(highlight("{}", "json")).resolves.toEqual({ kind: "failed" });
  });

  it("does not submit a block over the size cap", async () => {
    const { highlight } = await fresh();
    vi.stubGlobal("Worker", HungWorker);
    await expect(highlight("a\n".repeat(MAX_LINES), "json")).resolves.toEqual({ kind: "skipped" });
    expect(HungWorker.made).toBe(0);
  });
});

describe("highlightLangs", () => {
  it("maps fence tags to grammars, and nothing for an unknown one", () => {
    expect(grammarFor("ts")).toBe("typescript");
    expect(grammarFor("RS")).toBe("rust");
    expect(grammarFor("sh")).toBe("bash");
    expect(grammarFor("constructor")).toBeNull();
    expect(grammarFor("brainfuck")).toBeNull();
    expect(grammarFor(undefined)).toBeNull();
  });

  it("caps by lines and by characters", () => {
    expect(oversize("a\n".repeat(MAX_LINES - 1) + "a")).toBeNull();
    expect(oversize("a\n".repeat(MAX_LINES))).toBe("lines");
    expect(oversize("x".repeat(60_001))).toBe("chars");
    expect(countLines("a\nb\n")).toBe(3);
  });
});

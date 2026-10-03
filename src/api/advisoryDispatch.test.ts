import { QueryClient } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { advisoryDispatch } from "./advisoryDispatch";

it("settles a thousand canceled queued requests while the native command is still unresolved", async () => {
  const qc = new QueryClient();
  let finish!: () => void;
  let dispatched = 0;
  const running = advisoryDispatch(qc, "running", "stack", new AbortController().signal,
    () => new Promise<void>(resolve => { dispatched++; finish = resolve; }));
  await Promise.resolve();
  let canceled = 0;
  const waiting = Array.from({ length: 1000 }, (_, i) => {
    const controller = new AbortController();
    const result = advisoryDispatch(qc, `replaced-${i}`, "detail", controller.signal, async () => { dispatched++; })
      .catch((error: DOMException) => { if (error.name === "AbortError") canceled++; });
    controller.abort();
    return result;
  });
  await Promise.resolve();
  const settledBeforeNativeCompletion = canceled;
  finish(); await running; await Promise.all(waiting);
  expect(settledBeforeNativeCompletion).toBe(1000);
  expect(dispatched).toBe(1);
});

it("bounds nonaborted pending work while admitting selected detail ahead of strip overflow", async () => {
  const qc = new QueryClient();
  let finish!: () => void;
  const dispatched: string[] = [];
  const deferred: string[] = [];
  const live = advisoryDispatch(qc, "running", "stack", new AbortController().signal,
    () => new Promise<void>(resolve => { dispatched.push("running"); finish = resolve; }));
  await Promise.resolve();
  const waiting = Array.from({ length: 80 }, (_, i) => {
    const key = `row-${i}`;
    return advisoryDispatch(qc, key, i % 2 ? "stack" : "pusher", new AbortController().signal, async () => { dispatched.push(key); })
      .catch((error: DOMException) => { expect(error.name).toBe("QuotaExceededError"); deferred.push(key); });
  });
  const selected = advisoryDispatch(qc, "selected", "detail", new AbortController().signal, async () => { dispatched.push("selected"); });
  await Promise.resolve();
  const deferredBeforeNativeCompletion = [...deferred];
  finish(); await live; await selected; await Promise.all(waiting);
  expect(deferredBeforeNativeCompletion).toHaveLength(17);
  expect(deferredBeforeNativeCompletion).toContain("row-63");
  expect(dispatched).toHaveLength(65);
  expect(dispatched.slice(0, 4)).toEqual(["running", "selected", "row-0", "row-1"]);
  expect(dispatched).not.toContain("row-63");
});

it("defers detail overflow when every pending slot already belongs to selected work", async () => {
  const qc = new QueryClient();
  let finish!: () => void;
  let performed = 0;
  let deferred = 0;
  const live = advisoryDispatch(qc, "running", "stack", new AbortController().signal,
    () => new Promise<void>(resolve => { finish = resolve; }));
  await Promise.resolve();
  const waiting = Array.from({ length: 65 }, (_, i) => advisoryDispatch(qc, `detail-${i}`, "detail", new AbortController().signal,
    async () => { performed++; }).catch((error: DOMException) => { expect(error.name).toBe("QuotaExceededError"); deferred++; }));
  await Promise.resolve();
  const deferredBeforeNativeCompletion = deferred;
  finish(); await live; await Promise.all(waiting);
  expect(deferredBeforeNativeCompletion).toBe(1);
  expect(performed).toBe(64);
});

it("rejects already-aborted input without dispatch and detaches listeners once work dispatches", async () => {
  const qc = new QueryClient();
  let performed = 0;
  const aborted = new AbortController(); aborted.abort();
  await expect(advisoryDispatch(qc, "aborted", "detail", aborted.signal, async () => { performed++; }))
    .rejects.toMatchObject({ name: "AbortError" });
  expect(performed).toBe(0);
  const live = new AbortController();
  const remove = vi.spyOn(live.signal, "removeEventListener");
  let finish!: (value: string) => void;
  const result = advisoryDispatch(qc, "live", "stack", live.signal, () => new Promise<string>(resolve => { finish = resolve; }));
  await Promise.resolve();
  live.abort();
  finish("native completion");
  await expect(result).resolves.toBe("native completion");
  expect(remove).toHaveBeenCalledWith("abort", expect.any(Function));
  remove.mockRestore();
});

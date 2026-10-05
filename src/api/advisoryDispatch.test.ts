import { QueryClient } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { advisoryDispatch, retireAdvisoryDispatch } from "./advisoryDispatch";

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

it("retains the four-cell cursor across exhausted drains and charges only actual current-call admission", async () => {
  const qc = new QueryClient();
  const admitted: string[] = [];
  const invoked: string[] = [];
  for (let window = 0; window < 4; window++) {
    let remaining = 2;
    // Tail observers enqueue first, just as newly selected queries do at a
    // window boundary. Existing visible queries follow in the same commit.
    const demands = ["tail-stack", "tail-pusher", "visible-stack", "visible-pusher"] as const;
    await Promise.all(demands.map(key => advisoryDispatch(qc, key, key.endsWith("stack") ? "stack" : "pusher", new AbortController().signal, async () => {
      invoked.push(key);
      const allowed = remaining-- > 0;
      if (allowed) admitted.push(key);
      return { outcome: allowed ? "offered" as const : "deferred" as const, admitted: allowed };
    }, { preferred: () => key.startsWith("visible"), current: () => true, progress: value => value })));
  }
  expect(admitted).toEqual(["visible-pusher", "tail-stack", "visible-stack", "tail-pusher", "visible-pusher", "tail-stack", "visible-stack", "tail-pusher"]);
  expect(invoked).toHaveLength(16);
});

it("settles cache-only cells finitely without spending the absent cell's next real turn", async () => {
  const qc = new QueryClient();
  const order: string[] = [];
  const enqueue = (key: string, admitted: boolean) => advisoryDispatch(qc, key, key.endsWith("stack") ? "stack" : "pusher", new AbortController().signal, async () => {
    order.push(key); return { outcome: "offered" as const, admitted };
  }, { preferred: () => key.startsWith("visible"), current: () => true, progress: value => value });
  await Promise.all([enqueue("tail-stack", true), enqueue("visible-pusher", false)]);
  await Promise.all([enqueue("tail-pusher", true), enqueue("visible-stack", true)]);
  expect(order).toEqual(["visible-pusher", "tail-stack", "visible-stack", "tail-pusher"]);
});

it("acknowledges a canceled current-owner native admission but not a retired response", async () => {
  const qc = new QueryClient();
  const controller = new AbortController();
  const acknowledge = vi.fn();
  let current = true;
  let finish!: (value: { outcome: "partial"; admitted: boolean }) => void;
  const run = () => advisoryDispatch(qc, "old", "pusher", controller.signal,
    () => new Promise<{ outcome: "partial"; admitted: boolean }>(resolve => { finish = resolve; }),
    { preferred: () => true, current: () => current, progress: value => value, acknowledge });
  const live = run(); await Promise.resolve(); controller.abort();
  finish({ outcome: "partial", admitted: true }); await live;
  expect(acknowledge).toHaveBeenCalledTimes(1);
  const retired = advisoryDispatch(qc, "retired", "stack", new AbortController().signal,
    () => new Promise<{ outcome: "partial"; admitted: boolean }>(resolve => { finish = resolve; }),
    { preferred: () => false, current: () => current, progress: value => value, acknowledge });
  await Promise.resolve(); current = false;
  finish({ outcome: "partial", admitted: true }); await retired;
  expect(acknowledge).toHaveBeenCalledTimes(1);
  const order: string[] = [];
  await Promise.all(["visible-stack", "tail-stack"].map(key => advisoryDispatch(qc,key,"stack",new AbortController().signal,async()=>{
    order.push(key);return { outcome: "offered" as const, admitted: true };
  },{preferred:()=>key.startsWith("visible"),current:()=>true,progress:value=>value})));
  expect(order).toEqual(["tail-stack","visible-stack"]);
});

it("retirement between promise resolution and drain continuation cannot move the replacement cursor", async () => {
  const qc = new QueryClient();
  let finish!: (value: { outcome: "offered"; admitted: boolean }) => void;
  const first = advisoryDispatch(qc, "retiring", "pusher", new AbortController().signal,
    () => new Promise<{ outcome: "offered"; admitted: boolean }>(resolve => { finish = resolve; }),
    { preferred: () => true, current: () => true, progress: value => value });
  const retired = first.then(() => retireAdvisoryDispatch(qc));
  await Promise.resolve(); finish({ outcome: "offered", admitted: true }); await retired;
  const order: string[] = [];
  await Promise.all(["tail-stack", "visible-pusher"].map(key => advisoryDispatch(qc,key,key.endsWith("stack")?"stack":"pusher",new AbortController().signal,async()=>{
    order.push(key); return { outcome: "offered" as const, admitted: true };
  },{preferred:()=>key.startsWith("visible"),current:()=>true,progress:value=>value})));
  expect(order).toEqual(["visible-pusher", "tail-stack"]);
});

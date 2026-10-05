import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { afterEach, expect, it, vi } from "vitest";

const pending = vi.hoisted(() => [] as { resolve: (value: unknown) => void }[]);
const series = vi.hoisted(() => vi.fn(() => new Promise<unknown>(resolve => pending.push({ resolve }))));
vi.mock("./tauri", async original => ({ ...await original<Record<string, unknown>>(), statsSeries: series }));
import { useStatsSeries } from "./hooks";

afterEach(() => { cleanup(); pending.length = 0; series.mockClear(); });
const scope = { kind: "org" as const, value: "synthetic-org", subject: undefined };
function harness() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  return { client, wrapper };
}

it("shares active stats work and permits bounded native cache warming after the final observer leaves", async () => {
  const { client, wrapper } = harness();
  const one = renderHook(() => useStatsSeries(scope, 30, true), { wrapper });
  const two = renderHook(() => useStatsSeries(scope, 30, true), { wrapper });
  await waitFor(() => expect(series).toHaveBeenCalledTimes(1));
  const query = client.getQueryCache().getAll()[0];
  expect(query.getObserversCount()).toBe(2);
  one.unmount();
  expect(query.getObserversCount()).toBe(1);
  expect(query.state.fetchStatus).toBe("fetching");
  two.unmount();
  expect(query.getObserversCount()).toBe(0);
  // Native command admission, tested with real HTTP in commands.rs, keeps
  // this bounded warming in the Background lane after navigation.
  await act(async () => pending[0].resolve({ points: [] }));
  expect(query.state.status).toBe("success");
  client.clear();
});

it("rapid scopes stay distinct and a retired owner's late answer cannot repopulate the new query", async () => {
  const { client, wrapper } = harness();
  const old = renderHook(({ days }) => useStatsSeries(scope, days, true), { wrapper, initialProps: { days: 30 } });
  old.rerender({ days: 60 });
  await waitFor(() => expect(series).toHaveBeenCalledTimes(2));
  old.unmount();
  await client.cancelQueries();
  client.clear();
  renderHook(() => useStatsSeries(scope, 30, true), { wrapper });
  await waitFor(() => expect(series).toHaveBeenCalledTimes(3));
  await act(async () => { pending[0].resolve({ owner: "retired" }); pending[1].resolve({ owner: "retired" }); });
  expect(client.getQueryCache().getAll()).toHaveLength(1);
  expect(client.getQueryCache().getAll()[0].state.data).toBeUndefined();
  await act(async () => pending[2].resolve({ owner: "current", points: [] }));
  expect(client.getQueryCache().getAll()[0].state.data).toEqual({ owner: "current", points: [] });
  client.clear();
});

import { describe, expect, it, vi } from "vitest";
import { renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";

/// #1965: `usePrDetail` kept its mergeability backoff in
/// `useMemo(() => new DetailPollBackoff(), [repo, number])`, deps the body
/// never read. The backoff now lives in state keyed by the identity. What
/// the deps were FOR is pinned here: one backoff per pull request, kept
/// across re-renders, replaced when the pull request changes -- and the
/// poll interval reads the current one.

const invoke = vi.hoisted(() => vi.fn(() => new Promise<never>(() => {})));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

const made = vi.hoisted(() => [] as { delay: ReturnType<typeof vi.fn> }[]);
vi.mock("./detailPolling", () => ({
  DetailPollBackoff: class {
    delay = vi.fn(() => false as const);
    constructor() { made.push(this); }
  },
}));

const { usePrDetail } = await import("./hooks");

describe("usePrDetail's backoff", () => {
  it("is one per pull request: kept across renders, replaced on a new identity", () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={qc}>{children}</QueryClientProvider>
    );
    const { rerender } = renderHook(({ repo, number }) => usePrDetail(repo, number), {
      wrapper,
      initialProps: { repo: "o/r", number: 7 },
    });
    expect(made).toHaveLength(1);

    rerender({ repo: "o/r", number: 7 });
    rerender({ repo: "o/r", number: 7 });
    expect(made).toHaveLength(1);

    rerender({ repo: "o/r", number: 8 });
    expect(made).toHaveLength(2);
    rerender({ repo: "o/other", number: 8 });
    expect(made).toHaveLength(3);
    rerender({ repo: "o/other", number: 8 });
    expect(made).toHaveLength(3);

    // The live query's interval asks the CURRENT pull request's backoff.
    const query = qc.getQueryCache().find({ queryKey: ["pr-detail", "o/other", 8], exact: true })!;
    const interval = query.observers[0].options.refetchInterval as (q: typeof query) => unknown;
    const calls = () => made.map((b) => b.delay.mock.calls.length);
    const before = calls();
    interval(query);
    expect(calls()).toEqual([before[0], before[1], before[2] + 1]);
  });
});

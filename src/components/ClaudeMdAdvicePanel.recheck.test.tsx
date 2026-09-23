import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ClaudeMdAdviceResult } from "@/types/pr";

/// Re-check against the REAL query hooks (#1343).
///
/// The panel test stubs the hooks, so it can prove `refetch` is called
/// but not that a run starts: the defect was TanStack serving its cache
/// (`staleTime: 30_000`) or leaving an already-enabled query alone. Here
/// the command itself is the stub, and what is counted is how many times
/// the backend was asked for a fresh run.
const advice = vi.hoisted(() => vi.fn());

vi.mock("@/api/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/api/tauri")>()),
  claudeMdAdvice: advice,
  getUiPrefs: () => Promise.resolve({ terminal_command: "" }),
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

import { ClaudeMdAdvicePanel } from "./ClaudeMdAdvicePanel";
import { useFilters } from "@/store/filters";

const REPO = "/home/octocat/hello-world";

const result = (stale: boolean): ClaudeMdAdviceResult => ({
  report: {
    repo: REPO,
    findings: [],
    checks: [{ check: "imports", run: { state: "ran", findings: 0 } }],
    brief: "",
  },
  freshness: stale ? { state: "cached", stale: true } : { state: "fresh", recomputed: true },
  computedAt: "2026-01-01T00:00:00Z",
  build: "7.4.0",
});

const freshRuns = () => advice.mock.calls.filter((c) => c[1] === "fresh").length;

function mount() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <ClaudeMdAdvicePanel repo={REPO} activePath={undefined} onSelectFile={vi.fn()} />
    </QueryClientProvider>,
  );
}

async function recheck() {
  const button = await screen.findByRole("button", { name: "Re-check" });
  fireEvent.click(button);
}

beforeEach(() => {
  advice.mockReset();
  useFilters.setState({ filtersByView: {} as never, view: "claude-md" });
});

describe("Re-check", () => {
  /// Over a stale report the fresh run fires on its own; the first click
  /// after it must start ANOTHER run, not set a flag that changes nothing.
  it("starts a run on the first click over a stale report", async () => {
    advice.mockImplementation((_repo: string, mode: string) =>
      Promise.resolve(result(mode === "cached")),
    );
    mount();
    await waitFor(() => expect(freshRuns()).toBe(1));
    await recheck();
    await waitFor(() => expect(freshRuns()).toBe(2));
  });

  /// Two clicks inside `staleTime` are two runs.
  it("starts a run on each of two clicks within 30 seconds", async () => {
    advice.mockImplementation(() => Promise.resolve(result(false)));
    mount();
    await recheck();
    await waitFor(() => expect(freshRuns()).toBe(1));
    await recheck();
    await waitFor(() => expect(freshRuns()).toBe(2));
  });
});

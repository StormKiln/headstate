import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  InstalledPlugin,
  PluginDayCount,
  PluginUsage,
  PluginsReport,
} from "../types/pr";

const state = vi.hoisted(() => ({
  data: undefined as PluginsReport | undefined,
  loading: false,
  failed: false,
}));
const refetchFn = vi.hoisted(() => vi.fn());

vi.mock("../api/hooks", () => ({
  useClaudePlugins: () => ({
    data: state.data,
    isLoading: state.loading,
    isError: state.failed,
    error: "permission denied reading ~/.claude",
    refetch: refetchFn,
  }),
}));

// recharts needs a measured container jsdom does not give it, so the real
// chart draws an empty box. Stubbed so this file tests the PAGE: which
// panels appear, and what each says when a reading is absent.
vi.mock("./stats/PluginCallsChart", () => ({
  PluginCallsChart: ({ points, days }: { points: PluginDayCount[]; days: number }) => (
    <div data-testid="plugin-calls-chart" data-points={points.length} data-days={days} />
  ),
}));

import { ClaudePluginsPage } from "./ClaudePluginsPage";

function plugin(over: Partial<InstalledPlugin> = {}): InstalledPlugin {
  return {
    name: "playwright",
    marketplace: "claude-plugins-official",
    scope: "user",
    version: "1.0.0",
    install_path: "/cache/playwright/1.0.0",
    installed_at: "2026-01-01T00:00:00Z",
    last_updated: "2026-01-01T00:00:00Z",
    contribution: { mcp: true, skills: false, agents: false, commands: false, read: true },
    ...over,
  };
}

function usage(over: Partial<PluginUsage> = {}): PluginUsage {
  return {
    name: "playwright",
    mcp_calls: 0,
    skill_calls: 0,
    agent_calls: 0,
    command_calls: 0,
    failures: 0,
    last_called_at: null,
    measured: true,
    ...over,
  };
}

function window30(): PluginDayCount[] {
  return Array.from({ length: 30 }, (_, i) => ({
    day: `2026-09-${String(i + 1).padStart(2, "0")}`,
    calls: 0,
  }));
}

function report(over: Partial<PluginsReport> = {}): PluginsReport {
  return {
    installed: [],
    usage: [],
    activity: window30(),
    unreadable: [],
    inventory_failure: null,
    inventory_absent: false,
    scanned: 0,
    elapsed_ms: 12,
    ...over,
  };
}

beforeEach(() => {
  state.data = undefined;
  state.loading = false;
  state.failed = false;
  refetchFn.mockReset();
});
afterEach(cleanup);

describe("ClaudePluginsPage", () => {
  /// An installed-but-never-invoked plugin renders "no calls recorded",
  /// never a bare "0".
  ///
  /// The issue's second required test. The page's output is the input to
  /// "should I uninstall this?", so a zero we did not measure argues for
  /// discarding something the user may rely on.
  it("says 'no calls recorded' for a plugin with no reading, never a bare 0", () => {
    state.data = report({
      installed: [plugin({ name: "rust-analyzer-lsp" })],
      // `measured: false` is the state a plugin installed after the last
      // scan is in: we have no reading at all.
      usage: [usage({ name: "rust-analyzer-lsp", measured: false })],
    });
    render(<ClaudePluginsPage />);

    expect(screen.getByText(/no calls recorded/i)).toBeTruthy();

    // And no bare zero anywhere in that row's call cell. A "0" here is
    // the specific lie this test exists to prevent.
    const row = screen.getByText("rust-analyzer-lsp").closest("tr");
    expect(row).toBeTruthy();
    const cells = Array.from(row?.querySelectorAll("td") ?? []).map((c) => c.textContent ?? "");
    expect(cells.some((t) => t.trim() === "0")).toBe(false);
  });

  /// A plugin whose contribution is not tool-shaped is not called unused.
  ///
  /// The issue's third required test. `clangd-lsp` ships only a LICENSE
  /// and a README: it contributes background behaviour and will never
  /// produce a tool call, so "0 calls" would be true and misleading.
  it("does not present an LSP-shaped plugin as unused", () => {
    state.data = report({
      installed: [
        plugin({
          name: "clangd-lsp",
          contribution: {
            mcp: false,
            skills: false,
            agents: false,
            commands: false,
            read: true,
          },
        }),
      ],
      usage: [usage({ name: "clangd-lsp", measured: true })],
    });
    render(<ClaudePluginsPage />);

    const row = screen.getByText("clangd-lsp").closest("tr");
    const text = row?.textContent ?? "";
    expect(text).toMatch(/nothing here is counted/i);
    expect(text).not.toMatch(/\bunused\b/i);
    // It must not say "no calls" either -- that frames an expected
    // absence as a disappointing one.
    expect(text).not.toMatch(/no calls\b/i);
  });

  /// A measured zero on a countable plugin IS said plainly.
  ///
  /// The other side of the test above: the page must not become so
  /// cautious that it refuses to report a real finding. "7 of 22 plugins
  /// show any usage" is the point of the page.
  it("says 'no calls' for a countable plugin measured at zero", () => {
    state.data = report({
      installed: [plugin({ name: "context7" })],
      usage: [usage({ name: "context7", measured: true })],
    });
    render(<ClaudePluginsPage />);
    const row = screen.getByText("context7").closest("tr");
    expect(row?.textContent ?? "").toMatch(/no calls/i);
  });

  /// An unreadable transcript makes the totals a floor, and says so.
  ///
  /// The issue's fourth required test. The counts that DID read are
  /// still shown -- partial is not nothing -- but every figure is
  /// qualified and the paths are named.
  it("qualifies every count as a floor when a transcript could not be read", () => {
    state.data = report({
      installed: [plugin()],
      usage: [usage({ mcp_calls: 44, measured: true })],
      unreadable: ["/Users/x/.claude/projects/p/s.jsonl: permission denied"],
    });
    render(<ClaudePluginsPage />);

    // The notice names the path and the reason.
    const alerts = screen.getAllByRole("alert").map((a) => a.textContent ?? "");
    expect(alerts.some((t) => t.includes("s.jsonl"))).toBe(true);
    expect(alerts.some((t) => /permission denied/i.test(t))).toBe(true);
    expect(alerts.some((t) => /floor/i.test(t))).toBe(true);

    // And the figures are qualified rather than stated flat -- on EVERY
    // surface that prints one, asserted separately. A single
    // `getAllByText` over the page passes while any one of the three
    // still says a bare "44", which is the sabotage that caught this
    // test passing for the wrong reason: dropping the table cell's
    // qualification left the summary tile's, and the assertion held.
    const totalTile = screen.getByText(/calls counted/i).closest("div")?.parentElement;
    expect(totalTile?.textContent ?? "").toMatch(/at least 44/i);

    // The ranked bar, which uses the ≥ form.
    const ranked = screen.getByText(/most used/i).parentElement;
    expect(ranked?.textContent ?? "").toMatch(/≥\s*44/);

    // The table cell. Scoped to the table, because the name also
    // appears in the ranked bar above it.
    const row = screen.getByRole("rowheader", { name: /playwright/ }).closest("tr");
    expect(row?.textContent ?? "").toMatch(/at least 44/i);
    // Nowhere does a bare, unqualified 44 stand as the count.
    const cells = Array.from(row?.querySelectorAll("td") ?? []).map((c) => c.textContent ?? "");
    expect(cells.some((t) => t.trim() === "44")).toBe(false);
  });

  /// With a complete scan the counts are stated flat.
  ///
  /// Proves the qualification above is conditional, not decoration. A
  /// page that says "at least" unconditionally is not qualifying, it is
  /// hedging, and the reader learns to ignore it.
  it("states counts flat when the scan was complete", () => {
    state.data = report({
      installed: [plugin()],
      usage: [usage({ mcp_calls: 44, measured: true })],
    });
    render(<ClaudePluginsPage />);
    // "at least <n>" is the qualification; the "Used at least once" tile
    // label is fixed copy and not one, so this matches the number form.
    expect(screen.queryByText(/at least \d/i)).toBeNull();
    expect(screen.queryByText(/≥/)).toBeNull();
    expect(screen.getAllByText("44").length).toBeGreaterThan(0);
  });

  /// The denominator is the finding.
  it("reports how many of the installed plugins were used", () => {
    state.data = report({
      installed: [plugin({ name: "playwright" }), plugin({ name: "unused-one" })],
      usage: [
        usage({ name: "playwright", mcp_calls: 44, measured: true }),
        usage({ name: "unused-one", measured: true }),
      ],
    });
    render(<ClaudePluginsPage />);
    expect(screen.getByText(/of 2 installed/i)).toBeTruthy();
  });

  /// The page states the counting rule it used.
  ///
  /// A user who greps their own transcripts gets a wildly different
  /// number, and without this they conclude the page is broken rather
  /// than that they counted availability lists.
  it("says what was counted", () => {
    state.data = report();
    render(<ClaudePluginsPage />);
    expect(screen.getByText(/calls that were actually made/i)).toBeTruthy();
  });

  /// A failed load shows no figures at all.
  it("renders an error rather than zeroes when the read failed", () => {
    state.failed = true;
    render(<ClaudePluginsPage />);
    expect(screen.getByText(/could not be read/i)).toBeTruthy();
    expect(screen.queryByTestId("plugin-calls-chart")).toBeNull();
  });

  /// Not measured yet is a reserved frame, not a zeroed chart.
  it("reserves a frame while loading", () => {
    state.loading = true;
    const { container } = render(<ClaudePluginsPage />);
    expect(container.querySelector("[aria-busy='true']")).toBeTruthy();
    expect(screen.queryByTestId("plugin-calls-chart")).toBeNull();
  });

  /// No plugins installed is a settled answer, not a failure.
  it("says nothing is installed without dressing it as an error", () => {
    state.data = report({ inventory_absent: true });
    render(<ClaudePluginsPage />);
    expect(screen.getByText(/No plugins are installed/i)).toBeTruthy();
    expect(screen.queryByText(/could not be read/i)).toBeNull();
  });

  /// An unreadable inventory does not discard the usage counts.
  it("keeps the counts when only the inventory failed", () => {
    state.data = report({
      usage: [usage({ mcp_calls: 44, measured: true })],
      inventory_failure: "/Users/x/.claude/plugins/installed_plugins.json: permission denied",
    });
    render(<ClaudePluginsPage />);
    expect(screen.getAllByText("44").length).toBeGreaterThan(0);
    const alerts = screen.getAllByRole("alert").map((a) => a.textContent ?? "");
    expect(alerts.some((t) => /installed-plugin list could not be read/i.test(t))).toBe(true);
  });

  /// The chart gets the whole window.
  it("passes the full window to the chart", () => {
    state.data = report({ usage: [usage({ mcp_calls: 1, measured: true })] });
    render(<ClaudePluginsPage />);
    const chart = screen.getByTestId("plugin-calls-chart");
    expect(chart.getAttribute("data-points")).toBe("30");
    expect(chart.getAttribute("data-days")).toBe("30");
  });
});

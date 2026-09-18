import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  InstalledPlugin,
  PluginDayCount,
  PluginFootprint,
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
  // #1129. Empty by default: this file's tests are about the plugins
  // table, and the definitions section is a sibling with its own tests.
  useClaudeDefinitions: () => ({
    data: { definitions: [], unreadable: [] },
    isLoading: false,
    isError: false,
    error: null,
    refetch: () => {},
  }),
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
    // Untraceable by default, which is the common case: most plugins
    // own no directory we can follow, and their engagement is unknown
    // rather than zero.
    footprint: { calls: 0, by_tool: {}, install_reads: 0, owned_known: false },
    ...over,
  };
}

/// A footprint, for the plugins whose engagement IS traceable.
function footprint(over: Partial<PluginFootprint> = {}): PluginFootprint {
  return { calls: 0, by_tool: {}, install_reads: 0, owned_known: true, ...over };
}

/// The plugin's row in the "All plugins" TABLE.
///
/// A plugin with calls is named twice on the page -- once in the ranked
/// bar and once in the table -- so a bare `getByText(name)` is ambiguous
/// and throws. The table row is the one carrying the per-plugin cells
/// these tests are about.
function tableRow(name: string): HTMLTableRowElement {
  const row = screen
    .getAllByText(name)
    .map((el) => el.closest("tr"))
    .find((tr): tr is HTMLTableRowElement => tr !== null);
  if (row === undefined) throw new Error(`no table row for ${name}`);
  return row;
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

  /// Invocations and engagement are shown as TWO figures, never blended.
  ///
  /// #1082's first required test, at the UI. A plugin invoked once whose
  /// files are touched 100 times must show both: "1" and "100" each
  /// legible, and the blend (101) nowhere on the page. A single number
  /// would be a new confident-wrong answer, which is the defect this
  /// feature exists to avoid.
  it("shows invocations and engagement as two distinct figures", () => {
    state.data = report({
      installed: [plugin({ name: "superpowers" })],
      usage: [
        usage({
          name: "superpowers",
          skill_calls: 1,
          measured: true,
          footprint: footprint({ calls: 100, by_tool: { Bash: 70, Read: 30 } }),
        }),
      ],
    });
    render(<ClaudePluginsPage />);

    const row = tableRow("superpowers");
    const cells = Array.from(row.querySelectorAll("td")).map((c) => (c.textContent ?? "").trim());

    // The two readings live in DIFFERENT cells, each exact. Asserting
    // per cell rather than over the row's concatenated text, which runs
    // the figures together ("1" then "100" reads as "1100") and would
    // let a blended rendering pass.
    const callsCell = cells.find((t) => t === "1");
    const engagementCell = cells.find((t) => t.startsWith("100"));
    expect(callsCell).toBeTruthy();
    expect(engagementCell).toBeTruthy();
    expect(callsCell).not.toBe(engagementCell);

    // The tool that did the work is named, because the shape is the
    // argument.
    expect(engagementCell).toMatch(/Bash/);

    // And the blend appears in no cell -- the specific lie prevented.
    expect(cells.some((t) => t.startsWith("101"))).toBe(false);
  });

  /// `remember` -- 0 invocations, files written -- is not "unused".
  ///
  /// #1082's second required test, and the disproof the whole feature
  /// rests on. Its entire contribution is instructions the model then
  /// follows, so it scores zero invocations forever; a page that showed
  /// only that would argue for uninstalling it.
  it("does not present a plugin with engagement and no calls as unused", () => {
    state.data = report({
      installed: [
        plugin({
          name: "remember",
          contribution: {
            mcp: false,
            skills: true,
            agents: false,
            commands: true,
            read: true,
          },
        }),
      ],
      usage: [
        usage({
          name: "remember",
          measured: true,
          footprint: footprint({ calls: 16, by_tool: { Write: 16 } }),
        }),
      ],
    });
    render(<ClaudePluginsPage />);

    const row = screen.getByText("remember").closest("tr");
    const text = row?.textContent ?? "";

    // Its work is visible.
    expect(text).toMatch(/\b16\b/);
    expect(text).toMatch(/Write/);
    // And it is never called unused, nor is its engagement a zero.
    expect(text).not.toMatch(/\bunused\b/i);
    expect(text).not.toMatch(/none recorded/i);

    // It counts toward the activity tally, rather than being filtered
    // out as "never called" -- which is the defect at the summary level.
    //
    // Asserted against the tally element itself. Matching "1" anywhere
    // in the tile passes on the "of 1 installed" denominator even when
    // the tally reads 0, which is exactly how this assertion first
    // passed against a calls-only tally.
    expect(screen.getByTestId("activity-tally").textContent).toBe("1");
  });

  /// An untraceable footprint renders words, never a bare 0.
  ///
  /// #1082's third required test. Most plugins own no directory we can
  /// follow, so their engagement is UNKNOWN. A "0" there would say "this
  /// plugin did nothing" when what we mean is "we cannot see what it
  /// did" -- absent is not zero, in the new column.
  it("says the engagement was not traced rather than printing a bare 0", () => {
    state.data = report({
      installed: [plugin({ name: "playwright" })],
      usage: [
        usage({
          name: "playwright",
          mcp_calls: 44,
          measured: true,
          // No owned directory declared: untraceable.
          footprint: { calls: 0, by_tool: {}, install_reads: 0, owned_known: false },
        }),
      ],
    });
    render(<ClaudePluginsPage />);

    const row = tableRow("playwright");
    const text = row.textContent ?? "";
    expect(text).toMatch(/not traced/i);

    // The ENGAGEMENT cell specifically is words, not a zero. (A bare
    // "0" elsewhere in the row is fine and correct: this plugin was
    // called 44 times and none of them failed, which is a measured
    // zero in the failures column.)
    const engagementCell = Array.from(row.querySelectorAll("td")).find((c) =>
      /not traced/i.test(c.textContent ?? ""),
    );
    expect(engagementCell).toBeTruthy();
    expect((engagementCell?.textContent ?? "").trim()).not.toBe("0");
  });

  /// A traceable plugin with nothing found IS a measured zero.
  ///
  /// The other direction, so the page does not become so cautious it
  /// refuses to report a real finding. "Traceable, and we found nothing"
  /// is a fact worth stating.
  it("says 'none recorded' when a traceable plugin really had no engagement", () => {
    state.data = report({
      installed: [plugin({ name: "superpowers" })],
      usage: [usage({ name: "superpowers", measured: true, footprint: footprint() })],
    });
    render(<ClaudePluginsPage />);
    const row = screen.getByText("superpowers").closest("tr");
    const text = row?.textContent ?? "";
    expect(text).toMatch(/none recorded/i);
    expect(text).not.toMatch(/not traced/i);
  });

  /// The page states the limit on what calls measure.
  ///
  /// #1082's third requirement. Without it the calls column implies
  /// "value", and a reader acts on that.
  it("says a plugin can contribute without ever being called", () => {
    state.data = report({ installed: [plugin()], usage: [usage()] });
    render(<ClaudePluginsPage />);
    expect(screen.getByText(/contribute without ever being called/i)).toBeTruthy();
  });

  /// What a plugin ships is named, so a zero reads correctly.
  ///
  /// #1082's second requirement: a skills-only plugin and a 31-tool MCP
  /// server cannot be judged by the same number.
  it("names what each plugin contributes", () => {
    state.data = report({
      installed: [
        plugin({
          name: "frontend-design",
          contribution: {
            mcp: false,
            skills: true,
            agents: false,
            commands: false,
            read: true,
          },
        }),
      ],
      usage: [usage({ name: "frontend-design", measured: true })],
    });
    render(<ClaudePluginsPage />);
    const row = screen.getByText("frontend-design").closest("tr");
    expect(row?.textContent ?? "").toMatch(/skills/i);
  });

  /// An install path we could not read says so, rather than guessing.
  it("does not claim what a plugin ships when the path was unreadable", () => {
    state.data = report({
      installed: [
        plugin({
          name: "mystery",
          contribution: {
            mcp: false,
            skills: false,
            agents: false,
            commands: false,
            read: false,
          },
        }),
      ],
      usage: [usage({ name: "mystery", measured: true })],
    });
    render(<ClaudePluginsPage />);
    const row = screen.getByText("mystery").closest("tr");
    const text = row?.textContent ?? "";
    expect(text).toMatch(/not known/i);
    // And it must not be described as shipping nothing, which is a
    // different claim we cannot support.
    expect(text).not.toMatch(/background behaviour only/i);
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

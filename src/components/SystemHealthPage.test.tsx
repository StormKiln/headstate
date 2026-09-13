import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AlertReport, Footprint, HealthSample } from "@/types/pr";
import { stubViewport } from "@/test-utils";

const liveFn = vi.hoisted(() => vi.fn<() => Promise<HealthSample>>());
const historyFn = vi.hoisted(() => vi.fn<() => Promise<HealthSample[]>>());
const footprintFn = vi.hoisted(() => vi.fn<() => Promise<Footprint>>());
const alertsFn = vi.hoisted(() => vi.fn<() => Promise<AlertReport[]>>());

// The hooks this page uses, not the whole `api/hooks` module's
// transitive world: that file imports every command the app has, and
// mocking it wholesale would tie this test to all of them. The
// stand-ins keep the real TanStack behaviour so loading and error
// states are the genuine ones.
//
// There are three, and there used to be ten. The seven disk hooks --
// `useWorktrees`, `useArtifacts`, `useVenvs` and the four sizing ones --
// were here for the footprint panels' "Measure disk use" button, which
// #795 and #796 removed along with both panels. This page now reaches no
// slow command at all, and `SystemHealthPage.transport.test.tsx` is what
// proves that at the transport seam, where a forgotten `enabled` in
// `hooks.ts` would actually show up.
//
// `useSystemFootprint` is stubbed but never resolved, which is correct
// rather than sloppy: the component still imports it at module scope --
// a factory that omitted the export would throw on import -- but #795
// removed the overview's only caller of it, and every test in this file
// stays on the overview. `footprintFn` is therefore deliberately left
// unprimed; the CPU and Memory detail pages that DO call it are tested,
// with a populated fixture, in `SystemHealthPage.detail.test.tsx`.
vi.mock("../api/hooks", () => ({
  useSystemHealth: (enabled: boolean) =>
    useQuery({ queryKey: ["system-health"], queryFn: liveFn, enabled, retry: false }),
  useSystemHealthHistory: (enabled: boolean) =>
    useQuery({
      queryKey: ["system-health-history"],
      queryFn: historyFn,
      enabled,
      retry: false,
    }),
  useSystemFootprint: (enabled: boolean) =>
    useQuery({
      queryKey: ["system-footprint"],
      queryFn: footprintFn,
      enabled,
      retry: false,
    }),
  useHealthAlerts: (enabled: boolean) =>
    useQuery({ queryKey: ["health-alerts"], queryFn: alertsFn, enabled, retry: false }),
}));

// The build target, as a mock: `IS_MOBILE_BUILD` is read at module
// scope, so re-importing the component to flip it would lose every
// other mock in this file. Same shape as `WorktreesPage.test.tsx`.
const mobileBuild = vi.hoisted(() => ({ current: false }));
vi.mock("@/lib/target", () => ({
  get IS_MOBILE_BUILD() {
    return mobileBuild.current;
  },
  get IS_DESKTOP_BUILD() {
    return !mobileBuild.current;
  },
}));

// The connection state drives the desktop's NAME and the unreachable
// wording. Defaults to `local`, which is what the desktop build always
// has, so every pre-existing test in this file keeps its old meaning.
const connection = vi.hoisted(() => ({
  current: { kind: "local" } as { kind: string; desktop?: string; lastPoll?: string | null },
}));
vi.mock("@/api/connection", () => ({
  useConnectionState: () => connection.current,
  isStale: () => false,
}));

import { SystemHealthPage } from "./SystemHealthPage";
// The pure logic lives in `lib/health` rather than in the component, so
// the gap detection -- the single most important piece of correctness
// here -- is testable without rendering anything.
import {
  SAMPLE_INTERVAL_MS,
  barColor,
  formatUptime,
  medianSpacing,
  splitOnGaps,
  thermalColor,
  type Point,
} from "@/lib/health";

/// A fully populated sample, so each test overrides only the one field
/// it is about. Everything present by default means a test asserting an
/// ABSENT reading has to say so explicitly, which is the point.
const sample = (over: Partial<HealthSample> = {}): HealthSample => ({
  sampled_at: new Date().toISOString(),
  load: [1.25, 0.94, 0.71],
  cpu_percent: 18,
  cpu_per_core: [10, 26],
  memory: {
    total: 16 * 1024 ** 3,
    used: 8 * 1024 ** 3,
    available: 7 * 1024 ** 3,
    swap_total: 2 * 1024 ** 3,
    swap_used: 512 * 1024 ** 2,
  },
  gpus: [
    {
      name: "Apple M2 Max",
      utilization_percent: 7,
      memory_used: 1.5 * 1024 ** 3,
      memory_total: 10 * 1024 ** 3,
      unified_memory: true,
    },
  ],
  disks: [
    { mount: "/", total: 500 * 1024 ** 3, available: 100 * 1024 ** 3, is_root: true },
  ],
  // Charge 82, capacity 84 -- deliberately different numbers, so a
  // panel rendering one in place of the other is visible.
  battery: { percent: 82, on_ac: false, capacity_percent: 84, cycle_count: 413 },
  thermal: "nominal",
  networks: [{ name: "en0", rx_bytes: 1024 ** 3, tx_bytes: 512 * 1024 ** 2 }],
  uptime_secs: 3 * 86_400 + 4 * 3600,
  ...over,
});

/// A series at the sampler's own cadence, ending now.
function series(values: (number | null)[], stepMs = SAMPLE_INTERVAL_MS): Point[] {
  const end = Date.now();
  return values.map((v, i) => ({
    t: end - (values.length - 1 - i) * stepMs,
    v,
  }));
}

const show = () => {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <SystemHealthPage />
    </QueryClientProvider>,
  );
};

describe("splitOnGaps", () => {
  /// The requirement the whole view rests on. A laptop closed overnight
  /// leaves two points twelve hours apart, and a line drawn between
  /// them asserts a value nobody measured.
  it("cuts the series where the app was not running", () => {
    const step = SAMPLE_INTERVAL_MS;
    const end = Date.now();
    const points: Point[] = [
      { t: end - 10 * 3600_000 - 2 * step, v: 10 },
      { t: end - 10 * 3600_000 - step, v: 12 },
      // Ten hours of nothing: the machine was asleep.
      { t: end - step, v: 30 },
      { t: end, v: 32 },
    ];
    const runs = splitOnGaps(points);
    expect(runs).toHaveLength(2);
    expect(runs[0].map((p) => p.v)).toEqual([10, 12]);
    expect(runs[1].map((p) => p.v)).toEqual([30, 32]);
  });

  /// The other half of the same requirement: an unbroken run must NOT
  /// be chopped up. A gap detector that fires on ordinary spacing would
  /// turn every chart into confetti and be just as untrustworthy.
  it("leaves an evenly sampled series in one piece", () => {
    const runs = splitOnGaps(series([1, 2, 3, 4, 5, 6]));
    expect(runs).toHaveLength(1);
    expect(runs[0]).toHaveLength(6);
  });

  /// Downsampling returns every Nth row, so adjacent points in a dense
  /// series are legitimately N minutes apart. That is the resolution,
  /// not a gap, and the threshold is derived from the series' own
  /// spacing precisely so this case stays whole.
  it("does not mistake the downsampled stride for a gap", () => {
    // Twelve minutes apart -- what a full 24 hours downsamples to.
    const runs = splitOnGaps(series([1, 2, 3, 4, 5], 12 * SAMPLE_INTERVAL_MS));
    expect(runs).toHaveLength(1);
  });

  /// A null VALUE is the same kind of unknown as a closed app: the
  /// platform reported nothing for that moment. Interpolating across it
  /// would be the same lie in a different costume.
  it("breaks the line at a value that was never measured", () => {
    const runs = splitOnGaps(series([1, 2, null, 4, 5]));
    expect(runs).toHaveLength(2);
    expect(runs[0].map((p) => p.v)).toEqual([1, 2]);
    expect(runs[1].map((p) => p.v)).toEqual([4, 5]);
  });

  /// One measurement between two closures is real and is kept. Dropping
  /// it would under-report what the app actually saw.
  it("keeps a run of a single point", () => {
    const end = Date.now();
    const runs = splitOnGaps([
      { t: end - 6 * 3600_000, v: 40 },
      { t: end, v: 50 },
    ]);
    expect(runs).toHaveLength(2);
    expect(runs[0]).toHaveLength(1);
  });

  /// The blind spot in a purely relative threshold: two points six
  /// hours apart have a MEDIAN spacing of six hours, so nothing in the
  /// series exceeds `median * factor` and the pair would join into a
  /// line across a period nobody measured. The absolute ceiling is what
  /// catches it.
  it("cuts a wide pair even though the gap is its own median spacing", () => {
    const end = Date.now();
    const runs = splitOnGaps([
      { t: end - 6 * 3600_000, v: 40 },
      { t: end, v: 50 },
    ]);
    expect(runs).toHaveLength(2);
  });

  it("returns nothing for an empty series", () => {
    expect(splitOnGaps([])).toEqual([]);
  });
});

describe("medianSpacing", () => {
  /// The median, not the mean: the mean of a series containing a
  /// six-hour hole is dragged up BY that hole, raising the threshold
  /// above the very gap being looked for and hiding it.
  it("is not dragged upward by the gap it is meant to find", () => {
    const end = Date.now();
    const points: Point[] = [];
    for (let i = 20; i > 0; i -= 1) {
      points.push({ t: end - 8 * 3600_000 - i * SAMPLE_INTERVAL_MS, v: 1 });
    }
    points.push({ t: end, v: 1 });
    expect(medianSpacing(points)).toBe(SAMPLE_INTERVAL_MS);
  });

  it("never falls below the sampler's own cadence", () => {
    const end = Date.now();
    expect(
      medianSpacing([
        { t: end - 1000, v: 1 },
        { t: end, v: 1 },
      ]),
    ).toBe(SAMPLE_INTERVAL_MS);
  });
});

describe("formatUptime", () => {
  it("reads as days, hours and minutes", () => {
    expect(formatUptime(3 * 86_400 + 4 * 3600)).toBe("3d 4h");
    expect(formatUptime(5 * 3600 + 30 * 60)).toBe("5h 30m");
    expect(formatUptime(90)).toBe("1m");
  });
});

describe("barColor", () => {
  /// Thresholds are high on purpose: a machine at 70% memory is a
  /// machine being used. Colouring that amber teaches people to ignore
  /// the colour, which costs the red band its meaning.
  it("stays green through ordinary use and escalates only near full", () => {
    expect(barColor(10)).toBe("#3fb950");
    expect(barColor(70)).toBe("#3fb950");
    expect(barColor(80)).toBe("#d29922");
    expect(barColor(95)).toBe("#f85149");
  });
});

describe("thermalColor", () => {
  /// `nominal` is grey, not green: a machine at rest is the ordinary
  /// case, and green would read as an achievement.
  it("reserves colour for the states that need attention", () => {
    expect(thermalColor("nominal")).toBe("#8b949e");
    expect(thermalColor("fair")).toBe("#58a6ff");
    expect(thermalColor("serious")).toBe("#d29922");
    expect(thermalColor("critical")).toBe("#f85149");
  });
});

describe("SystemHealthPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    liveFn.mockResolvedValue(sample());
    historyFn.mockResolvedValue([]);
    // No conditions by default; a test that cares primes its own.
    alertsFn.mockResolvedValue([]);
    // Back to the desktop build on a local machine, so a mobile test
    // that flips these cannot leak into the one after it.
    mobileBuild.current = false;
    connection.current = { kind: "local" };
  });

  /// #864: the rules ran for 8.5 hours over a 12-process runaway and the
  /// page showed nothing, because `health_alerts` had no frontend caller
  /// at all. These four cover the panel that closes that gap, and the
  /// third is the one that matters most.
  it("names an active health condition on the overview", async () => {
    alertsFn.mockResolvedValue([
      {
        key: "diffuse_cpu",
        title: "Several processes are using the CPU",
        body: "About 72% of the CPU has been in use for 31 minutes, and no single process accounts for it.",
      },
    ]);
    show();
    expect(await screen.findByText("Several processes are using the CPU")).toBeTruthy();
    expect(screen.getByText(/no single process accounts for it/)).toBeTruthy();
  });

  it("says nothing when no condition is true and the reading is current", async () => {
    show();
    // The live sample must have arrived, or an empty panel would prove
    // only that the query had not resolved yet.
    expect(await screen.findByText("1.25")).toBeTruthy();
    expect(screen.queryByRole("group", { name: "Active health conditions" })).toBeNull();
    expect(screen.queryByText(/was not watching/)).toBeNull();
  });

  /// The distinction the incident turned on. An overnight runaway is
  /// exactly the case where the app may have been closed, so "no
  /// conditions found" over a stale sample is not good news and must not
  /// render as the silence above.
  it("does not call a stale reading clean", async () => {
    liveFn.mockResolvedValue(
      sample({ sampled_at: new Date(Date.now() - 3 * 60 * 60 * 1000).toISOString() }),
    );
    show();
    expect(await screen.findByText(/was not watching/)).toBeTruthy();
    expect(screen.getByText(/180 minutes old/)).toBeTruthy();
  });

  /// A failed read is not a clean result either -- the same rule the rest
  /// of this page follows for a failed scan (#858).
  it("says the check failed rather than showing nothing", async () => {
    alertsFn.mockRejectedValue(new Error("no"));
    show();
    expect(await screen.findByText(/Could not check for health conditions/)).toBeTruthy();
    expect(screen.getByText(/the rules did not run/)).toBeTruthy();
  });

  it("shows the live readings once they arrive", async () => {
    show();
    expect(await screen.findByText("1.25")).toBeTruthy();
    expect(screen.getByText("0.94")).toBeTruthy();
    expect(screen.getByText("0.71")).toBeTruthy();
    // `getAllBy`: the CPU figure is deliberately in two places now --
    // the pressure row above and the CPU panel here (#683). Both
    // render the same sample, so asserting it appears is still the
    // point; asserting it appears exactly once never was.
    expect(screen.getAllByText("18%").length).toBeGreaterThan(0);
    expect(screen.getByText("3d 4h")).toBeTruthy();
  });

  /// "We have not looked yet" and "we looked and there is nothing" are
  /// opposite answers. Panels full of dashes on first paint would say
  /// the second while the first is true.
  it("says it is still reading before the first sample lands", () => {
    liveFn.mockReturnValue(new Promise(() => {}));
    show();
    expect(screen.getByText(/Reading this machine's health/)).toBeTruthy();
  });

  /// THE absent-value rule. A desktop with no battery reports null, and
  /// "0%" there would say the machine is about to die.
  it("renders a machine with no battery as not measured, never as zero", async () => {
    liveFn.mockResolvedValue(sample({ battery: null }));
    show();
    await screen.findByText("1.25");
    expect(screen.queryByText("0%")).toBeNull();
    expect(screen.getAllByText(/Not measured/).length).toBeGreaterThan(0);
    expect(screen.getByText(/No battery on this machine/)).toBeTruthy();
  });


  /// #773's second half: the rate on the OVERVIEW, not only behind a
  /// click.
  ///
  /// The issue is explicit about why. #720's alert fires, the user
  /// opens this page, and the number that answers "why did I get that
  /// alert" should be on the panel they land on.
  it("shows the drain rate on the overview, not only on the detail page", async () => {
    liveFn.mockResolvedValue(
      sample({
        battery: {
          percent: 64,
          on_ac: false,
          capacity_percent: 84,
          cycle_count: 413,
          power: { watts: -18.4, milliamps: -1454, millivolts: 12654 },
        },
      }),
    );
    show();
    await screen.findByText("-18.4 W");
    expect(screen.getByText("Out of the battery")).toBeTruthy();
  });

  /// The #720 condition, said in words on the overview panel too --
  /// colour is never the only cue on this page.
  it("says on the overview when a plugged-in machine is losing charge", async () => {
    liveFn.mockResolvedValue(
      sample({
        battery: {
          percent: 64,
          on_ac: true,
          capacity_percent: 84,
          cycle_count: 413,
          power: { watts: -6.2, milliamps: -490, millivolts: 12654 },
        },
      }),
    );
    show();
    await screen.findByText(/losing charge while plugged in/i);
  });

  /// Absent is not zero, in the field where it is hardest to see: a
  /// full battery on mains genuinely draws 0 W, so a fabricated zero
  /// here is indistinguishable from a measurement.
  it("reports an unpublished rate as not measured, never as zero watts", async () => {
    liveFn.mockResolvedValue(
      sample({
        battery: {
          percent: 64,
          on_ac: true,
          capacity_percent: 84,
          cycle_count: 413,
          power: null,
        },
      }),
    );
    show();
    await screen.findByText("64%");
    expect(screen.queryByText("0.0 W")).toBeNull();
    expect(screen.getByText(/This platform does not publish it/)).toBeTruthy();
  });

  /// A desktop's absent rate is not a separate fact from its absent
  /// battery, and two "Not measured" rows for one absence is noise.
  it("does not add a rate row for a machine with no battery at all", async () => {
    liveFn.mockResolvedValue(sample({ battery: null }));
    show();
    await screen.findByText(/No battery on this machine/);
    expect(screen.queryByText("Rate")).toBeNull();
  });

  /// Windows reports no load average at all. Three silent dashes would
  /// look like a bug in Headstate rather than a platform limit.
  it("explains a platform with no load average instead of showing zeros", async () => {
    liveFn.mockResolvedValue(sample({ load: null, cpu_percent: null }));
    show();
    expect(
      await screen.findByText(/does not report load averages/),
    ).toBeTruthy();
    expect(screen.queryByText("0.00")).toBeNull();
  });

  /// A machine with swap turned off reports a zero total. That is "there
  /// is no swap", not "0% of swap is in use".
  it("distinguishes no swap from empty swap", async () => {
    liveFn.mockResolvedValue(
      sample({
        memory: { ...sample().memory, swap_total: 0, swap_used: 0 },
      }),
    );
    show();
    expect(await screen.findByText(/No swap configured/)).toBeTruthy();
  });

  it("reports an absent thermal reading rather than inventing one", async () => {
    liveFn.mockResolvedValue(sample({ thermal: null }));
    show();
    await screen.findByText("1.25");
    expect(screen.getAllByText(/Not measured/).length).toBeGreaterThan(0);
  });

  /// The label invites exactly one wrong reading -- that it is a
  /// temperature. The copy beside it has to say it is not, and say why
  /// degrees are unavailable.
  it("says the thermal label is a pressure rating and not a temperature", async () => {
    show();
    await screen.findByText("1.25");
    expect(screen.getByText("nominal")).toBeTruthy();
    const note = screen.getByText(/not a\s+temperature/i);
    expect(note.textContent).toMatch(/elevated privileges/i);
    // And it must never present the label in degrees.
    expect(screen.queryByText(/°/)).toBeNull();
  });

  it("names every mounted volume with what is free of what", async () => {
    show();
    await screen.findByText("1.25");
    expect(screen.getByText("/")).toBeTruthy();
    // Also in the Disk pressure card (#683), same sample, so the
    // question is whether the panel says it -- not whether the page
    // says it once.
    expect(screen.getAllByText(/100 GB free of 500 GB/).length).toBeGreaterThan(0);
    // The volume the app lives on is called out rather than left to be
    // guessed from the mount point.
    expect(screen.getByText("system")).toBeTruthy();
  });

  it("lists interfaces as totals since boot, not as speeds", async () => {
    show();
    await screen.findByText("1.25");
    expect(screen.getByText("en0")).toBeTruthy();
    expect(screen.getByText(/Totals since the machine booted/)).toBeTruthy();
  });

  /// The chart itself must show the break, not merely be capable of it.
  /// `data-runs` is the count of separately drawn runs, so two means the
  /// line really was cut rather than smoothed over.
  it("draws a history with a hole in it as two separate runs", async () => {
    const end = Date.now();
    const at = (msAgo: number) => new Date(end - msAgo).toISOString();
    historyFn.mockResolvedValue([
      sample({ sampled_at: at(10 * 3600_000 + 2 * 60_000), cpu_percent: 10 }),
      sample({ sampled_at: at(10 * 3600_000 + 60_000), cpu_percent: 12 }),
      // Ten hours with the app closed.
      sample({ sampled_at: at(60_000), cpu_percent: 30 }),
      sample({ sampled_at: at(0), cpu_percent: 32 }),
    ]);
    show();
    const chart = await screen.findByTestId("sparkline-CPU");
    expect(chart.getAttribute("data-runs")).toBe("2");
    // Two polylines, and crucially no single line joining all four
    // points across the hole.
    expect(chart.querySelectorAll("polyline")).toHaveLength(2);
  });

  /// The counterpart: continuous data must draw as one line, or the
  /// gap rendering would be noise rather than signal.
  it("draws a continuous history as one unbroken line", async () => {
    const end = Date.now();
    historyFn.mockResolvedValue(
      [0, 1, 2, 3, 4].map((i) =>
        sample({
          sampled_at: new Date(end - (4 - i) * 60_000).toISOString(),
          cpu_percent: 20 + i,
        }),
      ),
    );
    show();
    const chart = await screen.findByTestId("sparkline-CPU");
    expect(chart.getAttribute("data-runs")).toBe("1");
    expect(chart.querySelectorAll("polyline")).toHaveLength(1);
  });

  /// An empty series is "we have not collected anything yet", which is
  /// not the same as a flat line at zero -- and a flat line at zero is
  /// exactly what a naive chart would draw.
  it("says there is no history yet rather than drawing a flat zero", async () => {
    show();
    await screen.findByText("1.25");
    const charts = screen.getAllByLabelText(/no history yet/);
    expect(charts.length).toBeGreaterThan(0);
    expect(document.querySelectorAll("polyline")).toHaveLength(0);
  });

  /// A history that fails must not take the live panels with it: the
  /// current numbers are half of what people open this view for.
  it("keeps the live readings when only the history fails", async () => {
    historyFn.mockRejectedValue(new Error("db locked"));
    show();
    expect(await screen.findByText("1.25")).toBeTruthy();
    await waitFor(() =>
      expect(screen.getByText(/24-hour history could not be loaded/)).toBeTruthy(),
    );
  });

  it("reports a failure to read the machine at all", async () => {
    liveFn.mockRejectedValue(new Error("collector unavailable"));
    show();
    expect(
      await screen.findByText(/Could not read this machine's health/),
    ).toBeTruthy();
  });

  /// Per-core bars are meters, so their value is readable without
  /// inferring it from a pixel width.
  it("renders one meter per core at that core's value", async () => {
    show();
    await screen.findByText("1.25");
    const core0 = screen.getByLabelText("Core 0");
    const core1 = screen.getByLabelText("Core 1");
    expect(core0.getAttribute("aria-valuenow")).toBe("10");
    expect(core1.getAttribute("aria-valuenow")).toBe("26");
  });

  /// #960. `Stat` and `NotMeasured` exist so an unreadable reading is
  /// never coerced to 0 -- and the `Bar` beside them was handed `?? 0`
  /// and drew a green, empty, `aria-valuenow=0` meter.
  ///
  /// A disk reporting `total: 0` is what `percentOf` returns `null` for,
  /// and it is an ordinary shape on somebody else's machine: an unusual
  /// filesystem, a container mount, a platform read the collector could
  /// not complete. The row's own text line correctly said "Not measured"
  /// while a full-width bar sat empty and GREEN immediately above it --
  /// the two halves of one row disagreeing, with the wrong half the one
  /// the eye reads first.
  ///
  /// Scoped to that disk's own meter by its label, because a page-wide
  /// `aria-valuenow` search would find every other bar on the page.
  it("an unmeasurable disk draws no meter value rather than zero", async () => {
    liveFn.mockResolvedValue(
      sample({
        disks: [{ mount: "/", total: 0, available: 0, is_root: true }],
      }),
    );
    show();
    await screen.findByText("1.25");

    const meter = screen.getByLabelText("/ used");
    // The affirmative numeric claim is GONE. `role="meter"` with
    // `aria-valuenow=0` tells a screen reader the value IS zero, which is
    // a statement, not an absence.
    expect(meter.getAttribute("aria-valuenow")).toBeNull();
    expect(meter.getAttribute("aria-valuenow")).not.toBe("0");
    // And the meter says what it does not know, which is the only way a
    // meter can express "unknown".
    expect(meter.getAttribute("aria-valuetext")).toBe("Not measured");
    // The text line still says it, as it always did -- the bar now agrees.
    expect(screen.getAllByText(/Not measured/).length).toBeGreaterThan(0);
  });

  /// The other half of #960: a genuine 0% is a MEASURED reading and must
  /// still render as one. A fix that erased the real zero along with the
  /// fabricated one would trade one wrong answer for another, and
  /// `Bar`'s clamp comment records a real case that depends on it.
  it("still reports a measured zero as zero", async () => {
    liveFn.mockResolvedValue(
      sample({
        // Completely empty disk: a real, readable 0% used.
        disks: [
          { mount: "/", total: 500 * 1024 ** 3, available: 500 * 1024 ** 3, is_root: true },
        ],
      }),
    );
    show();
    await screen.findByText("1.25");

    const meter = screen.getByLabelText("/ used");
    expect(meter.getAttribute("aria-valuenow")).toBe("0");
    expect(meter.getAttribute("aria-valuetext")).toBeNull();
  });

  /// The memory equivalent (#960). `percentOf(used, total)` is `null` on
  /// a zero or unreadable total, and the Memory panel's bar laundered it
  /// the same way.
  it("an unmeasurable memory total draws no meter value rather than zero", async () => {
    liveFn.mockResolvedValue(
      sample({
        memory: { total: 0, used: 0, available: 0, swap_total: 0, swap_used: 0 },
      }),
    );
    show();
    await screen.findByText("1.25");

    const meter = screen.getByLabelText("Memory used");
    expect(meter.getAttribute("aria-valuenow")).toBeNull();
    expect(meter.getAttribute("aria-valuetext")).toBe("Not measured");
  });

  /// The GPU's utilization is a meter, like every other bar here, so
  /// its value is readable without inferring it from a pixel width.
  it("renders the GPU's utilization and the memory it holds", async () => {
    show();
    await screen.findByText("1.25");
    expect(screen.getByText("Apple M2 Max")).toBeTruthy();
    const bar = screen.getByLabelText("Apple M2 Max utilization");
    expect(bar.getAttribute("aria-valuenow")).toBe("7");
  });

  /// **The unified-memory rule (#686).** The Memory panel reports the
  /// same physical pool, so without this sentence a reader would add
  /// the GPU's gigabytes to the system's and conclude the machine has
  /// more RAM than it does -- the two panels would look like they
  /// disagreed about the size of the machine.
  it("says the GPU shares one pool with the system, not a second one", async () => {
    show();
    await screen.findByText("1.25");
    expect(screen.getByText(/unified memory/)).toBeTruthy();
    expect(screen.getByText(/should not be added to it/)).toBeTruthy();
  });

  /// A discrete GPU has its own VRAM, so the unified-memory note would
  /// be false there and must not appear.
  it("does not claim unified memory for a card with its own VRAM", async () => {
    liveFn.mockResolvedValue(
      sample({
        gpus: [
          {
            name: "card0",
            utilization_percent: 42,
            memory_used: 2 * 1024 ** 3,
            memory_total: 8 * 1024 ** 3,
            unified_memory: false,
          },
        ],
      }),
    );
    show();
    await screen.findByText("1.25");
    expect(screen.getByText("card0")).toBeTruthy();
    expect(screen.queryByText(/unified memory/)).toBeNull();
    expect(screen.getByText("Total VRAM")).toBeTruthy();
  });

  /// **No panel at all, rather than a panel of absences.**
  ///
  /// This is the rule #686 states explicitly, and it is where this
  /// page's usual "Not measured" would be the WRONG answer: a GPU
  /// panel full of dashes claims a GPU was found and could not be
  /// read. On Windows, and on Intel/NVIDIA Linux, nothing was found
  /// because there is no unprivileged way to look.
  it("renders no GPU panel when the platform reported none", async () => {
    liveFn.mockResolvedValue(sample({ gpus: [] }));
    show();
    await screen.findByText("1.25");
    expect(screen.queryByText("GPU")).toBeNull();
    expect(screen.queryByText("Utilization")).toBeNull();
  });

  /// A GPU that reported utilization but no memory figures shows the
  /// one it has and says nothing about the ones it does not -- never a
  /// confident 0 bytes.
  it("keeps a GPU's unreported memory absent rather than zero", async () => {
    liveFn.mockResolvedValue(
      sample({
        gpus: [
          {
            name: "card0",
            utilization_percent: 55,
            memory_used: null,
            memory_total: null,
            unified_memory: false,
          },
        ],
      }),
    );
    show();
    await screen.findByText("1.25");
    expect(screen.getByLabelText("card0 utilization").getAttribute("aria-valuenow")).toBe(
      "55",
    );
    expect(screen.queryByText("0 B")).toBeNull();
    expect(screen.getAllByText(/Not measured/).length).toBeGreaterThan(0);
  });

  /// Two GPUs are two cards, not one picked arbitrarily -- an Intel Mac
  /// with integrated and discrete graphics reports both.
  it("renders every GPU the machine reported", async () => {
    liveFn.mockResolvedValue(
      sample({
        gpus: [
          {
            name: "Intel Iris",
            utilization_percent: 3,
            memory_used: null,
            memory_total: null,
            unified_memory: false,
          },
          {
            name: "Radeon Pro",
            utilization_percent: 61,
            memory_used: null,
            memory_total: null,
            unified_memory: false,
          },
        ],
      }),
    );
    show();
    await screen.findByText("1.25");
    expect(screen.getByText("Intel Iris")).toBeTruthy();
    expect(screen.getByText("Radeon Pro")).toBeTruthy();
  });

  /// A machine that reported no per-core figures gets the absent
  /// treatment too -- not a row of empty bars, which would read as a
  /// perfectly idle CPU.
  it("says so when no per-core figures were reported", async () => {
    liveFn.mockResolvedValue(sample({ cpu_per_core: [] }));
    show();
    await screen.findByText("1.25");
    expect(screen.queryByLabelText("Core 0")).toBeNull();
    expect(screen.getAllByText(/Not measured/).length).toBeGreaterThan(0);
  });

  /// "Available" is not "total minus used" on any modern platform, and
  /// a reader who assumes it is will conclude one of the numbers is
  /// wrong. The hint is what stops that.
  it("explains that available memory includes reclaimable cache", async () => {
    show();
    await screen.findByText("1.25");
    expect(screen.getByText(/Reclaimable, incl\. cache/)).toBeTruthy();
  });

  /// Each chart carries the explanation beside it. An explanation two
  /// panels away is one nobody reads at the moment they need it.
  ///
  /// Three charts on this fixture: CPU, memory, and the GPU (#686),
  /// which the default sample reports.
  it("explains the breaks beside each chart", async () => {
    show();
    await screen.findByText("1.25");
    const notes = screen.getAllByText(/Headstate was not running/);
    expect(notes.length).toBe(3);
  });

  /// The GPU chart goes away with the GPU panel, so a machine with no
  /// discoverable GPU is back to two.
  it("drops the GPU chart along with its panel", async () => {
    liveFn.mockResolvedValue(sample({ gpus: [] }));
    show();
    await screen.findByText("1.25");
    expect(screen.getAllByText(/Headstate was not running/).length).toBe(2);
  });
});

describe("the view's place in the app", () => {
  /// The list is the single source: the migration's completeness test
  /// and the settings toggles both read from it, so a view missing here
  /// is a view that crashes on rehydrate.
  it("is one of the app's views", async () => {
    const { ALL_VIEWS } = await import("../store/filters");
    expect([...ALL_VIEWS]).toContain("system-health");
  });

  /// It describes the machine, so it has no repository axis. A picker
  /// beside it would be a control that changes nothing -- or, on a
  /// machine with no scanned checkouts, an empty list under a heading,
  /// which reads as a page that failed to load.
  it("gets a sidebar with no repository picker", async () => {
    const { useFilters } = await import("../store/filters");
    useFilters.getState().setView("system-health");
    expect(useFilters.getState().filtersByView["system-health"]).toEqual({});
    expect(useFilters.getState().filtersByView["system-health"].repo).toBeUndefined();
  });
});

describe("the panels are laid out for a desktop", () => {
  /// jsdom has no `matchMedia`, so `useIsMobile` reports false and this
  /// renders the desktop layout -- which is the one this issue is
  /// about. Asserted rather than assumed, so a future change to the
  /// viewport shim does not silently retarget every test above.
  it("renders without a viewport shim, on the desktop path", async () => {
    liveFn.mockResolvedValue(sample());
    historyFn.mockResolvedValue([]);
    expect(typeof window.matchMedia).not.toBe("function");
    const { container } = show();
    await screen.findByText("1.25");
    const grid = container.querySelector(".md\\:grid-cols-2");
    expect(grid).not.toBeNull();
    // Six panel headings -- the original five (CPU, Memory, Disk,
    // Battery, Network) plus GPU (#686), which the default sample
    // reports. Seven until #795 removed the "What Headstate is costing"
    // panel, which was the seventh.
    //
    // Counted rather than asserted loosely because a panel silently
    // disappearing from the layout is exactly the kind of regression
    // this catches -- #665 shipped once with its whole panel missing,
    // and a count is the only assertion that fails when the LOSS is the
    // bug. It is also what would fail loudly if a future edit brought a
    // footprint panel back without a decision being made about it.
    const headings = within(grid as HTMLElement).getAllByRole("heading");
    expect(headings.filter((h) => h.tagName === "H2").length).toBe(6);
    expect(headings.map((h) => h.textContent)).toContain("GPU");
    // And the removed panel is not back under any spelling (#795, #796).
    expect(headings.map((h) => h.textContent)).not.toContain(
      "What Headstate is costing",
    );
  });

  /// And the GPU panel is the one that comes and goes: a machine that
  /// reported none is back to five, with nothing left behind.
  ///
  /// The counterpart to the count above. Between them they pin both
  /// halves of the #686 rule -- the panel appears when there is
  /// something truthful in it, and vanishes entirely when there is not.
  it("drops to five panels on a machine with no discoverable GPU", async () => {
    liveFn.mockResolvedValue(sample({ gpus: [] }));
    historyFn.mockResolvedValue([]);
    const { container } = show();
    await screen.findByText("1.25");
    const grid = container.querySelector(".md\\:grid-cols-2");
    const headings = within(grid as HTMLElement).getAllByRole("heading");
    expect(headings.filter((h) => h.tagName === "H2").length).toBe(5);
    expect(headings.map((h) => h.textContent)).not.toContain("GPU");
  });
});

/// #683: the pressure row, above everything else.
describe("the at-a-glance pressure cards", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    liveFn.mockResolvedValue(sample());
    historyFn.mockResolvedValue([]);
    // No conditions by default; a test that cares primes its own.
    alertsFn.mockResolvedValue([]);
    mobileBuild.current = false;
    connection.current = { kind: "local" };
  });

  it("answers CPU, memory and disk without scrolling", async () => {
    show();
    // Scoped to the row: the same figures also appear in the panels
    // below, which is correct -- they are one source of truth read
    // twice -- but makes a page-wide query ambiguous.
    const row = await screen.findByRole("group", { name: /pressure at a glance/i });
    // The fixture is 8 of 16 GB used, and a 500 GB root with 100 GB
    // free -- so 50% and 80%.
    expect(within(row).getByText("50%")).toBeTruthy();
    expect(within(row).getByText("80%")).toBeTruthy();
    expect(within(row).getByText("18%")).toBeTruthy();
  });

  /// A percentage alone cannot tell a nearly-full small disk from a
  /// nearly-full large one, which is the difference between "ignore
  /// this" and "act today".
  it("shows the absolute figures behind each percentage", async () => {
    show();
    const row = await screen.findByRole("group", { name: /pressure at a glance/i });
    expect(within(row).getByText(/of 16 GB/)).toBeTruthy();
    expect(within(row).getByText(/free of 500 GB/)).toBeTruthy();
  });

  /// The page's central rule, applied where people look first. A
  /// confident 0% for something never measured is the one thing this
  /// view exists not to do.
  it("says a missing reading is not measured, never zero", async () => {
    liveFn.mockResolvedValue(sample({ cpu_percent: null }));
    show();
    const row = await screen.findByRole("group", { name: /pressure at a glance/i });
    expect(within(row).getByText("Not measured")).toBeTruthy();
    expect(within(row).queryByText("0%")).toBeNull();
  });

  /// Colour is a second cue, never the only one: the bar carries the
  /// band, the digits carry the value, and a reader who cannot
  /// distinguish the bands still gets the answer.
  it("labels each bar for a reader who cannot see its colour", async () => {
    show();
    expect(await screen.findByLabelText(/Memory: 50 percent/i)).toBeTruthy();
    expect(screen.getByLabelText(/Disk: 80 percent/i)).toBeTruthy();
  });

  /// The row must not become a second source of truth: the panels
  /// below stay exactly as they were.
  it("leaves the panels below untouched", async () => {
    show();
    await screen.findByRole("group", { name: /pressure at a glance/i });
    // The panel headings, which the row must not have replaced.
    for (const title of ["Load averages, per-core use, and the last 24 hours",
                         "Every mounted volume"]) {
      expect(screen.getByText(title)).toBeTruthy();
    }
  });
});

/// #666: the same view on the phone, describing the DESKTOP.
///
/// Every test here flips `mobileBuild.current` explicitly. A test that
/// forgets renders the desktop build whatever its name says -- the same
/// trap `stubViewport` sets for viewport-gated code, and the reason
/// #655 shipped a component with no mobile coverage at all.
describe("SystemHealthPage on the phone", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    liveFn.mockResolvedValue(sample());
    historyFn.mockResolvedValue([]);
    mobileBuild.current = true;
    connection.current = { kind: "connected", desktop: "studio", lastPoll: null };
  });

  it("says whose machine it is describing", async () => {
    show();
    // The distinction the whole issue is about: this page is full of
    // CPU and battery readings, rendered on a device that has its own.
    expect(await screen.findByText(/not this phone/i)).toBeTruthy();
    expect(screen.getByText("studio")).toBeTruthy();
  });

  it("says the desktop is unreachable rather than showing zeros", async () => {
    connection.current = { kind: "unreachable", desktop: "studio", lastPoll: null };
    liveFn.mockRejectedValue(new Error("unreachable"));
    show();
    expect(await screen.findByText(/cannot reach studio/i)).toBeTruthy();
    // The failure this codebase avoids everywhere else: a zero that
    // means "not measured". No reading may be on screen.
    expect(screen.queryByText("0%")).toBeNull();
    expect(screen.queryByText("18%")).toBeNull();
  });

  it("does not blame the desktop for a gap the phone may have caused", async () => {
    historyFn.mockResolvedValue([sample()]);
    show();
    // On the phone a hole has two causes, and naming only the first
    // would assert the desktop was off during a period it may have
    // been running perfectly well.
    const note = await screen.findAllByText(/could not reach it/i);
    expect(note.length).toBeGreaterThan(0);
  });

  it("names the desktop when its battery is absent", async () => {
    liveFn.mockResolvedValue(sample({ battery: null }));
    show();
    expect(await screen.findByText(/that desktop has no battery/i)).toBeTruthy();
  });

  it("falls back to a neutral name before the desktop is known", async () => {
    connection.current = { kind: "unknown" };
    show();
    // No hostname yet is not the same as no pairing; the page still
    // has to say it is not describing the phone.
    expect(await screen.findByText(/the paired desktop/i)).toBeTruthy();
  });
});

/// The other half of the same requirement: the desktop rendering is
/// unchanged. Asserted at a real desktop width rather than by
/// inspection, because that is what the issue asks for.
describe("SystemHealthPage on the desktop is untouched by the mobile work", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    liveFn.mockResolvedValue(sample());
    historyFn.mockResolvedValue([]);
    mobileBuild.current = false;
    connection.current = { kind: "local" };
  });

  it("never mentions a phone or a paired desktop at 1400px", async () => {
    const viewport = stubViewport(1400);
    try {
      show();
      await screen.findAllByText("18%");
      expect(screen.queryByText(/not this phone/i)).toBeNull();
      expect(screen.queryByText(/paired desktop/i)).toBeNull();
      expect(screen.queryByText(/could not reach it/i)).toBeNull();
    } finally {
      viewport.resize(1400);
    }
  });

  it("still says a gap means Headstate was not running", async () => {
    historyFn.mockResolvedValue([sample()]);
    show();
    const note = await screen.findAllByText(/Headstate was not running/i);
    expect(note.length).toBeGreaterThan(0);
    // The phone's second cause must not leak on to the desktop, where
    // there is no phone and the claim would be false.
    expect(screen.queryByText(/could not reach it/i)).toBeNull();
  });

  it("keeps its own wording when a local reading fails", async () => {
    liveFn.mockRejectedValue(new Error("boom"));
    show();
    expect(await screen.findByText(/could not read this machine's health/i)).toBeTruthy();
    expect(screen.queryByText(/cannot reach/i)).toBeNull();
  });
});

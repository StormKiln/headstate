import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  Footprint,
  FootprintProcess,
  FootprintProcessGroup,
  HealthGpu,
  HealthSample,
  NetProcess,
} from "@/types/pr";
import { stubViewport } from "@/test-utils";

/// The per-class detail pages (#687).
///
/// Separate from `SystemHealthPage.test.tsx` because that file's
/// subject is the OVERVIEW, which this issue must leave alone -- and
/// mixing the two would make it easy for a change here to quietly
/// rewrite an assertion there. The overview's own suite is the guard
/// that it stayed put; this one covers what was added beside it.
///
/// # Every process name below is synthetic
///
/// This repository is public and its screenshots go in store listings.
/// A real process list names what a person runs, so the fixtures use
/// invented names (`acme-render`, `widget-daemon`) rather than a
/// capture from any machine. Per `CONTRIBUTING.md`.

/// The Network page's own cadence (#718), restated for the mock and
/// pinned against the real module by a test below.
const { MOCK_POLL_MS, MOCK_SAMPLE_MS } = vi.hoisted(() => ({
  MOCK_POLL_MS: 15_000,
  MOCK_SAMPLE_MS: 5_000,
}));

const liveFn = vi.hoisted(() => vi.fn<() => Promise<HealthSample>>());
const netProcFn = vi.hoisted(() => vi.fn<() => Promise<NetProcess[]>>());
/// Off by default, so no test pays for a second reading it did not ask
/// for. The one test about the single-to-two-readings transition turns
/// it on.
const netProcPollMs = vi.hoisted(() => ({ current: false as number | false }));
const historyFn = vi.hoisted(() => vi.fn<() => Promise<HealthSample[]>>());
const footprintFn = vi.hoisted(() => vi.fn<() => Promise<Footprint>>());

// Only the hooks this page uses, keeping the real TanStack behaviour so
// `enabled` genuinely decides whether a query function runs -- the same
// reasoning as the overview's suite.
//
// The seven disk hooks that used to be stubbed here are gone with the
// "What Headstate is using" panel (#796): the Disk page now shows only
// the machine's volumes, which come out of `useSystemHealth` like the
// rest of the sample. The Worktrees, Artifacts and Docker pages own
// those measurements and test them.
vi.mock("../api/hooks", () => ({
  useSystemHealth: (enabled: boolean) =>
    useQuery({ queryKey: ["system-health"], queryFn: liveFn, enabled, retry: false }),
  // The current conditions (#864). The CPU page renders them, so the
  // hook has to exist here; resolved empty because these tests are about
  // the detail panels rather than the alert panel, which
  // `SystemHealthPage.test.tsx` covers.
  useHealthAlerts: (enabled: boolean) =>
    useQuery({
      queryKey: ["health-alerts"],
      queryFn: () => Promise.resolve([]),
      enabled,
      retry: false,
    }),
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
  // The per-process network table (#718). The cadence constants have
  // to be restated here because the module is mocked wholesale -- and
  // restating them is a drift risk, so `the cadence constants this file
  // restates match the real ones` below imports the real module and
  // pins them. Without that, a test asserting the page says "15
  // seconds" would keep passing after the shipped cadence changed.
  NET_PROCESSES_POLL_MS: MOCK_POLL_MS,
  NET_PROCESSES_SAMPLE_MS: MOCK_SAMPLE_MS,
  // `refetchInterval` is kept, at a test-sized 20ms rather than the
  // real fifteen seconds: the panel's whole reason for existing is what
  // happens when the SECOND reading lands, and a mock that only ever
  // fetches once could not exercise it. The interval is the one thing
  // shrunk; everything else is the real hook's shape.
  useNetworkProcesses: (enabled: boolean) =>
    useQuery({
      queryKey: ["system-network-processes"],
      queryFn: netProcFn,
      enabled,
      refetchInterval: enabled ? netProcPollMs.current : false,
      retry: false,
    }),
  // The sidebar renders `ViewSwitcher`, which reads the hidden-views
  // preference. Nothing hidden, so the switcher offers every view --
  // this file's subject is the class list beneath it.
  useUiPrefs: () => ({ prefs: { hidden_views: [] } }),
}));

const mobileBuild = vi.hoisted(() => ({ current: false }));
vi.mock("@/lib/target", () => ({
  get IS_MOBILE_BUILD() {
    return mobileBuild.current;
  },
  get IS_DESKTOP_BUILD() {
    return !mobileBuild.current;
  },
}));

vi.mock("@/api/connection", () => ({
  useConnectionState: () => ({ kind: "local" }),
  isStale: () => false,
}));

import { SystemHealthPage } from "./SystemHealthPage";
import { HEALTH_PAGES, SystemHealthSidebar, healthPagesFor } from "./SystemHealthSidebar";
import { ALL_HEALTH_PAGES, useFilters } from "@/store/filters";

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
  // Empty by DEFAULT, which is the majority platform: Windows, and
  // Intel or NVIDIA on Linux, report no readable GPU at all. The GPU
  // page (#717) must not exist on such a machine, and every test in
  // this file that does not opt into `gpu()` is asserting against that
  // machine. Empty rather than absent because that is what an
  // unreadable-GPU platform actually reports.
  gpus: [],
  disks: [
    { mount: "/", total: 500 * 1024 ** 3, available: 200 * 1024 ** 3, is_root: true },
    { mount: "/Volumes/spare", total: 1024 ** 4, available: 900 * 1024 ** 3, is_root: false },
  ],
  // Charge 82, capacity 84: two DIFFERENT numbers on purpose, so a
  // test that renders one where the other belongs cannot pass by them
  // happening to match.
  battery: { percent: 82, on_ac: false, capacity_percent: 84, cycle_count: 413 },
  thermal: "nominal",
  networks: [
    { name: "en0", rx_bytes: 8 * 1024 ** 3, tx_bytes: 2 * 1024 ** 3 },
    { name: "lo0", rx_bytes: 1024 ** 2, tx_bytes: 1024 ** 2 },
  ],
  uptime_secs: 3 * 86_400 + 4 * 3600,
  ...over,
});

/// Synthetic processes. Invented names, never a captured list -- this
/// repo is public. See the file header.
const proc = (
  pid: number,
  name: string,
  cpu_percent: number,
  memoryMb: number,
): FootprintProcess => ({ pid, name, cpu_percent, memory: memoryMb * 1024 ** 2 });

/// One GPU, as macOS reports it: a device figure plus the two pipeline
/// stages, and unified memory.
const gpu = (over: Partial<HealthGpu> = {}): HealthGpu => ({
  name: "Acme Graphics 900",
  utilization_percent: 62,
  memory_used: 3 * 1024 ** 3,
  memory_total: 10 * 1024 ** 3,
  unified_memory: true,
  renderer_percent: 71,
  tiler_percent: 14,
  ...over,
});

/// A grouped row. Invented names, like every other fixture here.
const group = (
  name: string,
  count: number,
  cpu_percent: number,
  memoryMb: number,
  cpu_unmeasured = 0,
): FootprintProcessGroup => ({
  name,
  count,
  cpu_percent,
  memory: memoryMb * 1024 ** 2,
  cpu_unmeasured,
});

/// The machine's processes, as the CPU and Memory pages read them.
///
/// `headstate` is in both lists on purpose: our own processes are
/// candidates on the same terms as everything else, and a page that hid
/// the app from its own top-eight would be the one view that lies about
/// the app. The three "are we why" fields this fixture used to carry
/// (`app`, `children`, `docker_daemon`) went with the footprint panel in
/// #795 -- what replaced the "is Headstate why" question is the plain
/// fact that we rank like anyone else.
const footprint = (over: Partial<Footprint> = {}): Footprint => ({
  sampled_at: new Date().toISOString(),
  top_cpu: [
    proc(701, "acme-render", 412, 900),
    proc(702, "widget-daemon", 96, 120),
    proc(4242, "headstate", 3, 220),
  ],
  top_memory: [
    proc(701, "acme-render", 412, 900),
    proc(4242, "headstate", 3, 220),
    proc(703, "cache-keeper", 0, 480),
  ],
  // Grouped (#721), shaped like the measurement on the issue: an
  // application running as many processes outranks, when summed, the
  // individually-larger single processes above -- and none of its
  // members appear in `top_cpu` at all.
  top_cpu_grouped: [
    group("acme-agent", 26, 13, 1690),
    group("acme-render", 1, 412, 900),
    group("widget-daemon", 2, 96, 240),
  ],
  top_memory_grouped: [
    group("acme-agent", 26, 13, 1690),
    group("acme-render", 1, 412, 900),
    group("cache-keeper", 1, 0, 480),
  ],
  process_count: 1436,
  ...over,
});

function renderPage() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <SystemHealthPage />
    </QueryClientProvider>,
  );
}

function renderSidebar() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <SystemHealthSidebar />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mobileBuild.current = false;
  liveFn.mockResolvedValue(sample());
  historyFn.mockResolvedValue([]);
  footprintFn.mockResolvedValue(footprint());
  // The majority platform by default: no unprivileged per-process
  // network attribution exists on Linux and none is built on Windows,
  // so an empty list is what most machines report. Tests that want the
  // macOS answer opt into it, exactly as `gpus` works above.
  netProcFn.mockResolvedValue([]);
  netProcPollMs.current = false;
  useFilters.setState({ healthPage: "overview" });
});

afterEach(() => {
  stubViewport(null);
  useFilters.setState({ healthPage: "overview" });
});

describe("the overview is still the landing page", () => {
  it("opens on the overview and not on a detail page", async () => {
    renderPage();
    // The pressure cards are the overview's own, and no detail page
    // renders them: seeing them is proof we did not land elsewhere.
    await screen.findByRole("group", { name: /system pressure at a glance/i });
    expect(
      screen.queryByRole("heading", { level: 1, name: "CPU" }),
    ).toBeNull();
  });

  /// The inverse of what this asserted before (#795).
  ///
  /// It used to pin "What Headstate is costing" to the overview -- first
  /// because #665 put it there, then because the Disk page's copy of the
  /// disk half must not be read as having MOVED it. Both panels are gone
  /// now, and the absence is asserted rather than simply untested: the
  /// panel's value proposition ("is Headstate why this machine is slow")
  /// is the kind a reasonable person re-adds, and a failing test is a
  /// cheaper place to read the argument than an issue thread.
  it("has no panel claiming to say whether Headstate is the problem", async () => {
    renderPage();
    // Landed, and fully: the pressure cards are the overview's own.
    await screen.findByRole("group", { name: /system pressure at a glance/i });
    expect(screen.queryByText("What Headstate is costing")).toBeNull();
    // Nor its two halves under any other heading. The live half named
    // our own process and the tools we spawn; the disk half sat behind
    // a Measure button.
    expect(screen.queryByText(/Tools Headstate is running/)).toBeNull();
    expect(
      screen.queryByRole("button", { name: /measure disk use/i }),
    ).toBeNull();
  });
});

describe("the CPU page names the processes responsible", () => {
  beforeEach(() => useFilters.setState({ healthPage: "cpu" }));

  it("lists the machine's top processes, biggest first", async () => {
    renderPage();
    await screen.findByText("What is using the CPU");
    const table = await screen.findByRole("table");
    const rows = within(table).getAllByRole("row").slice(1);
    // The order the Rust side chose, preserved: the first row is the
    // answer, and a client that re-sorted could disagree with the
    // count of what was left out.
    expect(rows[0].textContent).toContain("acme-render");
    expect(rows[1].textContent).toContain("widget-daemon");
  });

  it("says how many processes it is NOT showing", async () => {
    renderPage();
    // Eight rows presented as the whole machine is its own lie. The
    // count of the rest is what keeps a bounded list honest.
    await screen.findByText(/of 1436 processes running/i);
    await screen.findByText(/other 1433 are not listed/i);
  });

  it("shows CPU above 100% without clamping it", async () => {
    // `cpu_percent` is a share of ONE core, so a process using four
    // legitimately reads 412%. Clamping would report a busy process as
    // a merely saturated one, and hide the fan-out worth seeing.
    renderPage();
    await screen.findByText("412%");
  });

  it("renders an absent process list as not measured, never as an empty table", async () => {
    // A desktop too old to report this is not a machine with no
    // processes running -- which cannot happen on a booted machine.
    //
    // Reachable, not hypothetical, and it was checked when review
    // questioned it: the phone and desktop ship on independent tags and
    // are compatible on the wire PROTOCOL_VERSION, which adding fields
    // to a response does not bump. The version gate that would refuse
    // an older desktop applies to writes only, and this is a read. See
    // the note on `Footprint.top_cpu` in `types/pr.ts` for the full
    // argument.
    footprintFn.mockResolvedValue(footprint({ top_cpu: undefined }));
    renderPage();
    const panel = (await screen.findByText("What is using the CPU"))
      .closest("section") as HTMLElement;

    // Scoped to THIS panel: a page-wide search for "Not measured" would
    // pass on any other absent reading and prove nothing about this one.
    await waitFor(() =>
      expect(within(panel).getByText(/not measured/i)).toBeTruthy(),
    );
    // And no table at all, rather than a table of nothing. An empty
    // grid under a heading reads as "we looked and found none", which
    // is the opposite of what happened.
    expect(within(panel).queryByRole("table")).toBeNull();
    // The count sentence must be absent too: there is no total to
    // report, and inventing one would be the same class of lie.
    expect(within(panel).queryByText(/processes running/i)).toBeNull();
  });

  it("omits the count sentence when the total is missing but the list is not", async () => {
    // The three fields arrive together in practice, but they are
    // independently optional on the wire, and a list without a total
    // must not have one fabricated for it.
    footprintFn.mockResolvedValue(footprint({ process_count: undefined }));
    renderPage();
    const panel = (await screen.findByText("What is using the CPU"))
      .closest("section") as HTMLElement;
    await waitFor(() => expect(within(panel).getByRole("table")).toBeTruthy());
    expect(within(panel).queryByText(/processes running/i)).toBeNull();
  });

  it("explains that one pinned core hides inside the average", async () => {
    renderPage();
    await screen.findByText(/one core at 100% while the rest are idle is normal/i);
  });
});

describe("the Memory page names what is holding it", () => {
  beforeEach(() => useFilters.setState({ healthPage: "memory" }));

  it("lists the biggest resident sets, in the order Rust chose", async () => {
    renderPage();
    await screen.findByText("What is holding the memory");
    const table = await screen.findByRole("table");
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows[0].textContent).toContain("acme-render");
    // Chosen by MEMORY, so the 0%-CPU cache process is present and the
    // busy-but-small one is not -- proof the page reads `top_memory`
    // rather than re-sorting `top_cpu`, which would have dropped it.
    expect(rows[2].textContent).toContain("cache-keeper");
    expect(within(table).queryByText("widget-daemon")).toBeNull();
  });

  it("warns that resident sets do not add up to the total", async () => {
    renderPage();
    await screen.findByText(/counted once in every process holding it/i);
  });

  it("says a machine with no swap has none rather than 0% used", async () => {
    liveFn.mockResolvedValue(
      sample({
        memory: {
          total: 16 * 1024 ** 3,
          used: 8 * 1024 ** 3,
          available: 7 * 1024 ** 3,
          swap_total: 0,
          swap_used: 0,
        },
      }),
    );
    renderPage();
    await screen.findByText(/no swap configured, so nothing can be paged out/i);
  });
});

describe("the Disk page", () => {
  beforeEach(() => useFilters.setState({ healthPage: "disk" }));

  it("lists every volume and marks the system one", async () => {
    renderPage();
    await screen.findByText("Volumes");
    expect(screen.getByText("/Volumes/spare")).toBeTruthy();
    // The root volume carries the badge and the spare does not: the
    // one the app lives on is what a user filling their disk cares
    // about first, so it is named rather than left to be guessed.
    // `getAllByText` because the panel's own explanation says the word
    // too -- the badge is the first, inside the volume row.
    const badges = screen.getAllByText("system");
    expect(badges[0].className).toContain("rounded");
  });

  /// The inverse of what this asserted before (#796).
  ///
  /// It used to pin "What Headstate is using" to this page, behind a
  /// Measure button. The panel is gone: this page answers "is this disk
  /// filling up", a machine question the volumes above settle, while
  /// "which of that is ours" is answered better by the Worktrees,
  /// Artifacts and Docker pages -- which can act on it rather than only
  /// report it.
  ///
  /// Asserted rather than left untested, because the removal took the
  /// ~13s `size_worktrees` of #661 off this page entirely. An edit that
  /// brought any disk summary back would put it behind a button again,
  /// which is the shape that was already judged not worth having.
  it("shows the machine's volumes and nothing about Headstate's own use", async () => {
    renderPage();
    await screen.findByText("Volumes");
    expect(screen.queryByText("What Headstate is using")).toBeNull();
    expect(
      screen.queryByRole("button", { name: /measure disk use/i }),
    ).toBeNull();
  });
});

describe("the Network page", () => {
  beforeEach(() => useFilters.setState({ healthPage: "network" }));

  it("puts the busiest interface first", async () => {
    // Whatever order the platform listed them in: the interface that
    // carried the traffic is the one being looked for.
    liveFn.mockResolvedValue(
      sample({
        networks: [
          { name: "lo0", rx_bytes: 1024, tx_bytes: 1024 },
          { name: "en0", rx_bytes: 8 * 1024 ** 3, tx_bytes: 2 * 1024 ** 3 },
        ],
      }),
    );
    renderPage();
    await screen.findByText("Per interface");
    const meters = screen.getAllByRole("meter");
    expect(meters[0].getAttribute("aria-label")).toContain("en0");
  });

  /// #719: the panel that did not exist. A cumulative counter cannot
  /// show a spike; this is the chart that can.
  it("draws throughput over 24 hours, per interface", async () => {
    const at = (minsAgo: number) =>
      new Date(Date.now() - minsAgo * 60_000).toISOString();
    historyFn.mockResolvedValue([
      sample({
        sampled_at: at(3),
        networks: [{ name: "en0", rx_bytes: 0, tx_bytes: 0 }],
      }),
      sample({
        sampled_at: at(2),
        networks: [{ name: "en0", rx_bytes: 60_000_000, tx_bytes: 6_000_000 }],
      }),
      sample({
        sampled_at: at(1),
        networks: [{ name: "en0", rx_bytes: 120_000_000, tx_bytes: 12_000_000 }],
      }),
    ]);
    liveFn.mockResolvedValue(
      sample({ networks: [{ name: "en0", rx_bytes: 120_000_000, tx_bytes: 12_000_000 }] }),
    );
    renderPage();

    await screen.findByText("Throughput");
    // 60 MB per 60-second interval = 1 MB/s, and the unit says it is a
    // RATE rather than a quantity.
    await screen.findByText(/peak 1\.0 MB\/s/i);
    // Both directions are drawn, and separately: a single combined line
    // could not tell an upload from a download.
    expect(screen.getByTestId("sparkline-en0 received")).toBeTruthy();
    expect(screen.getByTestId("sparkline-en0 sent")).toBeTruthy();
  });

  /// **The mutation test, at the UI level.** A counter reset must reach
  /// the screen as a break in the line and a sentence naming it -- not
  /// as a plausible rate, and not as a silent zero.
  it("names a counter reset rather than drawing a rate through it", async () => {
    const at = (minsAgo: number) =>
      new Date(Date.now() - minsAgo * 60_000).toISOString();
    historyFn.mockResolvedValue([
      sample({
        sampled_at: at(4),
        networks: [{ name: "en0", rx_bytes: 4_000_000_000, tx_bytes: 0 }],
      }),
      sample({
        sampled_at: at(3),
        networks: [{ name: "en0", rx_bytes: 4_060_000_000, tx_bytes: 0 }],
      }),
      // The machine rebooted: the counter starts again from nothing.
      sample({
        sampled_at: at(2),
        networks: [{ name: "en0", rx_bytes: 1_000, tx_bytes: 0 }],
      }),
      sample({
        sampled_at: at(1),
        networks: [{ name: "en0", rx_bytes: 61_000, tx_bytes: 0 }],
      }),
    ]);
    liveFn.mockResolvedValue(
      sample({ networks: [{ name: "en0", rx_bytes: 61_000, tx_bytes: 0 }] }),
    );
    renderPage();

    await screen.findByText("Throughput");
    // The reset is explained, not left as an unexplained blank.
    await screen.findByText(/counter reset/i);
    // And the line is genuinely broken: the received sparkline reports
    // more than one measured run, which is what `data-runs` carries.
    const chart = screen.getByTestId("sparkline-en0 received");
    expect(Number(chart.getAttribute("data-runs"))).toBeGreaterThan(1);
  });

  /// #718: the panel that names what is using the network.
  ///
  /// Every process name in these fixtures is invented — see the file
  /// header.
  describe("what is using the network (#718)", () => {
    /// Synthetic rows, in the shape `nettop` reports after parsing.
    const netProc = (
      name: string,
      pid: number | null,
      bytes_in: number,
      bytes_out: number,
    ): NetProcess => ({ name, pid, bytes_in, bytes_out });

    /// **The ~5-second first reading is EXPLAINED, not spun through.**
    ///
    /// This is a state every user sees every time they open this page,
    /// and it lasts five seconds. A spinner that sits that long with no
    /// explanation is its own bug — the user's next move is to
    /// conclude the app has hung.
    it("says how long the first reading takes rather than spinning silently", async () => {
      // Never resolves: the panel is held in its waiting state for the
      // duration of the assertion, which is exactly the five seconds a
      // real user spends looking at it.
      netProcFn.mockReturnValue(new Promise<NetProcess[]>(() => {}));
      renderPage();
      await screen.findByText("What is using the network");
      // The duration is named, and so is the reason — "measuring" alone
      // would still leave the length of the wait a mystery.
      await screen.findByText(/takes about 5 seconds/i);
      await screen.findByText(/sampling for a full interval/i);
      await screen.findByText(/nothing is stuck/i);
    });

    /// **One reading is a TOTAL, and must not be presented as a rate.**
    ///
    /// `nettop` reports cumulative bytes since each process started, so
    /// the first reading can only rank by lifetime traffic. Labelling
    /// that "In/Out" per second would be a fabricated rate; the page
    /// says what the numbers are and when real rates arrive.
    it("labels the first reading as lifetime totals, not speeds", async () => {
      netProcFn.mockResolvedValue([
        netProc("acme-sync", 501, 6_000_000_000, 1_000_000),
        netProc("widget-daemon", 502, 1_000, 2_000),
      ]);
      renderPage();
      await screen.findByText("acme-sync");
      // The headings say quantity, not speed.
      expect(screen.getByRole("columnheader", { name: "Received" })).toBeTruthy();
      expect(screen.queryByRole("columnheader", { name: "In" })).toBeNull();
      // And the caveat is stated in words, including WHY and for how
      // long it applies.
      await screen.findByText(/lifetime totals/i);
      await screen.findByText(/a single reading cannot be a rate/i);
      await screen.findByText(/outranks one saturating the link right now/i);
    });

    /// Two readings ARE a rate, and the panel switches to saying so.
    ///
    /// The mutation this catches: a panel that always shows cumulative
    /// totals, never differencing, would keep the "lifetime totals"
    /// wording forever and quietly never answer the question the page
    /// exists for.
    it("shows a rate once a second reading lands", async () => {
      // The real cadence compressed to 20ms; see the mock. The counter
      // keeps climbing so every reading differences to a real rate.
      netProcPollMs.current = 20;
      let cumulative = 1_000;
      netProcFn.mockImplementation(() => {
        cumulative += 150_000;
        return Promise.resolve([netProc("acme-sync", 501, cumulative, 500)]);
      });
      renderPage();
      // First reading: totals only, because one reading is not a rate.
      await screen.findByText(/lifetime totals/i);
      // Second reading: the wording switches, and the "In"/"Out"
      // headings replace "Received"/"Sent".
      await screen.findByText(/rates over the last interval/i);
      expect(screen.getByRole("columnheader", { name: "In" })).toBeTruthy();
      expect(screen.queryByRole("columnheader", { name: "Received" })).toBeNull();
      // And the "lifetime totals" caveat is gone: leaving it up beside
      // real rates would be the same misreading in reverse.
      expect(screen.queryByText(/a single reading cannot be a rate/i)).toBeNull();
    });

    /// Two readings that share no process are not an empty table.
    ///
    /// Distinct from the empty-reading case below: here processes WERE
    /// reported, and none of them paired with the previous reading — a
    /// whole table turned over, which is what a laptop waking from
    /// sleep looks like. An empty table under live headings would read
    /// as a panel that failed to paint.
    it("explains a reading whose processes all turned over", async () => {
      netProcPollMs.current = 20;
      let generation = 0;
      netProcFn.mockImplementation(() => {
        generation += 1;
        // Every reading is a completely different set of PIDs, so
        // nothing ever pairs.
        return Promise.resolve([
          netProc(`acme-worker-${generation}`, 1000 + generation, 5_000, 5_000),
        ]);
      });
      renderPage();
      const note = await screen.findByText(/none of the 1 processes in this reading/i);
      // The whole explanation, not just the count: WHY there is no rate,
      // and that it is temporary.
      expect(note.textContent).toMatch(/none of them has an interval to measure/i);
      expect(note.textContent).toMatch(/rates return with the next pair/i);
      // And it is a sentence, not a table with no body rows.
      expect(screen.queryByRole("columnheader", { name: "In" })).toBeNull();
    });

    /// **A platform with no unprivileged route says so, with the
    /// reason.** #705's precedent: an evidenced "cannot be read" beats
    /// a silently empty panel, which reads as a broken one.
    it("names the reason on a platform that cannot be read unprivileged", async () => {
      netProcFn.mockResolvedValue([]);
      renderPage();
      await screen.findByText("What is using the network");
      // Not an empty table, and not "no processes are using the
      // network" — which on a booted machine would be a claim nobody
      // measured.
      expect(screen.queryByRole("columnheader", { name: "Process" })).toBeNull();
      await screen.findByText(/only macos reports network use per process/i);
      await screen.findByText(/per-namespace rather than per-process/i);
      await screen.findByText(/CAP_NET_ADMIN/);
    });

    /// A row whose label carried no parseable PID shows a dash rather
    /// than a fabricated number. The same "absent is not zero" rule as
    /// everywhere else here, and here it matters more than usual: a PID
    /// is the column a reader copies into `kill`.
    it("shows no pid rather than inventing one", async () => {
      netProcFn.mockResolvedValue([netProc("acme-relay", null, 5_000, 5_000)]);
      renderPage();
      const row = (await screen.findByText("acme-relay")).closest("tr");
      expect(row).not.toBeNull();
      expect(within(row as HTMLElement).getByText("Not measured")).toBeTruthy();
      expect(within(row as HTMLElement).queryByText("0")).toBeNull();
    });

    /// **The five-second reading must never reach the shared health
    /// timer.** #661's rule, and this is the worst case of it: one
    /// reading is as long as the whole poll interval, so on that timer
    /// the subprocesses would overlap forever.
    ///
    /// Asserted as "the page states its own cadence", which is the
    /// user-visible consequence: a panel driven by the health poll
    /// could not truthfully say it re-reads every fifteen seconds.
    it("states its own slower cadence and why", async () => {
      netProcFn.mockResolvedValue([netProc("acme-sync", 501, 1_000, 500)]);
      renderPage();
      await screen.findByText("acme-sync");
      await screen.findByText(
        new RegExp(`re-read every ${MOCK_POLL_MS / 1000} seconds`, "i"),
      );
      await screen.findByText(/only while this page is open/i);
      await screen.findByText(/too expensive for the poll driving the rest/i);
    });

    /// And it must not run on any OTHER page. The containment is that
    /// the component only mounts here; a panel added to the overview,
    /// or a hook enabled unconditionally, would put a five-second
    /// subprocess on every page in the view.
    it("does not read the per-process table from any other page", async () => {
      useFilters.setState({ healthPage: "cpu" });
      renderPage();
      await screen.findByText("What is using the CPU");
      expect(netProcFn).not.toHaveBeenCalled();
    });

    /// The cadence constants this file restates for the mock must match
    /// the real module's. Without this the prose assertions above would
    /// keep passing against numbers the shipped page no longer uses.
    it("restates the real cadence constants", async () => {
      const real = await vi.importActual<typeof import("@/api/hooks")>("@/api/hooks");
      expect(real.NET_PROCESSES_POLL_MS).toBe(MOCK_POLL_MS);
      expect(real.NET_PROCESSES_SAMPLE_MS).toBe(MOCK_SAMPLE_MS);
      // One reading must fit comfortably inside one interval, or two
      // `nettop` processes would be alive at once — the exact failure
      // that kept this off the five-second health poll.
      expect(real.NET_PROCESSES_SAMPLE_MS * 2).toBeLessThan(real.NET_PROCESSES_POLL_MS);
    });
  });

  it("says the bars are a share of traffic, not saturation", async () => {
    // Every other bar on this page means "how full". Reusing the shape
    // for a different meaning without saying so is how a reader
    // concludes an interface is 80% saturated.
    renderPage();
    await screen.findByText(/share of all traffic, not how saturated it is/i);
  });
});

describe("the Power page", () => {
  beforeEach(() => useFilters.setState({ healthPage: "power" }));

  it("distinguishes a mains-powered machine from a flat battery", async () => {
    liveFn.mockResolvedValue(sample({ battery: null }));
    renderPage();
    await screen.findByText(/mains-powered, which is not the same as a battery at zero/i);
  });

  /// #720's naming trap, guarded at the UI. "Battery health" normally
  /// means capacity relative to design, which is a DIFFERENT number
  /// from charge -- and the fixture deliberately gives them different
  /// values so a panel rendering one where the other belongs is
  /// visible rather than coincidentally right.
  it("keeps charge and capacity in separate panels", async () => {
    renderPage();

    // Charge, under a heading about charge.
    await screen.findByText("Battery");
    expect(screen.getByText("82%")).toBeTruthy();

    // Capacity, under its own heading, with the cycle count that makes
    // it readable.
    await screen.findByText("Battery capacity");
    expect(screen.getByText("84%")).toBeTruthy();
    expect(screen.getByText("413")).toBeTruthy();

    // And the page says in words that they are not the same figure --
    // the whole point, since "84%" beside a charge bar reads as charge.
    // Matched on a contiguous phrase: the sentence emphasises "not" in
    // its own element, so a regex spanning that would never match.
    await screen.findByText(/the charge above/i, { exact: false });
  });

  /// Capacity the platform will not report is said, not silently
  /// dropped: a panel that vanishes on Linux looks like a bug, and a
  /// zero would claim a dead cell.
  it("says capacity was not measured rather than showing a zero", async () => {
    liveFn.mockResolvedValue(
      sample({
        battery: { percent: 55, on_ac: false, capacity_percent: null, cycle_count: null },
      }),
    );
    renderPage();

    await screen.findByText("Battery capacity");
    // The charge is still a real reading.
    expect(screen.getByText("55%")).toBeTruthy();
    // The capacity is absent, and absent is never zero.
    expect(screen.queryByText("0%")).toBeNull();
    await screen.findByText(/macOS only/i);
  });


  /// #772, rendered.
  ///
  /// The reading that opened the issue: 103% of design on a new laptop,
  /// under copy telling its owner the battery had degraded. The panel
  /// must show the number -- it is correct -- and must not attach the
  /// worn-battery sentence to it.
  it("does not describe a battery above its design capacity as degraded", async () => {
    liveFn.mockResolvedValue(
      sample({
        battery: {
          percent: 96,
          on_ac: true,
          capacity_percent: 103,
          cycle_count: 12,
          power: null,
        },
      }),
    );
    renderPage();

    await screen.findByText("Battery capacity");
    // The number is unchanged: #772 is explicit that the calculation is
    // right and only the description is wrong.
    expect(screen.getByText("103%")).toBeTruthy();

    // The sentence that was false at this reading is gone...
    expect(screen.queryByText(/holds less than it once did/i)).toBeNull();
    // ...and the true one is there instead.
    await screen.findByText(/holds slightly more than the capacity it was rated for/i);
    await screen.findByText(/no wear has happened yet/i);

    // The cycle clause presumed wear too, and must not call twelve
    // cycles on a healthy cell "ordinary ageing".
    expect(screen.queryByText(/ordinary ageing/i)).toBeNull();
  });

  /// The visual half of #772: above 100 it must not overflow or read as
  /// an error state.
  it("draws a capacity over 100% inside its bar and not in red", async () => {
    liveFn.mockResolvedValue(
      sample({
        battery: {
          percent: 96,
          on_ac: true,
          capacity_percent: 103,
          cycle_count: 12,
          power: null,
        },
      }),
    );
    const { container } = renderPage();
    await screen.findByText("103%");

    const meter = container.querySelector(
      '[aria-label="Battery capacity relative to design"]',
    );
    expect(meter).toBeTruthy();
    const fill = meter!.querySelector("div") as HTMLElement;
    // Inside the track, so it cannot overflow or wrap its container.
    const width = Number.parseFloat(fill.style.width);
    expect(width).toBeGreaterThan(0);
    expect(width).toBeLessThanOrEqual(100);
    // Green, not the crimson the shared pressure scale would give a
    // reading this high.
    expect(fill.style.backgroundColor).not.toBe("rgb(248, 81, 73)");
  });

  /// A worn battery keeps the wording that was always correct for it,
  /// so the fix cannot have been made by deleting the sentence.
  it("still explains wear for a battery below its design capacity", async () => {
    renderPage();
    await screen.findByText("Battery capacity");
    // The default fixture is 84% after 413 cycles.
    await screen.findByText(/holds less than it once did/i);
    await screen.findByText(/wear after 413 cycles is ordinary ageing/i);
  });

  /// #773, on the page the issue asks for it.
  it("shows how fast the battery is draining, and which way", async () => {
    liveFn.mockResolvedValue(
      sample({
        battery: {
          percent: 64,
          on_ac: false,
          capacity_percent: 84,
          cycle_count: 413,
          // A DISCHARGE, which is the sign that would be nonsense if
          // the Rust side had read `Amperage` as unsigned.
          power: { watts: -18.4, milliamps: -1454, millivolts: 12654 },
        },
      }),
    );
    renderPage();

    await screen.findByText("Charge and drain rate");
    await screen.findByText("-18.4 W");
    // The direction in words as well as in the sign, since colour and a
    // minus sign are both easy to miss.
    await screen.findByText("discharging");
    // The two factors behind the product, so an implausible wattage can
    // be traced to whichever half is wrong.
    expect(screen.getByText("-1454 mA")).toBeTruthy();
    expect(screen.getByText("12.65 V")).toBeTruthy();
  });

  /// The #720 case, which is the reason #773 exists: the alert says it
  /// is happening, and this says how fast.
  it("calls out a battery draining while plugged in", async () => {
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
    renderPage();

    await screen.findByText("-6.2 W");
    await screen.findAllByText(/losing charge while plugged in/i);
  });

  /// Draining on BATTERY is completely ordinary and must not be
  /// flagged, or the flag stops meaning anything.
  it("does not flag an ordinary discharge on battery power", async () => {
    liveFn.mockResolvedValue(
      sample({
        battery: {
          percent: 64,
          on_ac: false,
          capacity_percent: 84,
          cycle_count: 413,
          power: { watts: -6.2, milliamps: -490, millivolts: 12654 },
        },
      }),
    );
    renderPage();
    await screen.findByText("-6.2 W");
    expect(screen.queryByText(/losing charge while plugged in/i)).toBeNull();
  });

  /// #705's rule, applied to the field this issue adds. A zero would be
  /// worse than a blank here than almost anywhere: zero watts is a real
  /// reading a full battery on mains sits at.
  it("says the rate was not measured rather than showing zero watts", async () => {
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
    renderPage();

    await screen.findByText("Charge and drain rate");
    expect(screen.queryByText("0.0 W")).toBeNull();
    // And the reason, per #705.
    await screen.findByText(/Windows publishes it too/i);
  });

  it("couples thermal and battery only when both facts are in hand", async () => {
    liveFn.mockResolvedValue(
      sample({
        thermal: "serious",
        // Capacity absent: this test is about the thermal coupling, and
        // a machine that does not report capacity must still render.
        battery: { percent: 40, on_ac: false, capacity_percent: null, cycle_count: null },
      }),
    );
    renderPage();
    await screen.findByText(/warm and on battery/i);
  });

  it("says nothing about the coupling when the machine is on AC", async () => {
    // An invented warning about a state the machine is not in would be
    // the page's own rule broken.
    liveFn.mockResolvedValue(
      sample({
        thermal: "serious",
        battery: { percent: 40, on_ac: true, capacity_percent: null, cycle_count: null },
      }),
    );
    renderPage();
    await screen.findByText("Thermal pressure");
    expect(screen.queryByText(/warm and on battery/i)).toBeNull();
  });
});

describe("the GPU page (#717)", () => {
  // Every test here needs a machine with a readable GPU: on one
  // without, there is deliberately no page, which is the block below.
  beforeEach(() => {
    liveFn.mockResolvedValue(sample({ gpus: [gpu()] }));
    useFilters.setState({ healthPage: "gpu" });
  });

  it("shows utilization over 24 hours, with gaps drawn as gaps", async () => {
    // The same treatment CPU and memory get, which is what #717 asks
    // for: two measured runs either side of a closure must be two
    // polylines, not one line drawn across the hours nobody measured.
    const now = Date.now();
    const at = (minsAgo: number, util: number) =>
      sample({
        sampled_at: new Date(now - minsAgo * 60_000).toISOString(),
        gpus: [gpu({ utilization_percent: util })],
      });
    historyFn.mockResolvedValue([
      at(600, 20),
      at(599, 22),
      // A six-hour hole: the app was not running.
      at(240, 40),
      at(239, 44),
    ]);
    renderPage();
    const chart = await screen.findByTestId("sparkline-GPU");
    expect(chart.getAttribute("data-runs")).toBe("2");
  });

  it("breaks the line where a sample carried no GPU reading", async () => {
    // Absent is not zero, in the chart: a sample taken while the app
    // was running but could not read the device must not be joined
    // through as if it were an idle GPU.
    const now = Date.now();
    const at = (minsAgo: number, util: number | null) =>
      sample({
        sampled_at: new Date(now - minsAgo * 60_000).toISOString(),
        gpus: [gpu({ utilization_percent: util })],
      });
    historyFn.mockResolvedValue([at(5, 30), at(4, null), at(3, 35)]);
    renderPage();
    const chart = await screen.findByTestId("sparkline-GPU");
    expect(chart.getAttribute("data-runs")).toBe("2");
  });

  it("reports the renderer and tiler separately from the device figure", async () => {
    // The whole reason the page carries a Pipeline stages panel: the
    // overview collapses these three into one number, and a GPU pinned
    // by its tiler is a different problem from one pinned by its
    // renderer.
    renderPage();
    const panel = (await screen.findByText("Pipeline stages"))
      .closest("section") as HTMLElement;
    // `getAllByText`: each stage figure appears twice in this panel --
    // once as a Stat and once beside its bar -- which is deliberate and
    // is why the assertion counts rather than demanding one.
    expect(within(panel).getAllByText("71%").length).toBeGreaterThan(0);
    expect(within(panel).getAllByText("14%").length).toBeGreaterThan(0);
    // And the device figure beside them, labelled as the one the
    // overview shows -- so the three are legible as three stages
    // rather than as a breakdown that fails to add up.
    expect(within(panel).getByText("62%")).toBeTruthy();
    // The bars are separately labelled, so a reader who cannot see the
    // two rows apart still hears which stage each belongs to.
    expect(
      within(panel).getByRole("meter", { name: /renderer utilization/i }),
    ).toBeTruthy();
    expect(within(panel).getByRole("meter", { name: /tiler utilization/i })).toBeTruthy();
    await screen.findByText(/tile-based deferred renderer/i);
  });

  it("says the platform does not split the stages rather than showing them as zero", async () => {
    // Linux/AMD: `gpu_busy_percent` is one figure with no breakdown.
    // Rows of "Not measured" would claim we tried to read two stages
    // that this hardware does not report at all, and a 0% renderer
    // would be a measurement nobody took.
    liveFn.mockResolvedValue(
      sample({
        gpus: [
          gpu({
            name: "card0",
            unified_memory: false,
            renderer_percent: null,
            tiler_percent: null,
          }),
        ],
      }),
    );
    renderPage();
    const panel = (await screen.findByText("Pipeline stages"))
      .closest("section") as HTMLElement;
    await waitFor(() =>
      expect(
        within(panel).getByText(/does not break it down by pipeline stage/i),
      ).toBeTruthy(),
    );
    expect(within(panel).queryByText(/^Renderer$/)).toBeNull();
  });

  it("treats a stage key the desktop never sent as absent, not as zero", async () => {
    // The fields are OPTIONAL on the wire as well as nullable -- a
    // desktop released before #717, or a sample stored before it,
    // carries no such key. `undefined` and `null` must render alike.
    liveFn.mockResolvedValue(
      sample({
        gpus: [
          {
            name: "Acme Graphics 900",
            utilization_percent: 62,
            memory_used: null,
            memory_total: null,
            unified_memory: true,
          } as HealthGpu,
        ],
      }),
    );
    renderPage();
    const panel = (await screen.findByText("Pipeline stages"))
      .closest("section") as HTMLElement;
    await waitFor(() =>
      expect(
        within(panel).getByText(/does not break it down by pipeline stage/i),
      ).toBeTruthy(),
    );
    // And no fabricated 0% anywhere in that panel.
    expect(within(panel).queryByText("0%")).toBeNull();
  });

  it("states the unified-memory caveat rather than implying the pools add up", async () => {
    // #705's caveat, at the depth a page allows. A reader comparing
    // this panel with the Memory page must not conclude the machine
    // has more RAM than it does.
    renderPage();
    await screen.findByText(/not additional memory/i);
    await screen.findByText(/more RAM than this one has/i);
  });

  it("says a discrete GPU's VRAM IS additional, which is the opposite claim", async () => {
    liveFn.mockResolvedValue(
      sample({ gpus: [gpu({ name: "card0", unified_memory: false })] }),
    );
    renderPage();
    await screen.findByText(/its own VRAM, separate from system memory/i);
    expect(screen.queryByText(/not additional memory/i)).toBeNull();
  });

  it("keeps an unreported memory figure absent rather than zero", async () => {
    liveFn.mockResolvedValue(
      sample({ gpus: [gpu({ memory_used: null, memory_total: null })] }),
    );
    renderPage();
    const panel = (await screen.findByText("Memory")).closest("section") as HTMLElement;
    await waitFor(() =>
      expect(within(panel).getAllByText(/not measured/i).length).toBeGreaterThan(0),
    );
    expect(within(panel).queryByText("0 B")).toBeNull();
  });

  it("gives each of two GPUs its own panels and its own history", async () => {
    // An Intel Mac reports integrated and discrete graphics. The
    // overview charts only the first; the page must not, or the second
    // device has a panel and no line.
    const two = sample({
      gpus: [gpu({ name: "Acme Integrated" }), gpu({ name: "Acme Discrete" })],
    });
    liveFn.mockResolvedValue(two);
    // History too: with no points a chart renders the "no history yet"
    // placeholder, and this test is about the SECOND device getting a
    // line of its own rather than about the placeholder appearing
    // twice. The second GPU's series differs, which is what proves the
    // charts are per-device rather than the first one drawn twice.
    const now = Date.now();
    historyFn.mockResolvedValue(
      [3, 2, 1].map((minsAgo) =>
        sample({
          sampled_at: new Date(now - minsAgo * 60_000).toISOString(),
          gpus: [
            gpu({ name: "Acme Integrated", utilization_percent: 10 }),
            gpu({ name: "Acme Discrete", utilization_percent: 90 }),
          ],
        }),
      ),
    );
    renderPage();
    await screen.findByText(/Acme Integrated — utilization/);
    await screen.findByText(/Acme Discrete — utilization/);
    await waitFor(() => expect(screen.getByTestId("sparkline-GPU 1")).toBeTruthy());
    expect(screen.getByTestId("sparkline-GPU 2")).toBeTruthy();
    // Two separate lines, each from its own device's series: a single
    // chart reused would give both the same run count AND the same
    // points, and the second device would be charting the first.
    const points = (id: string) =>
      screen.getByTestId(id).querySelector("polyline")?.getAttribute("points");
    expect(points("sparkline-GPU 1")).not.toBe(points("sparkline-GPU 2"));
  });

  it("explains why there is no list of what is using the GPU", async () => {
    // The CPU and Memory pages name the processes responsible, so a
    // reader expects it here. Saying why it is absent beats letting
    // them conclude the panel failed to load.
    renderPage();
    await screen.findByText(/does not attribute graphics work to individual processes/i);
  });
});

describe("a machine with no discoverable GPU gets no GPU page at all", () => {
  // #717's central rule, and #705's before it: an empty GPU page would
  // claim a device was found and could not be read, which on Windows
  // and on Intel or NVIDIA Linux is not what happened -- we did not
  // look, because there is no unprivileged way to.

  it("does not offer the row in the sidebar", async () => {
    renderSidebar();
    // The other pages are all there, so this is not a sidebar that
    // failed to render.
    await waitFor(() => expect(screen.getByRole("button", { name: "CPU" })).toBeTruthy());
    expect(screen.queryByRole("button", { name: "GPU" })).toBeNull();
  });

  it("offers the row once the machine reports one", async () => {
    liveFn.mockResolvedValue(sample({ gpus: [gpu()] }));
    // The sidebar reads the cache the page fills, so both are rendered
    // -- which is also how they sit in `App`.
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    render(
      <QueryClientProvider client={client}>
        <SystemHealthSidebar />
        <SystemHealthPage />
      </QueryClientProvider>,
    );
    await waitFor(() => expect(screen.getByRole("button", { name: "GPU" })).toBeTruthy());
  });

  it("renders the overview rather than an empty page if the page is opened anyway", async () => {
    // Reachable: the GPU can leave the sample after the page was
    // opened. Falling through to the overview is the honest answer --
    // there is nothing truthful to put on the page.
    useFilters.setState({ healthPage: "gpu" });
    renderPage();
    await screen.findByRole("group", { name: /system pressure at a glance/i });
    expect(screen.queryByText("Pipeline stages")).toBeNull();
  });

  it("hides the card from the phone's class list too", async () => {
    // NOTE: `stubViewport` is what makes this the phone layout --
    // jsdom has no `matchMedia`, so without it this renders desktop.
    stubViewport(400);
    renderPage();
    const nav = await screen.findByRole("navigation", { name: /system health sections/i });
    expect(within(nav).getByRole("button", { name: /CPU/ })).toBeTruthy();
    expect(within(nav).queryByRole("button", { name: /^GPU/ })).toBeNull();
  });

  it("shows the card on the phone once there is a GPU", async () => {
    stubViewport(400);
    liveFn.mockResolvedValue(sample({ gpus: [gpu()] }));
    renderPage();
    const nav = await screen.findByRole("navigation", { name: /system health sections/i });
    await waitFor(() =>
      expect(within(nav).getByRole("button", { name: /^GPU/ })).toBeTruthy(),
    );
  });

  it("filters the same list for both surfaces, from one helper", () => {
    // The sidebar and the phone's cards must not drift into offering
    // different pages -- the failure one shared `HEALTH_PAGES` array
    // exists to prevent, which a second copy of the GPU rule would
    // undo.
    expect(healthPagesFor(0).map((p) => p.id)).not.toContain("gpu");
    expect(healthPagesFor(1).map((p) => p.id)).toContain("gpu");
    expect(healthPagesFor(1).length).toBe(HEALTH_PAGES.length);
  });
});

describe("a core count that was never measured (#958)", () => {
  beforeEach(() => useFilters.setState({ healthPage: "cpu" }));

  /// A platform that reports load averages but not per-core usage is a
  /// real state, and it is the one this got wrong: the prose was guarded
  /// on `load === null`, so it rendered "On this machine's 0 cores, a load
  /// near 0 means it is busy but keeping up" -- telling a reader that a
  /// machine at load 2.41 with work queuing was coping.
  ///
  /// The paragraph exists to stop the most misread number on the page
  /// being misread, so in that state it caused the error it is for.
  it("does not claim the machine has zero cores", async () => {
    liveFn.mockResolvedValue(sample({ cpu_per_core: [], load: [2.41, 2.18, 1.95] }));
    renderPage();
    // The load figure still renders, so this is the has-load-no-cores
    // case rather than a page that failed to load.
    expect(await screen.findByText("2.41")).toBeTruthy();
    expect(screen.queryByText(/0 cores/)).toBeNull();
    expect(screen.queryByText(/a load near 0 /)).toBeNull();
  });

  /// And says the count was not measured, rather than printing a 0 that
  /// looks like a reading. `Stat`'s own doc says the value "is never
  /// coerced to 0"; the call site passed a template string, which skipped
  /// that branch entirely.
  it("renders the core count as not measured", async () => {
    liveFn.mockResolvedValue(sample({ cpu_per_core: [], load: [2.41, 2.18, 1.95] }));
    renderPage();
    await screen.findByText("2.41");
    expect(screen.getAllByText(/not measured/i).length).toBeGreaterThan(0);
  });

  /// The general explanation survives -- a load average still needs
  /// explaining, and dropping the paragraph entirely would lose that.
  /// Only the arithmetic that needs a core count goes.
  it("still explains what a load average is", async () => {
    liveFn.mockResolvedValue(sample({ cpu_per_core: [], load: [2.41, 2.18, 1.95] }));
    renderPage();
    await screen.findByText("2.41");
    expect(screen.getByText(/number of processes wanting to run/i)).toBeTruthy();
    expect(screen.getByText(/did not report its core count/i)).toBeTruthy();
  });

  /// The measured case is unchanged, which is what keeps this a fix to one
  /// state rather than a removal of the explanation.
  it("keeps the core arithmetic when the count IS measured", async () => {
    liveFn.mockResolvedValue(sample({ cpu_per_core: [10, 26], load: [2.41, 2.18, 1.95] }));
    renderPage();
    await screen.findByText("2.41");
    expect(screen.getByText(/On this machine's 2 cores/i)).toBeTruthy();
  });
});

describe("grouping processes by name (#721)", () => {
  beforeEach(() => useFilters.setState({ healthPage: "cpu" }));

  it("lists individual processes by default", async () => {
    // Individual is the view that makes no inference: each row is one
    // process the kernel reported. Grouping is a heuristic, and a
    // heuristic on by default is one nobody chose.
    renderPage();
    const table = await screen.findByRole("table");
    expect(within(table).getByText("acme-render")).toBeTruthy();
    expect(within(table).queryByText("acme-agent")).toBeNull();
    expect(
      screen.getByRole("button", { name: "Individual" }).getAttribute("aria-pressed"),
    ).toBe("true");
  });

  it("sums by name with the count when Grouped is chosen", async () => {
    // The measured case on the issue: an application running as many
    // processes is invisible individually and dominates when summed.
    renderPage();
    await screen.findByRole("table");
    fireEvent.click(screen.getByRole("button", { name: "Grouped" }));

    const table = screen.getByRole("table");
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows[0].textContent).toContain("acme-agent");
    // The count is what stops a summed row being read as one process.
    expect(rows[0].textContent).toContain("(26)");
    expect(rows[0].textContent).toContain("13%");
  });

  it("keeps the count of what is NOT listed truthful under grouping", async () => {
    // The subtle one. "1436 total minus 3 rows" reads perfectly and is
    // wrong by however many siblings each group holds: three grouped
    // rows here cover 26 + 1 + 2 = 29 processes, so 1407 are not
    // listed, not 1433.
    renderPage();
    await screen.findByText(/other 1433 are not listed/i);
    fireEvent.click(screen.getByRole("button", { name: "Grouped" }));
    await screen.findByText(/covering 29 of 1436 processes running/i);
    await screen.findByText(/other 1407 are not listed/i);
  });

  it("drops the PID column under grouping rather than naming one member", async () => {
    // A group has no PID. Printing one of the twenty-six would name a
    // process the row is not about, and it is the column a reader
    // copies into `ps`.
    renderPage();
    const table = await screen.findByRole("table");
    expect(within(table).getByText("PID")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Grouped" }));
    expect(within(screen.getByRole("table")).queryByText("PID")).toBeNull();
  });

  it("says grouping is by name and not by ancestry", async () => {
    // The heuristic's error stated where it is in effect: two unrelated
    // programs that share a name are one row here, and the app cannot
    // establish otherwise without walking ancestry it deliberately does
    // not walk.
    renderPage();
    await screen.findByRole("table");
    fireEvent.click(screen.getByRole("button", { name: "Grouped" }));
    await screen.findByText(/groups by/i);
    await screen.findByText(/not by which process started which/i);
  });

  it("does not count an unreadable CPU figure as zero inside a sum", async () => {
    // Absent is not zero, inside a sum -- and a partial total presented
    // as a complete one is the same class of lie as a fabricated zero.
    footprintFn.mockResolvedValue(
      footprint({
        top_cpu_grouped: [group("acme-agent", 26, 12.5, 1690, 1)],
      }),
    );
    renderPage();
    await screen.findByRole("table");
    fireEvent.click(screen.getByRole("button", { name: "Grouped" }));
    await screen.findByText(/did not report/i);
    await screen.findByText(/left out of the total rather than counted as zero/i);
  });

  it("says nothing about partial sums when every process was measured", async () => {
    // The note must not appear on an ordinary machine, or it stops
    // being read on the rare one where it matters.
    renderPage();
    await screen.findByRole("table");
    fireEvent.click(screen.getByRole("button", { name: "Grouped" }));
    expect(screen.queryByText(/left out of the total/i)).toBeNull();
  });

  it("offers no toggle at all when the desktop did not send grouped rows", async () => {
    // A control that switches to an empty view is worse than no
    // control: the user would read it as a machine on which nothing
    // groups. Reachable via version skew, exactly like `top_cpu`.
    footprintFn.mockResolvedValue(
      footprint({ top_cpu_grouped: undefined, top_memory_grouped: undefined }),
    );
    renderPage();
    await screen.findByRole("table");
    expect(screen.queryByRole("button", { name: "Grouped" })).toBeNull();
    // And the individual list is unaffected -- the page still answers
    // its question.
    expect(screen.getByText("acme-render")).toBeTruthy();
  });

  it("groups the Memory page by summed memory, not by re-sorting the CPU groups", async () => {
    // The two lists answer different questions, and the fixture is
    // built so re-sorting one by the other measure would drop a row:
    // `cache-keeper` is in the memory groups and not in the CPU ones.
    useFilters.setState({ healthPage: "memory" });
    renderPage();
    await screen.findByRole("table");
    fireEvent.click(screen.getByRole("button", { name: "Grouped" }));
    const table = screen.getByRole("table");
    expect(within(table).getByText("cache-keeper")).toBeTruthy();
    expect(within(table).queryByText("widget-daemon")).toBeNull();
  });

  it("does not persist the mode across a remount", async () => {
    // A view mode that changes what a ROW MEANS should not silently
    // follow you between sessions: `acme-agent (26)` at 13% read as one
    // process at 13% is a wrong number with no visible cause. Local
    // state, so leaving the page resets it.
    renderPage();
    await screen.findByRole("table");
    fireEvent.click(screen.getByRole("button", { name: "Grouped" }));
    await screen.findByText(/covering 29 of 1436/i);

    // Away and back, the way the sidebar moves you.
    useFilters.setState({ healthPage: "memory" });
    useFilters.setState({ healthPage: "cpu" });
    const again = renderPage();
    await waitFor(() =>
      expect(
        within(again.container).getByRole("button", { name: "Individual" }).getAttribute(
          "aria-pressed",
        ),
      ).toBe("true"),
    );
  });
});

describe("the page list stays complete", () => {
  /// Every page in the union is offered, and nothing is offered that
  /// is not in it.
  ///
  /// The same guard `SettingsDialog.test.tsx` puts on `ALL_VIEWS`, and
  /// for the same reason: `HEALTH_PAGES` is a hand-written array beside
  /// a union, so adding a page to the union without adding it here
  /// gives you a page reachable by no control -- and the sidebar looks
  /// complete either way, which is why nobody would notice. Derived
  /// from the union rather than written out, because a second literal
  /// list is one that gets edited to match and stops checking anything.
  it("offers exactly the pages the union declares", () => {
    expect(HEALTH_PAGES.map((p) => p.id).sort()).toEqual([...ALL_HEALTH_PAGES].sort());
  });

  it("renders a body for every page, not a fallback", async () => {
    // The detail shell's final branch is an `else`, so a page added to
    // the union and to HEALTH_PAGES but not to the switch would render
    // the Power page under someone else's heading -- wrong content
    // under a correct title, which is the hardest kind to spot.
    //
    // Each page's own first panel title, which no other page uses.
    const expected: Record<string, RegExp> = {
      cpu: /what is using the cpu/i,
      memory: /what is holding the memory/i,
      disk: /^volumes$/i,
      // GPU (#717) needs a machine that HAS one -- on the default
      // fixture the page correctly does not exist, which is its own
      // test below.
      gpu: /^pipeline stages$/i,
      // "Throughput", not "since boot": #719 gave the network page a
      // 24-hour history, and that chart is what the page now leads
      // with. The since-boot totals remain, lower down.
      network: /^throughput$/i,
      power: /^battery$/i,
    };
    // Every page needs a machine that can offer it. Only GPU is
    // conditional, so only GPU needs the sample swapped.
    liveFn.mockResolvedValue(sample({ gpus: [gpu()] }));
    for (const page of ALL_HEALTH_PAGES) {
      if (page === "overview") continue;
      useFilters.setState({ healthPage: page });
      const { unmount } = renderPage();
      // Awaited: the live sample has to land before any page has a
      // body, so a synchronous assertion would fail on all of them.
      await screen.findByText(expected[page]);
      unmount();
    }
  });
});

describe("navigation", () => {
  it("moves between pages from the sidebar", async () => {
    renderSidebar();
    fireEvent.click(screen.getByRole("button", { name: "Memory" }));
    expect(useFilters.getState().healthPage).toBe("memory");
    // Marked as a LOCATION rather than a pressed toggle: these are
    // navigation, and "pressed" describes a control that did something.
    expect(
      screen.getByRole("button", { name: "Memory" }).getAttribute("aria-current"),
    ).toBe("page");
  });

  it("returns to the overview when the view changes", async () => {
    // Coming back to System Health lands on the landing page. A
    // drill-down answered a question that is now behind you.
    useFilters.setState({ healthPage: "cpu" });
    useFilters.getState().setView("worktrees");
    expect(useFilters.getState().healthPage).toBe("overview");
  });

  it("is not persisted, so a relaunch opens on the overview", () => {
    // The store's `partialize` decides what survives a relaunch.
    // Persisting the drill-down would skip the page that says whether
    // there is anything worth drilling into today.
    useFilters.setState({ healthPage: "cpu" });
    const stored = JSON.parse(
      window.localStorage.getItem("headstate-filters") ?? "{}",
    ) as { state?: Record<string, unknown> };
    expect(stored.state ?? {}).not.toHaveProperty("healthPage");
  });
});

describe("navigation at phone width", () => {
  // NOTE: without `stubViewport` these render the DESKTOP layout
  // whatever the describe block is called -- jsdom has no `matchMedia`.
  beforeEach(() => stubViewport(400));

  it("puts the class list on the overview, since there is no sidebar", async () => {
    renderPage();
    const nav = await screen.findByRole("navigation", { name: /system health sections/i });
    // Every class the sidebar offers except the one you are on.
    for (const label of ["CPU", "Memory", "Disk", "Network", "Power and uptime"]) {
      expect(within(nav).getByRole("button", { name: new RegExp(label) })).toBeTruthy();
    }
    expect(within(nav).queryByRole("button", { name: /^Overview/ })).toBeNull();
  });

  it("opens a detail page from a card and comes back", async () => {
    renderPage();
    const nav = await screen.findByRole("navigation", { name: /system health sections/i });
    fireEvent.click(within(nav).getByRole("button", { name: /CPU/ }));
    await screen.findByText("What is using the CPU");

    // The back control is the ONLY way off a detail page on a phone,
    // which is why it is asserted rather than assumed.
    fireEvent.click(screen.getByRole("button", { name: /system health overview/i }));
    await screen.findByRole("group", { name: /system pressure at a glance/i });
  });

  it("shows no class cards on the desktop, where the sidebar has them", async () => {
    stubViewport(1400);
    renderPage();
    await screen.findByRole("group", { name: /system pressure at a glance/i });
    expect(
      screen.queryByRole("navigation", { name: /system health sections/i }),
    ).toBeNull();
  });

  it("shows no back control on the desktop, where the sidebar is persistent", async () => {
    stubViewport(1400);
    useFilters.setState({ healthPage: "cpu" });
    renderPage();
    await screen.findByText("What is using the CPU");
    expect(
      screen.queryByRole("button", { name: /system health overview/i }),
    ).toBeNull();
  });
});

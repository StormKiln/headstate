/// The pure logic behind the System Health view: deciding where a
/// series has holes in it, and turning raw readings into the words and
/// colours the panels show.
///
/// Separated from `SystemHealthPage` the way `lib/worktrees.ts` and
/// `lib/docker.ts` are separated from their views. Two reasons, and the
/// second is the one that matters: a module of plain functions is
/// testable without rendering anything, and the gap detection below is
/// the single most important piece of correctness in the whole view --
/// it is what decides whether the chart tells the truth.

/// The interval the Rust sampler writes at, in milliseconds.
///
/// Mirrors the 60-second sleep in `src-tauri/src/lib.rs`. Used as the
/// floor for what counts as ordinary spacing, so it must track that
/// loop; if the sampler's cadence changes, this changes with it.
export const SAMPLE_INTERVAL_MS = 60_000;

/// How many times the typical spacing makes a gap.
///
/// `store::health::history` downsamples by taking every Nth ROW, so two
/// adjacent points in a dense series are legitimately N minutes apart
/// -- that spacing is the resolution, not a hole. So the threshold is
/// derived from the series' own median spacing rather than from the raw
/// 60s cadence, and this multiplier is what separates "one sample
/// further apart than usual" from "the app was closed".
const GAP_FACTOR = 2.5;

/// A spacing that is a gap however typical it is for this series.
///
/// The relative threshold alone has a blind spot: a series of two
/// points six hours apart has a MEDIAN spacing of six hours, so nothing
/// in it exceeds `median * GAP_FACTOR` and the pair would join into a
/// line across a period nobody measured -- exactly the claim this
/// module exists to refuse.
///
/// Thirty minutes is comfortably above the coarsest legitimate spacing
/// the backend can produce: `MAX_POINTS` is 120 across a 24-hour
/// retention window, so even a completely full series is bucketed at
/// twelve minutes and nothing regular is ever wider than that.
///
/// MUST equal `GAP_MS` in `src-tauri/src/health/alerts.rs`, which applies
/// the same rule on the alert path. If the chart draws a series as broken
/// where the alerts compute a discharge rate across it, the user is told
/// a rate the picture refuses to draw.
///
/// EXPORTED for that assertion (#850). It was private, so `alerts.rs`'
/// `a_gap_here_is_a_gap_in_the_charts_too` -- the test whose own doc
/// claims the agreement is asserted "rather than left to whoever edits
/// one of them next" -- could only compare `GAP_MS` to a Rust literal,
/// reading one side twice. `mirroredConstants.test.ts` now reads both.
export const ABSOLUTE_GAP_MS = 30 * 60_000;

/// One point of a series: a time, and a value that may be absent.
export interface Point {
  /// Epoch milliseconds.
  t: number;
  /// `null` where the sample carried no reading. Such a point breaks
  /// the line exactly like a gap does -- for the same reason.
  v: number | null;
}

/// The median spacing between consecutive points, in ms.
///
/// The MEDIAN, not the mean: the mean of a series with a six-hour hole
/// in it is dragged upward by that hole, so the very gap being looked
/// for would raise the threshold above itself and hide. The median of a
/// mostly-regular series is simply the regular spacing.
export function medianSpacing(points: Point[]): number {
  if (points.length < 2) return SAMPLE_INTERVAL_MS;
  const deltas: number[] = [];
  for (let i = 1; i < points.length; i += 1) deltas.push(points[i].t - points[i - 1].t);
  deltas.sort((a, b) => a - b);
  const mid = Math.floor(deltas.length / 2);
  const median =
    deltas.length % 2 === 0 ? (deltas[mid - 1] + deltas[mid]) / 2 : deltas[mid];
  // Never below the sampler's own cadence: a very short series can have
  // a median of a few seconds if two rows land close together, and a
  // threshold derived from that would call every ordinary minute a gap.
  return Math.max(median, SAMPLE_INTERVAL_MS);
}

/// Split a series into the runs that were actually measured.
///
/// **This is the honesty requirement of the whole view.** The sampler
/// runs only while the app is open, so a laptop closed overnight leaves
/// two points twelve hours apart. Drawing a segment between them claims
/// the metric held that value all night -- a fact nobody collected. So
/// the series is cut wherever the spacing exceeds the threshold, and
/// each run is drawn as its own polyline with nothing between them.
///
/// A `null` VALUE cuts the run too. "The platform did not report CPU
/// for these ten minutes" is the same kind of unknown as "the app was
/// not running", and interpolating across it would be the same lie in a
/// different costume.
///
/// Runs of a single point survive: one measurement is still a
/// measurement, and the caller draws it as a dot rather than dropping
/// it, which would under-report what the app actually saw.
export function splitOnGaps(points: Point[]): Point[][] {
  // The tighter of the two rules. Relative spacing handles a
  // downsampled series, whose stride is legitimately wide; the absolute
  // ceiling handles a series too short for its own median to mean
  // anything.
  const threshold = Math.min(medianSpacing(points) * GAP_FACTOR, ABSOLUTE_GAP_MS);
  const runs: Point[][] = [];
  let run: Point[] = [];
  for (const p of points) {
    if (p.v === null) {
      // Not measured: close whatever run was open and skip the point.
      if (run.length > 0) runs.push(run);
      run = [];
      continue;
    }
    const prev = run[run.length - 1];
    if (prev && p.t - prev.t > threshold) {
      runs.push(run);
      run = [];
    }
    run.push(p);
  }
  if (run.length > 0) runs.push(run);
  return runs;
}

/// The colour a utilisation bar takes at a given percentage.
///
/// Three bands rather than a gradient: the question a user asks of this
/// view is "is anything about to run out", which has a yes/nearly/no
/// answer. The thresholds are deliberately high -- a machine at 70%
/// memory is a machine being used, not a machine in trouble, and
/// colouring that amber trains people to ignore the colour, which costs
/// the red band the only thing it has.
export function barColor(percent: number): string {
  if (percent >= 90) return "#f85149";
  if (percent >= 75) return "#d29922";
  return "#3fb950";
}

/// The colour of a thermal pressure label.
///
/// `nominal` is grey rather than green on purpose: green reads as an
/// achievement, and a machine at rest is the ordinary case, not a good
/// one. The escalation is where the colour earns attention.
export function thermalColor(label: string): string {
  switch (label) {
    case "critical":
      return "#f85149";
    case "serious":
      return "#d29922";
    case "fair":
      return "#58a6ff";
    default:
      return "#8b949e";
  }
}

/// What each thermal pressure label means, in one short phrase.
///
/// The labels come from the platform and are not self-explanatory:
/// "serious" alone does not tell anyone whether to close something.
export const THERMAL_MEANING: Record<string, string> = {
  nominal: "Running cool; nothing is being held back.",
  fair: "Warm. The system may be trimming performance slightly.",
  serious: "Hot. The system is actively slowing itself to cope.",
  critical: "Very hot. Performance is heavily limited.",
};

/// Seconds since boot, in words a person reads.
export function formatUptime(secs: number): string {
  const d = Math.floor(secs / 86_400);
  const h = Math.floor((secs % 86_400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m`;
}

/// Percentage of a total, or `null` when the total is zero.
///
/// Zero total means the platform reported nothing, and the two failures
/// this prevents are both real: a disk of size 0 is not a full disk,
/// and `used / 0` is NaN, which renders as the string "NaN%" and looks
/// like a bug because it is one.
export function percentOf(part: number, total: number): number | null {
  if (!total || !Number.isFinite(total)) return null;
  return (part / total) * 100;
}

/// # Network throughput: differencing a counter that can restart
///
/// `Interface.rx_bytes` / `tx_bytes` are CUMULATIVE since boot, so a
/// rate is the difference between two consecutive samples divided by
/// the time between them. Two things make that harder than it sounds,
/// and both are why the code below is not a one-line `map`.
///
/// **Counters reset.** An interface that goes down, or a machine that
/// reboots, restarts its byte count from zero. The naive difference is
/// then a large NEGATIVE number. Clamping it to zero would be worse
/// than useless: it draws a reset as a quiet moment, which is a claim
/// about the traffic rather than an admission that the counter is no
/// longer comparable. So a reset produces `null` -- and `null` is
/// already the value `splitOnGaps` breaks a run on, so the chart draws
/// the reset as a gap with no further arrangement.
///
/// **Gaps.** The same holes every other series has. They need no new
/// machinery either: a differenced point carries the NEWER sample's
/// timestamp, so a pair spanning a six-hour closure is a point six
/// hours after its neighbour, and `splitOnGaps` cuts it exactly as it
/// cuts a CPU series.
///
/// That is the whole design: emit `null` for anything not comparable,
/// and let the one existing gap rule handle both cases.

/// Bytes per second across one interval, or `null` where no rate can be
/// computed from it.
export interface RatePoint extends Point {
  /// Why this point has no value, for a UI that wants to say which.
  /// `undefined` on a point that HAS one.
  ///
  /// The distinction is worth carrying: "the app was not running" and
  /// "this counter restarted" are different facts about the same blank
  /// stretch, and a panel that can name which one saves the reader
  /// guessing.
  reason?: "gap" | "reset";
}

/// One sample's worth of one interface's counters.
///
/// Structural rather than importing `HealthSample`, keeping this module
/// free of the API types the way the rest of it is.
export interface CounterSample {
  /// Epoch milliseconds.
  t: number;
  rx_bytes: number;
  tx_bytes: number;
}

/// The smallest backwards step treated as a counter reset.
///
/// Exactly zero tolerance would be wrong: `sysinfo` reads the
/// platform's counters, and a reading taken mid-update can come back a
/// few bytes behind its predecessor without anything having restarted.
/// A byte or two backwards is noise; a counter that genuinely reset
/// drops by its whole accumulated value, which on any interface that
/// has carried traffic is orders of magnitude more than this.
///
/// Deliberately small. The failure to avoid is treating a REAL reset as
/// noise and emitting a plausible rate from it, so the tolerance stays
/// far below any reset worth catching.
const RESET_TOLERANCE_BYTES = 4096;

/// Bytes per second between consecutive samples of one counter.
///
/// `samples` is oldest-first. The result has one fewer point than the
/// input -- a rate belongs to an INTERVAL, not to an instant -- and each
/// point carries the newer sample's time, so it sits where the traffic
/// was measured rather than where the interval began.
///
/// Returns `null` values, never zeroes and never clamped negatives, for
/// every interval that cannot honestly be turned into a rate.
export function counterRates(
  samples: CounterSample[],
  pick: (s: CounterSample) => number,
): RatePoint[] {
  const out: RatePoint[] = [];
  for (let i = 1; i < samples.length; i += 1) {
    const prev = samples[i - 1];
    const cur = samples[i];
    const seconds = (cur.t - prev.t) / 1000;
    const delta = pick(cur) - pick(prev);

    if (delta < -RESET_TOLERANCE_BYTES) {
      // The counter restarted. NOT clamped to zero: that would draw a
      // reboot as an idle minute, a measurement nobody took.
      out.push({ t: cur.t, v: null, reason: "reset" });
      continue;
    }
    // Non-positive spacing means two rows share an instant or arrived
    // out of order; dividing by it yields Infinity, which renders as a
    // spike off the top of any chart.
    if (seconds <= 0) {
      out.push({ t: cur.t, v: null, reason: "gap" });
      continue;
    }
    // A small backwards step inside the tolerance is noise, and zero is
    // the honest reading for it -- the counter did not advance.
    out.push({ t: cur.t, v: Math.max(0, delta) / seconds });
  }
  return out;
}

/// Every interface's received and sent rate over the series.
///
/// Keyed by interface name. An interface that appears partway through
/// -- a VPN coming up, a cable being plugged in -- simply has no points
/// before it existed, which is the truth: nothing was measured for it
/// then.
///
/// Interfaces are NOT summed into a machine total. Two of them carrying
/// the same traffic (a bridge and its member, a VPN and the physical
/// link beneath it) would double-count, and #719 asks for per-interface
/// history precisely because an aggregate hides which link did the
/// work.
export function interfaceRates(
  samples: {
    sampled_at: string;
    networks: { name: string; rx_bytes: number; tx_bytes: number }[];
  }[],
): Map<string, { rx: RatePoint[]; tx: RatePoint[] }> {
  // Per interface, only the samples that actually carried it. A sample
  // in which an interface is absent is not a zero for that interface --
  // it is a moment the interface did not exist, and pairing across it
  // would difference two readings with an unmeasured stretch between.
  const byName = new Map<string, CounterSample[]>();
  for (const s of samples) {
    const t = Date.parse(s.sampled_at);
    if (!Number.isFinite(t)) continue;
    for (const n of s.networks) {
      const list = byName.get(n.name) ?? [];
      list.push({ t, rx_bytes: n.rx_bytes, tx_bytes: n.tx_bytes });
      byName.set(n.name, list);
    }
  }

  const out = new Map<string, { rx: RatePoint[]; tx: RatePoint[] }>();
  for (const [name, list] of byName) {
    out.set(name, {
      rx: counterRates(list, (s) => s.rx_bytes),
      tx: counterRates(list, (s) => s.tx_bytes),
    });
  }
  return out;
}

/// One reading of the per-process network table (#718).
///
/// Structural rather than importing `NetProcess`, keeping this module
/// free of the API types the way the rest of it is.
export interface NetProcessReading {
  /// Epoch milliseconds at which this reading was taken.
  t: number;
  processes: {
    name: string;
    pid: number | null;
    bytes_in: number;
    bytes_out: number;
  }[];
}

/// One process's traffic between two readings.
export interface NetProcessRate {
  name: string;
  pid: number | null;
  /// Bytes per second received across the interval.
  in_rate: number;
  /// Bytes per second sent across the interval.
  out_rate: number;
  /// Cumulative totals from the NEWER reading, carried alongside the
  /// rate because they answer a different and also real question: "what
  /// has this process moved altogether", which a rate cannot.
  bytes_in: number;
  bytes_out: number;
}

/// Turn two readings of the per-process table into rates.
///
/// # Why this function has to exist at all
///
/// `nettop` reports CUMULATIVE bytes since each process started, so a
/// single reading ranks a process that pulled 6 GB last week above one
/// saturating the link right now. Only the difference between two
/// readings is a rate, which is the entire reason the Network page is
/// ~20 seconds from opening to its first meaningful ordering rather
/// than ~5 -- and why the view must say so instead of looking broken.
///
/// # Matching by PID, then by name
///
/// A process is the same process across two readings when the PID
/// matches. Falling back to the NAME for a row with no PID is
/// deliberate but strictly second: PIDs are recycled, so matching on
/// name alone would difference two unrelated programs and produce a
/// spectacular fake rate. Where both readings carry PIDs, the name is
/// never consulted.
///
/// # What is NOT emitted, and why
///
/// - **A process only in the newer reading** (it started since) gets no
///   rate. Its cumulative total is not a delta from zero: it may have
///   been running and simply absent from the earlier table.
/// - **A process only in the older reading** (it exited) gets no rate.
///   There is nothing to difference, and reporting its last total as a
///   rate would attribute a week of traffic to fifteen seconds.
/// - **A counter that went backwards** gets no rate. Same reasoning as
///   `counterRates`: a PID recycled onto a different program looks
///   exactly like this, and a clamped zero would claim a measurement.
/// - **A non-positive interval** gets no rate, since dividing by it
///   yields `Infinity`.
///
/// Absent is not zero throughout: a process that cannot be turned into
/// a rate is simply not in the result, so the view can say how many
/// were dropped rather than showing them at 0 B/s.
export function netProcessRates(
  older: NetProcessReading,
  newer: NetProcessReading,
): NetProcessRate[] {
  const seconds = (newer.t - older.t) / 1000;
  if (!(seconds > 0)) return [];

  // PID first, name as the fallback key for rows that carry none. The
  // two key spaces are kept apart by the prefix, so a process named
  // "42" can never be matched against PID 42.
  const key = (p: { name: string; pid: number | null }) =>
    p.pid === null ? `n:${p.name}` : `p:${p.pid}`;
  const before = new Map(older.processes.map((p) => [key(p), p]));

  const out: NetProcessRate[] = [];
  for (const cur of newer.processes) {
    const prev = before.get(key(cur));
    // Started since the older reading, or matched nothing. No interval,
    // so no rate -- not a delta from zero.
    if (prev === undefined) continue;
    const dIn = cur.bytes_in - prev.bytes_in;
    const dOut = cur.bytes_out - prev.bytes_out;
    // Backwards on either counter means this is not the same process's
    // continuing accounting -- most likely a recycled PID. Dropped
    // rather than clamped, for the same reason `counterRates` emits
    // null on a reset.
    if (dIn < 0 || dOut < 0) continue;
    out.push({
      name: cur.name,
      pid: cur.pid,
      in_rate: dIn / seconds,
      out_rate: dOut / seconds,
      bytes_in: cur.bytes_in,
      bytes_out: cur.bytes_out,
    });
  }
  return out;
}

/// A bytes-per-second figure in words a person reads.
///
/// Separate from `formatSize` in `lib/worktrees` rather than wrapping
/// it: this is a RATE, and the unit has to say so. A panel that renders
/// throughput with the same helper as disk usage produces "4.2 MB"
/// where it means "4.2 MB/s", and the two read very differently to
/// anyone scanning the page.
///
/// Decimal rather than binary units: network throughput is quoted in
/// decimal everywhere -- a 1 Gb link, an ISP's advertised megabits --
/// and 1024 here would disagree with every figure a user compares this
/// against.
export function formatRate(bytesPerSecond: number): string {
  const units = ["B/s", "kB/s", "MB/s", "GB/s"];
  let v = bytesPerSecond;
  let u = 0;
  while (v >= 1000 && u < units.length - 1) {
    v /= 1000;
    u += 1;
  }
  return `${v < 10 && u > 0 ? v.toFixed(1) : Math.round(v)} ${units[u]}`;
}

/// The peak rate in a series, ignoring points that have no value.
///
/// Used as a chart's y-axis ceiling. `null` when nothing was measured,
/// which the caller renders as "Not measured" rather than as a chart
/// scaled to zero.
export function peakRate(points: RatePoint[]): number | null {
  let peak: number | null = null;
  for (const p of points) {
    if (p.v === null) continue;
    if (peak === null || p.v > peak) peak = p.v;
  }
  return peak;
}

/// # Battery capacity above 100% is normal, not a fault (#772)
///
/// `capacity_percent` is `NominalChargeCapacity / DesignCapacity`, and
/// `DesignCapacity` is a NAMEPLATE figure the manufacturer guarantees
/// -- not a ceiling the cell cannot exceed. New cells routinely measure
/// a few points above it, and 103% was the reading that opened #772.
///
/// The panel used to hardcode one sentence: "a battery at N% of its
/// original capacity still charges to 100%, it just holds less than it
/// once did". Below 100 that is correct. At 103 it is false in both
/// halves, and it tells someone with a brand-new laptop that their
/// battery has degraded.
///
/// So the wording branches on which side of 100 the reading falls, and
/// the branch lives here rather than inline in the view for the same
/// reason `splitOnGaps` does: it is a decision about what is TRUE, and
/// it should be testable without rendering anything.

/// Which of the three things a capacity reading means.
///
/// Three cases, not two. "Above" and "at" are collapsed easily and
/// should not be: a cell at exactly its nameplate has nothing to
/// report, while one above it is worth saying is above it -- otherwise
/// the number looks like a bug to the person reading it, which is
/// exactly what #772 describes.
export type CapacityStanding = "above" | "at" | "worn";

/// Where a capacity reading stands relative to its design figure.
///
/// The rounding is deliberate and matters: the panel prints
/// `toFixed(0)`, so a cell at 100.4% displays as "100%" and must not
/// then be described as holding MORE than its rated capacity -- the
/// sentence would be arguing with the number beside it. Both the
/// wording and the display therefore round the same way.
export function capacityStanding(percent: number): CapacityStanding {
  const shown = Math.round(percent);
  if (shown > 100) return "above";
  if (shown === 100) return "at";
  return "worn";
}

/// What the capacity panel says about a reading, in one sentence.
///
/// Written out per case rather than assembled from fragments: the three
/// sentences say genuinely different things, and a template with a
/// swapped adjective in the middle would be how the #772 wording drifts
/// back toward claiming wear.
export function capacityMeaning(percent: number): string {
  switch (capacityStanding(percent)) {
    case "above":
      // The #772 case. Note what this does NOT say: nothing about
      // charging to 100%, nothing about holding less. A cell above its
      // nameplate has lost nothing, and the reason the number can
      // exceed 100 is worth one clause -- otherwise it reads as an
      // arithmetic error.
      return (
        "This cell holds slightly more than the capacity it was rated for. " +
        "That rating is a figure the manufacturer guarantees rather than a " +
        "ceiling, so a new battery measuring a little over it is normal and " +
        "means no wear has happened yet."
      );
    case "at":
      return "This cell still holds the full capacity it was rated for — there is no wear to report.";
    case "worn":
      // The original sentence, which was always correct here.
      return (
        "It still charges to 100%, it just holds less than it once did — " +
        "which is ordinary, and happens slowly over years."
      );
  }
}

/// What the cycle count adds, given where the capacity stands.
///
/// `null` when there is nothing worth saying, which the caller renders
/// as nothing at all rather than as an empty sentence.
///
/// The old copy appended "wear after N cycles is ordinary ageing"
/// unconditionally, which presumes wear -- the same #772 bug one clause
/// further on. A cell at or above its nameplate has no wear to call
/// ordinary, so the count is reported as context for how much use the
/// battery has had instead.
export function cycleMeaning(
  percent: number,
  cycles: number | null,
): string | null {
  if (cycles === null) return null;
  return capacityStanding(percent) === "worn"
    ? `Read alongside the cycle count: wear after ${cycles} cycles is ordinary ageing.`
    : `It has been through ${cycles} charge ${cycles === 1 ? "cycle" : "cycles"} so far.`;
}

/// How far along its bar a capacity reading sits, 0-100.
///
/// # Why a capacity bar is not a percentage bar
///
/// Every other bar on this page fills toward 100 as "full". This one
/// cannot: the reading itself can exceed 100, and #772 asks that it
/// "should not overflow, wrap, or render as an error state".
///
/// So the bar's track is not 0-100 but 0-[`CAPACITY_BAR_MAX`], and a
/// healthy new cell simply sits a little past the four-fifths mark
/// rather than bursting out of a full one. The reading is still printed
/// as text beside it, so nothing is lost to the rescaling -- the bar
/// only has to carry "is this a lot or a little", and it does that
/// correctly in both directions.
export const CAPACITY_BAR_MAX = 120;

/// The bar's fill for a capacity reading, as a percentage of its track.
export function capacityBarFill(percent: number): number {
  return Math.max(0, Math.min(100, (percent / CAPACITY_BAR_MAX) * 100));
}

/// Where the "100%" scale mark belongs on the capacity track, as a
/// percentage of its width.
///
/// # Why this is a name rather than `justify-between`
///
/// The panel draws a label reading "Rated capacity: 100%" so the
/// stretched track above can be read at all. Before #981 that label was
/// the second child of a `justify-between` row, which pins it to the
/// container's RIGHT EDGE -- the `CAPACITY_BAR_MAX` end, i.e. the 120%
/// position. So a cell at exactly 100% drew its fill at 83.33% of the
/// track while the mark named "100%" sat at 100% of it: the one
/// comparison the panel exists to make read as "below nameplate" on a
/// battery that was exactly at nameplate, and every reading was
/// under-reported by the same 1.2x factor.
///
/// Derived from `capacityBarFill` rather than written as 83.33, because
/// the ONLY property that matters is that the mark and the fill share
/// one scale. A literal would be correct today and silently wrong the
/// first time `CAPACITY_BAR_MAX` is retuned -- which is exactly the
/// class of drift that produced the bug.
export const NAMEPLATE_MARK = capacityBarFill(100);

/// The colour of a capacity bar.
///
/// NOT `barColor`, and the inversion is the reason: `barColor` reads a
/// high percentage as pressure and turns it red, which is exactly
/// backwards for capacity, where high is healthy. A cell at 103% would
/// come out crimson -- the "renders as an error state" #772 names.
///
/// The bands are the ones Apple's own guidance implies: 80% of design
/// is where a battery is considered due for service, so that is the
/// amber threshold, and half its rated capacity is a cell that has
/// genuinely failed.
///
/// Ordered LOWEST first, unlike `barColor`. Both orderings read
/// naturally and only one is correct for each function -- capacity is
/// better when higher, pressure is worse when higher -- and getting it
/// backwards here makes every band after the first unreachable.
export function capacityColor(percent: number): string {
  if (percent < 50) return "#f85149";
  if (percent < 80) return "#d29922";
  return "#3fb950";
}

/// # Battery power flow: the rate, not the level (#773)

/// A wattage in words a person reads, with its direction in the sign.
///
/// Signed rather than "12.8 W in" / "12.8 W out": the minus sign is
/// read instantly and universally, and a chart axis crossing zero needs
/// the number to agree with it. One decimal place, because the reading
/// genuinely moves at that resolution and two would be false precision
/// from a gauge that reports whole milliamps.
export function formatWatts(watts: number): string {
  // `-0.0 W` is what `toFixed` produces for a tiny negative, and it
  // reads as a typo. Zero has no direction, so it prints without one.
  const v = Math.abs(watts) < 0.05 ? 0 : watts;
  return `${v.toFixed(1)} W`;
}

/// The band around zero that counts as no flow at all, in watts.
///
/// A tenth of a watt. Comfortably below any real charge or draw -- a
/// sleeping laptop draws several watts, a charging one tens -- and
/// comfortably above the jitter of a topped-up cell.
const IDLE_WATTS = 0.1;

/// Which way power is flowing, as a word.
///
/// The dead band matters. A battery sitting full on mains hovers within
/// a few tens of milliwatts of zero and flickers between a tiny charge
/// and a tiny discharge, and a label that flipped between "Charging"
/// and "Discharging" every five seconds would be alarming and
/// meaningless. Anything inside it is called idle, which is what it is.
export function powerDirection(watts: number): "charging" | "discharging" | "idle" {
  if (watts > IDLE_WATTS) return "charging";
  if (watts < -IDLE_WATTS) return "discharging";
  return "idle";
}

/// The colour a power reading takes.
///
/// Direction, not magnitude: a fast charge is not worse than a slow
/// one, so there is no escalation here of the kind `barColor` has.
/// Discharging is amber rather than red because being on battery is
/// completely normal -- red is reserved for the thing that is actually
/// wrong, which is discharging WHILE PLUGGED IN, and that is a
/// combination the panel checks rather than a wattage.
export function powerColor(watts: number, onAc: boolean): string {
  const dir = powerDirection(watts);
  // The #720 condition, and the one reading on this panel that is a
  // fault rather than a state: the adapter is not keeping up, or is not
  // really charging.
  if (dir === "discharging" && onAc) return "#f85149";
  if (dir === "discharging") return "#d29922";
  if (dir === "charging") return "#3fb950";
  return "#8b949e";
}

/// The battery power series, as points a chart can draw.
///
/// # Why this is not a `counterRates` job
///
/// `Interface.rx_bytes` is cumulative and has to be DIFFERENCED into a
/// rate. Watts are already a rate: each sample carries the flow
/// measured at that instant, so the series is a straight read with no
/// arithmetic across intervals at all -- and therefore none of
/// `counterRates`' reset handling either, since there is no counter to
/// reset.
///
/// What it does share is the `null` rule. A sample from before #773
/// shipped, or from a platform that does not publish the flow, has no
/// reading -- and that is an unknown `splitOnGaps` must break the line
/// across, exactly like a closed lid. It is emphatically not zero
/// watts, which is a real state a full plugged-in battery sits in.
export function powerSeries(
  samples: {
    sampled_at: string;
    battery: { power?: { watts: number } | null } | null;
  }[],
): Point[] {
  return samples.map((s) => ({
    t: Date.parse(s.sampled_at),
    v: s.battery?.power?.watts ?? null,
  }));
}

/// The largest ABSOLUTE flow in a series, for a chart's axis.
///
/// Absolute because the axis has to hold both directions: a series that
/// charged at 60 W and discharged at 12 W needs a ceiling of 60 either
/// side, or the discharge half would be drawn against a different scale
/// from the charge half and the two would not be comparable.
///
/// `null` when nothing was measured, which the caller renders as "Not
/// measured" rather than as a chart scaled to zero.
export function peakWatts(points: Point[]): number | null {
  let peak: number | null = null;
  for (const p of points) {
    if (p.v === null) continue;
    const m = Math.abs(p.v);
    if (peak === null || m > peak) peak = m;
  }
  return peak;
}

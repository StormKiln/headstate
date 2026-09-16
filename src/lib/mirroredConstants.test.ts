import { describe, expect, it } from "vitest";
// Vite's `?raw` rather than `node:fs`, for the reason
// `src/api/surfaceGuard.test.ts:3` already gives: the project
// deliberately carries no `@types/node`, so a filesystem read here would
// fail `yarn tsc -b`. `?raw` inlines the file at transform time and
// needs no ambient Node types.
import artifactsRs from "../../src-tauri/src/artifacts/mod.rs?raw";
import cleanupRs from "../../src-tauri/src/cleanup.rs?raw";
import alertsRs from "../../src-tauri/src/health/alerts.rs?raw";
import boardRs from "../../src-tauri/src/github/stats/board.rs?raw";
import companionRs from "../../src-mobile/src/companion.rs?raw";
import eventsRs from "../../src-tauri/src/remote/events.rs?raw";
import { ACTIVE_SECS } from "@/components/ArtifactsPage";
import { ACTIVITY_DAYS } from "@/components/ClaudeOverviewPage";
import { TOP_N } from "@/components/stats/Leaderboard";
import { ABSOLUTE_GAP_MS } from "./health";
import { AUTO_COMPACT_PRESSURE } from "./subagentDisagreement";
import { CANCELLED } from "./cancelled";
import {
  DEFAULT_STALE_DAYS,
  MAX_STALE_DAYS,
  MIN_STALE_DAYS,
  STALE_DAY_CHOICES,
  staleVenvDays,
} from "./staleVenv";

/// The constants that exist twice, once per language, and the assertion
/// that they still agree.
///
/// #850 found five such pairs whose doc comments said the agreement was
/// asserted. None of them read the other side. One -- the artifact
/// active-build window -- had already drifted to a different value in
/// each language while both comments went on claiming they matched, and
/// the drift was user-visible.
///
/// The shape of the failure each pair had is the reason this file exists
/// rather than more per-module tests. A Rust test asserting
/// `GAP_MS == 30 * 60_000` reads one side twice. A TS test asserting
/// `rows.length === TOP_N` reads the constant SYMBOLICALLY, so it passes
/// at any value. Both feel like agreement tests and neither can fail
/// when one side changes, which is the only event they exist to catch.
///
/// So every assertion here pulls the Rust literal out of Rust SOURCE and
/// compares it to the TypeScript value. The direction that matters is
/// that changing EITHER side alone fails: Rust-only changes break the
/// extracted number, TS-only changes break the imported one.
///
/// Two existing idioms do this already and both are deliberate choices
/// over a generated file: `src-mobile/src/surface.rs:250` reads the
/// desktop's table with `include_str!`, and `src/api/surfaceGuard.test.ts`
/// reads Rust source with `?raw`. `?raw` is used here because four of
/// the five pairs have their second copy in TypeScript.

/// The integer value of a Rust `const NAME: ty = <expr>;`, where the
/// expression is a product of literals -- `15 * 60`, `30 * 60 * 1000`,
/// `5`.
///
/// Parsed rather than regex-matched against one expected spelling,
/// because the spelling is not the invariant: `15 * 60` and `900` are
/// the same window, and a test that demanded the former would fail on a
/// correct edit while still passing on a wrong value written the
/// expected way. Rust's `_` digit separators are stripped for the same
/// reason -- `30 * 60_000` and `30 * 60000` are one number.
function rustConst(source: string, name: string, file: string): number {
  const m = source.match(new RegExp(`const ${name}\\s*:\\s*[A-Za-z0-9_]+\\s*=\\s*([^;]+);`));
  expect(m, `${file} must define ${name}`).toBeTruthy();
  const expr = m![1].trim();
  // A product of integer literals, and nothing else. Anything richer --
  // a reference to another constant, arithmetic this cannot evaluate --
  // must FAIL here rather than be silently approximated, because a
  // mirror test that quietly stops reading the real value is worse than
  // no mirror test: it would go on passing while describing a value the
  // other side no longer has.
  expect(expr, `${file}'s ${name} must be a product of integer literals, got ${expr}`).toMatch(
    /^[0-9_]+(\s*\*\s*[0-9_]+)*$/,
  );
  return expr
    .split("*")
    .map((part) => Number(part.trim().replace(/_/g, "")))
    .reduce((a, b) => a * b, 1);
}

/// The string value of a Rust `const NAME: &str = "...";`.
function rustStrConst(source: string, name: string, file: string): string {
  const m = source.match(new RegExp(`const ${name}\\s*:\\s*&str\\s*=\\s*"([^"]*)"\\s*;`));
  expect(m, `${file} must define ${name} as a string literal`).toBeTruthy();
  return m![1];
}

describe("the parser that reads the Rust side", () => {
  /// Guards the guard. Every assertion below is only as good as this
  /// extraction, and a regex that silently matched nothing -- or matched
  /// and mis-multiplied -- would make the whole file vacuously true.
  /// That is exactly the failure mode #850 is about, so it is asserted
  /// rather than assumed.
  it("evaluates a product of literals and strips digit separators", () => {
    expect(rustConst("const A: u64 = 15 * 60;", "A", "t")).toBe(900);
    expect(rustConst("const B: i64 = 30 * 60_000;", "B", "t")).toBe(1_800_000);
    expect(rustConst("const C: usize = 5;", "C", "t")).toBe(5);
    expect(rustStrConst('pub const D: &str = "x:y";', "D", "t")).toBe("x:y");
  });

  /// A constant this cannot evaluate must fail loudly. Silently reading
  /// `NaN` and comparing it to nothing would leave a dead mirror test
  /// looking alive.
  it("refuses an expression it cannot evaluate", () => {
    expect(() => rustConst("const E: u64 = OTHER * 60;", "E", "t")).toThrow();
  });
});

/// 1. The artifact active-build window: 15 minutes, in three places.
///
/// This is the pair that had already DRIFTED -- `15 * 60` in Rust
/// against `60 * 60` in the UI, with comments on both sides asserting
/// they matched. The consequence was user-visible in three ways at once
/// for any `target/` written between 15 and 60 minutes ago: the backend
/// would remove it, but the UI left it out of `removable` so the Remove
/// button under-counted, out of `removableBytes` so the reclaimable
/// figure under-reported, and counted it in `selectedActive` so the
/// dialog warned "something is building here" about a directory the
/// backend did not consider active.
///
/// Fifteen is the documented intent: `artifacts/mod.rs` chose it against
/// an hour in prose ("short enough that yesterday's work is not still
/// blocked today"), so the UI's hour was drift rather than a second
/// opinion, and the UI is the side that moved.
describe("the artifact active-build window", () => {
  it("is the same number in the UI, the delete gate, and the unattended pass", () => {
    const deleteGate = rustConst(artifactsRs, "ACTIVE_WINDOW_SECS", "artifacts/mod.rs");
    const unattended = rustConst(cleanupRs, "ACTIVE_WINDOW_SECS", "cleanup.rs");
    expect(ACTIVE_SECS).toBe(deleteGate);
    // `cleanup.rs`' copy says in so many words that it "Mirrors the
    // artifact view's rule, and the backend's delete-time one", so it is
    // the third side of the same triangle and asserted as one.
    expect(unattended).toBe(deleteGate);
  });

  /// The band where the drift lived, which no test touched on either
  /// side: `ArtifactsPage.test.tsx` uses ages in days only, and the Rust
  /// tests age their fixtures to 2024. A boundary test here is what
  /// makes the number itself -- not merely its agreement -- load-bearing.
  it("treats the 15-to-60-minute band as idle, which is where the drift lived", () => {
    // The page's two predicates, as the page spells them: a row is
    // removable at `age >= ACTIVE_SECS` and counts as active at
    // `age < ACTIVE_SECS`. Applied to ages rather than asserting on the
    // constant alone, so this fails if either comparison is ever
    // inverted as well as if the number moves.
    const removable = (age: number) => age >= ACTIVE_SECS;
    const active = (age: number) => age < ACTIVE_SECS;

    // 20 minutes: the drift's own example. The backend removes it, so the
    // page must offer it and must NOT call it active. Under the old hour
    // both of these were the other way round.
    expect(removable(20 * 60)).toBe(true);
    expect(active(20 * 60)).toBe(false);

    // 10 minutes: genuinely inside the window, on both sides.
    expect(removable(10 * 60)).toBe(false);
    expect(active(10 * 60)).toBe(true);

    // The boundary itself. Exactly 15:00 is idle, because the page's test
    // is `>=` -- and one second under is not.
    expect(removable(ACTIVE_SECS)).toBe(true);
    expect(removable(ACTIVE_SECS - 1)).toBe(false);

    // And the whole 15-60 band, not just one point in it: every minute
    // from 16 to 59 must be removable, which is the range the two sides
    // disagreed over.
    for (let min = 16; min < 60; min += 1) {
      expect(removable(min * 60), `${min} minutes must be removable`).toBe(true);
    }

    expect(ACTIVE_SECS).toBe(900);
  });
});

/// 2. The gap rule: 30 minutes, in the alerts and in the charts.
///
/// `alerts.rs` claimed the agreement was "asserted by
/// `a_gap_here_is_a_gap_in_the_charts_too` below rather than left to
/// whoever edits one of them next" -- but that test asserts `GAP_MS`
/// against a Rust literal, so it reads one side twice. And
/// `ABSOLUTE_GAP_MS` was not exported, so no TS test could reference it
/// either.
///
/// What the disagreement costs is the inversion that test's own doc
/// names: a series the chart draws as BROKEN must not be one the alerts
/// compute a discharge rate across, or the user is told a rate the
/// picture refuses to draw.
describe("the health gap rule", () => {
  it("is the same spacing in the alert path and the chart path", () => {
    const gapMs = rustConst(alertsRs, "GAP_MS", "health/alerts.rs");
    expect(ABSOLUTE_GAP_MS).toBe(gapMs);
    expect(ABSOLUTE_GAP_MS).toBe(30 * 60_000);
  });
});

/// 3. `TOP_N`: five rows, in the Rust cut and the UI heading.
///
/// `board.rs:1235` says TOP_N is "Pinned so the UI and the Rust side
/// cannot disagree about what 'top five' cuts at", but `top_n_is_five`
/// only sees Rust. On the TS side every existing test uses `TOP_N`
/// symbolically -- `expect(rows.length).toBe(TOP_N)`,
/// `getByText(`Top ${TOP_N} ...`)` -- which is self-consistent at any
/// value, so setting the TS copy to 3 passes the entire suite while the
/// page renders "Top 3 pull request authors" over a Rust-ranked list of
/// five, re-cut by a tie-break the UI never saw.
///
/// `branchName.test.ts` ↔ `apply.rs` is the pattern being copied: assert
/// the literal, not the symbol.
describe("the leaderboard cut", () => {
  it("is the same number the Rust side truncates at", () => {
    const rustTopN = rustConst(boardRs, "TOP_N", "github/stats/board.rs");
    expect(TOP_N).toBe(rustTopN);
    // The literal as well as the agreement, so a coordinated change to
    // both sides still has to be a deliberate one. #826 says top five.
    expect(TOP_N).toBe(5);
  });
});

/// 5. The biometric-cancel marker, which existed in three hand-written
/// copies: `companion.rs:43`, `cancelled.ts:4`, and a THIRD in
/// `cancelled.test.ts` that compared copy 2 to copy 3 and so could never
/// see copy 1 change.
///
/// What a mismatch costs: every dismissed Face ID prompt stops being
/// recognised as a decision and shows the user a toast reading
/// `headstate:cancelled` -- their own choice reported back as an error,
/// in marker syntax.
describe("the biometric cancel marker", () => {
  it("is the exact string the companion rejects with", () => {
    expect(CANCELLED).toBe(rustStrConst(companionRs, "CANCELLED", "src-mobile/src/companion.rs"));
  });
});

/// 6. The Claude Code activity window: 30 days, in the Rust bucketing and
/// in the sentence the chart prints under itself (#921).
///
/// Exactly the `TOP_N` shape one case up, and it fails the same way if
/// left unasserted: Rust's `fill_window` is what actually cuts the window,
/// and `ACTIVITY_DAYS` in `ClaudeOverviewPage` is only the number the
/// SUBTITLE quotes. `ClaudeOverviewPage.test.tsx` uses its constant
/// symbolically -- `data-points` against `String(ACTIVITY_DAYS)` -- which
/// is self-consistent at any value, so setting the TS copy to 14 passes
/// the whole suite while the page renders "the last 14 days" over 30 bars.
///
/// A mislabelled time axis is the specific misinformation a chart is worst
/// at: the reader has no way to count the columns against the claim, and
/// `SessionsChart`'s own test asserts the label states the INTENDED window
/// rather than the array it received, precisely so this drift is visible
/// as a mismatch rather than papered over.
describe("the Claude Code activity window", () => {
  it("is the same number of days the Rust side buckets", async () => {
    const overviewRs = (await import("../../src-tauri/src/claude/overview.rs?raw")).default;
    const rustDays = rustConst(overviewRs, "ACTIVITY_DAYS", "claude/overview.rs");
    expect(ACTIVITY_DAYS).toBe(rustDays);
    // The literal as well as the agreement, so a coordinated change to
    // both sides still has to be deliberate. 30 is measured: the corpus
    // spans 41 days with any activity, of which the last 30 hold 1,375 of
    // 1,461 sessions.
    expect(ACTIVITY_DAYS).toBe(30);
  });
});

/// 8. The auto-compaction pressure threshold: two, in the Rust flag and
/// in the sentence the UI puts under it (#1065).
///
/// Exactly the `ACTIVITY_DAYS` shape. `Compactions::under_pressure` is
/// what actually sets `ClaudeSession.context_pressure`, and the frontend
/// copy is what the row and the detail pane WORD the marker against. The
/// two must agree or the UI explains a flag raised on a different rule --
/// "compacted automatically several times" over a badge the backend put
/// there at one, or a pane that describes two as the threshold while the
/// backend flags at four.
///
/// Neither side's own tests can catch that: the Rust test asserts against
/// `AUTO_COMPACT_PRESSURE` symbolically and so does every TS test, which
/// is self-consistent at any value.
describe("the auto-compaction pressure threshold", () => {
  it("is the same count the Rust side flags at", async () => {
    const signalsRs = (await import("../../src-tauri/src/claude/signals.rs?raw")).default;
    const rustThreshold = rustConst(signalsRs, "AUTO_COMPACT_PRESSURE", "claude/signals.rs");
    expect(AUTO_COMPACT_PRESSURE).toBe(rustThreshold);
    // And the literal, so a coordinated change to both sides still has to
    // be deliberate. `signals.rs` argues for two in prose: "a single
    // auto-compaction is an ordinary long session and flagging it would
    // put a badge on a large share of real work; the signal #1065 wants
    // is the REPEATED case."
    expect(AUTO_COMPACT_PRESSURE).toBe(2);
  });
});

/// 7. The virtualenv staleness threshold: 90 days, in three places (#957).
///
/// The pair this file was missing, and the one whose own comment said the
/// duplication was safe: `VenvSection.tsx` carried
/// `const STALE_SECS = 90 * 24 * 60 * 60` beside *"Duplicated rather than
/// plumbed through because it is only used to LABEL rows here… and this
/// never gates a removal."*
///
/// It gated one. `displayState` returned `"stale"` from that constant and
/// `isRemovable` returns `true` for `"stale"`, so the frontend copy
/// decided whether a row's CHECKBOX was enabled while `remove_venvs`
/// decided whether the delete was permitted -- two numbers governing one
/// action, invisible only because both were 90.
///
/// #957 removed the TypeScript literal entirely: `VenvSection` now
/// resolves the user's `stale_venv_days` through `staleVenvDays`, which
/// is the TypeScript twin of `poll::stale_venv_days`. What is left to
/// assert is the DEFAULT, which is the number both languages fall back to
/// when nothing is stored -- and `poll::stale_venv_days` writes it as a
/// bare `0 => 90` rather than as a named constant, so the assertable Rust
/// side is `caches/mod.rs`'s `STALE_SECS`.
///
/// What a mismatch would cost is the pair of failures #957 names: above
/// the backend's threshold, a row reads Stale with an enabled checkbox
/// and the delete is then REFUSED by `RemovalPolicy` -- a confirmed
/// action that silently does nothing; below it, rows the backend would
/// delete are never offered.
describe("the virtualenv staleness default", () => {
  it("is the same number of days the Rust default resolves to", async () => {
    const cachesRs = (await import("../../src-tauri/src/caches/mod.rs?raw")).default;
    const rustSecs = rustConst(cachesRs, "STALE_SECS", "caches/mod.rs");
    expect(DEFAULT_STALE_DAYS * 24 * 60 * 60).toBe(rustSecs);
    // And the literal, so a coordinated change to both sides still has to
    // be deliberate. `caches/mod.rs` argues for 90 in prose: "a project
    // worked on seasonally is normal… the cost of nagging about a live
    // project is that the whole view stops being trusted."
    expect(DEFAULT_STALE_DAYS).toBe(90);
  });

  /// The OTHER half of the agreement, and the half that is new: the
  /// resolver, not just the constant. `poll::stale_venv_days` reads
  /// `0 => 90` and otherwise `clamp(30, 3650)`, and the TypeScript side
  /// has to read the same stored integer the same way or the two layers
  /// resolve one preference to two thresholds -- which is the whole
  /// defect, merely moved.
  ///
  /// Read out of Rust SOURCE rather than restated here, per this file's
  /// rule: a test that spelled the bounds twice in TypeScript would pass
  /// at any value Rust chose.
  it("clamps to the same bounds the Rust resolver does", async () => {
    const pollRs = (await import("../../src-tauri/src/poll.rs?raw")).default;
    const fn = pollRs.slice(pollRs.indexOf("pub fn stale_venv_days"));
    const body = fn.slice(0, fn.indexOf("\n}"));
    // `0 => 90,` and `d.clamp(30, 3650)`, as the function spells them.
    const zero = body.match(/0\s*=>\s*(\d+)\s*,/);
    expect(zero, "poll.rs must resolve 0 to a default").toBeTruthy();
    const clamp = body.match(/clamp\(\s*(\d+)\s*,\s*(\d+)\s*\)/);
    expect(clamp, "poll.rs must clamp the stored value").toBeTruthy();

    expect(DEFAULT_STALE_DAYS).toBe(Number(zero![1]));
    expect(MIN_STALE_DAYS).toBe(Number(clamp![1]));
    expect(MAX_STALE_DAYS).toBe(Number(clamp![2]));

    // And the resolver itself, applied -- so this fails if a comparison
    // is inverted as well as if a bound moves.
    expect(staleVenvDays(0)).toBe(DEFAULT_STALE_DAYS);
    expect(staleVenvDays(undefined)).toBe(DEFAULT_STALE_DAYS);
    expect(staleVenvDays(1)).toBe(MIN_STALE_DAYS);
    expect(staleVenvDays(999_999)).toBe(MAX_STALE_DAYS);
    expect(staleVenvDays(180)).toBe(180);
  });

  /// Every choice Settings offers must survive the clamp untouched.
  ///
  /// A select exists so the user cannot enter a value the app will
  /// silently rewrite. An option outside the bounds would reintroduce
  /// exactly that: a user picks 7 days, the backend enforces 30, and the
  /// two layers disagree again -- through the control added to stop them
  /// disagreeing.
  it("offers only choices the clamp leaves alone", () => {
    for (const d of STALE_DAY_CHOICES) {
      expect(staleVenvDays(d), `${d} is rewritten by the clamp`).toBe(d);
    }
  });
});

/// 4. The event allowlist, which is a LIST rather than a scalar, so the
/// agreement is completeness rather than equality.
///
/// `transport.test.ts`'s `POLL_EVENTS` listed 13 of the 14 names in
/// `EVENT_NAMES`, omitting `"update-run-progress"`. No live bug --
/// `useUpdateProgress` does go through the seam -- but nothing tied the
/// list to its source, unlike the wrapper test beside it which does
/// exactly that with a `toHaveLength`.
///
/// The cost of the gap is asymmetric and silent, which is why it is
/// worth an assertion despite no current bug: a hook importing Tauri's
/// `listen` directly works perfectly on the desktop and never fires on
/// the phone, so the page simply never fills in, with no error anywhere.
///
/// Asserted here rather than in `transport.test.ts` so it reads the Rust
/// source; the per-hook subscription checks stay where they are, since
/// they need the hooks and the mocked transport.
describe("the re-emitted event names", () => {
  /// Every name in `EVENT_NAMES`, read out of the Rust source.
  function eventNames(): string[] {
    const start = eventsRs.indexOf("pub const EVENT_NAMES");
    expect(start, "events.rs must define EVENT_NAMES").toBeGreaterThan(-1);
    const body = eventsRs.slice(start);
    const end = body.indexOf("];");
    expect(end, "EVENT_NAMES must close").toBeGreaterThan(-1);
    // An ENTRY is a quoted string alone on its line, ending in a comma --
    // matched per line rather than by scanning the body for quotes.
    //
    // The distinction is load-bearing, not pedantry: this table is
    // heavily commented, several entries carrying whole paragraphs on why
    // a payload was allowed, and those comments quote things. One already
    // quotes `"counts only"` today, and a future comment citing a bare
    // event name -- `the "worktree-size" rule` -- would be read as a
    // fifteenth entry by a body-wide scan, which then demands a
    // POLL_EVENTS row for an event that does not exist. Both forms return
    // the same 14 names on today's file; this one keeps doing so.
    return body
      .slice(0, end)
      .split("\n")
      .map((line) => line.trim().match(/^"([a-z0-9-]+)",$/))
      .filter((m): m is RegExpMatchArray => m !== null)
      .map((m) => m[1]);
  }

  it("reads a plausible list out of events.rs", () => {
    // Guards the guard: a regex that matched nothing would make the
    // comparison below vacuous in the one direction that matters.
    const names = eventNames();
    expect(names.length).toBeGreaterThan(10);
    expect(names).toContain("prs-updated");
    expect(names).toContain("update-run-progress");
    expect(new Set(names).size).toBe(names.length);
  });

  /// The completeness assertion `POLL_EVENTS` never had. Read from
  /// `transport.test.ts`' source rather than imported, because the list
  /// is local to that file's mocked-transport setup and exporting it
  /// would mean exporting a test fixture.
  it("are each covered by a POLL_EVENTS row in transport.test.ts", async () => {
    const transportTest = (await import("../api/transport.test.ts?raw")).default;
    const start = transportTest.indexOf("const POLL_EVENTS");
    expect(start, "transport.test.ts must define POLL_EVENTS").toBeGreaterThan(-1);
    const body = transportTest.slice(start);
    const end = body.indexOf("\n];");
    expect(end, "POLL_EVENTS must close").toBeGreaterThan(-1);
    // The rows are `["event-name", hook]`, so the event is the quoted
    // string at the head of each entry -- matched as `["..."` to
    // distinguish it from a name merely mentioned in the comments, which
    // several of these rows carry.
    const covered = new Set(
      [...body.slice(0, end).matchAll(/\[\s*"([a-z0-9-]+)"/g)].map((m) => m[1]),
    );
    expect(covered.size).toBeGreaterThan(10);
    for (const name of eventNames()) {
      expect(covered.has(name), `POLL_EVENTS has no row for "${name}"`).toBe(true);
    }
    // And nothing the desktop does not actually emit: a row for a name
    // `EVENT_NAMES` never re-emits would assert a seam that is not there.
    const emitted = new Set(eventNames());
    for (const name of covered) {
      expect(emitted.has(name), `POLL_EVENTS covers "${name}", absent from EVENT_NAMES`).toBe(true);
    }
  });
});

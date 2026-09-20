import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  ClaudeImported,
  ClaudePreview,
  ClaudeSession,
  ClaudeSessionDetail,
  ClaudeSessionList,
  ClaudeCostState,
  ClaudeUsage,
  ClaudeObservation,
  ClaudeSubagentRollup,
  Liveness,
  Worktree,
  WorktreeRepo,
} from "@/types/pr";
import { useFilters } from "@/store/filters";

const copyFn = vi.hoisted(() => vi.fn(() => Promise.resolve(null as string | null)));
const revealFn = vi.hoisted(() => vi.fn(() => Promise.resolve("/code/app")));
const launchFn = vi.hoisted(() => vi.fn(() => Promise.resolve()));
const toastError = vi.hoisted(() => vi.fn());
const toastSuccess = vi.hoisted(() => vi.fn());
/// #1219's two calls. `proposeFn` returns the evidence and `stopFn` does
/// the stop -- both mocked, because a test that reached the real ones
/// would read this machine's own registry and could signal a live pid.
const proposeFn = vi.hoisted(() => vi.fn());
const stopFn = vi.hoisted(() => vi.fn());
const rescanFn = vi.hoisted(() => vi.fn(() => Promise.resolve()));
const refetchFn = vi.hoisted(() => vi.fn());

const state = vi.hoisted(() => ({
  /// The configured terminal template, empty for none (#1126).
  terminal: "",
  list: undefined as ClaudeSessionList | undefined,
  loading: false,
  failed: false,
  imported: undefined as ClaudeImported | undefined,
  importFailed: false,
  importFetching: false,
  /// A FIXED `now`, which is the point: the page must never read the
  /// clock during render. Every relative time below is computed from
  /// this, so a `Date.now()` creeping in would make these assertions
  /// drift with wall-clock time rather than fail outright -- which is why
  /// `the_page_never_reads_the_clock_during_render` checks the source as
  /// well.
  now: Date.parse("2026-09-13T12:00:00Z"),
  /// What `useWorktrees` returns, for the #920 jump. `undefined` with
  /// `worktreesFailed: false` is the still-loading case, and with
  /// `worktreesFailed: true` it is the could-not-read case -- the two the
  /// section must not render alike.
  worktrees: undefined as WorktreeRepo[] | undefined,
  worktreesFailed: false,
  /// What `useClaudeSessionUsage` returns (#959). `undefined` with
  /// `usageFailed: false` is still-reading; with `usageFailed: true` it is
  /// the could-not-read case. The two must not render alike, and a
  /// `messages: 0` answer must not render like either -- it is a
  /// successful read of a transcript that carries no usage, which is 24 of
  /// 1,502 real transcripts.
  usage: undefined as ClaudeUsage | undefined,
  usageFailed: false,
  /// What `useClaudeSubagentRollup` returns (#1002), on the same
  /// three-way split as `usage` above: `undefined` with
  /// `rollupFailed: false` is still-reading, with `rollupFailed: true` it
  /// is the could-not-read case, and a `measured: 0` answer is a third
  /// thing again -- we read the children and could total none of them.
  /// Rendering any two of those alike is the #846 defect.
  rollup: undefined as ClaudeSubagentRollup | undefined,
  rollupFailed: false,
  /// #1062-#1064. `undefined` is STILL READING; a resolved
  /// `{ state: "unobserved" }` is "no hook was watching". Those are
  /// different renderings and the distinction is the whole feature, so
  /// the fixture keeps them apart rather than using one value for both.
  events: undefined as ClaudeObservation | undefined,
  eventsFailed: false,
  /// What `useClaudeTranscriptTail` returns (#982), on the same
  /// three-way split and for the same reason.
  preview: undefined as ClaudePreview | undefined,
  previewFailed: false,
  /// Every path `useClaudeTranscriptTail` was asked for while `enabled`
  /// was false, so the "behind a disclosure" property is testable: the
  /// preview costs a 256 KB read over the pairing transport and must not
  /// happen on selection.
  previewEnabledFor: [] as (string | null)[],
  /// What `useClaudeSessionDetail` returns (#985), keyed by session id.
  ///
  /// A MAP rather than one value, because the split made "the detail for
  /// the row I selected" a real question: a detail served for the wrong
  /// id is the failure this shape makes visible, and `fixtures` below
  /// fills it from the same object the list row comes from.
  details: new Map<string, ClaudeSessionDetail>(),
  /// `true` makes the detail read REJECT, which the pane must render as
  /// a reason with a retry rather than as an empty session.
  detailFailed: false,
  /// `true` makes it resolve `null` -- the store has no such id, a
  /// session deleted between two polls. Distinct from the arm above, and
  /// #846 is the rule that they must not render alike.
  detailMissing: false,
  /// Every session id the detail hook was asked for, so a test can
  /// assert it is fetched for ONE row rather than for the list.
  detailAskedFor: [] as (string | null)[],
}));

vi.mock("../api/hooks", () => ({
  // Empty by default, which is what every assertion in this file about
  // "Copy resume command" assumes (#1126). Set per-test to reach the
  // launch path.
  useUiPrefs: () => ({ prefs: { terminal_command: state.terminal } }),
  useClaudeSessions: () => ({
    list: {
      data: state.list,
      isLoading: state.loading,
      isError: state.failed,
      error: "database is locked",
      refetch: refetchFn,
    },
    imported: {
      data: state.imported,
      isError: state.importFailed,
      isFetching: state.importFetching,
      error: "Permission denied",
    },
    now: state.now,
    rescan: rescanFn,
  }),
  // The #920 jump reads the SAME query the Worktrees page does, so a
  // session detail costs a cache hit rather than a second scan.
  useWorktrees: () => ({
    data: state.worktrees,
    isError: state.worktreesFailed,
    error: state.worktreesFailed ? "could not list worktrees" : undefined,
  }),
  // #959. `isLoading` is derived from the same absence the real hook
  // derives it from, so the still-reading arm is reachable here exactly
  // when it is reachable in the app.
  useClaudeSessionUsage: (path: string | null) => ({
    data: state.usage,
    isError: state.usageFailed,
    error: state.usageFailed ? "Permission denied" : undefined,
    isLoading: path !== null && !state.usageFailed && state.usage === undefined,
  }),
  // #1002, on the same three-way split as the usage mock above and for
  // the same reason: still-reading, could-not-read and read-and-found-none
  // are three different renderings and a mock that collapses them makes
  // two of the three untestable.
  useClaudeSubagentRollup: (sessionId: string | null) => ({
    data: state.rollup,
    isError: state.rollupFailed,
    error: state.rollupFailed ? "Permission denied" : undefined,
    isLoading: sessionId !== null && !state.rollupFailed && state.rollup === undefined,
  }),
  // #1062, #1063, #1064. The same three-way split, plus a fourth state
  // this feature adds and the others do not have: `unobserved`, which is
  // a RESOLVED answer meaning "no hook was watching". It is data rather
  // than a flag precisely so a test can tell it from `undefined`, which
  // is the still-reading state -- collapsing those two is the defect the
  // whole feature is about.
  useClaudeSessionEvents: (sessionId: string | null) => ({
    data: state.events,
    isError: state.eventsFailed,
    error: state.eventsFailed ? "Permission denied" : undefined,
    isLoading: sessionId !== null && !state.eventsFailed && state.events === undefined,
  }),
  // #985. One row's detail, fetched on selection. Records the id so a
  // test can assert the list does not pull 1,474 of these.
  useClaudeSessionDetail: (sessionId: string | null, enabled: boolean) => {
    if (enabled && sessionId) state.detailAskedFor.push(sessionId);
    const data = state.detailMissing
      ? null
      : sessionId
        ? state.details.get(sessionId)
        : undefined;
    return {
      data: state.detailFailed ? undefined : data,
      isError: state.detailFailed,
      error: state.detailFailed ? "database is locked" : undefined,
      refetch: refetchFn,
    };
  },
  // #982. Records what it was asked for and whether the disclosure was
  // open, so a test can assert the read does not happen on selection.
  useClaudeTranscriptTail: (path: string | null, enabled: boolean) => {
    if (enabled) state.previewEnabledFor.push(path);
    return {
      data: state.preview,
      isError: state.previewFailed,
      error: state.previewFailed ? "Permission denied" : undefined,
      isLoading: enabled && !state.previewFailed && state.preview === undefined,
    };
  },
}));
vi.mock("sonner", () => ({ toast: { success: toastSuccess, error: toastError } }));
vi.mock("../lib/clipboard", () => ({ copyText: copyFn }));
vi.mock("../api/tauri", () => ({
  claudeRevealPath: revealFn,
  claudeLaunchSession: launchFn,
  claudeProposeStop: proposeFn,
  claudeStopSession: stopFn,
}));

import { ClaudeCodePage, ClaudeSessionColumn } from "./ClaudeCodePage";

/// The desktop pair, which is TWO components since #939.
///
/// `ClaudeSessionColumn` moved into `ClaudeCodeSidebar` and
/// `ClaudeCodePage` kept the banners and the detail, so a test rendering
/// only the page would be asserting on half the view -- every row click
/// below would find no row. This renders both, in the order `App` puts
/// them on screen.
///
/// Not the real `ClaudeCodeSidebar`, deliberately. That component carries
/// `ViewSwitcher`, which reads `useUiPrefs` and every view's label, so
/// pulling it in would make these tests depend on the whole navigation
/// chrome to assert something about a session's resume command.
/// `ClaudeCodeSidebar.test.tsx` is where the sidebar's own claims -- the
/// page order, and that the search box is in the column -- are asserted.
function renderView() {
  return render(
    <>
      <ClaudeSessionColumn />
      <ClaudeCodePage />
    </>,
  );
}

/// One session, in the shape it had BEFORE the #985 split.
///
/// Kept whole deliberately. A session is one thing to a reader, and the
/// list/detail split is a transport decision -- so the fixtures describe
/// sessions and `session()` below files each half where the mocked hooks
/// will find it. Every test written before the split therefore still
/// reads as a statement about a session rather than about a wire format.
type WholeSession = ClaudeSession &
  Omit<ClaudeSessionDetail, "registry_failure" | "liveness" | "kind" | "subagents">;

const whole = (over: Partial<WholeSession> = {}): WholeSession => ({
  opening_prompt: null,
  session_id: "e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
  name: "HeadState GitHub issues filing",
  cwd: "/Users/acme/code/widget",
  git_branch: "feat/spoon",
  claude_version: "2.1.270",
  transcript_path: "/Users/acme/.claude/projects/slug/e5dff3bd.jsonl",
  first_seen_at: "2026-09-11T09:00:00Z",
  last_activity_at: "2026-09-13T09:00:00Z",
  liveness: { state: "dead", why: "pid 14779 is no longer running" },
  cwd_state: { state: "exists" },
  // Defaults to present, because that is the real default: 0 of 1,461
  // measured transcripts were missing. A fixture defaulting to `gone`
  // would make the common case the one no test exercised.
  transcript_state: { state: "exists" },
  resume: {
    // Built from the id the caller asked for, not a fixed one: several
    // tests below render two sessions and assert the pane shows the
    // SELECTED one's command, which a constant string cannot express.
    command: `cd '/Users/acme/code/widget' && claude --resume ${
      over.session_id ?? "e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2"
    }`,
    caveat: null,
    anchored: true,
  },
  runs: 1,
  // Defaults to the user's OWN work, because that is the real default:
  // 1,133 of 1,524 measured rows. A fixture defaulting to a subagent
  // would make the common case the one no test exercised, and would hide
  // every row from the list by default.
  kind: { kind: "own" },
  subagents: 0,
  parent: null,
  unattributed: null,
  // #1067. Defaults to `never-observed`, which is the real default and
  // not a convenient one: no session that ran before the hook was
  // installed has a notification record, so this is what the entire
  // corpus sends. A fixture defaulting to `now` would put a waiting
  // indicator on every existing test's row and make the silent case the
  // one nothing exercised.
  waiting: { state: "no", reason: "never-observed" },
  // #1065, and `null` rather than `false` for the same reason: absent is
  // the pre-hook default. `false` would mean "we watched and it did not",
  // which is a measurement no fixture should claim by accident.
  context_pressure: null,
  // #1065 and #1066. `null` on both, again the pre-hook state: no
  // compaction was ever recorded and neither source has anything to say
  // about agent types.
  compactions: null,
  agent_types: null,
  ...over,
});

/// The LIST row, with this session's detail filed under its id.
///
/// The registration is the point: `useClaudeSessionDetail`'s mock reads
/// `state.details`, so a fixture built here is answerable by both hooks
/// and a test never has to set up the two halves separately.
const session = (over: Partial<WholeSession> = {}): ClaudeSession => {
  const w = whole(over);
  state.details.set(w.session_id, {
    session_id: w.session_id,
    claude_version: w.claude_version,
    transcript_path: w.transcript_path,
    first_seen_at: w.first_seen_at,
    // The same liveness the list row carries. The real command derives
    // it independently, and `sessions.rs` has the test that the two
    // agree; here they are one value so that a UI test cannot
    // accidentally depend on them differing.
    liveness: w.liveness,
    transcript_state: w.transcript_state,
    resume: w.resume,
    runs: w.runs,
    registry_failure: null,
    kind: w.kind,
    // The detail's `subagents` is the CHILD LIST, while the row's is a
    // count -- two different shapes for the same fact, so the fixture
    // derives the list from the count rather than letting them drift.
    subagents: Array.from({ length: w.subagents }, (_, i) => ({
      session_id: `child-${i}`,
      name: `Child ${i}`,
      agent_id: `a${i}`.padEnd(17, "0"),
    })),
    parent: w.parent,
    unattributed: w.unattributed,
    compactions: w.compactions,
    agent_types: w.agent_types,
    // ONE `waiting` across both halves, exactly as `liveness` above is
    // one value. The real backend derives it twice -- once per read --
    // and `signals.rs` has the test that the two agree; keeping them a
    // single value here means a UI test cannot accidentally depend on the
    // row and the pane disagreeing.
    waiting: w.waiting,
  });
  return {
    session_id: w.session_id,
    name: w.name,
    // #1133. The list row carries it; the detail does not need it.
    opening_prompt: w.opening_prompt ?? null,
    cwd: w.cwd,
    git_branch: w.git_branch,
    last_activity_at: w.last_activity_at,
    liveness: w.liveness,
    cwd_state: w.cwd_state,
    kind: w.kind,
    subagents: w.subagents,
    waiting: w.waiting,
    context_pressure: w.context_pressure,
  };
};

const listOf = (
  sessions: ClaudeSession[],
  over: Partial<ClaudeSessionList> = {},
): ClaudeSessionList => ({
  sessions,
  registry_failure: null,
  registry_unreadable: [],
  ...over,
});

const imported = (over: Partial<ClaudeImported> = {}): ClaudeImported => ({
  sessions: 1438,
  write_failures: [],
  subagent_files_skipped: 1375,
  unreadable_dirs: [],
  unreadable_files: [],
  metadata_beyond_first_record: 1438,
  elapsed_ms: 1374,
  // `null` is the real default: the directory exists on any machine that
  // has run Claude Code, which is every machine the other fixtures
  // describe. The never-ran shape is opted into explicitly (#970).
  absent_root: null,
  ...over,
});

/// One session's token rollup (#959).
///
/// The defaults are the shape of a REAL session: cache reads two orders
/// of magnitude above fresh input, which is what makes four separate
/// counters the right rendering and one summed total the wrong one.
const usage = (over: Partial<ClaudeUsage> = {}): ClaudeUsage => ({
  messages: 994,
  input_tokens: 1_988,
  output_tokens: 582_035,
  cache_read_tokens: 405_086_242,
  cache_creation_tokens: 4_971_059,
  models: [{ model: "claude-opus-5", messages: 994 }],
  truncated: false,
  bytes_read: 183_237,
  file_bytes: 183_237,
  /// No `cost-state` record, which is the majority of the corpus and
  /// therefore the right DEFAULT: a fixture that carried one by default
  /// would leave the absent arm -- the one #846 is about -- reachable
  /// only by a test that opted out of it. The tests that want a recorded
  /// cost opt IN, with `costState()` below.
  recorded_cost: null,
  ...over,
});

/// One session's `cost-state` record (#1210).
///
/// The defaults are a REAL record from the development machine
/// (`9e24f824`): two models, a sub-cent haiku slice beside a $1.32 opus
/// one, 68 ms of retry time, and `has_unknown_model_cost: false` -- which
/// is its value on every record measured, so the `true` path is opted
/// into explicitly and exists only in a test.
const costState = (over: Partial<ClaudeCostState> = {}): ClaudeCostState => ({
  total_cost_usd: 1.3242615,
  models: [
    { model: "claude-opus-5[1m]", cost_usd: 1.3230755 },
    { model: "claude-haiku-4-5-20251001", cost_usd: 0.001186 },
  ],
  total_api_ms: 84_690,
  total_api_without_retries_ms: 84_622,
  has_unknown_model_cost: false,
  ...over,
});

/// One transcript tail (#982).
const preview = (over: Partial<ClaudePreview> = {}): ClaudePreview => ({
  messages: [
    {
      role: "user",
      timestamp: "2026-09-13T11:00:00Z",
      model: null,
      blocks: [{ kind: "text", text: "run the tests", truncated: false }],
    },
    {
      role: "assistant",
      timestamp: "2026-09-13T11:00:05Z",
      model: "claude-opus-5",
      blocks: [
        { kind: "text", text: "Running them now.", truncated: false },
        {
          kind: "tool_use",
          name: "Bash",
          id: "toolu_base",
          args: { tool: "bash", command: "cargo test", description: null, truncated: false },
        },
      ],
    },
  ],
  truncated: false,
  bytes_read: 183_237,
  file_bytes: 183_237,
  non_conversation_records: 0,
  unparseable_records: 0,
  lifecycle: {
    queue: null,
    permission_mode: null,
    worktree: { state: "unknown" },
  },
  pairings: { toolu_base: "unanswered" },
  unanswered_calls: 1,
  results_above_window: 0,
  ...over,
});

beforeEach(() => {
  // BEFORE `session()` below, which repopulates it: a detail left over
  // from a previous test would answer for an id this one never defined,
  // which is exactly the cross-talk the map exists to make visible.
  state.details.clear();
  // Default: no terminal, the pre-#1126 behaviour.
  state.terminal = "";
  state.detailFailed = false;
  state.detailMissing = false;
  state.detailAskedFor = [];
  state.list = listOf([session()]);
  state.loading = false;
  state.failed = false;
  state.imported = imported();
  state.importFailed = false;
  state.importFetching = false;
  state.usage = usage();
  state.usageFailed = false;
  // #1062-#1064. A RESOLVED "nobody was watching" by default, matching
  // the fixture corpus: these hooks are new, so the honest default for a
  // session the other tests describe is that no failure record exists.
  // Reset here like every other field, because a leftover
  // `eventsFailed` would silently put unrelated tests in the error arm.
  state.events = { state: "unobserved" };
  state.eventsFailed = false;
  // #1219. Reset per test so a proposal or a stop from a previous one
  // cannot answer for a session this one never described.
  proposeFn.mockReset();
  stopFn.mockReset();
  state.preview = preview();
  state.previewFailed = false;
  state.previewEnabledFor = [];
  // A LOADED, empty listing by default -- not `undefined`. `undefined`
  // means "still loading or unreadable", and leaving it there would make
  // every unrelated test render the wrong one of the #920 section's three
  // arms.
  state.worktrees = [];
  state.worktreesFailed = false;
  // The REAL store, not a mock: the #920 jump's whole assertion is that
  // it writes `repo` and `view` the way `WorktreesPage` reads them, and a
  // mocked store would let those two drift apart while the test passed.
  useFilters.setState({ view: "claude-code" });
  useFilters.getState().setFilter("repo", undefined);
  // The search text and the selection live in the store since #939, and
  // the store is a MODULE singleton -- so without this a query typed by
  // one test would still be filtering the list in the next one, and a
  // selected id would open a detail pane nobody clicked.
  // `claudeFilter` too, as of #949, and for the same singleton reason: a
  // chip pressed by one test would silently shorten every list after it,
  // which is the failure mode where a suite goes green over an empty page.
  // `claudeShowSubagents` too, as of #1002, and for the same singleton
  // reason with the opposite sign: a test that revealed subagents would
  // leave 391 rows of machinery in every list after it, so a chip count
  // asserted later would be right about a list nobody meant to draw.
  useFilters.setState({
    claudeQuery: "",
    claudeSelected: undefined,
    claudeFilter: "all",
    claudeShowSubagents: false,
  });
  copyFn.mockClear();
  revealFn.mockClear();
  toastError.mockClear();
  toastSuccess.mockClear();
  rescanFn.mockClear();
});

/// Open a session's detail pane by clicking its row.
function open(name: string) {
  fireEvent.click(screen.getByRole("button", { name: new RegExp(name, "i") }));
}

/// #1062, #1063, #1064: the per-session failure and denial profile.
///
/// The single rule these share is the one the root `CLAUDE.md` records as
/// having shipped as a real defect: **absent is not zero**. A session with
/// no failure records may have had none, or may have run before the hooks
/// existed, and rendering the second as "0 failures" is the most legible
/// possible lie.
describe("what went wrong in a session", () => {
  const tally = (name: string | null, count: number, detail: string | null = null) => ({
    name,
    count,
    detail,
  });

  /// **The sabotage test of this feature.** A session no hook watched must
  /// say "not recorded" and must never show a zero.
  ///
  /// On a machine that adopted Headstate after using Claude Code, this is
  /// EVERY historical session -- `overview.rs` measured 1,461 of 1,461 in
  /// exactly this state -- so a zero here is not an edge case, it is the
  /// whole corpus reporting that it was clean when nobody was watching.
  ///
  /// Sabotage: replacing the `unobserved` arm with `<TroubleProfile>` over
  /// an empty profile renders "Nothing failed and nothing was declined",
  /// and this test fails on both assertions below.
  it("says a session from before the hooks was not recorded, never zero", () => {
    state.list = listOf([session({ name: "Ancient session" })]);
    state.events = { state: "unobserved" };
    renderView();
    open("Ancient session");

    expect(screen.getByText(/Not recorded\./)).toBeTruthy();
    expect(screen.getByText(/No hook was watching this session/)).toBeTruthy();
    // And NOT the measured-zero sentence, which is the claim this state
    // is not entitled to make.
    expect(screen.queryByText(/Nothing failed and nothing was declined/)).toBeNull();
  });

  /// The other half: a session that WAS watched and was clean says so.
  ///
  /// Without this the feature could never deliver good news, and a user
  /// whose sessions are genuinely fine would be told forever that nothing
  /// was measured.
  it("reports a watched session with no failures as a measured zero", () => {
    state.list = listOf([session({ name: "Clean session" })]);
    state.events = {
      state: "observed",
      profile: { turn_failures: [], tool_failures: [], denials: [] },
    };
    renderView();
    open("Clean session");

    expect(screen.getByText(/Nothing failed and nothing was declined/)).toBeTruthy();
    expect(screen.queryByText(/Not recorded\./)).toBeNull();
  });

  /// A failed read is not a clean session (#846).
  ///
  /// The error arm sits before the loading arm because `data` is
  /// undefined on a rejection exactly as it is before the first read.
  it("reports a failed read rather than showing zeros", () => {
    state.list = listOf([session({ name: "Unreadable session" })]);
    state.eventsFailed = true;
    renderView();
    open("Unreadable session");

    expect(screen.getByText(/Could not read what the hook recorded/)).toBeTruthy();
    expect(screen.queryByText(/Nothing failed and nothing was declined/)).toBeNull();
    expect(screen.queryByText(/Not recorded\./)).toBeNull();
  });

  /// An `error_type` this build has never seen renders as ITSELF (#1062).
  ///
  /// A newer Claude Code can add an error type at any time. Bucketing the
  /// unrecognised ones into "other" would mean the first user to hit a new
  /// failure mode sees the least about it, which inverts the point.
  it("renders an unknown error type verbatim rather than as other", () => {
    state.list = listOf([session({ name: "Odd session" })]);
    state.events = {
      state: "observed",
      profile: {
        turn_failures: [tally("quantum_decoherence", 3, "the turn collapsed")],
        tool_failures: [],
        denials: [],
      },
    };
    renderView();
    open("Odd session");

    expect(screen.getByText("quantum_decoherence")).toBeTruthy();
    expect(screen.queryByText(/^other$/i)).toBeNull();
  });

  /// A tool name this build has never seen renders as itself (#1063).
  ///
  /// MCP servers define arbitrary tool names, so the unknown case is the
  /// COMMON one rather than an edge.
  it("renders an unknown tool name verbatim", () => {
    state.list = listOf([session({ name: "MCP session" })]);
    state.events = {
      state: "observed",
      profile: {
        turn_failures: [],
        tool_failures: [tally("mcp__acme_widgets__reticulate", 2, "connection refused")],
        denials: [],
      },
    };
    renderView();
    open("MCP session");

    expect(screen.getByText("mcp__acme_widgets__reticulate")).toBeTruthy();
  });

  /// A denial is presented as a GUARDRAIL, not as an error (#1064).
  ///
  /// The wording rule as an assertion. Presenting auto mode's refusals as
  /// damage teaches the user to switch the guardrail off, which is the
  /// opposite of what the record is for.
  it("presents a denial as the guardrail working rather than as a failure", () => {
    state.list = listOf([session({ name: "Guarded session" })]);
    state.events = {
      state: "observed",
      profile: {
        turn_failures: [],
        tool_failures: [],
        denials: [tally("Write", 4, "auto mode refuses writes outside the worktree")],
      },
    };
    renderView();
    open("Guarded session");

    expect(screen.getByText(/Declined by auto mode/)).toBeTruthy();
    expect(screen.getByText(/That is the guardrail working/)).toBeTruthy();
    // A denial-only session must not be described as having failures.
    expect(screen.queryByText(/^Tool failures$/)).toBeNull();
    expect(screen.queryByText(/^Turns that died$/)).toBeNull();
  });

  /// A denial with no recorded reason SAYS so (#1064's own test).
  ///
  /// Inventing a plausible reason would be worse than showing none: the
  /// user would act on a sentence Headstate made up.
  it("says a denial had no recorded reason rather than inventing one", () => {
    state.list = listOf([session({ name: "Reasonless session" })]);
    state.events = {
      state: "observed",
      profile: {
        turn_failures: [],
        tool_failures: [],
        denials: [tally("Bash", 1, null)],
      },
    };
    renderView();
    open("Reasonless session");

    expect(screen.getByText("Bash")).toBeTruthy();
    // The count is there and no sentence has been conjured beside it.
    expect(screen.getByText(/Auto mode refused these tool calls/)).toBeTruthy();
  });

  /// A half install renders the counts as a FLOOR and names what is
  /// missing.
  ///
  /// "At least" is the qualify half of the house rule: only-low qualifies,
  /// possibly-wrong suppresses. The counts here are only-low.
  it("reports a half install as a floor and names the missing event", () => {
    state.list = listOf([session({ name: "Half-watched session" })]);
    state.events = {
      state: "partial",
      missing: ["PermissionDenied"],
      profile: {
        turn_failures: [],
        tool_failures: [tally("Bash", 2, "exited 1")],
        denials: [],
      },
    };
    renderView();
    open("Half-watched session");

    expect(screen.getByText(/At least these/)).toBeTruthy();
    expect(screen.getByText(/PermissionDenied/)).toBeTruthy();
    // Partial is not nothing: what WAS recorded is still shown.
    expect(screen.getByText("Bash")).toBeTruthy();
  });

  /// Failures concentrated in one tool are called out; spread ones are
  /// not (#1063).
  ///
  /// Both directions in one test, because a signal that always fires is
  /// not a signal and only the negative case proves it does not.
  it("flags failures concentrated in one tool and stays quiet when they are spread", () => {
    state.list = listOf([session({ name: "Fighting session" })]);
    state.events = {
      state: "observed",
      profile: {
        turn_failures: [],
        tool_failures: [tally("Bash", 14, "exited 1"), tally("Read", 1, "no such file")],
        denials: [],
      },
    };
    renderView();
    open("Fighting session");
    expect(screen.getByText(/Concentrated in Bash/)).toBeTruthy();

    cleanup();
    state.list = listOf([session({ name: "Spread session" })]);
    state.events = {
      state: "observed",
      profile: {
        turn_failures: [],
        tool_failures: [tally("Bash", 1), tally("Read", 1), tally("Edit", 1)],
        denials: [],
      },
    };
    renderView();
    open("Spread session");
    expect(screen.queryByText(/Concentrated in/)).toBeNull();
  });
});

describe("liveness renders as three states, not two", () => {
  /// **The sabotage test.** `unknown` must NOT render as "Not running".
  ///
  /// "Not running" is what offers Resume as a confident action, and
  /// resuming a session that is in fact alive starts a SECOND copy of it.
  /// Collapsing `unknown` into `dead` -- #841's `is_some_and` fail-open
  /// -- has to fail here, in the component, as well as in
  /// `claude::liveness`'s own tests.
  it("renders an unknown liveness as could-not-tell and never as not running", () => {
    state.list = listOf([
      session({
        name: "Unknowable session",
        liveness: {
          state: "unknown",
          why: "could not check whether pid 14779 is running: Operation not permitted",
        },
      }),
    ]);
    renderView();
    expect(screen.getAllByText(/could not tell/i).length).toBeGreaterThan(0);
    expect(screen.queryByText(/^Not running$/)).toBeNull();
  });

  it("renders a dead liveness as not running, with its reason in the detail", () => {
    state.list = listOf([
      session({
        liveness: {
          state: "dead",
          why: "pid 14779 is in the live session registry but is no longer running, so this session ended without shutting down",
        },
      }),
    ]);
    renderView();
    expect(screen.getAllByText(/not running/i).length).toBeGreaterThan(0);
    expect(screen.queryByText(/could not tell/i)).toBeNull();
    open("HeadState GitHub issues filing");
    // The REASON, because an orphaned registry entry is a crash and the
    // user has grounds to see. A verdict with no grounds is what the
    // three-state type exists to avoid.
    expect(screen.getByText(/ended without shutting down/i)).toBeTruthy();
  });

  it("renders a running liveness with its busy/idle refinement", () => {
    state.list = listOf([
      session({ liveness: { state: "running", pid: 14779, status: "busy" } }),
    ]);
    renderView();
    expect(screen.getAllByText(/running/i).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/busy/i).length).toBeGreaterThan(0);
  });

  /// A stored busy/idle can only ever appear beside a liveness we
  /// DERIVED. The type makes that structural -- `status` lives on the
  /// `running` variant alone -- and this pins the rendering.
  it("never shows a busy status for a session it did not find running", () => {
    state.list = listOf([
      session({ liveness: { state: "dead", why: "pid 1 is no longer running" } }),
    ]);
    renderView();
    expect(screen.queryByText(/busy/i)).toBeNull();
  });

  /// Running sessions are pinned ABOVE the date ordering.
  ///
  /// There are never more than a handful (three on the development
  /// machine) and they are why the view is open. A live session at
  /// position 900 because its last write was slow is the failure to
  /// avoid.
  it("pins running sessions above everything, whatever their activity date", () => {
    state.list = listOf([
      session({
        session_id: "recent-dead",
        name: "Touched ten minutes ago",
        last_activity_at: "2026-09-13T11:50:00Z",
      }),
      session({
        session_id: "stale-live",
        name: "Running but quiet",
        last_activity_at: "2026-01-01T00:00:00Z",
        liveness: { state: "running", pid: 7, status: "idle" },
      }),
    ]);
    renderView();
    // Was `{ pressed: false }`, which selected these rows only because they
    // carried `aria-pressed` -- the defect #977 removed. They are
    // single-select list rows, not toggles, so the ORDER is read off the
    // rows themselves.
    const titles = screen.getAllByRole("button").map((r) => r.textContent ?? "");
    const live = titles.findIndex((t) => t.includes("Running but quiet"));
    const dead = titles.findIndex((t) => t.includes("Touched ten minutes ago"));
    expect(live).toBeGreaterThanOrEqual(0);
    expect(live).toBeLessThan(dead);
  });
});

describe("the resume command carries the cwd that makes it work", () => {
  it("offers the cd-prefixed command with no caveat when the directory exists", () => {
    renderView();
    open("HeadState GitHub issues filing");
    expect(
      screen.getByText(
        "cd '/Users/acme/code/widget' && claude --resume e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
      ),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /copy resume command/i }));
    expect(copyFn).toHaveBeenCalledWith(
      "cd '/Users/acme/code/widget' && claude --resume e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
    );
  });

  /// The configured terminal (#1126). Empty is the default, asserted
  /// above: the button copies and says to paste.
  describe("with a terminal configured", () => {
    it("the button launches instead of copying, and says so", () => {
      state.terminal = "open -a Terminal {command}";
      renderView();
      open("HeadState GitHub issues filing");
      fireEvent.click(screen.getByRole("button", { name: /resume in terminal/i }));
      expect(launchFn).toHaveBeenCalledWith(
        "e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
        "/Users/acme/code/widget",
      );
      // It must NOT also copy: a button that does both is a button
      // whose label describes half of what it did.
      expect(copyFn).not.toHaveBeenCalled();
    });

    it("passes the id and cwd, never the built command string", () => {
      // Rust rebuilds the command from these, so `claude_launch_session`
      // can never become "run this text in a terminal".
      state.terminal = "open -a Terminal {command}";
      renderView();
      open("HeadState GitHub issues filing");
      fireEvent.click(screen.getByRole("button", { name: /resume in terminal/i }));
      const args = launchFn.mock.calls[0] as unknown[];
      expect(args.some((a) => typeof a === "string" && a.includes("claude --resume"))).toBe(
        false,
      );
    });

    it("still offers Copy beside it", () => {
      // The terminal is one user's choice of one tool; the raw string
      // is what you need to paste elsewhere or read before running.
      state.terminal = "open -a Terminal {command}";
      renderView();
      open("HeadState GitHub issues filing");
      fireEvent.click(screen.getByRole("button", { name: /^copy$/i }));
      expect(copyFn).toHaveBeenCalledWith(
        "cd '/Users/acme/code/widget' && claude --resume e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
      );
      expect(launchFn).not.toHaveBeenCalled();
    });

    it("stops telling the user to paste it somewhere", () => {
      // The old sentence -- "Headstate does not open one for you" --
      // would be a statement the app contradicts the moment the button
      // is pressed.
      state.terminal = "open -a Terminal {command}";
      renderView();
      open("HeadState GitHub issues filing");
      expect(screen.queryByText(/does not open one for you/)).toBeNull();
      expect(screen.getByText(/terminal you configured/i)).toBeTruthy();
    });

    it("says a terminal can be configured when none is", () => {
      // The default path must POINT somewhere: the old sentence stated
      // a permanent limitation, and it is now a setting.
      renderView();
      open("HeadState GitHub issues filing");
      expect(screen.getByText(/only if you configure it in Settings/i)).toBeTruthy();
    });
  });

  /// **The sabotage test for #918.** A bare command MUST show its caveat.
  ///
  /// 84.4% of the real corpus is in this state, and the command still
  /// WORKS -- resume resolves by id -- which is exactly why the caveat is
  /// mandatory: a command that works but lands in the wrong tree is worse
  /// than one that fails, because it looks like it worked. Dropping the
  /// caveat from the render has to fail here.
  it("shows the directory-is-gone caveat beside a bare command", () => {
    state.list = listOf([
      session({
        cwd: "/Users/acme/code/widget/.worktrees/deleted",
        cwd_state: { state: "gone" },
        resume: {
          command: "claude --resume e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
          caveat:
            "The directory this session ran in is gone (/Users/acme/code/widget/.worktrees/deleted), so this will resume in whatever directory you run it from.",
          anchored: false,
        },
      }),
    ]);
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.getByText(/is gone .*\.worktrees\/deleted/)).toBeTruthy();
    expect(screen.getByText(/resume in whatever directory you run it from/)).toBeTruthy();
    // And the row itself says so, so a user scanning the list can see
    // which sessions are archaeology without opening each one.
    expect(screen.getAllByText(/directory gone/).length).toBeGreaterThan(0);
  });

  /// A cwd we could not CHECK reads differently from one that is gone.
  ///
  /// Different remedies: a permission error means the tree may well be
  /// there and the `cd` would have worked. Collapsing them is the
  /// absent-is-not-zero mistake.
  it("distinguishes a cwd it could not check from one that is gone", () => {
    state.list = listOf([
      session({
        cwd_state: { state: "unknown", why: "Operation not permitted" },
        resume: {
          command: "claude --resume e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
          caveat:
            "Could not check whether /Users/acme/code/widget still exists (Operation not permitted), so this omits the `cd` and will resume in whatever directory you run it from.",
          anchored: false,
        },
      }),
    ]);
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.getByText(/Could not check whether/)).toBeTruthy();
    expect(screen.queryByText(/is gone \(/)).toBeNull();
    expect(screen.getAllByText(/directory unchecked/).length).toBeGreaterThan(0);
  });

  /// A running session is not offered Resume at all.
  ///
  /// `claude --help`: resuming a running session starts a COPY of it. A
  /// button labelled Resume would promise something it does not do.
  it("offers no resume command for a session that is already running", () => {
    state.list = listOf([
      session({ liveness: { state: "running", pid: 14779, status: "busy" } }),
    ]);
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.queryByRole("button", { name: /copy resume command/i })).toBeNull();
    expect(screen.getByText(/starts a second copy of it/i)).toBeTruthy();
  });

  /// An `unknown` liveness still offers Resume -- with the caveat that we
  /// did not establish it is over. It is probably over (that is what
  /// 1,400 imported rows are), but the label must not imply we checked.
  it("offers resume for an unknown liveness, saying it may already be running", () => {
    state.list = listOf([
      session({
        liveness: { state: "unknown", why: "this session's process was never observed" },
      }),
    ]);
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.getByRole("button", { name: /copy resume command/i })).toBeTruthy();
    expect(screen.getByText(/may already be open somewhere/i)).toBeTruthy();
  });

  it("reports a clipboard failure rather than doing nothing", async () => {
    copyFn.mockResolvedValueOnce("This window has no clipboard access.");
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /copy resume command/i }));
    await vi.waitFor(() =>
      expect(toastError).toHaveBeenCalledWith(
        expect.stringMatching(/could not copy/i),
        expect.objectContaining({ description: "This window has no clipboard access." }),
      ),
    );
  });
});

describe("absent is not zero", () => {
  /// A failed list read is an ERROR, never "you have no sessions".
  ///
  /// The precedent is #846 one view over: a `= []` default made a
  /// rejected scan read as "No CLAUDE.md files in this repository".
  it("renders a failed read as an error and not as an empty list", () => {
    state.list = undefined;
    state.failed = true;
    renderView();
    expect(screen.getByText(/could not read the claude code sessions/i)).toBeTruthy();
    expect(screen.getByText(/database is locked/)).toBeTruthy();
    expect(screen.queryByText(/no claude code sessions/i)).toBeNull();
  });

  /// The error arm must be REACHABLE -- ordered before the empty one.
  ///
  /// This is the half #846's own guard cannot check, and the heart of its
  /// fix: with data defaulting to `[]` the empty branch is reached first
  /// and the error arm never renders in the case it exists for. Asserted
  /// with BOTH a failure and an empty list present at once.
  it("prefers the error arm over the empty arm when both could apply", () => {
    state.list = listOf([]);
    state.failed = true;
    renderView();
    expect(screen.getByText(/could not read the claude code sessions/i)).toBeTruthy();
    expect(screen.queryByText(/no claude code sessions on this machine/i)).toBeNull();
  });

  it("says there are none only when the read succeeded", () => {
    state.list = listOf([]);
    renderView();
    expect(screen.getByText(/no claude code sessions on this machine/i)).toBeTruthy();
  });

  /// An unreadable live registry is stated, and does not silently mean
  /// "nothing is running". The directory is mode `0700`, so this happens.
  it("says it could not tell what is running when the registry failed", () => {
    state.list = listOf([session()], {
      registry_failure: "could not read /Users/acme/.claude/sessions: Permission denied",
    });
    renderView();
    expect(screen.getByText(/could not tell which sessions are running/i)).toBeTruthy();
    expect(screen.getByText(/not the same as .not running./i)).toBeTruthy();
    // And the rows are STILL shown: a partial answer labelled partial
    // beats an error page.
    expect(screen.getAllByRole("button", { name: /HeadState GitHub/i }).length).toBeGreaterThan(0);
  });

  it("counts the registry records it could not parse", () => {
    state.list = listOf([session()], {
      registry_unreadable: ["/Users/acme/.claude/sessions/1.json: expected value"],
    });
    renderView();
    expect(screen.getByText(/1 live-session record could not be read/i)).toBeTruthy();
  });

  /// A partial rescan says how much is missing, above a list that still
  /// renders. `Scan::is_partial`'s own doc comment states the rule.
  it("says the list is incomplete when the rescan could not read everything", () => {
    state.imported = imported({
      sessions: 1200,
      unreadable_dirs: ["/Users/acme/.claude/projects/secret: Permission denied"],
      unreadable_files: ["/Users/acme/.claude/projects/a/b.jsonl: Permission denied"],
    });
    renderView();
    expect(screen.getByText(/2 could not be/i)).toBeTruthy();
    expect(screen.getByText(/incomplete by an unknown amount/i)).toBeTruthy();
  });

  /// A machine that has NEVER run Claude Code is not accused of a failed
  /// read (#970).
  ///
  /// This is the shape no test covered from either side: `sessions: 0`
  /// with the ROOT named. The neighbouring case above seeds a permission
  /// error on a SUBdirectory with sessions present, and the default
  /// fixture has `unreadable_dirs: []`, so a new user's first screen was
  /// untested -- and it said "0 sessions read, but 1 could not be — this
  /// list is incomplete by an unknown amount", which is false. Nothing
  /// could not be read; there is nothing there.
  it("does not call a never-run machine's empty list incomplete", () => {
    state.list = listOf([]);
    state.imported = imported({
      sessions: 0,
      absent_root: "/Users/acme/.claude/projects",
      elapsed_ms: 4,
    });
    renderView();
    expect(screen.queryByText(/incomplete by an unknown amount/i)).toBeNull();
    expect(screen.queryByText(/could not be/i)).toBeNull();
    // And it still NAMES the path, which is why #970 kept the information
    // rather than dropping it.
    expect(screen.getByText("/Users/acme/.claude/projects")).toBeTruthy();
    expect(screen.getByText(/does not exist yet/i)).toBeTruthy();
  });

  /// A permission error on the root is STILL loud (#970).
  ///
  /// The pair to the test above, and the half that must not regress:
  /// `ENOENT` and `EACCES` produce the same empty list and have opposite
  /// remedies. A user whose history is behind a permission wall genuinely
  /// has an incomplete list, and telling them "you have no sessions" is
  /// #846 in the opposite direction.
  it("still says the list is incomplete when the root itself was unreadable", () => {
    state.list = listOf([]);
    state.imported = imported({
      sessions: 0,
      absent_root: null,
      unreadable_dirs: ["/Users/acme/.claude/projects: Permission denied"],
    });
    renderView();
    expect(screen.getByText(/incomplete by an unknown amount/i)).toBeTruthy();
    expect(screen.queryByText(/does not exist yet/i)).toBeNull();
  });

  /// A root that EXISTS and holds nothing gets its own sentence (#970).
  ///
  /// A user who ran `claude` once and cleared their history is not a user
  /// who has never run it, so the page must not claim the directory is
  /// missing. This is the third empty, and it keeps `absent_root` from
  /// becoming a second way of saying "zero".
  it("distinguishes a cleared history from a machine that never ran claude", () => {
    state.list = listOf([]);
    state.imported = imported({ sessions: 0, absent_root: null });
    renderView();
    expect(screen.getByText(/holds no session transcripts/i)).toBeTruthy();
    expect(screen.queryByText(/does not exist yet/i)).toBeNull();
  });

  /// The rescan's failure is separate from the list's: the list may still
  /// be perfectly readable, just stale.
  it("reports a failed rescan without hiding the stored sessions", () => {
    state.importFailed = true;
    renderView();
    expect(screen.getByText(/could not re-read the transcripts/i)).toBeTruthy();
    expect(screen.getByText(/newer ones may be missing/i)).toBeTruthy();
    expect(screen.getAllByRole("button", { name: /HeadState GitHub/i }).length).toBeGreaterThan(0);
  });
});

describe("the list at the real corpus size", () => {
  const many = (n: number) =>
    Array.from({ length: n }, (_, i) =>
      session({
        session_id: `session-${i}`,
        name: `Session number ${i}`,
        last_activity_at: new Date(Date.parse("2026-09-13T00:00:00Z") - i * 60_000).toISOString(),
      }),
    );

  /// A cap that STATES the total. Never a silently short list, which is
  /// the same defect class as an empty list on a failed read.
  it("caps the rendered rows and says how many there really are", () => {
    state.list = listOf(many(1438));
    renderView();
    expect(screen.getByText(/showing the 200 most recent of 1,438/i)).toBeTruthy();
    expect(screen.getByText(/1,438 sessions/i)).toBeTruthy();
    // `queryByText`, not `queryByRole(…, { name })` -- see the next test
    // for the measurement. Row 500 is past the cap, so it must be absent.
    expect(screen.queryByText("Session number 500")).toBeNull();
    // ...and row 0 is present, so the absence above is the CAP rather
    // than the list failing to render at all.
    expect(screen.getByText("Session number 0")).toBeTruthy();
  });

  /// The "show all" control genuinely renders the rest.
  ///
  /// Asserted with `getByText` rather than `getByRole(…, { name })`.
  /// That is not cosmetic: `getByRole` with a name builds the
  /// accessibility tree and computes an accessible name for every one of
  /// 1,438 buttons, which measured **6.5s locally against 453ms** for the
  /// capped case above -- and timed out at CI's 15s limit on a slower
  /// runner, which is how this was found. `getByText` matches one text
  /// node and costs milliseconds.
  ///
  /// The assertion is unchanged in meaning: row 500 exists only when the
  /// cap is lifted, and it is absent in the capped test above.
  it("shows every row when asked to", () => {
    state.list = listOf(many(1438));
    renderView();
    fireEvent.click(screen.getByRole("button", { name: /show all 1,438/i }));
    expect(screen.getByText("Session number 500")).toBeTruthy();
    expect(screen.queryByText(/showing the 200 most recent/i)).toBeNull();
  });

  /// Search covers four fields, because titles are not unique: 286 of
  /// 1,438 sessions share a title with another, and repeated
  /// `/security-review` runs are the bulk of them.
  it("searches the title, the directory, the branch and the id", () => {
    state.list = listOf([
      session({ session_id: "a", name: "Notarization fix", cwd: "/code/alpha", git_branch: "main" }),
      session({ session_id: "b-unique-id", name: "Something else", cwd: "/code/beta", git_branch: "feat/x" }),
    ]);
    const { container } = renderView();
    const search = within(container).getByLabelText(/search claude code sessions/i);

    fireEvent.change(search, { target: { value: "notariz" } });
    expect(screen.getByText(/1 of 2 match/i)).toBeTruthy();

    fireEvent.change(search, { target: { value: "/code/beta" } });
    expect(screen.getByRole("button", { name: /Something else/ })).toBeTruthy();

    fireEvent.change(search, { target: { value: "feat/x" } });
    expect(screen.getByRole("button", { name: /Something else/ })).toBeTruthy();

    fireEvent.change(search, { target: { value: "b-unique-id" } });
    expect(screen.getByRole("button", { name: /Something else/ })).toBeTruthy();
  });

  it("says nothing matched rather than claiming there are no sessions", () => {
    renderView();
    fireEvent.change(screen.getByLabelText(/search claude code sessions/i), {
      target: { value: "nothing whatsoever" },
    });
    expect(screen.getByText(/no session matches that search/i)).toBeTruthy();
    expect(screen.queryByText(/no claude code sessions on this machine/i)).toBeNull();
  });

  /// The two sessions in 1,438 with no `aiTitle` get their id, not a
  /// fabricated name. The id is at least true, and it is also the resume
  /// handle.
  it("falls back to the session id rather than inventing a name", () => {
    state.list = listOf([session({ name: null })]);
    renderView();
    expect(
      screen.getAllByRole("button", { name: /e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2/ }).length,
    ).toBeGreaterThan(0);
  });

  /// Every row carries a DATE, because title alone cannot identify one:
  /// 147 sessions in the largest directory share a title with a sibling.
  it("shows a relative date on every row, computed from the prop", () => {
    renderView();
    // 2026-09-13T09:00:00Z against a `now` of 12:00:00Z.
    expect(screen.getAllByText(/3 hours ago/).length).toBeGreaterThan(0);
  });

  it("says so rather than inventing a date when none was recorded", () => {
    state.list = listOf([session({ last_activity_at: null })]);
    renderView();
    expect(screen.getAllByText(/no recorded activity/i).length).toBeGreaterThan(0);
  });
});

describe("what the detail says about provenance", () => {
  /// `runs: 0` is the whole imported corpus. Saying so distinguishes "we
  /// never watched this process" from "we watched it and it ended", which
  /// is also the difference between two liveness answers.
  it("says a transcript-read session was never watched", () => {
    state.list = listOf([session({ runs: 0 })]);
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.getByText(/not watched while it ran/i)).toBeTruthy();
  });

  it("names the reason a reveal failed rather than appearing inert", async () => {
    revealFn.mockRejectedValueOnce("/Users/acme/code/widget no longer exists");
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /reveal directory/i }));
    await vi.waitFor(() =>
      expect(toastError).toHaveBeenCalledWith(
        expect.stringMatching(/could not reveal the directory/i),
        expect.objectContaining({
          description: expect.stringContaining("no longer exists"),
        }),
      ),
    );
  });

  it("rescans on request", () => {
    renderView();
    fireEvent.click(screen.getByRole("button", { name: /rescan transcripts/i }));
    expect(rescanFn).toHaveBeenCalled();
  });

  /// The measurement is shown, not merely claimed -- the same reason
  /// `Scan` carries `elapsed_ms`: it keeps the "no incremental
  /// machinery" decision checkable on someone else's machine.
  it("shows how long the rescan took", () => {
    renderView();
    expect(screen.getByText(/1,438 read in 1374/)).toBeTruthy();
  });

  /// A FIRST scan does not say "Rescanning…" (#978).
  ///
  /// `isFetching` is true during the first fetch as well, so keying the
  /// label only on it put a "Re-" prefix on a machine that had never
  /// scanned -- asserting work that did not happen. `imported.data ===
  /// undefined` is what separates the two.
  it("says Scanning rather than Rescanning on the very first scan", () => {
    state.imported = undefined;
    state.importFetching = true;
    renderView();
    expect(screen.getByRole("button", { name: /^scanning/i })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /rescanning/i })).toBeNull();
  });

  /// A SECOND scan does say "Rescanning…" (#978).
  ///
  /// The pair, and the one that keeps the fix from silently deleting the
  /// label: once a scan has returned, re-running it really is a rescan.
  it("says Rescanning once a scan has already returned", () => {
    state.importFetching = true;
    renderView();
    expect(screen.getByRole("button", { name: /rescanning/i })).toBeTruthy();
  });

  /// `0 read in 4ms` is suppressed, not qualified (#978).
  ///
  /// The figure exists to keep the "no incremental machinery" decision
  /// checkable, and a scan that found nothing is no evidence of that -- it
  /// is a developer-facing measurement shown to a first-run user as the
  /// OUTCOME of their scan, which reads as a failed load. The house rule
  /// (#976) is to qualify a figure a short read makes only LOW and to
  /// suppress one it makes misleading; this is the second.
  it("does not show a zero read count as the outcome of a first scan", () => {
    state.list = listOf([]);
    state.imported = imported({ sessions: 0, elapsed_ms: 4, absent_root: "/x/.claude/projects" });
    renderView();
    expect(screen.queryByText(/0 read in/i)).toBeNull();
    expect(screen.queryByText(/read in 4/i)).toBeNull();
  });

  /// A MEASURED non-zero count stays silent about nothing (#978).
  ///
  /// The pair to the test above. Suppressing at zero must not suppress the
  /// figure the line exists for -- `the_measurement_stays_on_a_real_scan`
  /// in prose. One row read is still a real measurement.
  it("still shows the measurement when the scan actually read something", () => {
    state.imported = imported({ sessions: 1, elapsed_ms: 7 });
    renderView();
    expect(screen.getByText(/1 read in 7/)).toBeTruthy();
  });

  /// The empty list is told what would FILL it (#978).
  ///
  /// The epic's first bullet is "an empty state that explains nothing".
  /// The zero is honest and stays; what was missing is the next step, and
  /// the page's whole subject -- that sessions come from transcripts
  /// Claude Code writes to disk when you run it -- was nowhere on screen.
  it("says what produces a session rather than only that there are none", () => {
    state.list = listOf([]);
    state.imported = imported({ sessions: 0, absent_root: "/Users/acme/.claude/projects" });
    renderView();
    // `textContent` rather than `getByText`: the sentence is broken across
    // `<span className="font-mono">` for the command name, so the accessible
    // text spans several nodes.
    const body = document.body.textContent ?? "";
    expect(body).toMatch(/run claude in any directory and it will appear here/i);
    expect(body).toMatch(/transcripts claude code writes to disk/i);
  });

  /// The detail pane does not tell the user to choose from nothing (#978).
  ///
  /// "Choose a session to see where it ran and how to resume it." printed
  /// beside a list with no sessions is an instruction a reader cannot
  /// follow, and one who tries reasonably concludes the list failed to
  /// load -- which is the one thing the #846 error arm above exists to
  /// distinguish an empty list from.
  it("does not invite a choice from an empty list", () => {
    state.list = listOf([]);
    state.imported = imported({ sessions: 0, absent_root: "/Users/acme/.claude/projects" });
    renderView();
    expect(screen.queryByText(/choose a session/i)).toBeNull();
    expect(screen.getByText(/nothing to show yet/i)).toBeTruthy();
  });

  /// And it DOES invite a choice when there is something to choose (#978).
  ///
  /// The pair: the prompt is right whenever the list has rows and none is
  /// selected, and removing it outright would lose the one sentence that
  /// tells a reader what the right-hand pane is for.
  it("still invites a choice when the list has rows", () => {
    renderView();
    expect(screen.getByText(/choose a session/i)).toBeTruthy();
  });
});

/// #919: the reveal buttons, and the three reasons one cannot fire.
///
/// The behaviour under test is that a reveal which CANNOT work is present
/// and disabled with a stated reason, rather than absent (indistinguishable
/// from "this app has no such action") or enabled and inert (on macOS,
/// revealing a deleted path silently opens the home folder).
describe("revealing a path that may be gone", () => {
  const revealDirectory = () => screen.getByRole("button", { name: /reveal directory/i });
  const revealTranscript = () => screen.getByRole("button", { name: /reveal transcript/i });

  it("reveals a directory that exists", () => {
    renderView();
    open("HeadState GitHub issues filing");
    expect(revealDirectory().hasAttribute("disabled")).toBe(false);
    fireEvent.click(revealDirectory());
    expect(revealFn).toHaveBeenCalledWith("/Users/acme/code/widget");
  });

  /// 83.0% of real rows. The button STAYS, disabled, with the reason --
  /// which is what distinguishes "this path is gone" from "this app
  /// cannot do that".
  it("disables the reveal for a gone directory and says why", () => {
    state.list = listOf([session({ cwd_state: { state: "gone" } })]);
    renderView();
    open("HeadState GitHub issues filing");
    const btn = revealDirectory();
    expect(btn.hasAttribute("disabled")).toBe(true);
    expect(screen.getByText(/the path no longer exists/i)).toBeTruthy();
    // And it really is inert: clicking a disabled button must not reach
    // the command.
    fireEvent.click(btn);
    expect(revealFn).not.toHaveBeenCalled();
  });

  /// **The sabotage test for #919.** "Could not check" must NOT read as
  /// "gone".
  ///
  /// This is the absent-is-not-zero rule at the UI boundary. The remedies
  /// differ -- one is "fix the permission", the other is "expect it to
  /// stay missing" -- so the two must not share a string. The assertion
  /// that the gone wording is ABSENT is the half that fails if someone
  /// collapses the two arms into one.
  it("distinguishes a path it could not check from one that is gone", () => {
    state.list = listOf([
      session({
        cwd_state: { state: "unknown", why: "Permission denied (os error 13)" },
      }),
    ]);
    renderView();
    open("HeadState GitHub issues filing");

    expect(revealDirectory().hasAttribute("disabled")).toBe(true);
    // The reason is NAMED, because a "could not check" with nothing to
    // act on is barely better than "gone".
    expect(screen.getByText(/could not check whether it exists/i)).toBeTruthy();
    expect(screen.getByText(/Permission denied \(os error 13\)/)).toBeTruthy();
    // And it must NOT claim the path is gone.
    expect(screen.queryByText(/no longer exists/i)).toBeNull();
  });

  it("says no path was recorded rather than calling it gone", () => {
    state.list = listOf([session({ cwd: null, cwd_state: { state: "not-recorded" } })]);
    renderView();
    open("HeadState GitHub issues filing");
    expect(revealDirectory().hasAttribute("disabled")).toBe(true);
    expect(screen.queryByText(/no longer exists/i)).toBeNull();
  });

  /// **The 83%-vs-0% asymmetry, as a render test.** This is the defect
  /// #919 is really about.
  ///
  /// The common real row has a deleted worktree AND a perfectly readable
  /// transcript -- 1,213 of 1,461 measured. The transcript button must
  /// therefore be live on exactly the rows where the directory button is
  /// dead. A single shared state, or a transcript gated on `cwd_state`,
  /// fails here.
  it("keeps the transcript reveal live when the directory is gone", () => {
    state.list = listOf([
      session({
        cwd_state: { state: "gone" },
        transcript_state: { state: "exists" },
      }),
    ]);
    renderView();
    open("HeadState GitHub issues filing");

    expect(revealDirectory().hasAttribute("disabled")).toBe(true);
    expect(revealTranscript().hasAttribute("disabled")).toBe(false);
    fireEvent.click(revealTranscript());
    expect(revealFn).toHaveBeenCalledWith("/Users/acme/.claude/projects/slug/e5dff3bd.jsonl");
  });

  /// The mirror: a transcript that IS gone disables its own button and
  /// leaves the directory's alone. Without this, a version that simply
  /// swapped the two fields would pass the test above.
  it("disables only the transcript when only the transcript is gone", () => {
    state.list = listOf([
      session({
        cwd_state: { state: "exists" },
        transcript_state: { state: "gone" },
      }),
    ]);
    renderView();
    open("HeadState GitHub issues filing");
    expect(revealDirectory().hasAttribute("disabled")).toBe(false);
    expect(revealTranscript().hasAttribute("disabled")).toBe(true);
  });

  it("disables the transcript reveal when none was recorded", () => {
    state.list = listOf([
      session({ transcript_path: null, transcript_state: { state: "not-recorded" } }),
    ]);
    renderView();
    open("HeadState GitHub issues filing");
    expect(revealTranscript().hasAttribute("disabled")).toBe(true);
  });
});

/// #920: jumping from a session to the worktree it ran in.
describe("the jump to a session's worktree", () => {
  const worktree = (over: Partial<Worktree> = {}): Worktree => ({
    path: "/Users/acme/code/widget",
    branch: "feat/spoon",
    head: "abc1234",
    size_bytes: 1024,
    safety: { kind: "safe" },
    is_main: false,
    merged_at: "2026-09-12",
    upstream: { kind: "current" },
    last_commit: "2026-09-12T10:00:00Z",
    ...over,
  });
  const repo = (worktrees: Worktree[]): WorktreeRepo => ({
    identity: "acme/widget",
    name: "widget",
    path: "/Users/acme/code/widget",
    worktrees,
  });

  it("offers the jump and navigates the way WorktreesPage reads it", () => {
    state.worktrees = [repo([worktree()])];
    renderView();
    open("HeadState GitHub issues filing");

    // The worktree's own state is what answers "what was this session
    // doing", so it is shown before the jump rather than only after it.
    expect(screen.getByText(/merged, pushed — safe to delete/i)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: /show in worktrees/i }));

    // BOTH writes, and against the real store: `WorktreesPage` selects a
    // repository with `filters.repo` and renders on `view`. Asserting
    // only the view would pass while landing on the wrong repository.
    const after = useFilters.getState();
    expect(after.view).toBe("worktrees");
    expect(after.filtersByView.worktrees.repo).toBe("/Users/acme/code/widget");
  });

  /// No jump unless a worktree actually matches. A button that navigated
  /// to a list where the row is absent is worse than no button -- and this
  /// is 1,213 of 1,461 real rows, so it is the common case.
  it("offers no jump when the directory is not a known worktree", () => {
    state.worktrees = [repo([worktree({ path: "/Users/acme/code/other" })])];
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.queryByRole("button", { name: /show in worktrees/i })).toBeNull();
    expect(screen.getByText(/not a worktree Headstate knows about/i)).toBeTruthy();
  });

  /// **The absent-is-not-zero test for #920.** A worktree listing that
  /// could not be READ must not render as "this is not a worktree".
  ///
  /// Opposite remedies: one is "retry, or check the configured
  /// directories", the other is "this directory never was one". #846 is
  /// the precedent -- a `= []` default made a failed scan read as a
  /// confident empty answer.
  it("says the worktree list could not be read rather than claiming no match", () => {
    state.worktrees = undefined;
    state.worktreesFailed = true;
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.getByText(/could not read the worktree list/i)).toBeTruthy();
    expect(screen.queryByText(/not a worktree Headstate knows about/i)).toBeNull();
    expect(screen.queryByRole("button", { name: /show in worktrees/i })).toBeNull();
  });

  it("says it is still looking while the listing loads", () => {
    state.worktrees = undefined;
    state.worktreesFailed = false;
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.getByText(/looking for a matching worktree/i)).toBeTruthy();
    // Not the could-not-read arm, and not the no-match arm.
    expect(screen.queryByText(/could not read the worktree list/i)).toBeNull();
    expect(screen.queryByText(/not a worktree Headstate knows about/i)).toBeNull();
  });

  /// MEASURED: the recorded branch disagrees with the current one on 54
  /// of 206 matches (26.2%). The jump is still offered -- the path is the
  /// key -- but the row must not read as "this session's branch".
  it("says so when the worktree has moved to another branch", () => {
    state.worktrees = [repo([worktree({ branch: "main" })])];
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.getByText(/has since moved to/i)).toBeTruthy();
    // The jump is NOT withheld: matching on the branch too would refuse a
    // quarter of the valid jumps.
    expect(screen.getByRole("button", { name: /show in worktrees/i })).toBeTruthy();
  });

  it("says nothing about branches when they agree", () => {
    state.worktrees = [repo([worktree()])];
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.queryByText(/has since moved to/i)).toBeNull();
  });

  it("says there is no worktree to find when no directory was recorded", () => {
    state.list = listOf([session({ cwd: null, cwd_state: { state: "not-recorded" } })]);
    state.worktrees = [repo([worktree()])];
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.getByText(/no directory was recorded/i)).toBeTruthy();
  });
});

/// #949: the two axes a user acts on, as controls rather than prose.
///
/// 1,474 sessions were filterable only by four TEXT fields, while
/// `cwd_state` and `liveness` -- both on every row, both read for display --
/// were never read for filtering. On the measured corpus that meant paging
/// through 1,295 rows whose directory was deleted to reach the 179 that can
/// be resumed into place.
describe("filtering the session list by state", () => {
  /// A mixed list covering all four chips plus the states that belong to
  /// NEITHER chip on their axis. Six rows, each one the only member of its
  /// bucket, so an assertion naming a row is an assertion about a predicate.
  ///
  /// Every title is a bird, deliberately: the chip labels are
  /// "Resumable"/"Running"/"Ended", and a fixture named "Resumable one"
  /// makes `getByRole("button", { name: /Resumable/ })` ambiguous between
  /// the chip and the row -- which is how the first draft of these tests
  /// failed. Names with no overlap at all keep each assertion about one
  /// thing. Synthetic per `CONTRIBUTING.md`.
  function mixed() {
    state.list = listOf([
      session({
        session_id: "r-1",
        name: "Kestrel",
        cwd_state: { state: "exists" },
        liveness: { state: "dead", why: "pid 1 is no longer running" },
      }),
      session({
        session_id: "g-1",
        name: "Merlin",
        cwd_state: { state: "gone" },
        liveness: { state: "dead", why: "pid 2 is no longer running" },
      }),
      session({
        session_id: "live-1",
        name: "Goshawk",
        cwd_state: { state: "exists" },
        liveness: { state: "running", pid: 99, status: "busy" },
      }),
      session({
        session_id: "unk-1",
        name: "Buzzard",
        // The directory check FAILED. Belongs to neither directory chip.
        cwd_state: { state: "unknown", why: "Permission denied" },
        liveness: { state: "dead", why: "pid 3 is no longer running" },
      }),
      session({
        session_id: "nolive-1",
        name: "Osprey",
        cwd_state: { state: "gone" },
        // Liveness could not be established. Belongs to neither liveness
        // chip -- this is the entire imported history on a real machine.
        liveness: { state: "unknown", why: "never observed" },
      }),
      session({
        session_id: "norec-1",
        name: "Harrier",
        cwd: null,
        cwd_state: { state: "not-recorded" },
        liveness: { state: "dead", why: "pid 4 is no longer running" },
      }),
    ]);
  }

  /// Every title in `mixed`, so a row assertion can be written as a set
  /// rather than a substring hunt. Alphabetical for readability only.
  const BIRDS = ["Buzzard", "Goshawk", "Harrier", "Kestrel", "Merlin", "Osprey"] as const;

  /// Scoped to the chip group, which is what makes this unambiguous even if
  /// a future fixture does share a word with a chip label.
  const chip = (name: string) =>
    within(screen.getByRole("group", { name: /filter sessions by state/i })).getByRole(
      "button",
      { name: new RegExp(`^${name}`, "i") },
    );

  /// Which of `mixed`'s rows are currently drawn, by title. Not "every
  /// button that looks like a row": an exact membership test over a known
  /// set, so a predicate that admits one row too many fails by NAME rather
  /// than by a count nobody can attribute.
  const rowNames = () =>
    BIRDS.filter((b) => screen.queryByRole("button", { name: new RegExp(b, "i") }) !== null);

  /// The chips exist, are labelled, and carry their POPULATION.
  ///
  /// The count is not decoration: a chip reading "Resumable" with no number
  /// tells the reader nothing about whether pressing it is worth it, and a
  /// count computed over the CURRENT subset rather than the whole list would
  /// read 0 on every chip but the active one.
  it("offers a chip per state, each with its count over the whole list", () => {
    mixed();
    renderView();

    const group = screen.getByRole("group", { name: /filter sessions by state/i });
    // 6 rows in, and the counts must partition honestly: 1 resumable,
    // 1 gone-and-not-running... except `nolive-1` is `gone` with UNKNOWN
    // liveness, which `!== "running"` admits. So gone is 2.
    expect(within(group).getByRole("button", { name: /^All 6/i })).toBeTruthy();
    expect(within(group).getByRole("button", { name: /^Resumable 1/i })).toBeTruthy();
    expect(within(group).getByRole("button", { name: /^Directory gone 2/i })).toBeTruthy();
    expect(within(group).getByRole("button", { name: /^Running 1/i })).toBeTruthy();
    expect(within(group).getByRole("button", { name: /^Ended 4/i })).toBeTruthy();
  });

  /// **The one that matters.** Each chip narrows the list to its own rows.
  ///
  /// SABOTAGE: make `matchesClaudeFilter` return `true` unconditionally and
  /// every case below fails, naming the row that should have been filtered
  /// out. A test that only asserted the chip renders would stay green.
  it.each([
    // Kestrel alone: `exists` AND not running. Goshawk's directory exists
    // too and is excluded because it IS running -- which is the overview's
    // own subtraction, and the reason the tile's 179 matches this list.
    ["Resumable", ["Kestrel"]],
    // Merlin (`gone` + dead) and Osprey (`gone` + liveness unknown). Osprey
    // belongs here because this chip is about the DIRECTORY and "not
    // running" admits a liveness we could not establish.
    ["Directory gone", ["Merlin", "Osprey"]],
    ["Running", ["Goshawk"]],
    // Four of six: everything `dead`. Not Goshawk (running) and not Osprey
    // (unknown).
    ["Ended", ["Buzzard", "Harrier", "Kestrel", "Merlin"]],
  ])("narrows the list to the %s rows", (label, expected) => {
    mixed();
    renderView();

    fireEvent.click(chip(label));

    // An EXACT set, not a superset. A predicate returning `true`
    // unconditionally fails here by naming the extra rows.
    expect(rowNames()).toEqual(expected);
  });

  /// **Absent is not zero, as a filter.** A directory whose check FAILED is
  /// in neither directory chip.
  ///
  /// This is the guard the issue asks for by name. `cwd_state` is a
  /// four-state and `revealRefusal` gives all four different wording
  /// precisely because collapsing them is the shrug the tri-state exists to
  /// prevent -- so a Resumable chip that swept `unknown` in with `exists`
  /// would tell the user a resume will land in the right tree when the app
  /// does not know whether the tree is there, and a Gone chip that swept it
  /// in with `gone` would send them looking for work that was never lost.
  ///
  /// The consequence is that the two chips do not sum to the total, which is
  /// asserted rather than glossed: 1 + 2 < 6 here, and the missing rows are
  /// the unchecked and the unrecorded ones.
  it("puts a session whose directory could not be checked in neither directory chip", () => {
    mixed();
    renderView();

    // Buzzard's check failed (`unknown`); Harrier never had a path
    // (`not-recorded`). Both are absent from BOTH directory chips.
    fireEvent.click(chip("Resumable"));
    expect(rowNames()).not.toContain("Buzzard");
    expect(rowNames()).not.toContain("Harrier");

    fireEvent.click(chip("Directory gone"));
    expect(rowNames()).not.toContain("Buzzard");
    expect(rowNames()).not.toContain("Harrier");

    // And the arithmetic does not close, which is the honest consequence:
    // 1 resumable + 2 gone < 6 sessions, with the shortfall being exactly
    // the two rows above. Asserted so a later "tidy-up" that folds
    // `unknown` into `gone` to make the numbers add up fails here.
    fireEvent.click(chip("All"));
    expect(rowNames().length).toBe(6);
  });

  /// And the same rule on the liveness axis: `unknown` is not `dead`.
  ///
  /// `Liveness`'s own doc calls rendering `unknown` as "not running" #841's
  /// fail-open, because "not running" is what offers Resume and resuming a
  /// live session starts a second copy. An Ended chip built on
  /// `!== "running"` would be that mistake as a control -- and it would
  /// sweep in the entire imported history, making the chip mean "all".
  it("does not treat a liveness that could not be established as ended", () => {
    mixed();
    renderView();

    fireEvent.click(chip("Ended"));
    // Osprey's liveness could not be established. Not ended.
    expect(rowNames()).not.toContain("Osprey");
    // But the four genuinely dead ones ARE there, so this is a real
    // narrowing and not an empty chip.
    expect(rowNames().length).toBe(4);
  });

  /// The chip and the search box COMPOSE, and the count line says which
  /// denominator it is reporting against.
  ///
  /// "Showing N of M" must stay exact in every mode. A chip narrows what the
  /// search searches, so "1 of 2 match in this filter" is a different claim
  /// from "1 of 6 match" -- and collapsing them would make the number on
  /// screen unattributable to either control.
  it("intersects with the search box and says which total it is counting against", () => {
    mixed();
    renderView();

    fireEvent.click(chip("Directory gone"));
    fireEvent.change(screen.getByLabelText(/search claude code sessions/i), {
      target: { value: "osprey" },
    });

    // Merlin is in the filter and does not match the search; Kestrel
    // matches neither. So one row from a filter holding two, out of six.
    expect(rowNames()).toEqual(["Osprey"]);
    expect(document.body.textContent).toMatch(/1 of 2 match in this filter/i);
    expect(document.body.textContent).toMatch(/6 sessions in all/i);
  });

  /// A chip that matches nothing says THAT, not "no sessions on this
  /// machine".
  ///
  /// The same distinction #846 draws between a failed read and an empty one,
  /// one level in: a reader who cannot tell "this filter is empty" from
  /// "this machine has never run Claude Code" will go looking for a rescan
  /// they do not need.
  it("distinguishes an empty filter from an empty machine", () => {
    state.list = listOf([
      session({ cwd_state: { state: "gone" }, liveness: { state: "dead", why: "gone" } }),
    ]);
    renderView();

    fireEvent.click(chip("Running"));
    expect(screen.getByText(/no session is in this filter/i)).toBeTruthy();
    // NOT `NoSessions`, which is a claim about the MACHINE: it names
    // `~/.claude/projects` and offers a rescan. Reaching it under an active
    // chip would tell a user with 1,474 sessions that they have none, and
    // send them to a rescan that would change nothing. This is the arm
    // ordering asserted, not just the wording (#949 over #970/#978).
    expect(screen.queryByText(/no claude code sessions on this machine/i)).toBeNull();
    expect(screen.queryByText(/~\/\.claude\/projects/)).toBeNull();
  });

  /// And the machine-empty arm still reaches `NoSessions` under no chip.
  ///
  /// The guard on the guard above: an ordering that sent every empty list to
  /// the filter sentence would satisfy it while hiding the one explanation a
  /// first-run machine needs.
  it("still explains an empty machine when no chip is active", () => {
    state.list = listOf([]);
    renderView();

    expect(screen.getByText(/no claude code sessions on this machine/i)).toBeTruthy();
    expect(screen.queryByText(/no session is in this filter/i)).toBeNull();
  });

  /// Which chip is on is available to a screen reader, not only as a
  /// background colour.
  ///
  /// The same rule the pressure row states about never letting colour be
  /// the only cue, applied to the one property of this control that matters
  /// most: a reader who cannot see which chip is pressed cannot tell a
  /// filtered list from a short one.
  it("says which chip is pressed", () => {
    mixed();
    renderView();

    fireEvent.click(chip("Resumable"));
    expect(chip("Resumable").getAttribute("aria-pressed")).toBe("true");
    expect(chip("All").getAttribute("aria-pressed")).toBe("false");
  });

  /// The running partition SURVIVES the chips.
  ///
  /// Running sessions are pinned above everything, and the chips filter the
  /// INPUT to that partition rather than replacing it -- re-deriving the
  /// ordering per chip would be a second description of a rule
  /// `sessions.rs` owns.
  it("keeps running sessions pinned to the top inside a filter", () => {
    state.list = listOf([
      // Dead FIRST in the data, so a partition that was dropped would show
      // this order back unchanged and fail below.
      session({
        session_id: "d-1",
        name: "Peregrine",
        cwd_state: { state: "exists" },
        liveness: { state: "dead", why: "pid 1 is no longer running" },
      }),
      session({
        session_id: "l-1",
        name: "Hobby",
        cwd_state: { state: "exists" },
        liveness: { state: "running", pid: 7, status: null },
      }),
    ]);
    renderView();

    // Both rows are in "All" and both have a live directory, so this is the
    // base ordering the chips filter the INPUT to. `textContent` order over
    // the rendered buttons, which is document order.
    const titles = screen
      .getAllByRole("button")
      .map((b) => b.textContent ?? "")
      .filter((t) => /Peregrine|Hobby/.test(t));
    expect(titles[0]).toMatch(/Hobby/);
    expect(titles[1]).toMatch(/Peregrine/);
  });
});

/// Subagent sessions are hidden, counted, and revealable (#1002).
///
/// 391 of 1,524 measured rows are sessions that ran inside an agent
/// worktree -- 25.7% of the list. With one live session the rows
/// immediately below it were that session's own machinery rather than the
/// user's past work, which is what #1002 reports from use.
///
/// Three properties, and each has its own test because each fails
/// independently:
///
/// 1. They are HIDDEN by default.
/// 2. The count is STATED (#975: a hidden exclusion that does not say how
///    many it hid leaves a user counting rows in disagreement with the app
///    and no way to find out why).
/// 3. They are REVEALABLE, and still real sessions when revealed.
describe("subagent sessions are hidden by default and say how many", () => {
  /// **The sabotage test.** Delete the `kind.kind !== "subagent"` filter
  /// in `useMatchedSessions` and this fails: the subagent row renders in
  /// the default list.
  it("does not show a subagent session in the default list", () => {
    state.list = listOf([
      session({ session_id: "own-1", name: "My own work" }),
      session({
        session_id: "sub-1",
        name: "VenvSection security review",
        cwd: "/Users/acme/code/widget/.claude/worktrees/agent-ad12506fcee31848a",
        kind: { kind: "subagent", agent_id: "ad12506fcee31848a" },
      }),
    ]);
    renderView();

    expect(screen.getByText("My own work")).toBeTruthy();
    expect(screen.queryByText("VenvSection security review")).toBeFalsy();
  });

  /// The happy-path pair: hiding must not eat the user's OWN sessions.
  /// A filter that hid everything would pass the test above.
  it("still shows every session that is not a subagent", () => {
    state.list = listOf([
      session({ session_id: "own-1", name: "My own work" }),
      session({ session_id: "own-2", name: "Another of mine" }),
    ]);
    renderView();

    expect(screen.getByText("My own work")).toBeTruthy();
    expect(screen.getByText("Another of mine")).toBeTruthy();
    // And with none to hide, the toggle is not offered at all: a control
    // promising to reveal nothing is noise.
    expect(screen.queryByText(/subagent session/)).toBeFalsy();
  });

  /// **The sabotage test for #975's rule.** Delete the count from the
  /// toggle's label and this fails. A hidden exclusion must state its
  /// size, or the user counting rows cannot find out why the app
  /// disagrees with them.
  it("says how many it hid", () => {
    state.list = listOf([
      session({ session_id: "own-1", name: "My own work" }),
      session({
        session_id: "sub-1",
        name: "Child one",
        kind: { kind: "subagent", agent_id: "a1" },
      }),
      session({
        session_id: "sub-2",
        name: "Child two",
        kind: { kind: "subagent", agent_id: "a2" },
      }),
    ]);
    renderView();

    expect(screen.getByText(/Show 2 subagent sessions/)).toBeTruthy();
  });

  it("reveals them when asked, and they are still real sessions", () => {
    state.list = listOf([
      session({ session_id: "own-1", name: "My own work" }),
      session({
        session_id: "sub-1",
        name: "VenvSection security review",
        kind: { kind: "subagent", agent_id: "ad12506fcee31848a" },
      }),
    ]);
    useFilters.setState({ claudeShowSubagents: true });
    renderView();

    // Not deleted, not a placeholder -- the row is back with its own name
    // and is selectable like any other. Several of these did substantial
    // work and remain resumable by id.
    expect(screen.getByText("VenvSection security review")).toBeTruthy();
    expect(screen.getByText("My own work")).toBeTruthy();
  });

  /// The chip counts must describe the list the chips actually open.
  ///
  /// #949's rule -- a chip reading 179 that opens a list of 181 is a chip
  /// that lied about where it went -- applied across the new axis. The
  /// subagent count itself is the exception and is over the WHOLE list,
  /// because it is the number the toggle offers to reveal.
  it("counts the chips over the sessions the toggle admits", () => {
    state.list = listOf([
      session({ session_id: "own-1", name: "Mine", liveness: { state: "running", pid: 1, status: null } }),
      session({
        session_id: "sub-1",
        name: "Machinery",
        liveness: { state: "running", pid: 2, status: null },
        kind: { kind: "subagent", agent_id: "a1" },
      }),
    ]);
    renderView();

    // One running session is VISIBLE, though two are running in all. The
    // chip's own count is the claim under test: a Running chip reading 2
    // that opens a list of 1 is a chip that lied about where it went.
    expect(screen.getByRole("button", { name: "Running 1" })).toBeTruthy();
    // And the toggle still offers the one it hid.
    expect(screen.getByText(/Show 1 subagent session/)).toBeTruthy();
  });
});

/// A parent's subagent tokens are a SEPARATE figure (#1002).
///
/// #959 kept four counters rather than one because cache reads run two to
/// three orders of magnitude above fresh input. The same argument one
/// level up: a parent's own tokens and its children's answer different
/// questions, and one summed figure would answer neither.
describe("the subagent rollup is beside the parent's own usage, never inside it", () => {
  const parentWithChildren = () =>
    session({
      session_id: "parent-1",
      name: "The parent",
      subagents: 2,
    });

  /// **The sabotage test.** Add the rollup's tokens into the parent's own
  /// figures in `SessionUsage` and this fails: the parent's own output
  /// figure stops being its own.
  it("shows the parent's own tokens and its subagents' tokens as two figures", async () => {
    state.list = listOf([parentWithChildren()]);
    state.usage = {
      messages: 10,
      input_tokens: 100,
      output_tokens: 200,
      cache_read_tokens: 300,
      cache_creation_tokens: 400,
      models: [],
      recorded_cost: null,
      truncated: false,
      bytes_read: 10,
      file_bytes: 10,
    };
    state.rollup = {
      sessions: 2,
      measured: 2,
      without_usage: 0,
      unreadable: [],
      truncated: 0,
      input_tokens: 1_000,
      output_tokens: 2_000,
      cache_read_tokens: 3_000,
      cache_creation_tokens: 4_000,
      messages: 20,
    };
    renderView();
    open("The parent");

    // Two headings, so the reader can tell which figure is which.
    expect(screen.getByText("How much work it did")).toBeTruthy();
    expect(screen.getByText("What its subagents did")).toBeTruthy();
    // The parent's own output is 200 and NOT 2,200: the two are never
    // added together.
    expect(screen.getByText("200")).toBeTruthy();
    expect(screen.getByText("2,000")).toBeTruthy();
    expect(screen.queryByText("2,200")).toBeFalsy();
  });

  /// **The sabotage test for absent-is-not-zero.** Make the rollup
  /// section render its four counters regardless of `measured` and this
  /// fails: zeros appear for a rollup that was never totalled.
  it("says it could not tell rather than showing zeros", () => {
    state.list = listOf([parentWithChildren()]);
    state.rollup = {
      sessions: 2,
      measured: 0,
      without_usage: 0,
      unreadable: ["/some/child.jsonl: could not open it"],
      truncated: 0,
      input_tokens: 0,
      output_tokens: 0,
      cache_read_tokens: 0,
      cache_creation_tokens: 0,
      messages: 0,
    };
    renderView();
    open("The parent");

    expect(
      screen.getByText(/None of their transcripts could be totalled/),
    ).toBeTruthy();
    // No figure at all for the rollup. "Cache read" cannot be the probe:
    // the PARENT's own usage panel carries that label too, and asserting
    // on it would pass for the wrong reason.
    expect(screen.queryByText("What its subagents did")).toBeTruthy();
    expect(screen.queryByText("20")).toBeFalsy();
  });

  /// The happy-path pair for the test above: a complete rollup must not
  /// wear a caveat it has not earned.
  it("does not add a caveat when every child was totalled", () => {
    state.list = listOf([parentWithChildren()]);
    state.rollup = {
      sessions: 2,
      measured: 2,
      without_usage: 0,
      unreadable: [],
      truncated: 0,
      input_tokens: 1,
      output_tokens: 1,
      cache_read_tokens: 1,
      cache_creation_tokens: 1,
      messages: 2,
    };
    renderView();
    open("The parent");

    expect(screen.queryByText(/floors rather than totals/)).toBeFalsy();
    expect(screen.queryByText(/floors, not totals/)).toBeFalsy();
  });

  it("states the denominator when only some children could be totalled", () => {
    state.list = listOf([parentWithChildren()]);
    state.rollup = {
      sessions: 2,
      measured: 1,
      without_usage: 0,
      unreadable: ["/some/child.jsonl: could not open it"],
      truncated: 0,
      input_tokens: 5,
      output_tokens: 5,
      cache_read_tokens: 5,
      cache_creation_tokens: 5,
      messages: 5,
    };
    renderView();
    open("The parent");

    expect(screen.getByText(/These cover 1 of 2 subagent sessions/)).toBeTruthy();
    expect(screen.getByText(/1 could not be read/)).toBeTruthy();
  });

  /// A rejected read must not render as a measurement (#846), and the
  /// error arm is before the empty arm for the reason the page states at
  /// length: `data` is undefined on a rejection exactly as it is before
  /// the first read.
  it("reports a failed rollup read rather than showing zeros", () => {
    state.list = listOf([parentWithChildren()]);
    state.rollupFailed = true;
    renderView();
    open("The parent");

    expect(screen.getByText(/Could not total what they used/)).toBeTruthy();
    // As above: the parent's own panel owns the "Cache read" label, so
    // the absence under test is the rollup's own message being the only
    // thing the section says.
    expect(screen.queryByText(/These cover/)).toBeFalsy();
  });

  /// A session with no subagents must not carry the section at all.
  it("adds no subagent section to an ordinary session", () => {
    state.list = listOf([session({ session_id: "plain", name: "Just mine" })]);
    renderView();
    open("Just mine");

    expect(screen.queryByText("What its subagents did")).toBeFalsy();
    expect(screen.queryByText("What ran this")).toBeFalsy();
  });
});

/// An unattributed subagent says so rather than being given a parent
/// (#1002).
///
/// The rule the whole feature turns on: where earliest-mention cannot
/// decide, the child stays unattributed. A wrong rollup is worse than no
/// rollup.
describe("a subagent whose parent could not be told says so", () => {
  /// **The sabotage test.** Make the detail pane fall back to a probable
  /// parent when `parent` is null and this fails: the pane names a
  /// session instead of admitting it could not tell.
  it("states that it could not tell, with the evidence", () => {
    state.list = listOf([
      session({
        session_id: "sub-1",
        name: "An orphan",
        kind: { kind: "subagent", agent_id: "ad12506fcee31848a" },
        parent: null,
        unattributed:
          "sess-a and sess-b both mention this agent first at 2026-09-14T02:23:29.713Z, " +
          "so which one spawned it cannot be told apart",
      }),
    ]);
    useFilters.setState({ claudeShowSubagents: true });
    renderView();
    open("An orphan");

    expect(screen.getByText(/which session started it could not be told/)).toBeTruthy();
    // The evidence, so the reader can see the app LOOKED rather than
    // shrugged.
    expect(screen.getByText(/cannot be told apart/)).toBeTruthy();
    expect(screen.queryByText("Spawned by")).toBeFalsy();
  });

  /// The happy-path pair: an attributed child names its parent and adds
  /// no "could not tell" noise.
  it("names the parent when it is known", () => {
    state.list = listOf([
      session({
        session_id: "sub-1",
        name: "A child",
        kind: { kind: "subagent", agent_id: "ad12506fcee31848a" },
        parent: {
          session_id: "e5dff3bd",
          name: "The spawning session",
          agent_id: "ad12506fcee31848a",
        },
        unattributed: null,
      }),
    ]);
    useFilters.setState({ claudeShowSubagents: true });
    renderView();
    open("A child");

    expect(screen.getByText("Spawned by")).toBeTruthy();
    expect(screen.getByText("The spawning session")).toBeTruthy();
    expect(
      screen.queryByText(/which session started it could not be told/),
    ).toBeFalsy();
  });
});

/// The page must be a pure function of its props and query data.
///
/// A source check, because the behaviour it forbids is invisible in a
/// render test: `Date.now()` during render produces correct output every
/// time and only misbehaves as a re-render under unchanged data. `yarn
/// lint`'s purity rule is the primary guard; this states the rule where a
/// reader of this file will see it, and catches the case of someone
/// disabling the rule inline.
describe("purity", () => {
  it("never reads the clock during render", async () => {
    const src = (await import("./ClaudeCodePage.tsx?raw")).default as string;
    const code = src
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .replace(/^\s*\/\/.*$/gm, "")
      .replace(/\/\/.*$/gm, "");
    expect(code).not.toMatch(/Date\.now\(\)/);
    // `new Date(now)` is fine -- it converts the PROP. `new Date()` with
    // no argument is a clock read.
    expect(code).not.toMatch(/new Date\(\s*\)/);
    expect(code).not.toMatch(/eslint-disable.*purity/);
  });
});

/// #975. The exclusion the app computes and never said.
///
/// A user counting files sees 2,904 on disk and 1,502 in the app, and
/// until now nothing on screen bridged the two -- so the obvious
/// conclusion is that the scan is broken. `subagent_files_skipped` has
/// crossed the IPC boundary since #914 with the comment "counted so the
/// exclusion is visible and testable rather than invisible", and it was
/// visible to a test and invisible to the person whose files they are.
describe("the subagent exclusion is stated, not merely counted", () => {
  /// **The sabotage test.** Delete the `subagent_files_skipped` clause
  /// from `Banners` and this fails naming the number. Nothing else
  /// catches it: the field is in the fixture and in the type, and every
  /// other test passes with it rendered nowhere -- which is exactly the
  /// state the issue reports.
  it("names how many files were skipped, and that they are not sessions", () => {
    state.imported = imported({ sessions: 1502, subagent_files_skipped: 1402 });
    renderView();
    expect(screen.getByText(/1,402 subagent transcripts skipped/i)).toBeTruthy();
    // The WORDING matters as much as the number. #914's correction
    // records that the naive glob "would list ~2x the real sessions, and
    // every phantom row would offer a `--resume` handle for something
    // that was never a session", so this must not read as "sessions
    // Headstate declined to show".
    expect(screen.getByText(/are not sessions and cannot be resumed/i)).toBeTruthy();
  });

  /// The happy-path pair: no noise when there is nothing to exclude.
  ///
  /// Zero subagent files is the common case on a new machine, and a
  /// clause reading "0 skipped" is noise about an exclusion that did not
  /// happen -- #976's rule, suppress rather than qualify when the figure
  /// would mislead.
  it("says nothing at all when no file was skipped", () => {
    state.imported = imported({ sessions: 12, subagent_files_skipped: 0 });
    renderView();
    expect(screen.queryByText(/subagent/i)).toBeNull();
    // The line it sits on is still there, so this is a suppressed clause
    // and not a suppressed line.
    expect(screen.getByText(/12 read in/i)).toBeTruthy();
  });

  /// It is GREY and factual, never the amber partial-read banner. The
  /// field's own comment says "Not failures -- correctly excluded work",
  /// and `is_partial()` deliberately does not consult it. Folding it in
  /// would tell a user with a complete list that it is "incomplete by an
  /// unknown amount".
  it("does not make the list look incomplete", () => {
    state.imported = imported({ sessions: 1502, subagent_files_skipped: 1402 });
    renderView();
    expect(screen.queryByText(/incomplete by an unknown amount/i)).toBeNull();
  });

  /// Singular reads as singular. A count of one rendering as "1 subagent
  /// transcripts" is the kind of seam that makes a reader doubt the
  /// number beside it.
  it("agrees with itself about one file", () => {
    state.imported = imported({ sessions: 3, subagent_files_skipped: 1 });
    renderView();
    expect(screen.getByText(/1 subagent transcript skipped/i)).toBeTruthy();
    expect(screen.queryByText(/1 subagent transcripts/i)).toBeNull();
  });
});

/// #959. How much work happened inside a session.
///
/// #910 cut this on "usage is not in the data I verified"; it is, on
/// 1,478 of 1,502 real transcripts. `claude/usage.rs` carries the
/// re-measurement and this is the rendering.
describe("how much work a session did", () => {
  /// **The sabotage test.** Fold the four counters into one total in
  /// `SessionUsage` and this fails on the second assertion. The spread is
  /// the whole point: cache reads run two to three orders of magnitude
  /// above fresh input, so one summed figure is a cache-read count
  /// wearing the word "tokens".
  it("reports the four counters separately, never one total", () => {
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.getByText("994")).toBeTruthy();
    expect(screen.getByText("582,035")).toBeTruthy();
    expect(screen.getByText("1,988")).toBeTruthy();
    expect(screen.getByText("405,086,242")).toBeTruthy();
    expect(screen.getByText("4,971,059")).toBeTruthy();
  });

  /// **The sabotage test for absent-is-not-zero.** Remove the
  /// `messages === 0` arm and this fails: the four `Field`s render with
  /// zeros, which is a measurement that was never taken wearing the shape
  /// of one that was. 24 of 1,502 real transcripts are exactly this.
  it("says a transcript records no usage rather than showing four zeros", () => {
    state.usage = usage({
      messages: 0,
      input_tokens: 0,
      output_tokens: 0,
      cache_read_tokens: 0,
      cache_creation_tokens: 0,
      models: [],
    });
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.getByText(/records no token usage/i)).toBeTruthy();
    expect(screen.queryByText("Output tokens")).toBeNull();
  });

  /// A failed read and a session that used nothing have opposite
  /// remedies, and only the second licenses a number. This is the #846
  /// arm, ordered BEFORE the empty one for the reason that issue records.
  it("names a failed read rather than reporting zeros", () => {
    state.usageFailed = true;
    state.usage = undefined;
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.getByText(/could not read its transcript/i)).toBeTruthy();
    expect(screen.queryByText(/records no token usage/i)).toBeNull();
    expect(screen.queryByText("0")).toBeNull();
  });

  /// A short read, STATED. Without this the reader cannot tell a complete
  /// sum from one that stopped part-way, which is #846 with a number on
  /// it.
  ///
  /// Since #1086 the 8 MB budget can no longer cause this on a SELECTED
  /// session -- that path reads whole -- so what it now guards is the
  /// backend's remaining `bytes_read < file_bytes` case: a transcript
  /// that shrank between being sized and being read. The rendering is
  /// unchanged, and deliberately: the rule is that a partial sum says so,
  /// whatever made it partial.
  it("says the figures are floors when the read came back short", () => {
    state.usage = usage({ truncated: true, bytes_read: 8_388_608, file_bytes: 76_740_099 });
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.getByText(/floors, not totals/i)).toBeTruthy();
    expect(screen.getByText(/73\.2 MB/)).toBeTruthy();
    expect(screen.getByText(/8\.0 MB/)).toBeTruthy();
  });

  /// The happy-path pair for the test above, and since #1086 the case
  /// for EVERY selected session rather than only the 97%+ of the corpus
  /// under the old cap. A complete sum must not wear a label it has not
  /// earned.
  it("claims no truncation on a transcript read whole", () => {
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.queryByText(/floors, not totals/i)).toBeNull();
  });

  /// `model` is per-MESSAGE and the corpus is mixed -- 12,512 opus-5
  /// against 912 opus-4-7 across 13,425 sampled messages -- so a session
  /// that used two gets both, with counts, rather than one picked.
  it("names every model a session used, with how many messages each wrote", () => {
    state.usage = usage({
      models: [
        { model: "claude-opus-5", messages: 900 },
        { model: "claude-opus-4-7", messages: 94 },
      ],
    });
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.getByText(/claude-opus-5 \(900\), claude-opus-4-7 \(94\)/)).toBeTruthy();
  });

  /// No DERIVED dollars anywhere, and this is asserted rather than left
  /// to review. A cost this app computed needs per-model rates, those
  /// rates change, and a quietly stale number with a currency symbol on
  /// it is the confident-wrong-answer failure #941 exists for.
  ///
  /// # What #1210 changed here, and what it did not
  ///
  /// The assertion used to be "no `$` on the page", which stopped being
  /// the right spelling of the rule the moment a session carried a
  /// `cost-state` record: Claude Code's own figure is transcribed and
  /// shown, attributed. What has NOT changed is that this app derives
  /// nothing — so the test now pins the case it always meant. The
  /// standing fixture carries `recorded_cost: null`, which is the
  /// majority of the corpus, and on that row a `$` anywhere would have to
  /// have been invented from the token counts beside it.
  ///
  /// **Sabotage:** make `SessionCost` fall back to a computed figure when
  /// `recorded_cost` is null — any rate at all, even a right one — and
  /// this fails on the first assertion.
  it("invents no dollar figure for a session Claude Code recorded no cost for", () => {
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.queryByText(/\$/)).toBeNull();
    expect(screen.getByText(/did not record a cost for this session/i)).toBeTruthy();
  });

  /// A row with no transcript is a real row. It must say there is nothing
  /// to read from, not fail and not report zeros.
  it("says there is nothing to read when no transcript was recorded", () => {
    state.list = listOf([
      session({ transcript_path: null, transcript_state: { state: "not-recorded" } }),
    ]);
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.getByText(/no transcript was recorded for this session/i)).toBeTruthy();
  });

  /// It reads the TRANSCRIPT's state, never the cwd's (#919): 1,213 of
  /// 1,461 real rows have a dead cwd and a live transcript, so a reading
  /// gated on the cwd would be absent on almost every row.
  it("still reads usage for a session whose directory is gone", () => {
    state.list = listOf([
      session({ cwd_state: { state: "gone" }, transcript_state: { state: "exists" } }),
    ]);
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.getByText("994")).toBeTruthy();
  });
});

/// What Claude Code recorded a session cost (#1210).
///
/// `usage.rs:33-41` rules out a figure this app DERIVES and that stands.
/// These tests pin the other half: a figure the vendor computed and wrote
/// to the transcript is transcribed, attributed, and — where there is no
/// record — replaced by a sentence about the recording rather than a zero.
describe("what Claude Code recorded a session cost", () => {
  /// **The sabotage test for the absent case, and the one that matters
  /// most.** Replace the `cost === null` arm with a `$0.00` field and
  /// this fails on both assertions. `$0.00` states that the vendor
  /// measured nothing spent — a confident wrong answer made MORE
  /// credible by the attribution standing next to it, which is #846 with
  /// a currency symbol on it.
  ///
  /// The standing fixture is this case, because it is the majority of
  /// the corpus.
  it("says Claude Code did not record a cost rather than showing $0.00", () => {
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.getByText(/Claude Code did not record a cost for this session/i)).toBeTruthy();
    expect(screen.queryByText("$0.00")).toBeNull();
  });

  /// The figure, ATTRIBUTED — in the visible label, not in a comment.
  ///
  /// **Sabotage:** relabel the field "Cost" and this fails. A reader who
  /// cannot tell a transcribed figure from a derived one has been handed
  /// the more dangerous of the two by default, so the provenance is part
  /// of the rendering rather than part of the documentation.
  it("attributes the recorded figure to Claude Code in the label", () => {
    state.usage = usage({ recorded_cost: costState() });
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.getByText(/as recorded by claude code/i)).toBeTruthy();
    expect(screen.getByText("$1.32")).toBeTruthy();
  });

  /// The per-model split, costliest first, each model's own `costUSD`.
  ///
  /// **Sabotage:** drop the `models` field from `SessionCost` and this
  /// fails. Nothing here is apportioned — the figures are the vendor's,
  /// per model, as written.
  it("shows the per-model split the record carries", () => {
    state.usage = usage({ recorded_cost: costState() });
    renderView();
    open("HeadState GitHub issues filing");
    expect(
      screen.getByText(/claude-opus-5\[1m\] \$1\.32, claude-haiku-4-5-20251001 \$0\.0012/),
    ).toBeTruthy();
  });

  /// `totalAPIDuration` minus `totalAPIDurationWithoutRetries`: time lost
  /// to retries, invisible everywhere else in the app.
  ///
  /// **Sabotage:** render `total_api_ms` instead of the difference and
  /// this fails — 84,690 ms is 84.7 s, not 68 ms.
  it("shows time lost to retries as the difference between the two durations", () => {
    state.usage = usage({ recorded_cost: costState() });
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.getByText("68 ms")).toBeTruthy();
  });

  /// **The arm no real transcript exercises.** `hasUnknownModelCost` is
  /// `false` on every record measured, so this fixture is the only place
  /// the floor path is ever run — which is exactly why it is handled
  /// rather than assumed away, on the argument `ToolVersion::CannotTell`
  /// already makes for a state nobody has hit.
  ///
  /// **Sabotage:** ignore the flag and label the figure a total, or drop
  /// the warning paragraph, and this fails. When it is set the recorded
  /// figure omits an unknown model's spend, so "total" understates it by
  /// an unknown amount.
  it("calls the figure a floor when Claude Code met a model it had no cost for", () => {
    state.usage = usage({ recorded_cost: costState({ has_unknown_model_cost: true }) });
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.getByText(/floor, not a total/i)).toBeTruthy();
    expect(screen.getByText(/at least, as recorded by claude code/i)).toBeTruthy();
    // The figure itself is unchanged — it is the LABEL that moves.
    expect(screen.getByText("$1.32")).toBeTruthy();
  });

  /// The happy-path pair for the test above: a record with the flag clear
  /// must not wear a floor label it has not earned.
  it("does not call an ordinary recorded figure a floor", () => {
    state.usage = usage({ recorded_cost: costState() });
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.queryByText(/floor, not a total/i)).toBeNull();
    expect(screen.queryByText(/at least, as recorded/i)).toBeNull();
  });

  /// A failed read and a session Claude Code recorded no cost for are
  /// different facts, and neither of them is "it cost nothing". The #846
  /// arm, ordered BEFORE the absent one for the reason that issue records.
  ///
  /// **Sabotage:** move the error arm below the `cost === null` arm and
  /// this fails on the second assertion — `data` is undefined on a
  /// rejection exactly as it is before the first read, so an error arm
  /// placed after never renders.
  it("names a failed read rather than claiming no cost was recorded", () => {
    state.usageFailed = true;
    state.usage = undefined;
    renderView();
    open("HeadState GitHub issues filing");
    expect(
      screen.getByText(/what Claude Code recorded it cost is unknown/i),
    ).toBeTruthy();
    expect(screen.queryByText(/did not record a cost for this session/i)).toBeNull();
    expect(screen.queryByText(/\$/)).toBeNull();
  });

  /// A sub-cent recorded figure keeps its precision. `$0.00` is the one
  /// string this whole section exists to never print, and rounding a real
  /// `0.001186` down to it would print it by accident — a wrong zero
  /// arrived at from a right number.
  ///
  /// **Sabotage:** drop the sub-cent branch in `formatUsd` and this
  /// fails, showing `$0.00` for a session that cost something.
  it("does not round a sub-cent recorded figure down to $0.00", () => {
    state.usage = usage({
      recorded_cost: costState({ total_cost_usd: 0.001186, models: [] }),
    });
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.queryByText("$0.00")).toBeNull();
    expect(screen.getByText("$0.0012")).toBeTruthy();
  });

  /// This app does not own the invariant between the two duration fields.
  /// A pair that disagrees the wrong way round yields no row at all —
  /// clamping to zero would state "no time was lost to retries" about a
  /// record that makes no sense.
  ///
  /// **Sabotage:** change `retryMillis` to clamp at zero and this fails,
  /// rendering "0 ms" for an incoherent record.
  it("shows no retry time rather than zero when the durations disagree", () => {
    state.usage = usage({
      recorded_cost: costState({
        total_api_ms: 100,
        total_api_without_retries_ms: 200,
      }),
    });
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.queryByText(/time lost to retries/i)).toBeNull();
    // The cost still renders: one incoherent pair of timings must not
    // suppress the figure the section exists for.
    expect(screen.getByText("$1.32")).toBeTruthy();
  });

  /// A record with a total and no breakdown is still a record of a cost.
  /// A panel that demanded the split would suppress a figure the vendor
  /// did write down.
  it("shows the total when the record carries no per-model split", () => {
    state.usage = usage({ recorded_cost: costState({ models: [] }) });
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.getByText("$1.32")).toBeTruthy();
    expect(screen.queryByText(/by model/i)).toBeNull();
  });

  /// A row with no transcript is a real row: it must say there is nothing
  /// to read from, not report a zero cost.
  it("says there is nothing to read when no transcript was recorded", () => {
    state.list = listOf([
      session({ transcript_path: null, transcript_state: { state: "not-recorded" } }),
    ]);
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.queryByText(/\$/)).toBeNull();
  });
});

/// #982. Reading a transcript in the app.
describe("reading a transcript rather than revealing it", () => {
  /// Behind a disclosure, and the read does NOT start on selection: a
  /// 256 KB read per row, over the pairing transport on the phone, for a
  /// pane the user may not want.
  ///
  /// **The sabotage test for the gate.** Pass `true` instead of `open` to
  /// `useClaudeTranscriptTail` and this fails on the first assertion.
  it("does not read the transcript until asked", () => {
    renderView();
    open("HeadState GitHub issues filing");
    expect(state.previewEnabledFor).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: /read the transcript/i }));
    expect(state.previewEnabledFor).toContain(
      "/Users/acme/.claude/projects/slug/e5dff3bd.jsonl",
    );
  });

  /// The four block kinds, four renderings. A renderer that assumed text
  /// would show nothing for the 12,903 of 13,425 assistant messages that
  /// stop on `tool_use`.
  it("renders text, thinking, a tool call and a tool result each as itself", () => {
    state.preview = preview({
      messages: [
        {
          role: "assistant",
          timestamp: "2026-09-13T11:00:05Z",
          model: "claude-opus-5",
          blocks: [
            { kind: "text", text: "Looking now.", truncated: false },
            { kind: "thinking", text: "weighing it up", truncated: false },
            {
              kind: "tool_use",
              name: "Bash",
              id: "toolu_1",
              args: {
                tool: "bash",
                command: "cargo test",
                description: null,
                truncated: false,
              },
            },
            {
              kind: "tool_result",
              text: "3 tests passed",
              truncated: false,
              tool_use_id: "toolu_1",
              is_error: false,
              change: null,
            },
          ],
        },
      ],
    });
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /read the transcript/i }));
    expect(screen.getByText("Looking now.")).toBeTruthy();
    expect(screen.getByText("weighing it up")).toBeTruthy();
    // The name AND the parsed arguments (#1209). "Ran Bash" and "ran
    // `cargo test`" are not the same sentence, and the second is the one
    // that stops a user leaving for a terminal.
    expect(screen.getByText("Bash")).toBeTruthy();
    expect(screen.getByText(/cargo test/)).toBeTruthy();
    expect(screen.getByText(/3 tests passed/)).toBeTruthy();
  });

  // -----------------------------------------------------------------
  // Tool pairing, arguments and diffs (#1209).
  // -----------------------------------------------------------------

  /// A helper for a transcript whose one call never got a result.
  ///
  /// The three orphan cases below differ ONLY in the session's liveness,
  /// so the transcript is held constant and the liveness varied. That is
  /// the claim under test: one recorded fact, three meanings, and the
  /// meaning comes from `liveness.rs` rather than from the transcript.
  const unansweredCall = () =>
    preview({
      messages: [
        {
          role: "assistant",
          timestamp: "2026-09-13T11:00:05Z",
          model: "claude-opus-5",
          blocks: [
            {
              kind: "tool_use",
              name: "Bash",
              id: "toolu_open",
              args: {
                tool: "bash",
                command: "cargo test --all",
                description: null,
                truncated: false,
              },
            },
          ],
        },
      ],
      pairings: { toolu_open: "unanswered" },
      unanswered_calls: 1,
    });

  /// Orphan case 2 of 3: a call with no result on a RUNNING session.
  ///
  /// Nothing is wrong. The result has not been written yet.
  it("says an unanswered call on a running session is still executing", () => {
    state.list = listOf([
      session({ liveness: { state: "running", pid: 14779, status: "busy" } }),
    ]);
    state.preview = unansweredCall();
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /read the transcript/i }));
    expect(screen.getByText(/still running, so the call may still be executing/i)).toBeTruthy();
    // And emphatically NOT the crash wording, which would report a
    // healthy session as a dead one.
    expect(screen.queryByText(/did not come back/i)).toBeNull();
  });

  /// Orphan case 3 of 3: a call with no result on a DEAD session.
  ///
  /// This is the signature of a crash mid tool call, and it is real
  /// information -- it says WHERE the session died, which is exactly what
  /// a user about to resume it wants. Rendering it the same as case 2
  /// throws away the only one of the two worth surfacing.
  it("says an unanswered call on a dead session never came back", () => {
    state.list = listOf([
      session({ liveness: { state: "dead", why: "pid 14779 is no longer running" } }),
    ]);
    state.preview = unansweredCall();
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /read the transcript/i }));
    expect(screen.getByText(/no result was recorded.*did not come back/i)).toBeTruthy();
    expect(screen.queryByText(/may still be executing/i)).toBeNull();
  });

  /// And the tri-state's third arm, which is not a shade of dead.
  ///
  /// A check that could not be completed must not report a crash: the
  /// session may be alive and mid-work. `Liveness`'s own rule, applied
  /// where it is rendered.
  it("does not call an unanswered call a crash when liveness is unknown", () => {
    state.list = listOf([
      session({
        liveness: { state: "unknown", why: "could not read the live session registry" },
      }),
    ]);
    state.preview = unansweredCall();
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /read the transcript/i }));
    expect(
      screen.getByText(/whether the call is still running could not be determined/i),
    ).toBeTruthy();
    expect(screen.queryByText(/did not come back/i)).toBeNull();
    expect(screen.queryByText(/may still be executing/i)).toBeNull();
  });

  /// Orphan case 1 of 3: a result whose call is above the window.
  ///
  /// The 256 KB tail cut between the call and its result. Nothing is
  /// wrong with the session at all, so this must read as a fact about the
  /// WINDOW and not about the session -- and in particular must not wear
  /// either of the two call-side wordings.
  it("says an orphan result's call is above the window, not that it is missing", () => {
    state.preview = preview({
      messages: [
        {
          role: "user",
          timestamp: "2026-09-13T11:00:05Z",
          model: null,
          blocks: [
            {
              kind: "tool_result",
              text: "output with nothing that asked for it",
              truncated: false,
              tool_use_id: "toolu_gone",
              is_error: null,
              change: null,
            },
          ],
        },
      ],
      pairings: { toolu_gone: "call_above_window" },
      unanswered_calls: 0,
      results_above_window: 1,
    });
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /read the transcript/i }));
    expect(screen.getByText(/the call this answers is above the window/i)).toBeTruthy();
    // The distinction that makes the feature worth having: this is not
    // the crash case and not the in-flight case.
    expect(screen.queryByText(/did not come back/i)).toBeNull();
    expect(screen.queryByText(/may still be executing/i)).toBeNull();
  });

  /// All three orphan cases produce DIFFERENT sentences.
  ///
  /// Asserted as one claim as well as three, because the failure this
  /// guards against is collapse: a refactor that routed two of them
  /// through one message would leave each individual test above passing
  /// on a substring while the distinction was gone.
  it("gives each of the three orphan cases its own distinct message", () => {
    const said = (liveness: Liveness, view: ClaudePreview) => {
      state.list = listOf([session({ liveness })]);
      state.preview = view;
      const { unmount } = renderView();
      open("HeadState GitHub issues filing");
      fireEvent.click(screen.getByRole("button", { name: /read the transcript/i }));
      // Read through `screen`, not the render's own `container`:
      // `renderView` mounts into the shared document body and `open`
      // queries it globally, so a per-render container can lag the pane
      // these assertions are actually about.
      const text = document.body.textContent ?? "";
      unmount();
      cleanup();
      return text;
    };

    const aboveWindow = said(
      { state: "dead", why: "pid 1 is no longer running" },
      preview({
        messages: [
          {
            role: "user",
            timestamp: "2026-09-13T11:00:05Z",
            model: null,
            blocks: [
              {
                kind: "tool_result",
                text: "orphan output",
                truncated: false,
                tool_use_id: "toolu_gone",
                is_error: null,
                change: null,
              },
            ],
          },
        ],
        pairings: { toolu_gone: "call_above_window" },
        results_above_window: 1,
      }),
    );
    const stillRunning = said(
      { state: "running", pid: 14779, status: "busy" },
      unansweredCall(),
    );
    const neverCameBack = said(
      { state: "dead", why: "pid 14779 is no longer running" },
      unansweredCall(),
    );

    // Three panes, three different sentences. Pairwise, because
    // "at least two differ" would pass with two of the three collapsed.
    expect(aboveWindow).not.toEqual(stillRunning);
    expect(stillRunning).not.toEqual(neverCameBack);
    expect(aboveWindow).not.toEqual(neverCameBack);
    expect(aboveWindow).toMatch(/above the window/i);
    expect(stillRunning).toMatch(/still be executing/i);
    expect(neverCameBack).toMatch(/did not come back/i);
  });

  /// A recorded diff and a reconstructed one are LABELLED differently.
  ///
  /// The substance of the diff half of #1209. A diff built from content
  /// the transcript wrote down shows the surrounding lines as the file
  /// actually was; one reconstructed from `old_string`/`new_string` has
  /// no context at all. Rendering both as "a diff" tells the reader the
  /// second has context it does not have.
  it("labels a recorded diff differently from a reconstructed one", () => {
    state.preview = preview({
      messages: [
        {
          role: "user",
          timestamp: "2026-09-13T11:00:05Z",
          model: null,
          blocks: [
            {
              kind: "tool_result",
              text: "ok",
              truncated: false,
              tool_use_id: "t1",
              is_error: false,
              change: {
                file_path: "/a.rs",
                source: "recorded",
                hunks: [
                  {
                    old_start: 10,
                    new_start: 10,
                    lines: [
                      { op: "context", text: "fn main() {" },
                      { op: "removed", text: "  let x = 1;" },
                      { op: "added", text: "  let x = 2;" },
                    ],
                    lines_omitted: 0,
                  },
                ],
                hunks_omitted: 0,
                created: false,
              },
            },
            {
              kind: "tool_result",
              text: "ok",
              truncated: false,
              tool_use_id: "t2",
              is_error: false,
              change: {
                file_path: "/b.rs",
                source: "reconstructed",
                hunks: [
                  {
                    old_start: null,
                    new_start: null,
                    lines: [
                      { op: "removed", text: "let y = 1;" },
                      { op: "added", text: "let y = 2;" },
                    ],
                    lines_omitted: 0,
                  },
                ],
                hunks_omitted: 0,
                created: null,
              },
            },
          ],
        },
      ],
      pairings: { t1: "call_above_window", t2: "call_above_window" },
      results_above_window: 2,
    });
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /read the transcript/i }));

    // The recorded one says its context is real.
    expect(
      screen.getByText(/recorded at the time, so the surrounding lines are the file as it was/i),
    ).toBeTruthy();
    // The reconstructed one says it has none -- and says WHY, so the
    // reader does not read the absence as "nothing else changed".
    expect(
      screen.getByText(/reconstructed from the replaced text alone.*no surrounding lines/i),
    ).toBeTruthy();
    // And the forbidden third construction leaves no trace: nothing
    // claims to have read the file as it is now.
    expect(screen.queryByText(/current contents/i)).toBeNull();
  });

  /// A clipped diff says so, PER HUNK.
  ///
  /// Two hunks, one clipped and one not. A single pane-level notice would
  /// pass a test that only looked for the words; it would not tell the
  /// reader which of the two regions is short, and the reader would read
  /// the intact hunk as complete either way.
  it("says which hunk was clipped rather than warning once for the pane", () => {
    state.preview = preview({
      messages: [
        {
          role: "user",
          timestamp: "2026-09-13T11:00:05Z",
          model: null,
          blocks: [
            {
              kind: "tool_result",
              text: "ok",
              truncated: false,
              tool_use_id: "t1",
              is_error: false,
              change: {
                file_path: "/big.rs",
                source: "recorded",
                hunks: [
                  {
                    old_start: 1,
                    new_start: 1,
                    lines: [{ op: "added", text: "one of many" }],
                    lines_omitted: 40,
                  },
                  {
                    old_start: 900,
                    new_start: 900,
                    lines: [{ op: "added", text: "all of it" }],
                    lines_omitted: 0,
                  },
                ],
                hunks_omitted: 3,
                created: false,
              },
            },
          ],
        },
      ],
      pairings: { t1: "call_above_window" },
      results_above_window: 1,
    });
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /read the transcript/i }));

    // The clipped hunk says how much is missing.
    expect(screen.getAllByText(/40 more lines in this hunk are not shown/i)).toHaveLength(1);
    // And EXACTLY ONE hunk carries a clip notice at all. Counting every
    // notice, not only the one with the right number, is what makes this
    // a per-hunk claim: a notice hoisted to the pane and repeated under
    // each hunk still renders the right sentence under the clipped one,
    // and would pass a test that only looked for that sentence. It
    // renders "0 more lines … are not shown" under the intact one, which
    // is a false statement about a hunk that is complete.
    expect(screen.getAllByText(/more lines? in this hunk are not shown/i)).toHaveLength(1);
    // And the whole-file omission is its own separate statement, because
    // "this hunk is short" and "three regions are missing entirely" are
    // different facts.
    expect(
      screen.getByText(/3 more changed regions in this file are not shown/i),
    ).toBeTruthy();
  });

  /// A tool this build does not know reports its KEYS.
  ///
  /// `Block::Other`'s guarantee, one level down: not dropped (a call with
  /// an invisible hole in it) and not dumped (the 40 KB blob the original
  /// reasoning was right to refuse).
  it("names an unknown tool's argument keys without showing their values", () => {
    state.preview = preview({
      messages: [
        {
          role: "assistant",
          timestamp: "2026-09-13T11:00:05Z",
          model: "claude-opus-5",
          blocks: [
            {
              kind: "tool_use",
              name: "mcp__enclave__enclave_sql",
              id: "tm",
              args: { tool: "other", keys: ["enclave", "query"] },
            },
          ],
        },
      ],
      pairings: { tm: "paired" },
      unanswered_calls: 0,
    });
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /read the transcript/i }));
    expect(screen.getByText(/does not know how to show/i)).toBeTruthy();
    expect(screen.getByText(/enclave, query/)).toBeTruthy();
  });

  /// **The sabotage test for the truncation label.** Delete the
  /// `data.truncated` branch and this fails. A pane that silently showed
  /// a tail is #846 in its purest form: the reader cannot tell a short
  /// conversation from a truncated one, and #910's own design asked for
  /// "a 'showing the last N lines of a large file' label".
  it("says it is showing a tail, and of how large a file", () => {
    state.preview = preview({
      truncated: true,
      bytes_read: 262_144,
      file_bytes: 76_740_099,
    });
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /read the transcript/i }));
    expect(screen.getByText(/the last 2 messages/i)).toBeTruthy();
    expect(screen.getByText(/final 256 KB of a 73\.2 MB transcript/i)).toBeTruthy();
    expect(screen.getByText(/earlier exchanges are not shown/i)).toBeTruthy();
  });

  /// The happy-path pair: a transcript read whole says so, rather than
  /// wearing a tail label it has not earned. 97.4% of the corpus is under
  /// 1 MB, so this is the common case.
  it("says it is showing everything when it read the whole file", () => {
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /read the transcript/i }));
    expect(screen.getByText(/all 2 messages in this transcript/i)).toBeTruthy();
    expect(screen.queryByText(/earlier exchanges are not shown/i)).toBeNull();
  });

  /// 44.2% of records are machinery. A pane showing two messages out of a
  /// 300-record window has to say where the rest went, or the reader
  /// concludes the reader is broken -- the same argument
  /// `subagent_files_skipped` carries one banner up.
  it("says how many records in the window were bookkeeping", () => {
    state.preview = preview({ non_conversation_records: 298 });
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /read the transcript/i }));
    expect(screen.getByText(/298 bookkeeping records in that window/i)).toBeTruthy();
  });

  /// A future block kind is NAMED, never dropped. Claude Code owns this
  /// format, and a pane that silently omitted an unknown kind would show
  /// an exchange with an invisible hole in it.
  it("names a block kind it does not know rather than omitting it", () => {
    state.preview = preview({
      messages: [
        {
          role: "assistant",
          timestamp: null,
          model: null,
          blocks: [{ kind: "other", block_type: "image" }],
        },
      ],
    });
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /read the transcript/i }));
    expect(screen.getByText("image")).toBeTruthy();
    expect(screen.getByText(/does not know how to show/i)).toBeTruthy();
  });

  /// The #846 arm, and it is ordered before the empty one: `data` is
  /// undefined on a rejection exactly as it is before the first read.
  it("names a failed read rather than showing an empty conversation", () => {
    state.previewFailed = true;
    state.preview = undefined;
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /read the transcript/i }));
    expect(screen.getByText(/could not read its transcript/i)).toBeTruthy();
    expect(screen.getByText(/not the same as the session having said nothing/i)).toBeTruthy();
  });

  /// Read, and there genuinely was no conversation. Distinguished from
  /// the failure above and from an empty file, because "300 machinery
  /// records" and "an empty file" are different facts.
  it("distinguishes a window with no conversation from a failed read", () => {
    state.preview = preview({
      messages: [],
      non_conversation_records: 300,
      unparseable_records: 2,
    });
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /read the transcript/i }));
    expect(screen.getByText(/no conversation in the last/i)).toBeTruthy();
    expect(screen.getByText(/300 bookkeeping records and 2 that could not be read/i))
      .toBeTruthy();
    expect(screen.queryByText(/could not read its transcript/i)).toBeNull();
  });

  /// The tri-state's three wordings, not one shared shrug.
  /// `revealRefusal`'s doc argues at length that collapsing `gone` and
  /// `unknown` destroys the point of the third state, and this pane uses
  /// the same function so the two controls cannot drift.
  it("refuses a gone transcript differently from one it could not check", () => {
    state.list = listOf([session({ transcript_state: { state: "gone" } })]);
    renderView();
    open("HeadState GitHub issues filing");
    // Scoped to this section's own sentence, because the disabled Reveal
    // transcript button is stating the same refusal clause a few lines
    // up. Two identical sentences side by side read as two failures,
    // which is why this one carries a prefix naming what IT cannot do.
    expect(
      screen.getByText(/there is nothing to read here: the path no longer exists/i),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: /read the transcript/i })).toBeNull();
  });

  it("names the reason a transcript check failed", () => {
    state.list = listOf([
      session({ transcript_state: { state: "unknown", why: "Permission denied" } }),
    ]);
    renderView();
    open("HeadState GitHub issues filing");
    // `unknown` is worded DIFFERENTLY from `gone` above, which is the
    // whole point of the tri-state: the path may well be there and the
    // remedy is to fix whatever blocked the check.
    expect(
      screen.getByText(
        /there is nothing to read here: could not check whether it exists \(Permission denied\)/i,
      ),
    ).toBeTruthy();
    expect(screen.queryByText(/no longer exists/i)).toBeNull();
  });
});

/// #977: the session row announced "pressed" -- a toggle the user had just
/// operated, inviting a second press to un-press it. The list is
/// single-select (`selectClaudeSession` into one slot), so that second
/// click is a no-op: the announcement described a control that does not
/// exist, and gave a screen-reader user nothing to locate themselves by.
///
/// `aria-current="true"` rather than `"page"`, matching `RepoSidebar`
/// rather than `ClaudeCodeSidebar`: the row picks the detail pane's
/// subject within this page rather than navigating to a different one.
/// On a phone selecting a row IS navigation to a second screen, which
/// makes "pressed" worse rather than better -- a toggle for an action
/// that replaced the whole screen.
describe("which session the list says you are on", () => {
  /// The name also appears in the detail pane once a row is selected, so
  /// every lookup here goes through the row BUTTON rather than the text.
  const rowFor = (name: string) =>
    screen
      .getAllByRole("button")
      .find((b) => b.textContent?.includes(name)) as HTMLElement;

  it("marks the selected row as current, never as pressed", () => {
    state.list = listOf([session({ session_id: "s-1", name: "Kestrel" })]);
    renderView();

    expect(rowFor("Kestrel").getAttribute("aria-pressed")).toBeNull();
    fireEvent.click(rowFor("Kestrel"));
    expect(rowFor("Kestrel").getAttribute("aria-current")).toBe("true");
    expect(rowFor("Kestrel").getAttribute("aria-pressed")).toBeNull();
  });

  /// `undefined`, never `"false"` -- the attribute's absence is how "not
  /// current" is spelled, and `aria-current="false"` is announced by some
  /// readers. Exactly one row at a time, because the list is single-select.
  it("leaves the attribute off every row that is not selected", () => {
    state.list = listOf([
      session({ session_id: "s-1", name: "Kestrel" }),
      session({ session_id: "s-2", name: "Merlin" }),
    ]);
    renderView();

    fireEvent.click(rowFor("Kestrel"));
    const merlin = rowFor("Merlin");
    expect(merlin.hasAttribute("aria-current")).toBe(false);
    expect(merlin.getAttribute("aria-pressed")).toBeNull();
  });
});


/// The two-tier read, and what the pane says when the second half is not
/// there (#985).
///
/// The list carries what it draws, searches, filters and counts on; the
/// resume command, the transcript and the version are fetched for the one
/// selected session. These are the properties that make that split
/// invisible to a user and honest when it fails.
describe("the detail is fetched for one session, not for the list", () => {
  /// The whole point of the change, as a property rather than a byte
  /// count: 1,474 rows on screen must not be 1,474 detail reads.
  it("asks for no detail until a row is picked, then for exactly that one", () => {
    state.list = listOf([
      session({ session_id: "s-1", name: "Kestrel" }),
      session({ session_id: "s-2", name: "Merlin" }),
      session({ session_id: "s-3", name: "Hobby" }),
    ]);
    renderView();

    // Three rows drawn, nothing selected: the detail is a read the list
    // does not make.
    expect(state.detailAskedFor).toEqual([]);

    open("Merlin");
    expect(state.detailAskedFor).toEqual(["s-2"]);
  });

  /// The resume command comes from the detail, and it must be the
  /// SELECTED session's. A pane that served a cached detail for the
  /// previously-open row would offer a command that resurrects the wrong
  /// session -- which is the #918 failure with a new cause.
  it("shows the picked session's own resume command", () => {
    state.list = listOf([
      session({ session_id: "s-1", name: "Kestrel" }),
      session({ session_id: "s-2", name: "Merlin" }),
    ]);
    renderView();

    open("Merlin");
    expect(screen.getByText(/claude --resume s-2/)).toBeTruthy();
    expect(screen.queryByText(/claude --resume s-1/)).toBeNull();
  });

  /// A rejected detail read is a REASON with a retry, never an empty
  /// pane. #846's rule, applied to the half of the row that now arrives
  /// separately: "we could not read this" and "there is nothing here"
  /// have opposite remedies.
  it("states the reason when the detail could not be read", () => {
    state.list = listOf([session({ session_id: "s-1", name: "Kestrel" })]);
    state.detailFailed = true;
    renderView();

    open("Kestrel");
    expect(screen.getByText(/could not be read/i)).toBeTruthy();
    // And NOT a resume command built from nothing.
    expect(screen.queryByText(/claude --resume/)).toBeNull();
  });

  /// The list half still renders when the detail half fails. The title,
  /// the liveness and its reason come from the row, so a failed second
  /// read must not blank out the answer the user came for -- which on the
  /// phone is "did the thing I left running die?".
  it("still answers the liveness question when the detail read fails", () => {
    state.list = listOf([
      session({
        session_id: "s-1",
        name: "Kestrel",
        liveness: { state: "dead", why: "pid 14779 is no longer running" },
      }),
    ]);
    state.detailFailed = true;
    renderView();

    open("Kestrel");
    expect(screen.getByText(/pid 14779 is no longer running/)).toBeTruthy();
  });

  /// A resolved `null` is a DIFFERENT sentence from a rejection: the
  /// store does not have this id, which is what a session deleted between
  /// two polls produces. Collapsing the two would tell a user whose disk
  /// is unreadable that their session no longer exists.
  it("says the session is gone when the detail resolves to nothing", () => {
    state.list = listOf([session({ session_id: "s-1", name: "Kestrel" })]);
    state.detailMissing = true;
    renderView();

    open("Kestrel");
    expect(screen.getByText(/no longer in the store/i)).toBeTruthy();
    expect(screen.queryByText(/could not be read/i)).toBeNull();
  });
});

/// The waiting indicator, and the tense it is allowed to use (#1067).
///
/// The issue's own constraint is that a STALE indicator is worse than no
/// indicator, because it sends the user to a session that does not need
/// them. `ClaudeWaiting` makes the present-tense claim unconstructible
/// without a live process on the Rust side; these are the rendering half
/// of the same rule, and the dead-session case below is the one that
/// actually fails if the component ever treats the two arms alike.
describe("the waiting indicator", () => {
  it("says a live session at an idle prompt is waiting, in the present tense", () => {
    state.list = listOf([
      session({
        name: "Kestrel",
        liveness: { state: "running", pid: 14779, status: null },
        waiting: { state: "now", kind: "idle_prompt", at: "2026-09-13T11:45:00Z" },
      }),
    ]);
    renderView();

    expect(screen.getAllByText(/waiting for you/i).length).toBeGreaterThan(0);
    // And NOT the past-tense wording, which is the other arm's.
    expect(screen.queryByText(/last seen waiting/i)).toBeNull();
  });

  /// The two #1067 names are DIFFERENT stories -- one session idling and
  /// one blocked on a decision -- so they do not share a sentence.
  it("says a permission prompt is asking permission, not merely waiting", () => {
    state.list = listOf([
      session({
        name: "Kestrel",
        liveness: { state: "running", pid: 14779, status: null },
        waiting: { state: "now", kind: "permission_prompt", at: "2026-09-13T11:45:00Z" },
      }),
    ]);
    renderView();

    expect(screen.getAllByText(/asking permission/i).length).toBeGreaterThan(0);
    expect(screen.queryByText(/waiting for you/i)).toBeNull();
  });

  /// **The sabotage test, and the load-bearing one.**
  ///
  /// A dead session with a waiting record must say it was LAST SEEN
  /// waiting and must never say it is waiting now. The present-tense
  /// absence is asserted explicitly rather than left implied: a component
  /// that rendered both arms through one sentence would still pass the
  /// "last seen" half, and the failure #1067 is about is precisely the
  /// present tense surviving the process.
  it("says a dead session was LAST SEEN waiting, never that it is waiting now", () => {
    state.list = listOf([
      session({
        name: "Kestrel",
        liveness: { state: "dead", why: "pid 14779 is no longer running" },
        waiting: {
          state: "last-seen",
          kind: "idle_prompt",
          at: "2026-09-13T09:05:00Z",
          why: "pid 14779 is no longer running",
        },
      }),
    ]);
    renderView();

    expect(screen.getAllByText(/last seen waiting at \d\d:\d\d/i).length).toBeGreaterThan(0);
    // THE assertion. Neither wording of the present tense may appear
    // anywhere on the page for a session whose process is gone.
    expect(screen.queryByText(/waiting for you/i)).toBeNull();
    expect(screen.queryByText(/asking permission/i)).toBeNull();
  });

  /// `why` is the liveness reason, carried so the past tense has visible
  /// grounds rather than looking like an arbitrary hedge.
  it("puts the reason the present tense was refused in the title", () => {
    state.list = listOf([
      session({
        name: "Kestrel",
        liveness: { state: "dead", why: "pid 14779 is no longer running" },
        waiting: {
          state: "last-seen",
          kind: "idle_prompt",
          at: "2026-09-13T09:05:00Z",
          why: "the process could not be checked: Operation not permitted",
        },
      }),
    ]);
    renderView();

    const badge = screen.getAllByText(/last seen waiting at/i)[0];
    expect(badge.getAttribute("title")).toMatch(/Operation not permitted/);
  });

  /// Every `no` reason renders nothing. `never-observed` is the whole
  /// pre-hook corpus, and an indicator saying "we were not watching this
  /// one either" on 1,500 rows is the noise that gets a feature ignored.
  it("renders no indicator at all when the session is not waiting", () => {
    for (const reason of ["never-observed", "superseded", "not-a-prompt"] as const) {
      state.list = listOf([session({ name: "Kestrel", waiting: { state: "no", reason } })]);
      const view = renderView();
      expect(screen.queryByText(/waiting/i), `reason ${reason}`).toBeNull();
      expect(screen.queryByText(/asking permission/i), `reason ${reason}`).toBeNull();
      view.unmount();
    }
  });

  /// Unknown enum values render as THEMSELVES. There is no "other"
  /// bucket anywhere in this epic: a notification kind Claude Code grows
  /// tomorrow must show up under its own name, so the reader sees what
  /// was actually sent rather than a relabelling that hides it.
  it("renders an unknown notification kind verbatim", () => {
    state.list = listOf([
      session({
        name: "Kestrel",
        liveness: { state: "running", pid: 14779, status: null },
        waiting: { state: "now", kind: "elicitation_requested", at: "2026-09-13T11:45:00Z" },
      }),
    ]);
    renderView();

    expect(screen.getAllByText(/elicitation_requested/).length).toBeGreaterThan(0);
    expect(screen.queryByText(/\bother\b/i)).toBeNull();
  });

  /// The detail pane states it too, on the DETAIL's own `waiting` --
  /// derived against the same liveness read the pane's badge shows, so
  /// the pane cannot disagree with itself.
  it("states it in the detail pane as well as on the row", () => {
    state.list = listOf([
      session({
        name: "Kestrel",
        liveness: { state: "running", pid: 14779, status: null },
        waiting: { state: "now", kind: "idle_prompt", at: "2026-09-13T11:45:00Z" },
      }),
    ]);
    renderView();

    open("Kestrel");
    // Two: the row and the heading.
    expect(screen.getAllByText(/waiting for you/i).length).toBe(2);
  });
});

/// The context-pressure marker, and its three states (#1065).
///
/// `null` and `false` both render nothing, and that is deliberate rather
/// than a collapse: neither is a finding. What the absent-is-not-zero
/// rule forbids is rendering `null` as a MEASURED answer, and "no
/// compactions" on the entire pre-hook corpus would be exactly that.
describe("the context-pressure marker", () => {
  it("marks a session that compacted automatically several times", () => {
    state.list = listOf([session({ name: "Kestrel", context_pressure: true })]);
    renderView();

    expect(screen.getByText(/compacted repeatedly/i)).toBeTruthy();
  });

  /// **The absent-is-not-zero assertion.** An unmeasured session must not
  /// acquire a claim about compactions on the row -- and above all must
  /// not be told it had none, which is a measurement nobody took.
  it("renders nothing when no compaction was ever recorded", () => {
    state.list = listOf([session({ name: "Kestrel", context_pressure: null })]);
    renderView();

    expect(screen.queryByText(/compacted/i)).toBeNull();
    expect(screen.queryByText(/no compactions/i)).toBeNull();
  });

  it("renders nothing when it was measured and there was no pressure", () => {
    state.list = listOf([session({ name: "Kestrel", context_pressure: false })]);
    renderView();

    expect(screen.queryByText(/compacted/i)).toBeNull();
  });
});

/// The compactions panel, and its five renderings (#1065).
///
/// `SessionUsage`'s shape one field along. The arm that matters is the
/// first: `null` is the state of EVERY session that predates the hook,
/// so drawing it as zeros would put a measured-looking figure on the
/// whole corpus.
describe("the compactions panel", () => {
  it("says no compaction was recorded, and that the hook may not have been installed", () => {
    state.list = listOf([session({ name: "Kestrel", compactions: null })]);
    renderView();

    open("Kestrel");
    expect(screen.getByText(/no compaction has been recorded for this session/i)).toBeTruthy();
    // The second sentence is load-bearing: without it this reads as "it
    // never compacted", which is a measurement nobody took.
    expect(screen.getByText(/may not have been installed/i)).toBeTruthy();
    // And NOT the measured-zero wording, which is a different claim.
    expect(screen.queryByText(/never compacted/i)).toBeNull();
  });

  /// A measured zero is a real answer and gets its own sentence. This is
  /// the distinction the arm above exists to protect: we were watching,
  /// and nothing happened.
  it("says a measured zero differently from an absent one", () => {
    state.list = listOf([
      session({
        name: "Kestrel",
        compactions: { manual: 0, auto: 0, unknown: [], untriggered: 0 },
      }),
    ]);
    renderView();

    open("Kestrel");
    expect(screen.getByText(/never compacted/i)).toBeTruthy();
    expect(screen.queryByText(/no compaction has been recorded/i)).toBeNull();
    expect(screen.queryByText(/may not have been installed/i)).toBeNull();
  });

  /// Split, never summed: a user who compacts by hand has made a choice,
  /// and a session that compacts automatically has hit a wall.
  it("splits manual from automatic", () => {
    state.list = listOf([
      session({
        name: "Kestrel",
        compactions: { manual: 1, auto: 3, unknown: [], untriggered: 0 },
      }),
    ]);
    renderView();

    open("Kestrel");
    const panel = screen.getByText(/how often it compacted/i).closest("section")!;
    expect(within(panel).getByText("Automatic").nextSibling?.textContent).toBe("3");
    expect(within(panel).getByText("Manual").nextSibling?.textContent).toBe("1");
  });

  /// **Unknown enum values render as themselves.** A trigger this app has
  /// never heard of must appear under its own name -- never folded into
  /// `auto`, which would overstate the pressure figure, and never
  /// relabelled "other", which would hide from the reader that the
  /// vocabulary has grown.
  it("renders an unknown trigger as itself, not as 'other'", () => {
    state.list = listOf([
      session({
        name: "Kestrel",
        compactions: { manual: 0, auto: 1, unknown: [["emergency", 2]], untriggered: 0 },
      }),
    ]);
    renderView();

    open("Kestrel");
    const panel = screen.getByText(/how often it compacted/i).closest("section")!;
    expect(within(panel).getByText("emergency")).toBeTruthy();
    expect(within(panel).queryByText(/\bother\b/i)).toBeNull();
    // And it did NOT quietly increment `auto`.
    expect(within(panel).getByText("Automatic").nextSibling?.textContent).toBe("1");
  });

  /// A record whose `trigger` field was absent says the payload SHAPE
  /// moved, which is a different problem from the vocabulary growing --
  /// so it is counted apart rather than summed into one bucket.
  it("counts a record with no trigger apart from an unrecognised one", () => {
    state.list = listOf([
      session({
        name: "Kestrel",
        compactions: { manual: 0, auto: 0, unknown: [], untriggered: 2 },
      }),
    ]);
    renderView();

    open("Kestrel");
    // It is a real compaction, so the panel must NOT say the session
    // never compacted.
    expect(screen.queryByText(/never compacted/i)).toBeNull();
    expect(screen.getByText(/no trigger recorded/i)).toBeTruthy();
  });
});

/// What the hook says the subagents WERE, and the one case in which it
/// contradicts the directory rule (#1066).
describe("the subagent agent types", () => {
  it("names the stated types beside the inferred count", () => {
    state.list = listOf([
      session({
        name: "Kestrel",
        subagents: 3,
        agent_types: {
          stated: [
            ["general-purpose", 2],
            ["code-reviewer", 1],
          ],
          untyped: 0,
          inferred_children: 3,
        },
      }),
    ]);
    renderView();

    open("Kestrel");
    expect(screen.getByText(/2 general-purpose, 1 code-reviewer/)).toBeTruthy();
    // The directory rule's own count is still there: the hook is an
    // additional source, never a replacement.
    expect(screen.getByText(/3 subagent sessions ran under this one/i)).toBeTruthy();
  });

  /// A type this app has never heard of renders as itself.
  it("renders an unknown agent type verbatim", () => {
    state.list = listOf([
      session({
        name: "Kestrel",
        subagents: 1,
        agent_types: { stated: [["flux-capacitor", 1]], untyped: 0, inferred_children: 1 },
      }),
    ]);
    renderView();

    open("Kestrel");
    expect(screen.getByText(/1 flux-capacitor/)).toBeTruthy();
    expect(screen.queryByText(/\bother\b/i)).toBeNull();
  });

  /// **The pre-hook case, and it must not read as an error.** Every
  /// session that already exists looks like this: children found by their
  /// working directories, and no hook record to name them. Saying which
  /// source the grouping came from is what stops it reading as a gap.
  it("says the grouping came from the directory layout when the hook said nothing", () => {
    state.list = listOf([
      session({
        name: "Kestrel",
        subagents: 4,
        agent_types: { stated: [], untyped: 0, inferred_children: 4 },
      }),
    ]);
    renderView();

    open("Kestrel");
    expect(screen.getByText(/grouped by their working directories/i)).toBeTruthy();
    // NOT the disagreement note. This direction is the normal state of
    // the entire corpus and flagging it would fire everywhere.
    expect(screen.queryByText(/directory rule does not recognise/i)).toBeNull();
  });

  /// **The disagreement, in the one direction that is one.** The hook
  /// recorded spawns and the cwd rule attributed no children, which means
  /// subagents are running somewhere the rule does not look.
  it("reports the disagreement when the hook saw spawns the directory rule did not", () => {
    state.list = listOf([
      session({
        name: "Kestrel",
        subagents: 0,
        agent_types: {
          stated: [["general-purpose", 2]],
          untyped: 0,
          inferred_children: 0,
        },
      }),
    ]);
    renderView();

    open("Kestrel");
    expect(screen.getByText(/2 subagent starts/)).toBeTruthy();
    expect(screen.getByText(/directory rule does not recognise/i)).toBeTruthy();
  });

  /// The same finding on a session that DOES have attributed children is
  /// not a finding, and the asymmetry is asserted from the component as
  /// well as from `subagentDisagreement`'s own unit test -- the note is
  /// rendered on two different code paths here, and only one of them is
  /// covered by the case above.
  it("does not report a disagreement when both sources found subagents", () => {
    state.list = listOf([
      session({
        name: "Kestrel",
        subagents: 2,
        agent_types: {
          stated: [["general-purpose", 2]],
          untyped: 0,
          inferred_children: 2,
        },
      }),
    ]);
    renderView();

    open("Kestrel");
    expect(screen.queryByText(/directory rule does not recognise/i)).toBeNull();
  });

  /// A session with no children and no hook record renders no section at
  /// all -- the majority of rows, and a section saying "this has no
  /// subagents" on all of them is noise.
  it("renders no subagent section for an ordinary session", () => {
    state.list = listOf([session({ name: "Kestrel", subagents: 0, agent_types: null })]);
    renderView();

    open("Kestrel");
    expect(screen.queryByText(/what its subagents did/i)).toBeNull();
  });
});

/// #1133: the opening ask, under the title.
///
/// 286 of 1,438 real sessions share their `aiTitle` with another, so a
/// list showing only titles cannot tell two rows apart at the moment
/// someone is choosing which to resume.
describe("the opening prompt", () => {
  it("renders under the title", () => {
    state.list = listOf([session({ name: "Fix the retry", opening_prompt: "make the backoff jittered" })]);
    renderView();
    expect(screen.getByText("make the backoff jittered")).toBeTruthy();
  });

  /// `null` renders as NOTHING. Never the title repeated, never the
  /// UUID: a fabricated stand-in cannot be told from a real prompt,
  /// which is the rule this page already states about titleless
  /// sessions.
  it("renders nothing when there is no prompt", () => {
    state.list = listOf([session({ name: "Fix the retry", opening_prompt: null })]);
    const { container } = renderView();
    // The title is there once; nothing stands in for the missing prompt.
    expect(screen.getAllByText("Fix the retry").length).toBe(1);
    expect(container.textContent).not.toContain("undefined");
  });

  it("is searchable", () => {
    state.list = listOf([
      session({ session_id: "a", name: "One", opening_prompt: "fix the notarization bug" }),
      session({ session_id: "b", name: "Two", opening_prompt: "add a chart" }),
    ]);
    renderView();
    fireEvent.change(screen.getByPlaceholderText(/Search title, prompt/), {
      target: { value: "notarization" },
    });
    expect(screen.getByText("One")).toBeTruthy();
    expect(screen.queryByText("Two")).toBeNull();
  });
});

/// #1135: what the transcript corpus costs on disk.
///
/// Headstate reports a footprint for worktrees, artifacts, venvs, Docker
/// and packages. The one corpus it reads most had none.
describe("the transcript footprint", () => {
  it("states what the session transcripts cost", () => {
    state.imported = imported({ session_bytes: 916_000_000, subagent_bytes: 0 });
    renderView();
    expect(screen.getByText(/of transcripts/)).toBeTruthy();
  });

  /// The two halves stay APART: sessions you can resume against work
  /// they delegated. Roughly half the .jsonl files on disk are subagent
  /// transcripts.
  it("keeps subagent transcripts apart from sessions", () => {
    state.imported = imported({ session_bytes: 916_000_000, subagent_bytes: 400_000_000 });
    renderView();
    expect(screen.getByText(/of subagent transcripts/)).toBeTruthy();
  });

  /// A size we could not take is not a size of zero, so the total is a
  /// floor and says so.
  it("qualifies the total when a size could not be read", () => {
    state.imported = imported({ session_bytes: 916_000_000, unsized_files: 3 });
    renderView();
    expect(screen.getByText(/at least/)).toBeTruthy();
  });

  /// Nothing measured renders nothing, rather than "0 B" -- which would
  /// read as a corpus that costs nothing.
  it("says nothing when no bytes were measured", () => {
    state.imported = imported({ session_bytes: 0 });
    renderView();
    expect(screen.queryByText(/of transcripts/)).toBeNull();
  });
});

/// Stopping a live session (#1219).
///
/// Every one of these drives the two mocked calls. Nothing here reaches
/// the real `claudeStopSession`, so no test in this file can signal a
/// process on the machine running it -- which is the precondition for
/// having these tests at all.
describe("stopping a session", () => {
  const running = () =>
    session({ liveness: { state: "running", pid: 14779, status: "busy" } });

  const proposal = (over: Record<string, unknown> = {}) => ({
    session_id: "e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
    action: "proposed",
    pid: 14779,
    refusal: null,
    why: "pid 14779 is running and its start time matches what the registry recorded",
    evidence: {
      name: "widget-c3",
      cwd: "/Users/acme/code/widget",
      status: "busy",
      uptime_secs: 7_200,
      auto_compactions: 4,
      last_turn: "Running the integration suite against the staging cluster",
    },
    ...over,
  });

  /// A dead session has nothing to stop, so the affordance is absent
  /// rather than present and disabled.
  it("offers nothing for a session that is not running", () => {
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.queryByRole("button", { name: /review stopping it/i })).toBeNull();
  });

  /// "Could not tell" is the one state in which signalling would be a
  /// guess. Withheld, and it SAYS it was withheld -- a missing button
  /// with no explanation reads as the app being broken.
  it("refuses an unknown liveness and says why rather than showing nothing", () => {
    state.list = listOf([
      session({ liveness: { state: "unknown", why: "the registry could not be read" } }),
    ]);
    renderView();
    open("HeadState GitHub issues filing");
    expect(screen.queryByRole("button", { name: /review stopping it/i })).toBeNull();
    expect(screen.getByText(/could not be confirmed/i)).toBeTruthy();
    // `getAllBy`: the heading states the same reason for its own badge,
    // and the two saying the same thing is correct -- a section that
    // withheld the button without a reason is what this pins against.
    expect(screen.getAllByText(/the registry could not be read/i).length).toBeGreaterThan(0);
  });

  /// The evidence is shown BEFORE any stop is offered, and the last turn
  /// is part of it. This is the issue's requirement and the reason this
  /// is a proposal rather than a confirmation dialog.
  it("shows the last turn and the evidence before offering the stop", async () => {
    proposeFn.mockResolvedValueOnce([proposal()]);
    state.list = listOf([running()]);
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /review stopping it/i }));
    await vi.waitFor(() =>
      expect(screen.getByText(/Running the integration suite/)).toBeTruthy(),
    );
    expect(screen.getByText(/2h 0m/)).toBeTruthy();
    expect(screen.getByText("4")).toBeTruthy();
    // And the stop is only offered once the evidence is on screen.
    expect(screen.getByRole("button", { name: /stop this session/i })).toBeTruthy();
  });

  /// SIGTERM-first is stated to the user, not just implemented. The
  /// escalation and what it costs are named before it can happen.
  it("states that SIGTERM goes first and what SIGKILL would cost", async () => {
    proposeFn.mockResolvedValueOnce([proposal()]);
    state.list = listOf([running()]);
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /review stopping it/i }));
    await vi.waitFor(() => expect(screen.getByText(/SIGTERM first/i)).toBeTruthy());
    expect(screen.getByText(/leaves no end record/i)).toBeTruthy();
  });

  /// The SESSION ID is what crosses, never the pid on screen. The pid is
  /// re-derived in Rust at the moment of the stop, because the list it
  /// came from is ten seconds stale.
  it("sends the session id and never the rendered pid", async () => {
    proposeFn.mockResolvedValueOnce([proposal()]);
    stopFn.mockResolvedValueOnce({
      session_id: "e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
      pid: 14779,
      signal: "terminated",
      waited_ms: 300,
    });
    state.list = listOf([running()]);
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /review stopping it/i }));
    await vi.waitFor(() =>
      expect(screen.getByRole("button", { name: /stop this session/i })).toBeTruthy(),
    );
    fireEvent.click(screen.getByRole("button", { name: /stop this session/i }));
    await vi.waitFor(() =>
      expect(stopFn).toHaveBeenCalledWith("e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2"),
    );
    expect(stopFn).toHaveBeenCalledTimes(1);
    // One argument, and it is not the pid.
    expect(stopFn.mock.calls[0]).toHaveLength(1);
    expect(stopFn.mock.calls[0][0]).not.toBe(14779);
  });

  /// Which signal ended it is REPORTED, because the two outcomes leave
  /// the user with different things: SIGTERM wrote a transcript tail and
  /// SIGKILL did not.
  it("says which signal ended the session", async () => {
    proposeFn.mockResolvedValue([proposal()]);
    stopFn.mockResolvedValueOnce({
      session_id: "e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
      pid: 14779,
      signal: "killed",
      waited_ms: 5_000,
    });
    state.list = listOf([running()]);
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /review stopping it/i }));
    await vi.waitFor(() =>
      expect(screen.getByRole("button", { name: /stop this session/i })).toBeTruthy(),
    );
    fireEvent.click(screen.getByRole("button", { name: /stop this session/i }));
    await vi.waitFor(() =>
      expect(toastSuccess).toHaveBeenCalledWith(expect.stringMatching(/SIGKILL/)),
    );
  });

  /// A REFUSAL is rendered as one. The pid-reuse case is the reason the
  /// pid is re-derived at all, and hiding it would leave the user with a
  /// button that appeared to do nothing.
  it("renders a pid-reuse refusal rather than dropping it", async () => {
    proposeFn.mockResolvedValueOnce([
      proposal({
        action: "refused",
        pid: null,
        refusal: { kind: "pid_reused", pid: 14779, drift_secs: 28_800 },
        why: "pid 14779 is running but started 28800s from the recorded time, so the number has been reused by a different process -- nothing was signalled",
      }),
    ]);
    state.list = listOf([running()]);
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /review stopping it/i }));
    await vi.waitFor(() => expect(screen.getByText(/reused by a different process/)).toBeTruthy());
    expect(screen.getByText(/nothing was signalled/)).toBeTruthy();
    // And no stop is offered on a refusal.
    expect(screen.queryByRole("button", { name: /stop this session/i })).toBeNull();
  });

  /// Absent is not zero: a session with no compaction record must not
  /// render "0", which would be a confident wrong answer about a session
  /// that may have compacted many times before the hook existed.
  it("does not render a missing compaction count as zero", async () => {
    proposeFn.mockResolvedValueOnce([
      proposal({ evidence: { ...proposal().evidence, auto_compactions: null } }),
    ]);
    state.list = listOf([running()]);
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /review stopping it/i }));
    await vi.waitFor(() => expect(screen.getByText(/no compaction record/i)).toBeTruthy());
    expect(screen.getByText(/not the same as none/i)).toBeTruthy();
  });

  /// A transcript that could not be read is said so, rather than
  /// rendering a blank that reads as the session having said nothing.
  it("names an unreadable transcript rather than showing a blank", async () => {
    proposeFn.mockResolvedValueOnce([
      proposal({ evidence: { ...proposal().evidence, last_turn: null } }),
    ]);
    state.list = listOf([running()]);
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /review stopping it/i }));
    await vi.waitFor(() =>
      expect(screen.getByText(/transcript could not be read/i)).toBeTruthy(),
    );
  });

  /// Nothing acts unattended. Opening the pane proposes nothing, and a
  /// proposal alone signals nothing -- two clicks, and the first only
  /// reads.
  it("signals nothing until the user asks twice", async () => {
    proposeFn.mockResolvedValueOnce([proposal()]);
    state.list = listOf([running()]);
    renderView();
    open("HeadState GitHub issues filing");
    // Selecting the session proposed nothing.
    expect(proposeFn).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /review stopping it/i }));
    await vi.waitFor(() =>
      expect(screen.getByRole("button", { name: /stop this session/i })).toBeTruthy(),
    );
    // And the proposal signalled nothing.
    expect(stopFn).not.toHaveBeenCalled();
  });

  /// The pane does not read as the app advising a kill.
  /// `health::runaway`'s Notice-vs-Alert split: a stuck session is an
  /// INDICATOR.
  it("does not recommend stopping", async () => {
    proposeFn.mockResolvedValueOnce([proposal()]);
    state.list = listOf([running()]);
    const { container } = renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /review stopping it/i }));
    await vi.waitFor(() =>
      expect(screen.getByRole("button", { name: /stop this session/i })).toBeTruthy(),
    );
    expect(screen.getByText(/does not recommend stopping it/i)).toBeTruthy();
    const text = container.textContent ?? "";
    expect(text).not.toMatch(/you should stop|we recommend stopping|this session should be/i);
  });

  /// A refused stop NAMES the reason. A generic "could not stop" would
  /// throw away the most useful thing this feature can say.
  it("names the refusal when the stop itself is refused", async () => {
    proposeFn.mockResolvedValueOnce([proposal()]);
    stopFn.mockRejectedValueOnce(
      "pid 14779 is running but started 28800s from the recorded time, so the number has been reused by a different process -- nothing was signalled",
    );
    state.list = listOf([running()]);
    renderView();
    open("HeadState GitHub issues filing");
    fireEvent.click(screen.getByRole("button", { name: /review stopping it/i }));
    await vi.waitFor(() =>
      expect(screen.getByRole("button", { name: /stop this session/i })).toBeTruthy(),
    );
    fireEvent.click(screen.getByRole("button", { name: /stop this session/i }));
    await vi.waitFor(() =>
      expect(toastError).toHaveBeenCalledWith(
        expect.stringMatching(/nothing was signalled/i),
        expect.objectContaining({
          description: expect.stringMatching(/reused by a different process/),
        }),
      ),
    );
  });
});

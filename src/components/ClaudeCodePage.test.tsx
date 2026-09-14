import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  ClaudeImported,
  ClaudeSession,
  ClaudeSessionList,
  Worktree,
  WorktreeRepo,
} from "@/types/pr";
import { useFilters } from "@/store/filters";

const copyFn = vi.hoisted(() => vi.fn(() => Promise.resolve(null as string | null)));
const revealFn = vi.hoisted(() => vi.fn(() => Promise.resolve("/code/app")));
const toastError = vi.hoisted(() => vi.fn());
const toastSuccess = vi.hoisted(() => vi.fn());
const rescanFn = vi.hoisted(() => vi.fn(() => Promise.resolve()));
const refetchFn = vi.hoisted(() => vi.fn());

const state = vi.hoisted(() => ({
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
}));

vi.mock("../api/hooks", () => ({
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
}));
vi.mock("sonner", () => ({ toast: { success: toastSuccess, error: toastError } }));
vi.mock("../lib/clipboard", () => ({ copyText: copyFn }));
vi.mock("../api/tauri", () => ({ claudeRevealPath: revealFn }));

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

const session = (over: Partial<ClaudeSession> = {}): ClaudeSession => ({
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
    command: "cd '/Users/acme/code/widget' && claude --resume e5dff3bd-1b5f-40cf-8d4b-5e0cc89393e2",
    caveat: null,
    anchored: true,
  },
  runs: 1,
  ...over,
});

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

beforeEach(() => {
  state.list = listOf([session()]);
  state.loading = false;
  state.failed = false;
  state.imported = imported();
  state.importFailed = false;
  state.importFetching = false;
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
  useFilters.setState({ claudeQuery: "", claudeSelected: undefined });
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
    const rows = screen.getAllByRole("button", { pressed: false });
    const titles = rows.map((r) => r.textContent ?? "");
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

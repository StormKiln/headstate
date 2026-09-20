import { useDeferredValue, useEffect, useMemo, useState } from "react";
import { Bot, Circle, FolderOpen, GitBranch, RefreshCw, Search, Terminal } from "lucide-react";
import { toast } from "sonner";
import type {
  ClaudeAgentTypes,
  ClaudeCompactions,
  ClaudeCostState,
  ClaudePreviewBlock,
  ClaudeSession,
  ClaudeObservation,
  ClaudeProfile,
  ClaudeSessionDetail,
  ClaudeStopProposal,
  ClaudeTally,
  ClaudeWaiting,
  CwdState,
  Liveness,
} from "@/types/pr";
import {
  useClaudeSessionDetail,
  useClaudeSessionEvents,
  useClaudeSessionUsage,
  useClaudeSubagentRollup,
  useClaudeSessions,
  useClaudeTranscriptTail,
  useWorktrees,
  useUiPrefs,
} from "@/api/hooks";
import {
  claudeLaunchSession,
  claudeProposeStop,
  claudeRevealPath,
  claudeStopSession,
} from "@/api/tauri";
import { current } from "@/lib/ariaCurrent";
import { copyText } from "@/lib/clipboard";
import { segments } from "@/lib/findOverData";
import { IS_MOBILE_BUILD } from "@/lib/target";
import { AUTO_COMPACT_PRESSURE, subagentDisagreement } from "@/lib/subagentDisagreement";
import { relativeTime } from "@/lib/time";
import { useIsMobile } from "@/lib/useIsMobile";
import { useRowCursor } from "@/lib/useRowCursor";
import {
  formatSize, pathBasename, safetyReason, sessionWorktree } from "@/lib/worktrees";
import { type ClaudeSessionFilter, useFilters } from "@/store/filters";
import { QueryError, errorMessage } from "./QueryError";
import { ExternalLink } from "./ExternalLink";

/// How many rows are drawn before the list stops and says so.
///
/// # Why a cap at all, and why it is not truncation
///
/// 1,438 real sessions, measured. Every one is a button with a title, a
/// path and a derived liveness, and drawing all of them costs a long
/// first paint for a list nobody scrolls to the end of.
///
/// The house rule forbids quietly showing fewer rows than exist, so the
/// footer states the real total and offers the rest. That is the
/// difference between a cap and a truncation: a truncation lies about how
/// much there is.
///
/// 200 rather than 50 because of how tightly the corpus clusters.
/// Measured by last activity:
///
/// ```text
/// within  1 day    80
/// within  3 days  276
/// within  7 days  425
/// within 30 days 1422   of 1,459
/// ```
///
/// 97% of sessions are inside a month, so there is no cap that cleanly
/// separates "recent" from "old" -- which is exactly why the cap is a
/// RENDERING budget with the total stated, and not a filter pretending to
/// be a useful cutoff. Search is what narrows this list; the cap only
/// decides how much is drawn before the reader asks for the rest.
///
/// 200 covers a full day's work several times over and the whole of
/// yesterday, so the first screen is never missing something from this
/// morning. It does NOT cover three days (276), and an earlier version of
/// this comment claimed it did -- corrected by measurement rather than
/// left as a plausible-sounding number, since a reader checking the claim
/// is exactly who this paragraph is for.
///
/// # It is still only a rendering budget, and #985 kept it that way
///
/// This cap says nothing about TRANSPORT, and never did -- the whole
/// list arrives and this decides how much is painted. #985 found the
/// transport cost that was never part of the design (1,474 rows, 1.35 MB,
/// every ten seconds, over the pairing transport on the phone) and did
/// NOT fix it by turning this into a server-side limit, which would have
/// made the total a separate claim and quietly narrowed search to the
/// first 200 rows.
///
/// It split the ROW instead: the list carries what it draws, searches,
/// filters and counts on, and the rest is fetched for the one selected
/// session. 990 -> 315 bytes per row, measured, with this cap, the
/// stated total and the four search fields all unchanged.
/// `claude::sessions::SessionList` carries the breakdown.
const RENDER_CAP = 200;

/// Claude Code sessions on this machine, and how to get one back.
///
/// # What this component holds, after #939
///
/// The selected session's DETAIL, and the banners saying what could not
/// be read. Not the list: the search box and the session rows moved into
/// `ClaudeCodeSidebar` as `ClaudeSessionColumn`, because a `w-96` column of
/// searchable rows inside the main panel was a second sidebar standing
/// beside the real one. This file still states the rules the list obeys --
/// ordering, the four search fields, the cap -- because they are rules
/// about the same data this page is about, and `useMatchedSessions` below
/// is the one place they are implemented.
///
/// The phone is the exception, and it is why the list is a component
/// rather than a block of JSX in the sidebar: below `MOBILE_BREAKPOINT`
/// the sidebar is a `Sheet` that closes on navigation, so this page mounts
/// `ClaudeSessionColumn` in the main panel instead and keeps the
/// list-then-detail pair of screens it always had. The sidebar's doc
/// comment carries the table of both mount points.
///
/// # What the list is ordered by, and why it is not grouped
///
/// **Most recent activity first, with running sessions pinned above
/// everything.** Not start time: a session touched ten minutes ago
/// matters more than one started earlier and abandoned. Running sessions
/// leave the ordering entirely because there are never more than a
/// handful (three on the development machine at its busiest) and they are
/// the reason the view is open -- a live session at position 900 because
/// its last write was slow is the failure to avoid.
///
/// It is a FLAT list, and that is a decision against the obvious one.
/// Grouping by directory was measured first and does not work here:
///
/// ```text
/// distinct cwds                       662  (over 1,438 sessions)
/// distinct repo roots (worktrees folded) 346
///   of those, holding 1 session        217   <- 63% singletons
///   of those, holding >100 sessions      2   <- 55% of ALL sessions
/// ```
///
/// So a collapsed tree would be 217 one-row groups plus one group
/// holding 662 rows. That is a flat list with extra clicks in front of
/// it, and the big group still needs search to be usable. Search is
/// therefore the primary navigation and grouping is not offered.
///
/// # Why search covers four fields
///
/// `aiTitle` names 1,436 of 1,438 sessions, so title-first search is
/// what makes the list usable -- "the one about notarization" is how
/// people remember sessions. But titles are NOT unique: 286 sessions
/// (19.9%) share a title with another, and inside the largest group 147
/// do, mostly repeated `/security-review` runs ("scan.rs security
/// review" appears ten times). So the row shows a date and a path
/// alongside the title, and search also covers the directory, the branch
/// and the id -- the id for pasting one in from somewhere else.
///
/// # No `Date.now()` anywhere
///
/// `now` arrives from `useClaudeSessions` as the poll's
/// `dataUpdatedAt`. Reading the clock during render would make this
/// impure, which `yarn lint` rejects and which `Sparkline` and
/// `HealthConditions` both carry comments about: a repaint triggered by
/// anything at all would otherwise slide every relative time under
/// unchanged data.
///
/// # Absent is not zero: seven conditions, seven renderings
///
/// | condition | rendering |
/// |---|---|
/// | the session list could not be read | `QueryError` with the reason. NOT "no sessions". |
/// | the live registry could not be read | a banner; every row's liveness becomes "could not tell" |
/// | the transcript rescan partly failed | a line saying how many could not be read, above a list that still shows |
/// | `~/.claude/projects` does not exist | `NoSessions` names the path and what creates it. NOT a partial read (#970). |
/// | the SEARCH matched nothing | "No session matches that search" -- about the query, not the machine |
/// | the CHIP matched nothing | "No session is in this filter" (#949) -- about the control, not the machine |
/// | genuinely nothing | `NoSessions` -- only when the read SUCCEEDED and nothing was narrowed |
///
/// The count was "four" until #970 and stayed there through the fifth row
/// it added; it is corrected here rather than left, since a heading that
/// undercounts its own table is the reader's first reason to stop trusting
/// it.
///
/// The `~/.claude/projects` row is #970's correction, and it was a failure
/// hiding inside the partial-read one: the absent root travelled in
/// `unreadable_dirs`, so a machine that had never run Claude Code was told
/// "0 sessions read, but 1 could not be -- this list is incomplete by an
/// unknown amount". It now arrives in `absent_root`, which `is_partial()`
/// does not consult, and `NoSessions` renders it as the explanation it
/// always was.
///
/// The last three rows all render an EMPTY list and must not be confused,
/// which is why `ClaudeSessionColumn` tests them in that order: `NoSessions`
/// is a claim about the machine -- it names the path and offers a rescan --
/// so it may only be reached when nothing was narrowed. Under an active chip
/// it would tell a user with 1,474 sessions that they have none.
///
/// The precedent is `ClaudeMdPage` (#846), one view over, where a `= []`
/// default made a rejected scan read as "No CLAUDE.md files in this
/// repository" -- a confident wrong answer to a question the app could
/// not answer. The error arm is ordered BEFORE the empty arm here for
/// that exact reason: with a `[]` default the empty branch is reached
/// first and an error arm after it is unreachable in the case it exists
/// for.
export function ClaudeCodePage() {
  const { list, imported, now, rescan } = useClaudeSessions(true);
  const { all, matched } = useMatchedSessions();
  const selected = useFilters((f) => f.claudeSelected);
  const selectSession = useFilters((f) => f.selectClaudeSession);
  // Read here only to word the empty detail pane (#978): "no session
  // matches that search" and "nothing has been read yet" are different
  // emptinesses, and the pane must not offer "choose one" for either.
  const query = useFilters((f) => f.claudeQuery);
  const isMobile = useIsMobile();

  // By LOOKUP against the current list, never a remembered session. The
  // store holds only the id (see `claudeSelected`), so a transcript
  // deleted between two polls makes this `undefined` and the
  // choose-a-session prompt renders -- a detail pane assembled from a
  // copy of a row that no longer exists cannot happen by construction.
  const active = matched?.ordered.find((s) => s.session_id === selected);
  // On a phone the two panes are two screens, so which is showing keys
  // off whether the user has PICKED a session -- the pattern
  // `ClaudeMdPage` uses. No first-row fallback here, deliberately: with
  // 1,438 rows, opening straight into an arbitrary session's detail
  // would bury the search box this list depends on.
  //
  // `active` rather than `selected`, as of #939: a stale id whose session
  // has gone must send the phone BACK to the list, because the alternative
  // is a detail screen with nothing on it but a back link.
  const showingList = !isMobile || active === undefined;

  if (list.isLoading) {
    return <p className="p-4 text-sm text-[#8b949e]">Reading Claude Code sessions…</p>;
  }
  // BEFORE the arm that would say "choose a session", per #846: `active`
  // is undefined on a rejection exactly as it is when nothing is selected,
  // so an error arm placed after it would never render in the case it
  // exists for -- the pane would invite the user to choose from a list
  // that could not be read.
  //
  // Says something DIFFERENT from `ClaudeSessionColumn`'s arm, which is
  // showing at the same moment on the desktop. That one is about the rows
  // it cannot draw; this one is about the detail it cannot resolve, and it
  // is the pane that carries the retry because it is the larger surface.
  // Two copies of one sentence side by side would read as two failures.
  if (list.isError || !matched || !all) {
    return (
      <div className="p-4">
        <QueryError
          title="No session detail to show"
          // The REASON is stated once, by the column, which is where the
          // list that failed was going to be. Repeating it here would put
          // the same string on screen twice and read as two failures.
          message="The Claude Code session list could not be read, so there is nothing to select from."
          onRetry={() => void list.refetch()}
        />
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <Banners
        registryFailure={list.data?.registry_failure ?? null}
        registryUnreadable={list.data?.registry_unreadable ?? []}
        imported={imported}
        onRescan={rescan}
      />
      <div className={isMobile ? "flex min-h-0 flex-1 flex-col" : "flex min-h-0 flex-1"}>
        {/* The phone's mount point for the list (#939). On the desktop it
            is `ClaudeCodeSidebar` that renders `ClaudeSessionColumn`, and
            this branch renders nothing at all -- that comment carries the
            table of both mount points and why the phone cannot use the
            sidebar's. `hidden` rather than unmounted for the detail
            screen, so scrolling back to the list keeps its position. */}
        {isMobile ? (
          <div className={showingList ? "flex min-h-0 flex-1 flex-col" : "hidden"}>
            <ClaudeSessionColumn />
          </div>
        ) : null}

        <div
          className={
            isMobile
              ? showingList
                ? "hidden"
                : "flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto p-4"
              : "min-w-0 flex-1 overflow-y-auto p-4"
          }
        >
          {isMobile && !showingList ? (
            <button
              type="button"
              onClick={() => selectSession(undefined)}
              className="tap-target -ml-1 mb-2 flex items-center self-start rounded px-2 text-sm text-[#58a6ff] hover:bg-[#161b22]"
            >
              ← All sessions
            </button>
          ) : null}
          {active ? (
            <SessionDetail session={active} now={now} />
          ) : matched.ordered.length === 0 ? (
            // NOT "choose a session" (#978). There is nothing to choose,
            // and an instruction a reader cannot follow makes them
            // conclude the list failed to load -- which is the one thing
            // the #846 error arm above exists to distinguish this from.
            //
            // The column beside this one carries the explanation and the
            // next step, so this pane points at it rather than repeating
            // it: two copies of one sentence side by side read as two
            // separate findings.
            <p className="text-sm text-[#8b949e]">
              {query.trim()
                ? "Nothing to show — narrow or clear the search to pick a session."
                : "Nothing to show yet — a session appears here once there is one to pick."}
            </p>
          ) : (
            <p className="text-sm text-[#8b949e]">
              Choose a session to see where it ran and how to resume it.
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

/// Whether one session belongs in the chip's subset (#949).
///
/// A pure function over the two readings every row already carries, so it
/// can be tested without a DOM and so the five predicates are stated once.
/// Exported for that test: these are the definitions the chip labels
/// promise, and a chip whose label and predicate disagree is worse than no
/// chip.
///
/// # `unknown` belongs to neither directory chip
///
/// `cwd_state` is a four-state, and only `exists` and `gone` are claims
/// about the directory. `unknown` means the CHECK failed -- the tree may
/// well be there and the `cd` would have worked -- and `not-recorded`
/// means there was never a path to look for. Neither is "gone", and
/// `revealRefusal` below gives all four different wording precisely so a
/// single bucket cannot collapse them into a shrug. The overview counts
/// them as neither too (`cwd_unknown`, "counted as neither"), so the chip
/// and the tile agree.
///
/// Consequence, stated because it is a real one: Resumable and Directory
/// gone do not sum to the total. On the measured corpus that is 179 + 1,295
/// out of 1,474, and the missing rows are the ones whose directory could
/// not be checked. The counts beside the chips are what makes that visible
/// rather than a silent shortfall.
export function matchesClaudeFilter(s: ClaudeSession, filter: ClaudeSessionFilter): boolean {
  switch (filter) {
    case "all":
      return true;
    case "resumable":
      // "Not running, AND the directory still exists", which is the
      // overview's own predicate verbatim (`overview.rs`: "not running +
      // cwd exists"). Matching it is the point rather than an accident:
      // #948 makes that tile navigate HERE, and a tile reading 179 that
      // opens a list of 181 is a tile that lied about where it went. The
      // difference is only the handful that can be live at once, which is
      // exactly the size of gap nobody would notice and everybody would
      // eventually trip over.
      //
      // It is also the right predicate on its own terms: resuming a
      // session that is already alive starts a SECOND copy of it, which is
      // the failure both banners on the overview are worded to prevent.
      return s.liveness.state !== "running" && s.cwd_state.state === "exists";
    case "gone":
      // Same subtraction, same reason -- `overview.rs` counts `archived`
      // as "not running + cwd gone", and the three cwd buckets plus
      // running sum to the total so a reader can check the arithmetic.
      return s.liveness.state !== "running" && s.cwd_state.state === "gone";
    case "running":
      return s.liveness.state === "running";
    case "ended":
      // `dead` and not `!== "running"`, which would sweep in `unknown`.
      // `unknown` is the state where the check could not be completed, and
      // it is the entire imported history on any machine that adopted
      // Headstate after using Claude Code -- so folding it in here would
      // make this chip mean "everything", which is what `all` is for.
      return s.liveness.state === "dead";
  }
}

/// The sessions the current search and chip match, running ones first.
///
/// A hook rather than a prop, because the list and the detail render in
/// two different columns since #939 -- `ClaudeSessionColumn` in the sidebar
/// and `ClaudeCodePage` in the main panel -- with no ancestor between them
/// to hold this. Both call `useClaudeSessions(true)`, which is one query
/// and therefore one poll: react-query serves the second caller from the
/// cache, so the split costs nothing on the wire.
///
/// Stating the matching ONCE is the point. The rule has four parts that
/// must not drift -- which fields search covers, which subset the chip
/// selects (#949), running-first ordering, and no `= []` default -- and two
/// copies of it would be two chances for the list and the detail to
/// disagree about which session the same id names.
function useMatchedSessions() {
  const { list } = useClaudeSessions(true);
  const typed = useFilters((f) => f.claudeQuery);
  // Deferred so typing stays responsive over a corpus this size: the
  // input updates immediately and the 1,400-row filter catches up. NOT a
  // debounce -- a debounce drops keystrokes; this renders every
  // character and only lets React deprioritise the expensive pass.
  const query = useDeferredValue(typed);
  const filter = useFilters((f) => f.claudeFilter);
  const showSubagents = useFilters((f) => f.claudeShowSubagents);

  // NO `= []` default (#846). A rejected read must reach the caller's
  // error arm rather than arriving there as an empty list that reads as
  // "you have no sessions".
  const all = list.data?.sessions;

  const matched = useMemo(() => {
    if (!all) return undefined;
    const q = query.trim().toLowerCase();
    // The chip FIRST, then the text, and the order is only about reading
    // clearly -- an `&&` of two predicates over one pass would be the same
    // set. The counts below need the chip's subset independently of the
    // query, which is the actual reason `chipped` is a named binding.
    // Subagents FIRST, then the chip, then the text (#1002). The order
    // is only about reading clearly -- an `&&` of the predicates over one
    // pass would be the same set -- but the subsets below need each stage
    // independently, which is why each is a named binding.
    //
    // Hidden by DEFAULT, never dropped from the payload: the count beside
    // the toggle is over the whole list, and the rows are one click away.
    const visible = showSubagents ? all : all.filter((s) => s.kind.kind !== "subagent");
    const chipped = visible.filter((s) => matchesClaudeFilter(s, filter));
    const hits = q
      ? chipped.filter((s) =>
          // The opening prompt joins the searched fields (#1133): the
          // thing a user remembers is often a phrase they typed, and
          // the placeholder below says so.
          [s.name, s.cwd, s.git_branch, s.session_id, s.opening_prompt].some((f) =>
            f?.toLowerCase().includes(q),
          ),
        )
      : chipped;
    // Running first, then the backend's newest-activity-first order,
    // which `claude_session_activity` indexes and `sessions.rs` states.
    // A stable partition rather than a re-sort: re-deriving the date
    // ordering here would be a second description of the same rule, and
    // one that could disagree with the query's.
    //
    // The chips filter the INPUT to this partition rather than replacing
    // it (#949): the running-first rule is about what a reader needs to see
    // at the top and is true of any subset, so a chip that re-ordered would
    // be a second ordering rule.
    const live = hits.filter((s) => s.liveness.state === "running");
    const rest = hits.filter((s) => s.liveness.state !== "running");
    return { live, rest, ordered: [...live, ...rest], chipped };
  }, [all, query, filter, showSubagents]);

  // Every chip's population, over the WHOLE list and not the current
  // subset (#949). A count that shrank to zero on every chip but the
  // active one would tell the reader nothing about where to go next, and a
  // Resumable chip reading 0 while 179 sessions are resumable is the
  // confident-wrong-answer failure with a number on it.
  //
  // Computed here rather than in the column so the counts and the rows come
  // from one pass over one list; `all` is undefined on a rejected read and
  // this stays undefined with it rather than reporting five zeros.
  const counts = useMemo(() => {
    if (!all) return undefined;
    // The chips count over the set the toggle admits, not over `all`
    // (#1002). A Running chip reading 12 while the list it opens holds 4
    // is the confident-wrong-answer failure with a number on it -- the
    // same argument #949 makes for counting over the whole list rather
    // than the current chip's subset. The denominator that moves is the
    // one the user is actually choosing among.
    const visible = showSubagents ? all : all.filter((s) => s.kind.kind !== "subagent");
    return {
      all: visible.length,
      resumable: visible.filter((s) => matchesClaudeFilter(s, "resumable")).length,
      gone: visible.filter((s) => matchesClaudeFilter(s, "gone")).length,
      running: visible.filter((s) => matchesClaudeFilter(s, "running")).length,
      ended: visible.filter((s) => matchesClaudeFilter(s, "ended")).length,
      // Over the WHOLE list, always: this is the number the toggle offers
      // to reveal, so it must not be computed over a set that already
      // excludes them. #975's rule -- a hidden exclusion states its size,
      // or a user counting rows disagrees with the app and cannot find
      // out why.
      subagents: all.filter((s) => s.kind.kind === "subagent").length,
    };
  }, [all, showSubagents]);

  return { list, all, matched, counts };
}

/// The chips, in the order they are offered (#949).
///
/// A table rather than five blocks of JSX, so the label, the predicate key
/// and the count key cannot drift apart -- and so the order is a single
/// declaration. `All` first because it is the default and the way back;
/// then the two directory states, which is the split that decides whether a
/// resume lands in the right tree; then the two liveness states.
const CLAUDE_CHIPS: ReadonlyArray<{
  filter: ClaudeSessionFilter;
  label: string;
  /// What the chip promises, in the `title` -- the predicate said in words,
  /// because "Resumable" alone does not tell a reader that a directory
  /// which could not be CHECKED is in neither of the two directory chips.
  hint: string;
}> = [
  { filter: "all", label: "All", hint: "Every session Headstate has a row for" },
  {
    filter: "resumable",
    label: "Resumable",
    hint: "Not running, and the directory it ran in still exists — so a resume lands in the right tree. The same figure the overview's Resumable tile shows",
  },
  {
    filter: "gone",
    label: "Directory gone",
    hint: "Not running, and the directory is definitely not there — normal for an agent worktree, and these are still resumable by id",
  },
  {
    filter: "running",
    label: "Running",
    hint: "The process is alive and its start time matches what was recorded",
  },
  {
    filter: "ended",
    label: "Ended",
    hint: "The process has finished. Whether it shut down cleanly or crashed is in the reason on each row, not in this filter",
  },
];

/// The search box and the session rows, wherever they are mounted (#939).
///
/// # Two mount points, one component
///
/// `ClaudeCodeSidebar` renders this under its `Sessions` row on the
/// desktop; `ClaudeCodePage` renders it in the main panel on the phone,
/// because that column is a `Sheet` there and a sheet closes the moment
/// you tap a result. The sidebar's doc comment carries the table and the
/// reasoning. One component at both points rather than a phone copy, per
/// `useIsMobile`'s rule: a component that forks drifts from its twin the
/// first time one of them is touched.
///
/// It therefore lays itself out to FILL its parent (`min-h-0 flex-1`) and
/// sets no width of its own. The `w-96` the old in-panel column carried
/// is gone with the column; the sidebar's `w-64` and the phone's full
/// width are both decided by the parent, which is the only thing that
/// knows how much room there is.
///
/// # Its own loading and error arms, not the page's
///
/// On the desktop this is the only thing on screen that is about the list,
/// so a failed read has to be stated HERE -- the page beside it is showing
/// the overview or a detail prompt and would otherwise leave the column
/// simply blank. The arms are in #846's order for the reason the page's
/// doc comment gives at length: the error arm BEFORE the empty arm, and no
/// `= []` default, so a rejected read can never render as "no sessions".
export function ClaudeSessionColumn() {
  // `counts` is #949's per-chip population.
  const { list, all, matched, counts } = useMatchedSessions();
  // `imported` as well as `now` since #970/#978: the empty state has to say
  // WHY it is empty, and only the scan knows whether `~/.claude/projects`
  // is there. Same query as the page's, so this costs a cache hit.
  const { now, imported } = useClaudeSessions(true);
  const query = useFilters((f) => f.claudeQuery);
  const setQuery = useFilters((f) => f.setClaudeQuery);
  const filter = useFilters((f) => f.claudeFilter);
  const setFilter = useFilters((f) => f.setClaudeFilter);
  const showSubagents = useFilters((f) => f.claudeShowSubagents);
  const setShowSubagents = useFilters((f) => f.setClaudeShowSubagents);
  const selected = useFilters((f) => f.claudeSelected);
  const selectSession = useFilters((f) => f.selectClaudeSession);
  // The single app-wide keyboard cursor (#953). One cursor, owned by
  // whichever view has claimed it -- `filters.ts` holds one value and
  // #953 forbids a second, because two lists owning two cursors is the
  // drift this repo refuses elsewhere.
  const cursor = useFilters((f) => f.cursor);
  const setCursor = useFilters((f) => f.setCursor);
  // Local, not in the store: unlike the query and the selection nothing
  // outside this component reads it, and it is a statement about how much
  // of ONE rendering of the list has been asked for.
  const [showAll, setShowAll] = useState(false);

  // The rows `j`/`k`/`Enter` walk (#953). The longest list in the app --
  // ~1,474 rows with "Show all" pressed, each one a focusable button with
  // no roving tabindex -- and the one the issue measures as unusable by
  // Tab.
  //
  // `capped`, not `matched.ordered`: the cursor must walk what is DRAWN.
  // Below the "Show all" button the remaining rows are not in the DOM, so
  // a cursor that could reach index 500 of 1,474 would highlight nothing
  // and `Enter` would open a session the user cannot see.
  //
  // No `toggle`: sessions have no bulk action, so `x` does nothing here
  // rather than inventing a selection with nothing to act on it. The
  // shortcut help in Settings says "pull request" for that key, which
  // stays accurate.
  //
  // Declared BEFORE the early returns below, as the Rules of Hooks
  // require -- which is why the loading and failed cases are spelled out
  // here rather than left to the fact that those branches return early.
  //
  // They are NOT the same absence. `useMatchedSessions` reads its own
  // query and goes on returning the previous `ordered` while `list` is
  // refetching or has rejected, so a target built from `matched` alone
  // reported rows for a column that was drawing a spinner or a retry
  // button. `j` would then move a cursor through rows that are not on
  // screen and `Enter` would open one of them -- the exact stale-list
  // hazard #953 forbids, arriving through the error path rather than
  // through filtering.
  //
  // An empty array is the honest answer for both: there is no list to
  // walk, so the keys do nothing.
  const rowsDrawn = list.isLoading || list.isError ? [] : (matched?.ordered ?? []);
  const visibleRows = showAll ? rowsDrawn : rowsDrawn.slice(0, RENDER_CAP);
  useRowCursor({
    rows: () => visibleRows.length,
    open: (i) => {
      const s = visibleRows[i];
      if (s) selectSession(s.session_id);
    },
  });
  // A cursor past the end of a newly-narrowed list points at nothing.
  // Clamped here for the reason `App.tsx` gives about the PR list:
  // clamping at render time keeps it correct for DRAWING -- the ring --
  // and not only for the next key press. Typing into the search box is
  // the common way this list shrinks under a cursor.
  useEffect(() => {
    if (cursor !== null && cursor >= visibleRows.length) {
      setCursor(visibleRows.length > 0 ? visibleRows.length - 1 : null);
    }
  }, [cursor, visibleRows.length, setCursor]);

  if (list.isLoading) {
    return <p className="p-3 text-xs text-[#8b949e]">Reading Claude Code sessions…</p>;
  }
  // BEFORE the empty arm, per #846. `list.data` is undefined on a
  // rejection, so an error arm placed after the empty one would never
  // render in the case it exists for -- it would be reached with an empty
  // `ordered` and say "No Claude Code sessions on this machine", which is
  // a confident wrong answer to a question we could not answer.
  if (list.isError || !matched || !all) {
    return (
      <div className="p-3">
        <QueryError
          title="Could not read the Claude Code sessions"
          message={errorMessage(list.error)}
          onRetry={() => void list.refetch()}
        />
      </div>
    );
  }

  // `visibleRows` above, narrowed: `matched` is non-null past the error
  // arm. One list, so the rows the cursor walks and the rows drawn cannot
  // drift apart -- which is the whole hazard a registered cursor has.
  const capped = visibleRows;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="shrink-0 border-b border-[#30363d] p-3">
        <label className="flex items-center gap-2 rounded-md border border-[#30363d] bg-[#0d1117] px-2">
          <Search className="h-3.5 w-3.5 shrink-0 text-[#8b949e]" aria-hidden="true" />
          <input
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search title, prompt, directory, branch or id"
            aria-label="Search Claude Code sessions"
            className="min-w-0 flex-1 bg-transparent py-1.5 text-xs text-[#e6edf3] outline-none placeholder:text-[#8b949e]"
          />
        </label>
        {/* The chips (#949). Below the search box because search is still
            the primary navigation -- 1,436 of 1,438 sessions have a title
            and "the one about notarization" is how people find a session.
            The chips are the second axis, for the two questions a title
            cannot answer: can I resume this into place, and did it finish.

            Not grouping, which stays rejected: these shorten the list
            rather than nesting it, so the flat ordering below survives
            intact. */}
        <div
          className="mt-2 flex flex-wrap gap-1"
          role="group"
          aria-label="Filter sessions by state"
        >
          {CLAUDE_CHIPS.map((c) => {
            const active = filter === c.filter;
            const n = counts?.[c.filter];
            return (
              <button
                key={c.filter}
                type="button"
                // `aria-pressed` rather than colour alone: which chip is on
                // is the single most important thing on this control, and a
                // reader who cannot distinguish the two backgrounds would
                // otherwise have no way to tell -- the same rule the
                // pressure row states about never letting colour be the
                // only cue.
                aria-pressed={active}
                title={c.hint}
                onClick={() => setFilter(c.filter)}
                className={`tap-target rounded-full border px-2 text-[11px] ${
                  active
                    ? "border-[#1f6feb] bg-[#1f6feb]/15 text-[#58a6ff]"
                    : "border-[#30363d] text-[#8b949e] hover:bg-[#161b22]"
                }`}
              >
                {c.label}
                {/* The population, beside every chip and over the whole
                    list. Absent rather than 0 when the count could not be
                    established, which on a rejected read is the whole set
                    -- though the error arm above has already returned by
                    then, so this is the belt to that braces. */}
                {n === undefined ? "" : ` ${n.toLocaleString()}`}
              </button>
            );
          })}
        </div>
        {/* The subagent toggle (#1002).

            A SEPARATE control from the chips above, not a sixth chip,
            because it is a separate axis: the five chips are one axis by
            construction (`ClaudeSessionFilter` sets out why), and "the
            running subagents" is a question a mutually-exclusive sixth
            chip would make unaskable. Crossed with the chip instead, which
            costs no new states.

            Rendered ONLY when there are some. A control offering to reveal
            nothing is noise on a machine that has never used subagents,
            and a zero beside it would invite the reader to wonder what
            they are missing. Suppressed at zero, exactly as the subagent
            file-skip notice above the list is.

            The COUNT is the point (#975): a hidden exclusion that does not
            say how many it hid leaves a user counting rows in disagreement
            with the app and no way to find out why. */}
        {counts !== undefined && counts.subagents > 0 ? (
          <label className="mt-2 flex items-center gap-1.5 text-[11px] text-[#8b949e]">
            <input
              type="checkbox"
              checked={showSubagents}
              onChange={(e) => setShowSubagents(e.target.checked)}
              className="tap-target h-3 w-3 accent-[#1f6feb]"
            />
            <span>
              Show {counts.subagents.toLocaleString()} subagent session
              {counts.subagents === 1 ? "" : "s"}
            </span>
          </label>
        ) : null}
        {/* The counts, always. With a cap in play the footer alone
            would not say how much the SEARCH removed, and "showing
            200 of 1,438" is a different fact from "12 of 1,438
            match".

            THREE modes now rather than two (#949), and they stay exact
            rather than collapsing: a chip narrows the denominator the
            search reports against, so "12 of 179 match" is a different
            claim from "12 of 1,474 match" and the chip's own count is
            already on the chip. The rule the old comment states -- that
            "how much the search removed" and "how much of what survived is
            drawn" are two facts -- is unchanged; there is now a third,
            which is which subset is being searched. */}
        <p className="mt-2 text-[11px] text-[#8b949e]">
          {query.trim()
            ? filter === "all"
              ? `${matched.ordered.length.toLocaleString()} of ${all.length.toLocaleString()} match`
              : `${matched.ordered.length.toLocaleString()} of ${matched.chipped.length.toLocaleString()} match in this filter · ${all.length.toLocaleString()} sessions in all`
            : filter === "all"
              ? `${all.length.toLocaleString()} sessions`
              : `${matched.ordered.length.toLocaleString()} of ${all.length.toLocaleString()} sessions`}
          {matched.live.length > 0 ? ` · ${matched.live.length} running now` : ""}
        </p>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-2">
        {/* Only when the read SUCCEEDED, which the error arm above
            has already established. A machine that has never run
            Claude Code genuinely has none. */}
        {matched.ordered.length === 0 ? (
          /* FOUR empties now, and which one it is has to be said exactly.
             #970/#978 separated "this machine has never run Claude Code"
             from "the search matched nothing"; #949 adds a third axis, and
             a chip that matches nothing is neither of those.

             The order is what makes it correct. `NoSessions` is a claim
             about the MACHINE -- it names `~/.claude/projects` and offers
             a rescan -- so it may only render when nothing was filtered
             out at all. Reaching it under an active chip would tell a user
             with 1,474 sessions that they have none, and send them looking
             for a rescan that would change nothing. So the narrowing
             controls are tested first, and `NoSessions` is the arm for a
             genuinely empty list. */
          query.trim() ? (
            <p className="p-2 text-sm text-[#8b949e]">
              {filter === "all"
                ? "No session matches that search."
                : "No session in this filter matches that search."}
            </p>
          ) : filter !== "all" ? (
            <p className="p-2 text-sm text-[#8b949e]">No session is in this filter.</p>
          ) : (
            <NoSessions imported={imported} />
          )
        ) : (
          capped.map((s, i) => (
            <SessionEntry
              key={s.session_id}
              session={s}
              now={now}
              active={s.session_id === selected}
              // The keyboard cursor, drawn as a ring (#953). `PrList`
              // passes it the same way and for the same reason: only the
              // list knows a row's index, and the cursor is an index.
              //
              // DISTINCT from `active`, which is the selection. Both can
              // be on at once and they mean different things -- the
              // cursor is where the next `Enter` lands, the selection is
              // what the detail pane is showing -- so the ring is drawn
              // over the blue rather than instead of it.
              cursored={cursor === i}
              onSelect={() => selectSession(s.session_id)}
            />
          ))
        )}
        {/* A cap that STATES the total, never a silent short list.
            The house rule (#846) is that showing fewer rows than
            exist without saying so is the same defect as an empty
            list on a failed read.

            A DIFFERENT fact from the "N of M match" count above, which is
            why both are rendered: one says how much the search removed,
            the other how much of what survived is drawn. */}
        {!showAll && matched.ordered.length > RENDER_CAP ? (
          <div className="mt-2 rounded-md border border-[#30363d] bg-[#161b22] p-2 text-center">
            <p className="text-[11px] text-[#8b949e]">
              Showing the {RENDER_CAP} most recent of{" "}
              {matched.ordered.length.toLocaleString()}.
            </p>
            <button
              type="button"
              onClick={() => setShowAll(true)}
              className="tap-target mt-1 rounded px-2 text-xs text-[#58a6ff] hover:bg-[#21262d]"
            >
              Show all {matched.ordered.length.toLocaleString()}
            </button>
          </div>
        ) : null}
      </div>
    </div>
  );
}

/// The empty list, with what would fill it (#978, #970).
///
/// # Why a statement of fact was not enough
///
/// "No Claude Code sessions on this machine." is TRUE, and that is the
/// hard half -- it renders only after the error arm has established the
/// read succeeded, which is the #846 ordering. What was missing is the
/// easy half: a first-run user is told a fact with no next step, beside a
/// right-hand pane inviting them to choose from the nothing. The epic's
/// first bullet is exactly this: "an empty state that explains nothing".
///
/// So this says where sessions come from. The page's whole subject is
/// transcripts under `~/.claude/projects`, and nothing on screen said that
/// running `claude` in any directory is what populates the list, nor that
/// the history is read off disk rather than out of an account.
/// `WorktreeJump`'s "most agent worktrees are deleted once their work
/// lands" is the page's own model for an empty state that explains.
///
/// # Three empties, not one
///
/// | condition | what the user is told |
/// |---|---|
/// | `absent_root` is set | the directory does not exist yet, named, and what makes it |
/// | the scan has not returned | it is still looking, and nothing is stuck |
/// | the root exists and is empty | there is no history here, and what makes some |
///
/// The first two are new. `absent_root` is #970's channel and exists
/// because the path used to travel in `unreadable_dirs`, where
/// `is_partial()` reads it and turned a brand-new machine into "0 sessions
/// read, but 1 could not be — this list is incomplete by an unknown
/// amount". The third is a user who HAS run Claude Code and has no
/// transcripts left, which is a different sentence from never having run
/// it, and `absent_root === null` is what distinguishes them.
///
/// The "nothing is stuck" line is copied in spirit from
/// `SystemHealthPage`'s network panel, which is the house model for a
/// first-run wait: it says how long, why it cannot be sooner, and that the
/// app has not hung.
function NoSessions({
  imported,
}: {
  imported: ReturnType<typeof useClaudeSessions>["imported"];
}) {
  // The scan has not come back yet, so "there is nothing here" is not
  // established. `imported.data === undefined` rather than `isFetching`:
  // the question is whether an answer exists, not whether a request is in
  // flight.
  //
  // NOT an error arm -- `Banners` above already renders `imported.isError`
  // with the reason, and a second copy of one failure reads as two.
  if (imported.data === undefined) {
    return (
      <div className="p-2 text-sm text-[#8b949e]">
        <p>Looking for Claude Code transcripts on this machine…</p>
        <p className="mt-1 text-xs">
          The whole of <span className="font-mono">~/.claude/projects</span> is read on the
          first open, which takes about a second for a large history. Nothing is stuck.
        </p>
      </div>
    );
  }

  const absent = imported.data.absent_root;
  return (
    <div className="p-2 text-sm text-[#8b949e]">
      <p className="text-[#e6edf3]">No Claude Code sessions on this machine.</p>
      {absent !== null ? (
        // The path is NAMED, which is why #970 kept it rather than
        // dropping it: a reader who sees which directory was looked in
        // learns where this history lives. It is stated as "not there
        // yet", because that is the truth and it is also the reason
        // there is nothing to show.
        <p className="mt-1 text-xs">
          <span className="break-all font-mono">{absent}</span> does not exist yet — Claude
          Code creates it the first time it runs.
        </p>
      ) : (
        // The directory IS there and holds no session transcripts. Not
        // the same machine state, so not the same sentence: this user has
        // run Claude Code and has no history left.
        <p className="mt-1 text-xs">
          <span className="break-all font-mono">~/.claude/projects</span> is there but holds no
          session transcripts.
        </p>
      )}
      {/* The NEXT STEP, which is the half that was missing. Read-only is
          worth saying: it is why there is no "connect an account" button
          to look for, and it is the app's own constraint against
          `~/.claude`. */}
      <p className="mt-2 text-xs">
        Run <span className="font-mono">claude</span> in any directory and it will appear here.
        Headstate reads these from the transcripts Claude Code writes to disk — it never signs
        in on your behalf and never writes to <span className="font-mono">~/.claude</span>.
      </p>
    </div>
  );
}

/// What could not be read, above the list rather than instead of it.
///
/// Three separate conditions, three separate lines, and none of them
/// replaces the rows: a partial answer labelled partial beats both a
/// silent truncation and an error page. `Scan`'s own doc comment in
/// `transcript.rs` states the same rule for the same data.
function Banners({
  registryFailure,
  registryUnreadable,
  imported,
  onRescan,
}: {
  registryFailure: string | null;
  registryUnreadable: string[];
  imported: ReturnType<typeof useClaudeSessions>["imported"];
  onRescan: () => Promise<void>;
}) {
  const partial =
    imported.data &&
    (imported.data.unreadable_dirs.length > 0 ||
      imported.data.unreadable_files.length > 0 ||
      imported.data.write_failures.length > 0);

  return (
    <div className="shrink-0 space-y-2 p-3 pb-0">
      {/* The phone is looking at the DESKTOP's sessions, and the view
          has to say so. On `IS_MOBILE_BUILD` rather than
          `useIsMobile()`, per `SystemHealthPage`'s rule: a desktop user
          who drags their window narrow is still looking at their own
          machine. */}
      {IS_MOBILE_BUILD ? (
        <p className="text-[11px] text-[#8b949e]">
          These are the paired desktop's Claude Code sessions, not this phone's.
        </p>
      ) : null}
      {/* GREY, not amber. `NotMeasured` in `SystemHealthPage` is
          deliberately grey because "an absent reading is not a warning,
          and amber would tell the user to act on something the app
          simply did not look at". A registry we could not read is that
          exactly -- but it is stated loudly, because every row's
          liveness below is affected. */}
      {registryFailure ? (
        <div
          role="status"
          className="rounded-md border border-[#30363d] bg-[#161b22] px-3 py-2 text-xs text-[#8b949e]"
        >
          Could not tell which sessions are running — the live session registry could not be
          read ({registryFailure}). Every session below reads as “could not tell”, which is
          not the same as “not running”.
        </div>
      ) : null}
      {registryUnreadable.length > 0 ? (
        <div
          role="status"
          className="rounded-md border border-[#30363d] bg-[#161b22] px-3 py-2 text-xs text-[#8b949e]"
        >
          {registryUnreadable.length} live-session record
          {registryUnreadable.length === 1 ? "" : "s"} could not be read, so any session they
          describe reads as “could not tell”.
        </div>
      ) : null}
      {/* The rescan's own failure, separately from the list's. They are
          different questions: the rescan failing means the list may be
          INCOMPLETE, while the list failing means there is no list. */}
      {imported.isError ? (
        <div
          role="status"
          className="rounded-md border border-[#d29922]/40 bg-[#d29922]/5 px-3 py-2 text-xs text-[#d29922]"
        >
          Could not re-read the transcripts on disk ({errorMessage(imported.error)}). The
          sessions below are whatever was already stored, so newer ones may be missing.
        </div>
      ) : null}
      {partial && imported.data ? (
        <div
          role="status"
          className="rounded-md border border-[#d29922]/40 bg-[#d29922]/5 px-3 py-2 text-xs text-[#d29922]"
        >
          {imported.data.sessions.toLocaleString()} sessions read, but{" "}
          {imported.data.unreadable_dirs.length + imported.data.unreadable_files.length} could
          not be — this list is incomplete by an unknown amount.
        </div>
      ) : null}
      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={() => void onRescan()}
          disabled={imported.isFetching}
          className="tap-target flex items-center gap-1.5 rounded-md border border-[#30363d] bg-[#21262d] px-2 py-1 text-xs text-[#e6edf3] hover:bg-[#30363d] disabled:opacity-60"
        >
          <RefreshCw
            className={`h-3 w-3 ${imported.isFetching ? "animate-spin" : ""}`}
            aria-hidden="true"
          />
          {/* Three labels, not two (#978). `isFetching` is true during the
              FIRST fetch as well, so keying only on it put "Rescanning…"
              on a machine that had never scanned -- the "Re-" prefix
              asserting work that did not happen. `imported.data ===
              undefined` is what separates the two: no scan has returned
              yet, so there is nothing to re-do. */}
          {imported.isFetching
            ? imported.data === undefined
              ? "Scanning…"
              : "Rescanning…"
            : "Rescan transcripts"}
        </button>
        {/* The measurement, shown rather than only claimed -- the same
            reason `Scan` carries `elapsed_ms`: it keeps the "no
            incremental machinery" decision checkable on someone else's
            machine.

            Suppressed at zero (#978), not qualified. The figure is a
            developer-facing proof that a full rescan is affordable, and
            "0 read in 4ms" is not evidence of that -- it is the outcome of
            a scan that found nothing, offered to a first-run user as if it
            were a result. A number that measures nothing is worse than no
            number: it reads as a failed load. The house rule is qualify
            when a short read makes a figure only LOW, suppress when it
            makes it misleading (#976), and this is the second. The empty
            state below says what happened instead, in words. */}
        {imported.data && imported.data.sessions > 0 ? (
          <span className="text-[11px] text-[#8b949e]">
            {imported.data.sessions.toLocaleString()} read in {imported.data.elapsed_ms}
            ms
            {/* The unstated denominator, stated (#975). `Scan` carries
                `subagent_files_skipped` with the comment "counted so the
                exclusion is visible and testable rather than invisible",
                and it was rendered nowhere -- so a user who runs `find
                ~/.claude/projects -name '*.jsonl' | wc -l` sees 2,904 and
                the app says 1,502, with nothing on screen bridging the
                two. Measured: every excluded file is under `subagents/`,
                and the split is exactly 1,502 + 1,402.

                GREY and factual, on the same line as `elapsed_ms`, NOT in
                the amber partial-read banner. The field's own comment says
                "Not failures -- correctly excluded work", and
                `is_partial()` deliberately does not consult it. `NotMeasured`'s
                rule is that an absent reading is not a warning; this is
                not even absent, it is deliberately excluded, so it
                warrants less emphasis than grey-for-unknown rather than
                more.

                The wording says these are NOT SESSIONS, which is the
                point. #914's correction records that the naive glob
                "would list ~2x the real sessions, and every phantom row
                would offer a `--resume` handle for something that was
                never a session" -- so this must not read as "sessions
                Headstate declined to show".

                Suppressed at zero, not qualified: zero subagent files is
                the common case on a new machine and a clause reading "0
                skipped" is noise about an exclusion that did not happen.
                That is the same call the `sessions > 0` gate above makes,
                and #976's rule -- qualify when a short read makes a figure
                only LOW, suppress when it makes it misleading. */}
            {imported.data.subagent_files_skipped > 0 ? (
              <>
                {" · "}
                {imported.data.subagent_files_skipped.toLocaleString()} subagent transcript
                {imported.data.subagent_files_skipped === 1 ? "" : "s"} skipped — they are not
                sessions and cannot be resumed
              </>
            ) : null}
            {/* What the corpus costs on disk (#1135).
                Headstate reports a footprint for worktrees, artifacts,
                venvs, Docker and packages; the one corpus it reads most
                had none. The two halves stay APART because they mean
                different things -- sessions you can resume against work
                they delegated.

                Qualified when a size could not be read: a size we could
                not take is not a size of zero, so the total is a floor
                and says so. */}
            {(imported.data.session_bytes ?? 0) > 0 ? (
              <>
                {" · "}
                {(imported.data.unsized_files ?? 0) > 0 ? "at least " : ""}
                {formatSize(imported.data.session_bytes ?? 0)} of transcripts
                {(imported.data.subagent_bytes ?? 0) > 0
                  ? `, plus ${formatSize(imported.data.subagent_bytes ?? 0)} of subagent transcripts`
                  : ""}
              </>
            ) : null}
          </span>
        ) : null}
      </div>
    </div>
  );
}

/// The three liveness states, rendered as three different things.
///
/// Not a colour difference on one shape: `running` gets a filled dot,
/// `dead` a hollow one, and `unknown` a dashed grey one with different
/// words. `HealthConditions` is the pattern -- three renderings, not one
/// with a variable tint.
///
/// Grey for `unknown`, following `NotMeasured`: amber would tell the
/// user to act on something the app did not manage to look at.
function LivenessBadge({ liveness }: { liveness: Liveness }) {
  if (liveness.state === "running") {
    return (
      <span className="flex items-center gap-1 text-[#3fb950]" title={`pid ${liveness.pid}`}>
        <Circle className="h-2.5 w-2.5 fill-current" aria-hidden="true" />
        Running
        {/* Only ever beside a liveness we DERIVED ourselves. A stored
            busy/idle that a killed session never corrects is #841's
            fail-open; as a refinement of "we found the process" it is
            merely extra detail. */}
        {liveness.status ? <span className="text-[#8b949e]">· {liveness.status}</span> : null}
      </span>
    );
  }
  if (liveness.state === "dead") {
    return (
      <span className="flex items-center gap-1 text-[#8b949e]" title={liveness.why}>
        <Circle className="h-2.5 w-2.5" aria-hidden="true" />
        Not running
      </span>
    );
  }
  return (
    <span className="flex items-center gap-1 text-[#8b949e]" title={liveness.why}>
      {/* A DASHED ring, so "could not tell" is not merely a paler
          "not running". The two enable different actions. */}
      <span
        className="h-2.5 w-2.5 rounded-full border border-dashed border-[#8b949e]"
        aria-hidden="true"
      />
      Could not tell
    </span>
  );
}

/// The wall-clock time a notification was recorded, as `HH:MM`.
///
/// #1067 asks for "last seen waiting at HH:MM" in so many words, and a
/// relative time cannot carry it: `relativeTime` floors at "just now"
/// under a minute and rounds to months past thirty days, so two sessions
/// that stopped four hours apart yesterday both read "1 day ago". The
/// question the past-tense arm answers is WHEN, precisely enough to match
/// against what the reader was doing.
///
/// Local time, because the reader's own day is the frame: a UTC clock
/// would make "09:00" mean nothing to anyone west of Greenwich.
///
/// No clock read anywhere in here -- the timestamp is the argument. See
/// the page's doc comment on why that rule is absolute in this file.
function clockTime(iso: string): string {
  const at = new Date(iso);
  // An unparseable timestamp must not render as `NaN:NaN`. Returning the
  // raw string is the honest fallback: it is what we were sent, and a
  // reader can see that it is not a time rather than read a broken one as
  // one.
  if (Number.isNaN(at.getTime())) return iso;
  return `${String(at.getHours()).padStart(2, "0")}:${String(at.getMinutes()).padStart(2, "0")}`;
}

/// How to word ONE notification kind, in the present tense.
///
/// The two #1067 names are different stories and get different sentences:
/// a session sitting at an idle prompt is waiting for instructions, and a
/// session at a permission prompt is blocked on a decision about
/// something it has already decided to do. A single "Waiting" for both
/// would lose the distinction the epic exists to draw.
///
/// Anything else renders VERBATIM. There is no "other" bucket in this
/// epic: a kind Claude Code grows tomorrow shows up as its own name, so
/// the reader sees what was actually sent rather than a relabelling that
/// hides it.
function waitingPhrase(kind: string): string {
  if (kind === "permission_prompt") return "Asking permission";
  if (kind === "idle_prompt") return "Waiting for you";
  return kind;
}

/// Whether this session is waiting on the user, in the tense the evidence
/// supports (#1067).
///
/// # The staleness rule, as rendering
///
/// `ClaudeWaiting` makes the present-tense claim unconstructible without
/// a live process, and this is the other half of that: `last-seen` must
/// never be worded as though it were `now`. A dead session that stopped
/// to ask a question is worth seeing -- it is a reason to resume it --
/// but "Waiting for you" on a process that no longer exists sends the
/// user somewhere nothing is happening, which #1067 calls worse than
/// showing nothing at all.
///
/// So the two arms do not share a sentence, a tense or a colour:
///
/// | state | rendering |
/// |---|---|
/// | `now` | amber, present tense, a filled dot -- act on this |
/// | `last-seen` | muted, past tense with the clock time, `why` in the title |
/// | `no` | nothing |
///
/// Amber is the file's existing qualified/partial colour and is the
/// strongest thing here on purpose: #1067 asks for no notification, no
/// sound and no badge. This is a word on a row the user is already
/// looking at.
///
/// # Why `no` renders nothing
///
/// All three of its reasons are absences from the reader's point of view,
/// and the commonest -- `never-observed` -- is every session that ran
/// before the hook existed. An indicator that said "we were not watching
/// this one" on 1,500 rows is the noise that gets a feature ignored. The
/// reason is not discarded, it is simply not the row's business; the
/// detail pane is where a session's own silence can be explained.
function WaitingBadge({ waiting, muted = false }: { waiting: ClaudeWaiting; muted?: boolean }) {
  if (waiting.state === "no") return null;
  if (waiting.state === "now") {
    return (
      <span
        className={`flex items-center gap-1 ${muted ? "" : "text-[#d29922]"}`}
        title={`${waiting.kind} at ${clockTime(waiting.at)}`}
      >
        <Circle className="h-2.5 w-2.5 fill-current" aria-hidden="true" />
        {waitingPhrase(waiting.kind)}
      </span>
    );
  }
  // PAST TENSE, always, and muted rather than amber: this is a fact about
  // a moment that has gone, not a call to act. `why` -- the liveness
  // reason -- goes in the title so the hedge has visible grounds rather
  // than looking arbitrary.
  return (
    <span
      className={`flex items-center gap-1 ${muted ? "" : "text-[#8b949e]"}`}
      title={`${waiting.kind}: ${waiting.why}`}
    >
      <Circle className="h-2.5 w-2.5" aria-hidden="true" />
      Last seen waiting at {clockTime(waiting.at)}
    </span>
  );
}

/// The context-pressure marker, when there is one to draw (#1065).
///
/// Three states, and only one of them draws anything:
///
/// | value | rendering |
/// |---|---|
/// | `true` | "compacted repeatedly" -- muted, one word's worth of space |
/// | `false` | nothing. We watched and it did not. |
/// | `null` | nothing. No compaction was ever RECORDED for this session. |
///
/// The last two render alike here and are NOT the same thing -- which is
/// the one place this file deliberately draws two states the same way, so
/// it is worth saying why. Neither is a finding: one is "no" and the
/// other is "we were not watching", and a row is not where the difference
/// between them can be explained. What the absent-is-not-zero rule
/// forbids is rendering `null` as a MEASURED answer, and printing "no
/// compactions" on the entire pre-hook corpus would be exactly that.
/// `SessionCompactions` in the detail pane is where the distinction is
/// drawn, because there is room there to say which silence it is.
///
/// Muted, not amber. A session that compacted twice is not in trouble; it
/// is a long session, and the flag is context for a reader deciding which
/// of several sessions to open.
function ContextPressure({ pressure }: { pressure: boolean | null }) {
  if (pressure !== true) return null;
  return (
    <span className="truncate" title={`compacted automatically at least ${AUTO_COMPACT_PRESSURE} times`}>
      compacted repeatedly
    </span>
  );
}

/// What to say about the recorded directory, in one short phrase.
function cwdNote(state: CwdState): string | null {
  switch (state.state) {
    case "exists":
      return null;
    case "gone":
      return "directory gone";
    case "unknown":
      return "directory unchecked";
    case "not-recorded":
      return "no directory recorded";
  }
}

/// The search query's matches inside one field, marked (#1200).
///
/// Reads the query from the store rather than taking it as a prop: the
/// row already subscribes to the store, and threading it through every
/// caller would be a second copy of a value that is authoritative in one
/// place.
///
/// Renders ONE field. A highlight never spans two of them -- see
/// `findOverData`'s header for why a cross-field match would make the
/// highlights disagree with the count beside the search box.
function Highlight({ value }: { value: string }) {
  const query = useFilters((f) => f.claudeQuery);
  const parts = useMemo(() => segments(value, query), [value, query]);
  // No search: render the string itself rather than a single-element
  // span, so the common case adds no DOM.
  if (parts.length === 1 && !parts[0].hit) return <>{value}</>;
  return (
    <>
      {parts.map((part, i) =>
        part.hit ? (
          // `key` is the index because the segments ARE positional: two
          // runs of the same text at different offsets are different
          // segments, so text would be an unstable key.
          <mark key={i} className="rounded-sm bg-[#9e6a03] px-0.5 text-[#e6edf3]">
            {part.text}
          </mark>
        ) : (
          <span key={i}>{part.text}</span>
        ),
      )}
    </>
  );
}

function SessionEntry({
  session: s,
  now,
  active,
  cursored = false,
  onSelect,
}: {
  session: ClaudeSession;
  /// The poll's timestamp. See the page's doc comment: never
  /// `Date.now()`.
  now: number;
  active: boolean;
  /// Whether the keyboard row cursor is on this row (#953).
  ///
  /// Distinct from `active`: `active` is the SELECTION, which the detail
  /// pane is showing, and this is where the next `Enter` would land.

  /// `PrRow` draws the same distinction with the same ring, and its
  /// comment gives the reason the ring is not a background -- "a cursor
  /// that looked like a hover" is not a cursor.
  ///
  /// Defaulted, so the phone's mount point and any future caller that
  /// does not drive a cursor need not pass it.
  cursored?: boolean;
  onSelect: () => void;
}) {
  const note = cwdNote(s.cwd_state);
  return (
    <button
      type="button"
      onClick={onSelect}
      aria-current={current(active)}
      // The name is stated rather than computed from the children
      // (#1200). `Highlight` wraps matched runs in `<mark>`, which
      // splits the text into several nodes; a computed name over those
      // nodes came out EMPTY, so a screen reader announced nothing and
      // the row became unreachable by name. Search highlighting is
      // presentation and must not be able to change what the control is
      // called -- so it does not.
      aria-label={s.name ?? s.session_id}
      className={`mb-1 flex w-full flex-col items-start gap-0.5 rounded px-2 py-1.5 text-left ${
        active ? "bg-[#1f6feb] text-white" : "text-[#e6edf3] hover:bg-[#161b22]"
      } ${cursored ? "ring-2 ring-inset ring-[#1f6feb]" : ""}`}
    >
      <span className="flex w-full items-center gap-1.5">
        <Bot className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
        {/* `aiTitle`, or the id. NOT a fabricated name: the two
            transcripts in 1,438 with no title get their id, which is at
            least true and is also the resume handle. */}
        <span className="min-w-0 flex-1 truncate text-xs font-medium">
          <Highlight value={s.name ?? s.session_id} />
        </span>
      </span>
      {/* The opening ask, under the title (#1133).
          Absent entirely when there is none, rather than a placeholder:
          `null` must render as NOTHING -- never the title repeated,
          never the UUID -- because a fabricated stand-in cannot be told
          from a real prompt, which is the rule this page already states
          about the two titleless sessions above. */}
      {s.opening_prompt ? (
        <span
          className={`w-full truncate text-[11px] ${active ? "text-white/80" : "text-[#6e7681]"}`}
        >
          <Highlight value={s.opening_prompt} />
        </span>
      ) : null}
      {/* Full white on the selected row rather than `white/70`: at 12px
          on #1f6feb the dimmed variants measure under the 4.5:1
          threshold, and the row is already distinguished by the blue.
          Same finding as `ClaudeMdPage`'s FileEntry. */}
      <span
        className={`flex w-full items-center gap-1.5 text-[11px] ${
          active ? "text-white" : "text-[#8b949e]"
        }`}
      >
        <LivenessBadge liveness={s.liveness} />
        {/* A DATE on every row, not only in the detail. 147 sessions in
            the largest directory share a title with a sibling (mostly
            repeated `/security-review` runs), so the title alone cannot
            identify a row. */}
        {s.last_activity_at ? (
          <span className="truncate">· {relativeTime(s.last_activity_at, new Date(now))}</span>
        ) : (
          <span className="truncate">· no recorded activity</span>
        )}
      </span>
      {/* #1067 and #1065, on their OWN line rather than appended to the
          liveness row above. Both are conditional and usually absent, so
          sharing a line would make the row's height jump between
          neighbouring sessions -- and the waiting indicator is the one
          thing here a user scans a column of rows FOR, which a badge
          wedged after a truncating date would defeat.

          No notification, no sound, no badge that interrupts: this is a
          word on a row the user is already reading, which is exactly what
          #1067 asks for.

          `muted` on the selected row for `LivenessBadge`'s own reason,
          measured on the same blue: at 11px the amber and the grey both
          fall under 4.5:1 against #1f6feb, and the row is already
          distinguished. */}
      {s.waiting.state !== "no" || s.context_pressure === true ? (
        <span
          className={`flex w-full items-center gap-1.5 text-[11px] ${
            active ? "text-white" : "text-[#8b949e]"
          }`}
        >
          <WaitingBadge waiting={s.waiting} muted={active} />
          <ContextPressure pressure={s.context_pressure} />
        </span>
      ) : null}
      <span
        className={`w-full truncate font-mono text-[11px] ${
          active ? "text-white" : "text-[#8b949e]"
        }`}
      >
        {s.cwd === null ? "no directory recorded" : <Highlight value={s.cwd} />}
        {note ? ` · ${note}` : ""}
      </span>
    </button>
  );
}

/// One session: where it ran, whether it is alive, and how to get it
/// back.
///
/// # Where the second read happens (#985)
///
/// The row arrives in the list; everything else about this session is
/// fetched here, for this session only. The list used to carry the
/// resume command, the transcript path and its stat, the version, the
/// start time and the run count for all 1,474 rows on every ten-second
/// poll, to render them for one -- 65% of a 1.35 MB payload, and the
/// phone paid it over the pairing transport.
///
/// # Three arms, because a failed read is not an empty one
///
/// | condition | rendering |
/// |---|---|
/// | still reading | the fields the LIST already knows, and a note that the rest is coming |
/// | the read was REJECTED | `QueryError` with the reason, and a retry |
/// | resolved `null` | the session is gone from the store, and says so |
///
/// The first arm is why this does not render a spinner over the whole
/// pane: the title, the liveness and the directory are already in hand
/// from the list, and blanking them for a round-trip would make the
/// pane flicker on every selection. It shows what it knows and says
/// what it is waiting for.
///
/// The second and third must not be collapsed, which is #846's rule: a
/// rejected read means the database could not be read, and `null` means
/// this id is not in it. Opposite remedies, and only one of them is
/// about the session.
function SessionDetail({
  session: s,
  now,
}: {
  session: ClaudeSession;
  now: number;
}) {
  const detail = useClaudeSessionDetail(s.session_id, true);
  const copy = (value: string, what: string) => {
    void copyText(value).then((failure) =>
      failure === null
        ? toast.success(`${what} copied to the clipboard`)
        : toast.error(`Could not copy the ${what.toLowerCase()}`, { description: failure }),
    );
  };
  /// Whether a terminal is configured (#1126).
  ///
  /// Empty is the default: the buttons copy and no launch affordance
  /// appears, which is the behaviour every build had until now. Never
  /// on the phone -- `claude_launch_session` is `Class::Local`, so the
  /// remote surface refuses it and the button would only ever error.
  const { prefs } = useUiPrefs();
  const terminalConfigured =
    !IS_MOBILE_BUILD && (prefs?.terminal_command ?? "").trim() !== "";

  /// Open a session's resume command in the configured terminal.
  ///
  /// Offers copy as the remedy rather than silently falling back to it:
  /// a launch that quietly copied instead would leave the user watching
  /// for a terminal that never opens.
  const launchResume = (sessionId: string, cwd: string | null) => {
    void claudeLaunchSession(sessionId, cwd).then(
      () => toast.success("Opening the session in your terminal"),
      (e: unknown) =>
        toast.error("Could not open your terminal", {
          description: errorMessage(e),
          action: {
            label: "Copy instead",
            onClick: () =>
              detail.data ? copy(detail.data.resume.command, "Resume command") : undefined,
          },
        }),
    );
  };

  const reveal = (path: string, what: string) => {
    void claudeRevealPath(path).then(
      (shown) => toast.success(`Revealed ${shown}`),
      (e: unknown) =>
        // NAMES the reason. Revealing a deleted worktree silently opens
        // the home directory on macOS, which looks like the button
        // misfired rather than like the directory being one of the 84%
        // that are gone.
        toast.error(`Could not reveal the ${what}`, { description: errorMessage(e) }),
    );
  };

  return (
    <div className="flex flex-col gap-4">
      {/* ONCE, here, with whatever the detail read has so far. Rendering
          it again inside `SessionBody` would put two copies of the
          liveness reason on screen -- caught by
          `a crashed session states the crash`, which found two matches
          for one sentence. */}
      <SessionHeading session={s} now={now} detail={detail.data ?? undefined} />
      {detail.isError ? (
        <QueryError
          title="No detail for this session"
          message="The rest of this session's detail could not be read, so the resume command and the transcript are not shown."
          onRetry={() => void detail.refetch()}
        />
      ) : detail.data === null ? (
        // Resolved, and the store does not have it. A session deleted
        // between two polls -- distinct from the arm above, which is the
        // read itself failing.
        <p className="text-xs text-[#8b949e]">
          This session is no longer in the store, so there is nothing more to show about it.
        </p>
      ) : detail.data === undefined ? (
        <p className="text-xs text-[#8b949e]">Reading the rest of this session…</p>
      ) : (
        <>
          <SessionBody
            session={s}
            detail={detail.data}
            copy={copy}
            reveal={reveal}
            launchResume={launchResume}
            terminalConfigured={terminalConfigured}
          />
          {/* Directly after "where it ran", because it answers the
              question a reader asks next: did this session ship
              anything. Headstate knew about pull requests and knew about
              sessions and the two never met, while the transcripts
              carried the join key all along (#1132). */}
          <SessionPullRequests detail={detail.data} />
          {/* Both BELOW "Where it ran" and above the worktree jump, which is
              the order the questions are asked in: what is this, how much was
              it, what was it saying, and where do I go next. The preview is
              last of the two because it is the one that costs a read. */}
          <SessionUsage detail={detail.data} />
          {/* Directly after the token figures, because it is the same
              question in the unit a reader actually budgets in -- and
              because the two must be seen to come from DIFFERENT places.
              The tokens above are summed here, per message. The figure
              below was computed by Claude Code and is transcribed. Nothing
              in this app multiplies one into the other (#1210). */}
          <SessionCost detail={detail.data} />
          {/* Right after the token figures, because it answers the same
              question from the other side: the tokens say how much work
              happened, and this says what that volume cost the session in
              context (#1065). */}
          <SessionCompactions detail={detail.data} />
          {/* Directly after "how much work IT did", because the question
              this answers is the same one one level down: how much work
              happened UNDERNEATH it. Adjacent so the two figures can be
              read against each other, and separate so neither is mistaken
              for the other (#1002). */}
          <SessionSubagents detail={detail.data} />
          {/* After the two "how much" sections and before the transcript:
              the questions run what is this, how much was it, what went
              WRONG in it, and what was it saying. The failure profile is
              the one a user acts on, so it sits above the preview rather
              than below it. */}
          <SessionTrouble sessionId={s.session_id} />
          {/* LAST of the action sections and directly above the
              transcript, which is the order the decision is made in:
              read what went wrong, see what it last said, then decide.
              Rendering it higher would put an end-this button above the
              evidence for it, which is the arrangement #1219 exists to
              avoid (#1219). */}
          <StopSession session={s} detail={detail.data} />
          <TranscriptPreview detail={detail.data} />
        </>
      )}
      {/* OUTSIDE the detail gate: the jump is derived from `cwd` and
          `git_branch`, both of which the list row already carries, so a
          detail read that failed must not also cost the user the one
          action that never depended on it. */}
      <WorktreeJump session={s} />
    </div>
  );
}

/// Jump from a session to the worktree it ran in (#920).
///
/// # Why this is the link that makes the feature part of the app
///
/// "What was this session doing" is usually answered by the worktree's
/// own state -- whether the branch merged, whether there is uncommitted
/// work, whether it is safe to remove -- and that state already has a
/// view. So the session detail states the verdict and offers the jump,
/// rather than reproducing the Worktrees page inside itself.
///
/// # No jump is offered unless one actually matches
///
/// A minority of sessions match a registered worktree, and the ones that
/// do are overwhelmingly the ones whose directory still exists. The rest
/// are agent worktrees deleted when their work landed, and for those there
/// is genuinely nothing to jump to. A button that navigated to a list where
/// the row is absent would be worse than no button, so the section renders
/// the reason instead.
///
/// The exact rates are deliberately NOT stated here. They were ("206 of
/// 1,461 -- 14.1% of all sessions, 83.1% of the 248 whose directory still
/// exists"), and they drifted: a figure measured once is correct on the day
/// it is written and decays from then on (#969). The share is also
/// per-machine -- a property of how the reader works, not of this code --
/// so a number here describes the author's laptop rather than the reader's.
///
/// What the design rests on is the RULE, not the rate: a match requires the
/// directory to still exist, so a session whose cwd is gone can never have
/// one. That is a property of `sessionWorktree`, and `worktrees.test.ts`
/// asserts it by measuring a corpus rather than by remembering a number.
///
/// # Three absences, three renderings
///
/// | condition | rendering |
/// |---|---|
/// | the worktree listing could not be read | says so. NOT "no worktree". |
/// | it loaded and nothing matched | says the directory is not a worktree Headstate knows |
/// | a worktree matched | the verdict, the branch caveat, and the jump |
///
/// The first two are the absent-is-not-zero rule: a failed scan and a
/// successful scan that found nothing have opposite remedies, and only
/// the second licenses "this is not a worktree".
function WorktreeJump({ session: s }: { session: ClaudeSession }) {
  const setView = useFilters((f) => f.setView);
  const setFilter = useFilters((f) => f.setFilter);
  // The same query the Worktrees page and the sidebar use, so opening a
  // session detail costs a cache hit rather than a second scan. `enabled`
  // is left at its default true for the reason `useWorktrees`' own
  // comment gives: the three callers that discover repositories all want
  // this, and this is now a fourth.
  //
  // `unreadable` since #951: a "no match" verdict is a claim about the
  // WHOLE scan, so an incomplete one cannot support it. This is the
  // residual shape `caches/mod.rs` refuses a deletion for -- unlike the
  // orphan count on `WorktreesPage`, which is a positive per-path finding
  // -- so here the verdict really is qualified rather than merely
  // annotated. Nothing is deleted on the strength of it, so it is said in
  // prose rather than gated.
  const { data: repos, isError, error, unreadable = [] } = useWorktrees();
  const match = sessionWorktree(s.cwd, s.git_branch, repos);

  return (
    <section className="rounded-md border border-[#30363d] bg-[#161b22] p-3">
      <h3 className="text-xs font-semibold text-[#e6edf3]">Its worktree</h3>
      {isError ? (
        // A failed scan is NOT "no worktree" (#846). The remedies differ:
        // one is "retry or check the configured directories", the other
        // is "this directory was never a worktree".
        <p className="mt-2 text-xs text-[#8b949e]">
          Could not read the worktree list, so whether this session ran in one is unknown
          {errorMessage(error) ? ` (${errorMessage(error)})` : ""}.
        </p>
      ) : repos === undefined ? (
        <p className="mt-2 text-xs text-[#8b949e]">Looking for a matching worktree…</p>
      ) : match === null ? (
        <p className="mt-2 text-xs text-[#8b949e]">
          {s.cwd === null
            ? // Nothing to do with the scan: there is no directory to
              // match, so a short scan changes nothing about this answer.
              "No directory was recorded for this session, so there is no worktree to find."
            : unreadable.length > 0
              ? // "Not a worktree we know about" is a claim over the whole
                // scan, and the scan came back short (#951). The honest
                // answer names that rather than converting a gap in the
                // walk into a fact about this directory.
                `This directory did not match any worktree Headstate could read — and ${unreadable.length} path${unreadable.length === 1 ? "" : "s"} could not be read, so it may be one of them rather than not a worktree at all.`
              : "This directory is not a worktree Headstate knows about — most agent worktrees are deleted once their work lands."}
        </p>
      ) : (
        <>
          <dl className="mt-2 space-y-1.5 text-xs">
            <Field label="Repository">{match.repoName}</Field>
            <Field label="Worktree">
              <span className="break-all font-mono">{pathBasename(match.worktree.path)}</span>
            </Field>
            <Field label="On branch">
              <span className="break-all font-mono">{match.worktree.branch}</span>
            </Field>
            {/* The verdict is the ANSWER to "what was this session doing"
                -- merged, dirty, safe to remove. `safetyReason` is the
                same prose the Worktrees page shows, so the two cannot
                disagree about the same tree. */}
            <Field label="State">{safetyReason(match.worktree.safety)}</Field>
          </dl>
          {/* MEASURED: the recorded branch disagrees with the current one
              on 54 of 206 matches (26.2%), because a main checkout
              accumulates sessions across every branch it held. Saying so
              is what stops the row above reading as "this session's
              branch". */}
          {match.movedOnFrom !== null ? (
            <p className="mt-2 text-xs text-[#d29922]">
              This session recorded the branch{" "}
              <span className="font-mono">{match.movedOnFrom}</span>, but the worktree has since
              moved to <span className="font-mono">{match.worktree.branch}</span>.
            </p>
          ) : null}
          <button
            type="button"
            onClick={() => {
              // `setView` FIRST, and the order is load-bearing.
              //
              // `setFilter` writes into `filtersByView[state.view]` --
              // the CURRENT view -- so calling it before the switch files
              // the repo under `claude-code`, where nothing reads it, and
              // the Worktrees page opens on its default repository
              // instead. A test asserting only `view` would not have
              // noticed; `the jump navigates the way WorktreesPage reads
              // it` asserts both and caught exactly this.
              //
              // Safe in this order because `setView` clears only the
              // selection state (`selectedPr`, `checked`, `cursor`), never
              // `filtersByView`.
              setView("worktrees");
              setFilter("repo", match.repoPath);
            }}
            className="tap-target mt-3 flex items-center gap-1.5 rounded-md border border-[#30363d] bg-[#21262d] px-2 py-1 text-xs text-[#e6edf3] hover:bg-[#30363d]"
          >
            <GitBranch className="h-3 w-3" aria-hidden="true" />
            Show in Worktrees
          </button>
        </>
      )}
    </section>
  );
}

/// The heading: the title, the liveness and the two dates.
///
/// Split out of [`SessionBody`] for #985. Every field here comes from the
/// LIST row, except `first_seen_at` -- so this renders immediately on
/// selection rather than after the detail round-trip, which is what keeps
/// the pane from flickering each time the user picks a row.
///
/// "Started" is therefore conditional: it is the detail's, and an absent
/// detail must leave the line out rather than print a fabricated or
/// zeroed date beside two real ones (#846).
function SessionHeading({
  session: s,
  now,
  detail,
}: {
  session: ClaudeSession;
  now: number;
  detail?: ClaudeSessionDetail;
}) {
  return (
    <div>
      <h2 className="text-sm font-semibold text-[#e6edf3]">{s.name ?? s.session_id}</h2>
      <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-[#8b949e]">
        <LivenessBadge liveness={s.liveness} />
        {s.last_activity_at ? (
          <span>Last active {relativeTime(s.last_activity_at, new Date(now))}</span>
        ) : (
          <span>No recorded activity</span>
        )}
        {detail ? <span>Started {relativeTime(detail.first_seen_at, new Date(now))}</span> : null}
        {/* From the DETAIL, not the row (#1067). Both halves carry a
            `waiting`, and the detail's is derived against the same
            liveness read this heading's own badge is showing -- so the
            pane cannot say "waiting for you" beside a "Not running" it
            derived a moment later. Absent until the detail arrives, which
            is the same rule "Started" follows one line up: a tense
            asserted from a stale half is the exact staleness #1067 is
            about. */}
        {detail ? <WaitingBadge waiting={detail.waiting} /> : null}
      </div>
      {/* The REASON, for the two states that have one. A "not running"
          established by an orphaned registry entry is a crash and says
          so; a "could not tell" says what stopped us. Hiding either
          leaves the user with a verdict and no grounds.

          From the LIST row, whose `why` is the interned sentence
          resolved in `hydrateClaudeSessions` -- the same string, so this
          reads exactly as it did before #985. */}
      {s.liveness.state !== "running" ? (
        <p className="mt-1.5 text-xs text-[#8b949e]">{s.liveness.why}</p>
      ) : null}
    </div>
  );
}

/// The session detail's own fields, split from [`SessionDetail`] so the
/// worktree section can sit beside them without this function growing a
/// second concern.
///
/// Takes BOTH halves since #985: `session` is the list row and `detail`
/// is what was fetched for it. Each field reads from whichever half
/// actually carries it, so there is one place to check that the split
/// lost nothing.
function SessionBody({
  session: s,
  detail: d,
  copy,
  reveal,
  launchResume,
  terminalConfigured,
}: {
  session: ClaudeSession;
  detail: ClaudeSessionDetail;
  copy: (value: string, what: string) => void;
  reveal: (path: string, what: string) => void;
  /// Open the resume command in the configured terminal (#1126).
  launchResume: (sessionId: string, cwd: string | null) => void;
  /// Whether a terminal is configured, which decides whether the
  /// primary button launches or copies.
  terminalConfigured: boolean;
}) {
  // A fragment, not a wrapper: `SessionDetail` owns the column gap so
  // that `WorktreeJump` is spaced from these sections by the same rule
  // they are spaced from each other.
  return (
    <>
      <Resume
        session={s}
        detail={d}
        onCopy={copy}
        onLaunch={launchResume}
        terminalConfigured={terminalConfigured}
      />

      <section className="rounded-md border border-[#30363d] bg-[#161b22] p-3">
        <h3 className="text-xs font-semibold text-[#e6edf3]">Where it ran</h3>
        <dl className="mt-2 space-y-1.5 text-xs">
          <Field label="Directory">
            <span className="break-all font-mono">{s.cwd ?? "not recorded"}</span>
            {cwdNote(s.cwd_state) ? (
              <span className="ml-1.5 rounded-full bg-[#21262d] px-2 py-0.5 text-[11px] text-[#8b949e]">
                {cwdNote(s.cwd_state)}
              </span>
            ) : null}
          </Field>
          {s.git_branch ? (
            <Field label="Branch">
              <span className="break-all font-mono">{s.git_branch}</span>
            </Field>
          ) : null}
          {d.claude_version ? <Field label="Claude">{d.claude_version}</Field> : null}
          <Field label="Session id">
            <span className="break-all font-mono">{s.session_id}</span>
          </Field>
          {/* `runs: 0` is the whole imported corpus, and saying so is
              what distinguishes "we never watched this process" from
              "we watched it and it ended" -- which is also the
              difference between two liveness answers. */}
          <Field label="Observed runs">
            {d.runs === 0
              ? "none — this session was read from its transcript, not watched while it ran"
              : d.runs.toLocaleString()}
          </Field>
        </dl>
        <div className="mt-3 flex flex-wrap gap-2">
          <button
            type="button"
            onClick={() => copy(s.session_id, "Session id")}
            className="tap-target rounded-md border border-[#30363d] bg-[#21262d] px-2 py-1 text-xs text-[#e6edf3] hover:bg-[#30363d]"
          >
            Copy session id
          </button>
          {/* Behind `IS_MOBILE_BUILD`, per `surfaceGuard.test.ts`:
              `claude_reveal_path` is `Class::Local`, so the phone would
              render a control that can only reject. */}
          {!IS_MOBILE_BUILD ? (
            <RevealButton
              label="Reveal directory"
              path={s.cwd}
              state={s.cwd_state}
              what="directory"
              onReveal={reveal}
            />
          ) : null}
          {!IS_MOBILE_BUILD ? (
            <RevealButton
              label="Reveal transcript"
              path={d.transcript_path}
              /* The TRANSCRIPT's own state, not the cwd's (#919). These
                 were the same expression until the corpus was measured
                 and the overwhelming majority of rows turned out to have
                 a dead cwd and a live transcript -- so a shared reading
                 disables the button that works on almost every row.

                 The count that was here (measured at the time: ~83%) is
                 gone rather than updated. It is per-machine and it decays
                 (#969), and the decision does not rest on its value: any
                 material disagreement between the two states is enough,
                 and re-measuring only ever strengthened it. */
              state={d.transcript_state}
              what="transcript"
              onReveal={reveal}
            />
          ) : null}
        </div>
      </section>
    </>
  );
}


/// How much work happened inside this session (#959).
///
/// # Why this section exists after #910 cut it
///
/// #910's UI design cut tokens on "not in the data I verified", which was
/// correct on the evidence it had and false in fact: `usage` is on
/// `assistant.message`, on 1,478 of 1,502 real transcripts (98.4%).
/// `claude/usage.rs` carries the re-measurement.
///
/// It earns its space on #921's own test -- "does this help me see what is
/// going on, or resurrect something?" -- because the spread is the useful
/// part. Measured on four real sessions: 994 assistant messages against 4,
/// and 405 million cache-read tokens against 111 thousand. Both render
/// today as a title, a path and a relative time, and nothing distinguishes
/// the session worth resuming from the typo.
///
/// # Tokens, never a dollar figure
///
/// A cost needs per-model rates, those rates change, and this app cannot
/// keep a hardcoded table true. A quietly stale cost with a currency
/// symbol in front of it is the confident-wrong-answer failure #941 is
/// about, dressed to look authoritative. `cost-state` carries a real
/// `totalCostUSD` -- and on 43 of 1,502 sessions (2.9%), so a panel built
/// on it would appear on 43 rows and vanish on 1,459.
///
/// # Four absences, four renderings
///
/// | condition | rendering |
/// |---|---|
/// | no transcript path on the row | says so, and why: nothing to read |
/// | the read failed | the reason. NOT zeros. |
/// | it is still reading | says so |
/// | read, and NO usage found | "this transcript records no token usage" -- not four zeros |
///
/// The last is the absent-is-not-zero rule with a number on it: 24 of
/// 1,502 real transcripts carry no usage block, and rendering 0 for those
/// states a measurement that was never taken. `Usage::observed()` is the
/// gate, and `Tile`'s `value: number | null` one page over is the same
/// pattern.
///
/// The error arm is BEFORE the empty arm, per #846: `data` is undefined on
/// a rejection exactly as it is before the first read, so an error arm
/// placed after would never render in the case it exists for.
function SessionUsage({ detail: d }: { detail: ClaudeSessionDetail }) {
  // The transcript's OWN state, never the cwd's (#919): 1,213 of 1,461
  // rows have a dead cwd and a live transcript, so a reading gated on the
  // cwd would be absent on almost every row.
  //
  // Both readings come from the DETAIL since #985 -- the transcript path
  // and its stat left the list row together, so there is no way for this
  // to pair a path with someone else's state.
  const readable = d.transcript_path !== null && d.transcript_state.state !== "gone";
  const { data, isError, error, isLoading } = useClaudeSessionUsage(
    readable ? d.transcript_path : null,
  );

  return (
    <section className="rounded-md border border-[#30363d] bg-[#161b22] p-3">
      <h3 className="text-xs font-semibold text-[#e6edf3]">How much work it did</h3>
      {d.transcript_path === null ? (
        <p className="mt-2 text-xs text-[#8b949e]">
          No transcript was recorded for this session, so there is nothing to read this from.
        </p>
      ) : d.transcript_state.state === "gone" ? (
        <p className="mt-2 text-xs text-[#8b949e]">
          Its transcript is no longer on disk, so how much work it did cannot be read.
        </p>
      ) : isError ? (
        // NOT zeros (#846). A failed read and a session that used nothing
        // have opposite remedies, and the second is a claim this cannot
        // make.
        <p className="mt-2 text-xs text-[#8b949e]">
          Could not read its transcript, so how much work it did is unknown
          {errorMessage(error) ? ` (${errorMessage(error)})` : ""}.
        </p>
      ) : isLoading || data === undefined ? (
        <p className="mt-2 text-xs text-[#8b949e]">Reading its transcript…</p>
      ) : data.messages === 0 ? (
        // 24 of 1,502 real transcripts. Four zeros here would be a
        // measurement that was never taken, with a credible shape.
        /* The "that is unusual" clause originally named the measured
           ratio, and #969's guard (`measuredFigures.test.ts`) caught it
           on the merge: a corpus count rendered as a STRING is correct on
           the day it is written and decays from then on, and on someone
           else's machine it describes the author's. That this is the rare
           case is what the reader needs; the figure behind it lives in
           `claude/usage.rs`'s module docs, where it is a historical
           observation about a design decision rather than a claim about
           the machine it is printed on. */
        <p className="mt-2 text-xs text-[#8b949e]">
          Its transcript records no token usage, so there is nothing to total. That is unusual —
          nearly every transcript carries it.
        </p>
      ) : (
        <>
          <dl className="mt-2 space-y-1.5 text-xs">
            <Field label="Assistant messages">{data.messages.toLocaleString()}</Field>
            {/* Four counters, never one total. Cache reads run two to
                three orders of magnitude above fresh input on every real
                session measured, so a single summed "tokens" figure would
                be a cache-read count wearing a misleading name. */}
            <Field label="Output tokens">{data.output_tokens.toLocaleString()}</Field>
            <Field label="Input tokens">{data.input_tokens.toLocaleString()}</Field>
            <Field label="Cache read">{data.cache_read_tokens.toLocaleString()}</Field>
            <Field label="Cache written">{data.cache_creation_tokens.toLocaleString()}</Field>
            {/* `model` is per-MESSAGE and the corpus is mixed -- 12,512
                opus-5 against 912 opus-4-7 across 13,425 sampled messages
                -- so "which model was this session" has no single answer
                and this states the real one rather than picking. */}
            {data.models.length > 0 ? (
              <Field label={data.models.length === 1 ? "Model" : "Models"}>
                {data.models
                  .map((m) =>
                    data.models.length === 1
                      ? m.model
                      : `${m.model} (${m.messages.toLocaleString()})`,
                  )
                  .join(", ")}
              </Field>
            ) : null}
          </dl>
          {/* A short read, STATED. Without this the reader cannot tell a
              complete sum from one that stopped early, which is the #846
              defect with a number on it.

              Since #1086 a SELECTED session reads whole, so the 8 MB cap
              can no longer draw this line -- `BUDGET_BYTES` still bounds
              the bulk paths, but not this one. What remains is the one
              case that survives an uncapped read: a transcript that
              SHRANK between being sized and being read. Rare, and the
              reason this stays rather than being deleted with the cap --
              a partial sum has to say so however it came to be partial.

              The old sentence "Reading it whole would hang this pane" is
              gone with the cap: it was never true (160 ms measured for the
              largest transcript in the corpus) and it now describes
              something the code does not do. */}
          {data.truncated ? (
            <p className="mt-2 text-xs text-[#d29922]">
              These are floors, not totals: the transcript measured{" "}
              {formatMb(data.file_bytes)} and only {formatMb(data.bytes_read)} could be read.
            </p>
          ) : null}
        </>
      )}
    </section>
  );
}

/// How often this session compacted, and on whose initiative (#1065).
///
/// # Five absences, five renderings
///
/// `SessionUsage` above is the pattern this follows, one field along:
///
/// | condition | rendering |
/// |---|---|
/// | no compaction was ever RECORDED | says so, AND that the hook may not have been installed |
/// | recorded, and the total is zero | "it never compacted" -- a measured answer |
/// | manual and auto, split | the two counts, apart |
/// | a trigger we do not recognise | its own name, verbatim |
/// | a record with no trigger at all | counted separately from an unknown value |
///
/// The first row is the one that matters, and it is the state of EVERY
/// session that already exists: the hook writes `claude_hook_event` rows
/// from the moment it is installed and nothing backfills the sessions
/// that ran before it. So `null` is the overwhelming default, and drawing
/// it as "0 compactions" would put a measured-looking zero on the entire
/// corpus. That is the root `CLAUDE.md` rule with a number on it, and
/// #846 is the same defect one view over.
///
/// It is also not enough to say "none recorded" and stop: a reader who
/// does not know a hook is involved reads that as "this session never
/// compacted", which is the confident wrong answer in a quieter voice.
/// So the sentence names the reason it might be silent.
///
/// The second row is the distinction the first exists to protect. Once
/// the hook IS installed, `total === 0` is a real measurement -- we were
/// watching and nothing happened -- and it gets its own, differently
/// worded, sentence.
///
/// # Unknown triggers render as themselves
///
/// `manual` and `auto` are the two documented values. Anything else is
/// kept verbatim in `unknown` and printed under its own name: a future
/// trigger called `emergency` appears as `emergency`. It is never folded
/// into `auto` -- which would overstate the pressure figure -- and never
/// relabelled "other", which would hide from the reader that the
/// vocabulary has grown. `untriggered` is counted apart again, because a
/// record whose `trigger` field is missing says the payload SHAPE moved,
/// which is a different problem with a different fix.
function SessionCompactions({ detail: d }: { detail: ClaudeSessionDetail }) {
  const c = d.compactions;
  return (
    <section className="rounded-md border border-[#30363d] bg-[#161b22] p-3">
      <h3 className="text-xs font-semibold text-[#e6edf3]">How often it compacted</h3>
      {c === null ? (
        // ABSENT, not zero. The second sentence is load-bearing: without
        // it this reads as "it never compacted", which is a measurement
        // nobody took.
        <p className="mt-2 text-xs text-[#8b949e]">
          No compaction has been recorded for this session, so how often it compacted is unknown.
          The hook that records them may not have been installed when this session ran, which is
          the case for every session that predates it.
        </p>
      ) : compactionTotal(c) === 0 ? (
        // MEASURED zero, and worded so it cannot be mistaken for the arm
        // above. We were watching; nothing happened.
        <p className="mt-2 text-xs text-[#8b949e]">
          This session never compacted: compactions were recorded for it and there were none.
        </p>
      ) : (
        <>
          <dl className="mt-2 space-y-1.5 text-xs">
            {/* Split, never summed. A user who compacts by hand has made
                a choice; a session that compacts automatically has hit a
                wall. One total would answer neither question. */}
            <Field label="Automatic">{c.auto.toLocaleString()}</Field>
            <Field label="Manual">{c.manual.toLocaleString()}</Field>
            {/* Verbatim, under the trigger's own name. Never "other". */}
            {c.unknown.map(([trigger, n]) => (
              <Field key={trigger} label={trigger}>
                {n.toLocaleString()}
              </Field>
            ))}
            {c.untriggered > 0 ? (
              // A record we READ whose `trigger` was absent -- the
              // payload shape moved, rather than the vocabulary growing.
              // Counted apart from an unknown value for that reason.
              <Field label="No trigger recorded">{c.untriggered.toLocaleString()}</Field>
            ) : null}
          </dl>
          {c.auto >= AUTO_COMPACT_PRESSURE ? (
            /* The same threshold the row's marker uses, from the same
               constant -- `mirroredConstants.test.ts` pins it to the Rust
               side that actually sets `context_pressure`, so the badge and
               this sentence cannot describe different rules.

               Amber and a sentence, not a warning: a session that
               compacted automatically several times was working against
               its context window, which is context for a reader, not a
               fault to fix. */
            <p className="mt-2 text-xs text-[#d29922]">
              It compacted automatically {c.auto.toLocaleString()} times, so it was working
              against its context window rather than simply being long.
            </p>
          ) : null}
        </>
      )}
    </section>
  );
}

/// Every compaction counted, whatever its trigger.
///
/// The TypeScript twin of `Compactions::total`, and it sums the unknown
/// and untriggered ones too: they ARE compactions, and a total that
/// quietly omitted them would be short by an amount the reader cannot
/// see. That matters most for the gate above -- a session whose only
/// compactions had an unrecognised trigger would otherwise render as
/// "this session never compacted" while holding records that say
/// otherwise.
function compactionTotal(c: ClaudeCompactions): number {
  return (
    c.manual + c.auto + c.untriggered + c.unknown.reduce((sum, [, n]) => sum + n, 0)
  );
}

/// What the hook says this session's subagents WERE (#1066).
///
/// "2 general-purpose, 1 code-reviewer" is the whole point of the field:
/// the directory rule can count children and cannot name them, and this
/// is the sentence that turns "3 subagents" into three identifiable ones.
///
/// # Three silences, and only one of them is a finding
///
/// Renders nothing when `types` is `null` -- neither source has anything
/// to say -- and nothing when `stated` is empty and the hook saw no
/// untyped spawns either. That second case is the ordinary pre-hook
/// session, and the section's existing count is already the right answer
/// for it; a line reading "no types recorded" would turn the normal case
/// into a reported deficiency. What it DOES say, when the hook observed
/// spawns it could not name, is how many were untyped -- the hook fired
/// and named nothing, which is a fact about the payload and not a
/// guessable type.
///
/// Type names are printed VERBATIM. A type this app has never heard of
/// renders as itself; there is no "other" bucket.
function AgentTypeBreakdown({ types }: { types: ClaudeAgentTypes | null }) {
  if (types === null) return null;
  if (types.stated.length === 0 && types.untyped === 0) {
    // The pre-hook session: children found by their directories, and no
    // hook record to name them. Stated as the ordinary thing it is --
    // the grouping HAS a source, and saying which one is what stops this
    // reading as a gap.
    if (types.inferred_children === 0) return null;
    return (
      <p className="mt-2 text-xs text-[#8b949e]">
        They were grouped by their working directories rather than by the hook, so what kind of
        agent each one was is not recorded.
      </p>
    );
  }
  return (
    <p className="mt-2 text-xs text-[#8b949e]">
      The hook recorded{" "}
      {types.stated
        .map(([type, n]) => `${n.toLocaleString()} ${type}`)
        .concat(
          // Counted, never guessed at, and never folded into a named
          // bucket: a blank rendered as a type would be a fabricated
          // answer.
          types.untyped > 0
            ? [`${types.untyped.toLocaleString()} with no type recorded`]
            : [],
        )
        .join(", ")}
      .
    </p>
  );
}

/// #1066's disagreement, when there is one.
///
/// Prominent but NOT alarming, which is the balance the issue asks for.
/// Amber -- this file's qualified/partial colour, the same one
/// `WorktreeJump` uses for "the branch moved on" -- rather than a red
/// error panel, because nothing is broken: a session reported spawning
/// subagents and the directory rule did not find them, which is a finding
/// about the INFERENCE, not a failure of this session.
///
/// The sentence itself carries both numbers -- how many starts the hook
/// recorded, and that the rule found none of them -- so this renders it
/// and adds nothing. Restating them here would be a second copy of a
/// claim that has to agree with the Rust one word for word.
function SubagentDisagreement({ note }: { note: string | null }) {
  if (note === null) return null;
  return <p className="mt-2 text-xs text-[#d29922]">{note}</p>;
}

/// This session's subagents, and what they cost (#1002).
///
/// # Three audiences, one section
///
/// A session is in exactly one of three states here, and the section says
/// which:
///
/// | state | what it shows |
/// |---|---|
/// | has attributed subagents | how many, and their tokens as a SEPARATE figure |
/// | is a subagent with a known parent | which session spawned it |
/// | is a subagent nobody could be traced for | that we looked, and why we could not tell |
///
/// A session that is neither renders nothing at all: the overwhelming
/// majority of rows have no subagents and are not one, and a section
/// saying "this has no subagents" on 1,100 rows is noise.
///
/// # The tokens are NEVER added to the parent's own
///
/// #959 kept four counters rather than one because cache reads run two to
/// three orders of magnitude above fresh input. The same argument applies
/// one level up: a parent's own tokens and its children's answer different
/// questions, and one summed figure would answer neither -- a parent that
/// delegated everything would show a large number describing work it did
/// not do, indistinguishable from one that did the work itself. So this is
/// its own section with its own heading, beside "How much work it did" and
/// never inside it.
///
/// # Absent is not zero
///
/// `measured === 0` with subagents present means we could not total them,
/// and it renders as "could not tell" rather than as four zeros. A child
/// whose transcript could not be read contributes nothing to the sums, so
/// a non-empty `unreadable` makes every figure a floor and the section
/// says so. `caches/mod.rs:550`'s rule, one level up from
/// `Usage::observed()`.
///
/// The error arm is BEFORE the loading/empty arms per #846: `data` is
/// undefined on a rejection exactly as it is before the first read.
///
/// # What the hook adds, and the one case where it contradicts (#1066)
///
/// `d.subagents` is what the DIRECTORY RULE found, and it stays the
/// section's spine: it is the only source for every session that exists
/// today. `d.agent_types` is the hook's account of the same session, and
/// it answers a question the directory rule cannot -- an `agent-<hex>`
/// cwd yields an opaque id, never "this was a code-reviewer". So the two
/// are laid out as ONE section rather than two: same subagents, two
/// things known about them.
///
/// Three combinations, and none of them is an error:
///
/// | `stated` | `inferred_children` | rendering |
/// |---|---|---|
/// | empty | > 0 | the normal pre-hook session -- say the grouping came from the layout |
/// | non-empty | > 0 | the breakdown, beside the inferred count |
/// | non-empty | 0 | `subagentDisagreement`'s note, prominent but not alarming |
///
/// The first must not read as a failure. It is what every session that
/// predates the install looks like, and it is the case in which the hook
/// simply has nothing to say -- so the copy names the directory layout as
/// the source rather than reporting a missing type.
///
/// The third is #1066's finding, and the rule that only ONE direction is
/// a disagreement lives in `src/lib/subagentDisagreement.ts` with its
/// argument. It is computed there rather than inline here for the reason
/// the file gives: Rust has the same logic as a METHOD, which is never
/// serialised, so this is a re-derivation that has to be testable against
/// the Rust one rather than a second rule hidden in JSX.
function SessionSubagents({ detail: d }: { detail: ClaudeSessionDetail }) {
  // #1066's finding. Computed before the branch below, because a session
  // the hook saw spawns for but the directory rule attributed no children
  // to has `d.subagents.length === 0` -- it takes the non-parent path,
  // and that path is precisely where this must be said. Rendering it only
  // under `isParent` would hide the note in exactly the case it exists
  // for.
  const disagreement = subagentDisagreement(d.agent_types);
  const isParent = d.subagents.length > 0;
  // Only asked when there is something to roll up. A session with no
  // attributed children must not issue a query that resolves to zeros --
  // the zeros would be true and the section would still be noise.
  const { data, isError, error, isLoading } = useClaudeSubagentRollup(
    isParent ? d.session_id : null,
  );

  // A subagent's own view: who spawned it, or why that could not be told.
  if (!isParent) {
    if (d.kind.kind !== "subagent") {
      // Not a subagent and no attributed children -- so ordinarily
      // nothing to say, which is the majority of rows. The exception is
      // #1066's disagreement: the hook watched this session START
      // subagents and the directory rule found none of them, and that is
      // a finding about a session with no children rather than about a
      // parent. Silence here would drop the note on the one shape it
      // exists to report.
      if (disagreement === null) return null;
      return (
        <section className="rounded-md border border-[#30363d] bg-[#161b22] p-3">
          <h3 className="text-xs font-semibold text-[#e6edf3]">What its subagents did</h3>
          <SubagentDisagreement note={disagreement} />
        </section>
      );
    }
    return (
      <section className="rounded-md border border-[#30363d] bg-[#161b22] p-3">
        <h3 className="text-xs font-semibold text-[#e6edf3]">What ran this</h3>
        {d.parent !== null ? (
          <dl className="mt-2 space-y-1.5 text-xs">
            <Field label="Spawned by">{d.parent.name ?? d.parent.session_id}</Field>
            <Field label="Agent">{d.kind.agent_id}</Field>
          </dl>
        ) : (
          /* Unattributed, and SAYING SO with the evidence. Never a
             probable parent: a wrong rollup is worse than no rollup, and
             the sentence carries which sessions tied and when so the
             reader can see the app looked rather than shrugged. */
          <>
            <p className="mt-2 text-xs text-[#8b949e]">
              This ran in an agent worktree, but which session started it could not be told.
              {d.unattributed ? ` ${d.unattributed}.` : ""}
            </p>
            <dl className="mt-2 space-y-1.5 text-xs">
              <Field label="Agent">{d.kind.agent_id}</Field>
            </dl>
          </>
        )}
      </section>
    );
  }

  return (
    <section className="rounded-md border border-[#30363d] bg-[#161b22] p-3">
      <h3 className="text-xs font-semibold text-[#e6edf3]">What its subagents did</h3>
      <p className="mt-2 text-xs text-[#8b949e]">
        {d.subagents.length.toLocaleString()} subagent session
        {d.subagents.length === 1 ? "" : "s"} ran under this one. They are hidden from the list
        by default and are still resumable on their own.
      </p>
      {/* What the hook knows about those same subagents (#1066). BESIDE
          the count above rather than replacing it: the count is the
          directory rule's, which is the only source for every session
          that predates the install, and nothing here overrides it. */}
      <AgentTypeBreakdown types={d.agent_types} />
      <SubagentDisagreement note={disagreement} />
      {isError ? (
        // NOT zeros (#846). A failed read and subagents that used nothing
        // have opposite remedies, and the second is a claim this cannot
        // make.
        <p className="mt-2 text-xs text-[#8b949e]">
          Could not total what they used
          {errorMessage(error) ? ` (${errorMessage(error)})` : ""}.
        </p>
      ) : isLoading || data === undefined ? (
        <p className="mt-2 text-xs text-[#8b949e]">Reading their transcripts…</p>
      ) : !data.measured ? (
        /* Has children, totalled none of them. Four zeros here would be a
           measurement that was never taken. */
        <p className="mt-2 text-xs text-[#8b949e]">
          None of their transcripts could be totalled, so how much they used is unknown.
        </p>
      ) : (
        <>
          <dl className="mt-2 space-y-1.5 text-xs">
            {/* Four counters, never one total, and never added into the
                parent's own -- see this component's doc comment. */}
            <Field label="Assistant messages">{data.messages.toLocaleString()}</Field>
            <Field label="Output tokens">{data.output_tokens.toLocaleString()}</Field>
            <Field label="Input tokens">{data.input_tokens.toLocaleString()}</Field>
            <Field label="Cache read">{data.cache_read_tokens.toLocaleString()}</Field>
            <Field label="Cache written">{data.cache_creation_tokens.toLocaleString()}</Field>
          </dl>
          {/* The denominator, whenever it is not the whole set. Without it
              a sum over 3 of 12 children reads exactly like a sum over all
              12 -- the #846 defect with a number on it. */}
          {data.measured < data.sessions ? (
            <p className="mt-2 text-xs text-[#d29922]">
              These cover {data.measured.toLocaleString()} of{" "}
              {data.sessions.toLocaleString()} subagent sessions
              {data.without_usage > 0
                ? `; ${data.without_usage.toLocaleString()} recorded no token usage`
                : ""}
              {data.unreadable.length > 0
                ? `; ${data.unreadable.length.toLocaleString()} could not be read`
                : ""}
              , so they are floors rather than totals.
            </p>
          ) : data.truncated > 0 ? (
            /* The 8 MB cap, stated. Binds on the handful of very large
               transcripts and on nothing else, which is exactly why it
               must be there when it does. */
            <p className="mt-2 text-xs text-[#d29922]">
              These are floors, not totals: {data.truncated.toLocaleString()} of their
              transcripts were too large to read whole.
            </p>
          ) : null}
        </>
      )}
    </section>
  );
}

/// What Claude Code recorded this session cost (#1210).
///
/// # Transcribed, never computed
///
/// `usage.rs:33-41` rules out a dollar figure this app DERIVES, and that
/// argument is untouched: no rate table ships, nothing here multiplies a
/// token count by anything. What it never ruled out is reading a number
/// the vendor already computed. Claude Code writes a `cost-state` record
/// carrying `totalCostUSD`, measured while the session ran. Rendering it
/// is the same act as rendering `output_tokens` one section up.
///
/// The distinction only survives if the LABEL carries it, which is why
/// the figure is never shown as "Cost". It is shown as **"as recorded by
/// Claude Code"**, in the visible text and not merely in this comment,
/// because a reader who cannot tell a transcribed figure from a derived
/// one has been given the more dangerous of the two by default.
///
/// # A session panel, and deliberately nothing wider
///
/// No tile, no chart, no corpus total. The record is on a minority of
/// sessions lifetime, and a sum over that minority would be read as a sum
/// over all of them however the denominator was printed beside it -- the
/// confident-wrong-answer failure at aggregate scale rather than on one
/// row. The measured coverage lives in `claude/usage.rs`'s module docs,
/// where a figure that decays is a historical note about a decision
/// rather than a claim printed at a user (#969).
///
/// # Five absences, five renderings
///
/// `SessionUsage` above is the pattern, one question along:
///
/// | condition | rendering |
/// |---|---|
/// | no transcript path on the row | says so, and why: nothing to read |
/// | the read failed | the reason. NOT `$0.00`. |
/// | it is still reading | says so |
/// | read, and NO `cost-state` record | "Claude Code did not record a cost for this session." |
/// | recorded, `hasUnknownModelCost` | the figure, labelled a FLOOR |
///
/// The fourth row is the whole point and is the #846 rule with a currency
/// symbol on it. `$0.00` here would assert that the vendor measured
/// nothing spent -- a confident wrong answer made MORE credible by the
/// attribution standing next to it. The sentence is a fact about Claude
/// Code's RECORDING, never about the session's spend, because this app
/// knows the first and cannot know the second.
///
/// The fifth is the one nobody has hit. `hasUnknownModelCost` is `false`
/// on every record measured, so this arm ships unexercised by any real
/// transcript and is handled anyway -- `ToolVersion::CannotTell` exists on
/// exactly that argument. When it is set the recorded figure omits an
/// unknown model's spend, so calling it a total would understate it by an
/// unknown amount, and it is called a floor instead.
///
/// The error arm is BEFORE the absent arm, per #846: `data` is undefined
/// on a rejection exactly as it is before the first read, so an error arm
/// placed after would never render in the case it exists for.
function SessionCost({ detail: d }: { detail: ClaudeSessionDetail }) {
  // The same reading, the same key, the same query as `SessionUsage`.
  // React Query dedupes on the key, so this is one read of the transcript
  // rendered in two places rather than two reads -- which matters, since
  // the largest real transcript is 76.7 MB.
  const readable = d.transcript_path !== null && d.transcript_state.state !== "gone";
  const { data, isError, error, isLoading } = useClaudeSessionUsage(
    readable ? d.transcript_path : null,
  );
  const cost = data?.recorded_cost ?? null;
  const retryMs = retryMillis(cost);

  return (
    <section className="rounded-md border border-[#30363d] bg-[#161b22] p-3">
      <h3 className="text-xs font-semibold text-[#e6edf3]">What Claude Code recorded it cost</h3>
      {/* Each sentence below names WHAT is unknown -- "what it cost", not
          a bare "cannot be read". The token section one up renders the
          same four absences about a different question, and two
          identically worded paragraphs would leave a reader unable to
          tell which figure was missing. */}
      {d.transcript_path === null ? (
        <p className="mt-2 text-xs text-[#8b949e]">
          This session has no transcript, so there is nothing to read a recorded cost from.
        </p>
      ) : d.transcript_state.state === "gone" ? (
        <p className="mt-2 text-xs text-[#8b949e]">
          Its transcript is no longer on disk, so what Claude Code recorded it cost cannot be read.
        </p>
      ) : isError ? (
        // NOT $0.00 (#846). A failed read and a session Claude Code
        // recorded nothing for are different facts, and neither of them
        // is "it cost nothing".
        <p className="mt-2 text-xs text-[#8b949e]">
          Its transcript could not be read, so what Claude Code recorded it cost is unknown
          {errorMessage(error) ? ` (${errorMessage(error)})` : ""}.
        </p>
      ) : isLoading || data === undefined ? (
        <p className="mt-2 text-xs text-[#8b949e]">Reading its transcript for a recorded cost…</p>
      ) : cost === null ? (
        // The majority of sessions. A fact about the RECORDING, never
        // about the spend: this app knows what Claude Code wrote down and
        // cannot know what the session cost. The second sentence exists
        // so a reader who does not know a vendor record is involved does
        // not read the first as "it was free".
        <p className="mt-2 text-xs text-[#8b949e]">
          Claude Code did not record a cost for this session. Claude Code only began writing this
          figure into transcripts recently, so older sessions carry no record of it — which is not
          the same as having cost nothing.
        </p>
      ) : (
        <>
          <dl className="mt-2 space-y-1.5 text-xs">
            {/* The attribution is IN THE LABEL, not in a footnote and not
                only in the comment above. "Cost" alone would read as a
                figure this app stands behind, and it is not one -- it is
                Claude Code's measurement, quoted. */}
            <Field
              label={
                cost.has_unknown_model_cost
                  ? "At least, as recorded by Claude Code"
                  : "As recorded by Claude Code"
              }
            >
              {formatUsd(cost.total_cost_usd)}
            </Field>
            {/* The split is the vendor's own `modelUsage`, costliest
                first. Nothing is apportioned here: each figure is that
                model's `costUSD` as written. */}
            {cost.models.length > 0 ? (
              <Field label={cost.models.length === 1 ? "Model" : "By model"}>
                {cost.models.map((m) => `${m.model} ${formatUsd(m.cost_usd)}`).join(", ")}
              </Field>
            ) : null}
            {/* `totalAPIDuration` minus `totalAPIDurationWithoutRetries`:
                time lost to retries, which nothing else in this app shows
                and which is a direct "is this going badly" signal.
                Rendered only when the subtraction makes sense -- see
                `retryMillis`. */}
            {retryMs !== null ? (
              <Field label="Time lost to retries">{formatRetry(retryMs)}</Field>
            ) : null}
          </dl>
          {cost.has_unknown_model_cost ? (
            // The untested arm, stated rather than assumed away. The
            // recorded figure omits a model Claude Code had no cost for,
            // so it is a floor and the real spend is that or more.
            <p className="mt-2 text-xs text-[#d29922]">
              That is a floor, not a total: Claude Code recorded that it met a model it had no cost
              for, so whatever that model cost is missing from the figure above.
            </p>
          ) : null}
        </>
      )}
    </section>
  );
}

/// Time lost to retries, or `null` when the record cannot support the
/// subtraction (#1210).
///
/// `totalAPIDuration` minus `totalAPIDurationWithoutRetries`. This app
/// does not own the invariant between the two fields — Claude Code writes
/// them — so a pair that disagrees the wrong way round yields `null` and
/// the row simply does not render. Clamping to zero would state "no time
/// was lost to retries" about a record that does not make sense, which is
/// the confident wrong answer in its quietest form.
///
/// Exported to nothing: it exists as a named function rather than an
/// inline expression so the guard above is one thing with one test,
/// rather than a subtraction that gets simplified back into a clamp by
/// the next person who reads it.
function retryMillis(cost: ClaudeCostState | null): number | null {
  if (cost === null) return null;
  const lost = cost.total_api_ms - cost.total_api_without_retries_ms;
  return lost >= 0 ? lost : null;
}

/// A transcribed dollar figure, to the cent.
///
/// Two decimal places, because the question is "what did this cost" and
/// the recorded value carries seven (`1.3242615`) — digits that state a
/// precision the reader has no use for and that make the figure look
/// derived rather than quoted.
///
/// Sub-cent totals are the exception and keep their precision: a session
/// that recorded `0.001186` would round to `$0.00`, which is the exact
/// string this whole section exists to never print. `$0.0012` is small
/// and true; `$0.00` is a claim that nothing was spent.
function formatUsd(usd: number): string {
  if (usd > 0 && usd < 0.005) return `$${usd.toFixed(4)}`;
  return `$${usd.toFixed(2)}`;
}

/// Milliseconds of retry time, in the largest unit that keeps it legible.
///
/// The measured values run from tens of milliseconds to whole minutes on
/// a bad session, and "68 ms" and "2.3 min" are both answers a reader can
/// act on where a raw millisecond count is not.
function formatRetry(ms: number): string {
  if (ms < 1000) return `${ms.toLocaleString()} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  return `${(ms / 60_000).toFixed(1)} min`;
}

/// Bytes as MB, for the two truncation labels.
///
/// One decimal place, because the figures it renders are 8.0 and 76.7 and
/// the difference between them is the whole point of the sentence.
function formatMb(bytes: number): string {
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/// The last few exchanges of this session's transcript (#982).
///
/// # Why reading beats revealing, and why the phone is the stronger case
///
/// Until now the only action touching a transcript was Reveal in Finder,
/// which is `Class::Local` and hands the user a 176 KB JSONL file --
/// double-clicking which opens nothing useful on a default macOS install.
/// And on a phone `claude_reveal_path` is unreachable by construction, so
/// a companion user who could see that a session died could not see one
/// word of what it was doing.
///
/// The question it answers is "is this the right session": 286 of 1,438
/// sessions share a title with another, and 147 do inside the largest
/// directory. The titles are not enough and the last exchange is.
///
/// # Behind a disclosure, not open by default
///
/// A 256 KB read per selection, over the pairing transport on the phone,
/// for a pane the user may not want. `enabled` on the query is what makes
/// the button an opt-in rather than a lazy render of something already
/// fetched.
///
/// # It is NOT the resume path
///
/// The primary action is whatever the Resume section's is -- a copy, or
/// the configured terminal once one is set (#1126). It is not decided
/// HERE: this pane is for deciding, not for doing, and `Resume` owns
/// that button. Nothing is guessed either way, which is the part of
/// `claudify_command`'s reasoning that still holds.
/// What the hook recorded about this session's failures and denials
/// (#1062, #1063, #1064).
///
/// # The four conditions, and why none may render as another
///
/// | condition | rendering |
/// |---|---|
/// | the read failed | the reason. NOT zeros. |
/// | still reading | says so |
/// | `unobserved` | "not recorded", and why — NEVER "0 failures" |
/// | observed | the counts, a zero among them being a real zero |
///
/// The third row is the one this section exists to get right, and it is
/// the house rule (#846) in its most convincing disguise. A session that
/// ran before the hooks were installed has no records, and "0 failures"
/// for it is a sentence that looks like good news and is actually a
/// measurement nobody took. On a machine that adopted Headstate after
/// using Claude Code that is EVERY historical session.
///
/// The error arm is before the loading arm, per #846: `data` is undefined
/// on a rejection exactly as it is before the first read, so an error arm
/// placed after would never render in the case it exists for.
///
/// # Denials are not failures
///
/// #1064 is explicit that a denial is a guardrail working rather than
/// something going wrong, and the wording here must not imply otherwise.
/// They get their own heading, their own count and neutral verbs — "auto
/// mode declined", never "blocked" or "failed".
function SessionTrouble({ sessionId }: { sessionId: string }) {
  const { data, isError, error, isLoading } = useClaudeSessionEvents(sessionId);

  return (
    <section className="rounded-md border border-[#30363d] bg-[#161b22] p-3">
      <h3 className="text-xs font-semibold text-[#e6edf3]">What went wrong in it</h3>
      {isError ? (
        // NOT zeros. A failed read and a clean session have opposite
        // remedies, and the second is a claim this cannot make.
        <p className="mt-2 text-xs text-[#8b949e]">
          Could not read what the hook recorded, so failures and denials for this session are
          unknown
          {errorMessage(error) ? ` (${errorMessage(error)})` : ""}.
        </p>
      ) : isLoading || data === undefined ? (
        <p className="mt-2 text-xs text-[#8b949e]">Reading what the hook recorded…</p>
      ) : data.state === "unobserved" ? (
        // The absent-is-not-zero arm. It says what is true — nobody was
        // watching — and names the remedy, rather than showing a zero
        // that would read as "this session was clean".
        <p className="mt-2 text-xs text-[#8b949e]">
          Not recorded. No hook was watching this session, so whether anything failed or was
          declined is unknown — this is normal for sessions that ran before the hooks were
          installed.
        </p>
      ) : (
        <TroubleProfile observation={data} />
      )}
    </section>
  );
}

/// The measured half of [`SessionTrouble`]: a profile that really was
/// recorded.
///
/// Split out so the caller's guard chain stays readable, and so the
/// partial-install banner has one place to live.
function TroubleProfile({ observation }: { observation: ClaudeObservation }) {
  if (observation.state === "unobserved") return null;
  const p: ClaudeProfile = observation.profile;
  const floor = observation.state === "partial";
  const concentrated = concentratedTool(p.tool_failures);

  return (
    <>
      {floor ? (
        /* A half install. The counts below are a FLOOR and saying so is
           the difference between a number and a wrong number. `missing`
           names the event, because "reinstall the hooks" is only
           actionable if the reader knows what is not being recorded —
           the same reason `install::Status::Stale` carries a sentence. */
        <p className="mt-2 text-xs text-[#d29922]">
          At least these — {observation.missing.join(" and ")} {observation.missing.length === 1
            ? "is"
            : "are"}{" "}
          not installed, so anything it would have recorded is missing. Reinstall the hooks to
          record all of it.
        </p>
      ) : null}
      {p.turn_failures.length === 0 && p.tool_failures.length === 0 && p.denials.length === 0 ? (
        /* A real, MEASURED zero, and it is allowed to read as good news
           precisely because the unobserved case above never reaches
           here. */
        <p className="mt-2 text-xs text-[#8b949e]">
          {floor
            ? "Nothing was recorded by the hooks that are installed."
            : "Nothing failed and nothing was declined while this session was being watched."}
        </p>
      ) : null}

      {p.turn_failures.length > 0 ? (
        <TallyList
          label="Turns that died"
          hint="Why the turn ended. A recurring rate limit is a different problem from a recurring overload, and only the first is one you can pace around."
          tallies={p.turn_failures}
        />
      ) : null}

      {p.tool_failures.length > 0 ? (
        <TallyList
          label="Tool failures"
          hint={
            concentrated !== null
              ? `Concentrated in ${concentrated}, which is the shape worth looking at.`
              : undefined
          }
          tallies={p.tool_failures}
        />
      ) : null}

      {p.denials.length > 0 ? (
        /* NOT an error, and the wording carries that (#1064). A denial is
           auto mode doing the job it was asked to do; presenting it as
           damage would teach the user to switch the guardrail off. */
        <TallyList
          label="Declined by auto mode"
          hint="Auto mode refused these tool calls. That is the guardrail working — worth a look only if something you meant to allow is on the list."
          tallies={p.denials}
        />
      ) : null}
    </>
  );
}

/// One breakdown: a label, an optional sentence, and the named tallies.
///
/// `name` is rendered VERBATIM and is never mapped to a friendlier label.
/// An `error_type` or tool name this build has never seen — an MCP
/// server's tool, or an error type a newer Claude Code added — must render
/// as itself, because it is the only thing the record carries (#1062,
/// #1063).
function TallyList({
  label,
  hint,
  tallies,
}: {
  label: string;
  hint?: string;
  tallies: ClaudeTally[];
}) {
  return (
    <div className="mt-3">
      <h4 className="text-xs font-semibold text-[#e6edf3]">{label}</h4>
      {hint !== undefined ? <p className="mt-0.5 text-xs text-[#8b949e]">{hint}</p> : null}
      <dl className="mt-1.5 space-y-1.5 text-xs">
        {tallies.map((t) => (
          <Field key={`${label}:${t.name ?? "\u0000"}`} label={t.name ?? "Not recorded"}>
            <span>{t.count.toLocaleString()}</span>
            {/* Untrusted vendor text, rendered as text by React and never
                interpolated into anything else. One example rather than
                every message: the question is what KIND of thing is going
                wrong. */}
            {t.detail !== null ? (
              <span className="ml-2 break-all text-[#8b949e]">{t.detail}</span>
            ) : null}
          </Field>
        ))}
      </dl>
    </div>
  );
}

/// The tool holding a strict majority of the failures, when there is one.
///
/// Mirrors `Profile::concentrated_tool` on the Rust side, and the
/// thresholds are the same for the same reasons: a STRICT majority
/// because a 2/2/1 split has no story, and more than one failure because
/// a single failure is trivially 100% concentrated and pointing at it
/// would make the signal constant.
///
/// Computed here rather than sent, because it is a presentation choice
/// over data the client already holds.
function concentratedTool(tallies: ClaudeTally[]): string | null {
  const total = tallies.reduce((n, t) => n + t.count, 0);
  if (total < 2) return null;
  const top = tallies.find((t) => t.count * 2 > total && t.name !== null);
  return top?.name ?? null;
}

/// How long a session has been up, in the terms a reader thinks in.
///
/// `null` in, `null` out: an uptime we could not read is not "0s", which
/// is the same absent-is-not-zero rule the rest of this page follows.
function uptimeLabel(secs: number | null): string | null {
  if (secs === null) return null;
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m`;
  return `${secs}s`;
}

/// Stop a live session, proposed with its evidence (#1219).
///
/// # Why this exists at all
///
/// Headstate already tells the user this session auto-compacted
/// repeatedly, that its tools are failing in a concentrated pattern
/// (`SessionTrouble`, directly above), that it has waited for hours
/// (`WaitingBadge`), and that the machine is oversubscribed (the System
/// Health page). It surfaced every input to "should I stop this" and then
/// made the user go and find, in Activity Monitor, a pid Headstate
/// already holds.
///
/// # This is an indicator, not advice
///
/// `health::runaway`'s Notice-vs-Alert split is the rule: a stuck session
/// is an INDICATOR. So this section states FACTS -- how long it has run,
/// how often it auto-compacted, what it last said -- and the button is
/// secondary-styled and plainly worded. Nothing here says the session
/// should be stopped, and nothing here acts unattended: the proposal is
/// fetched on an explicit click, and the stop needs a second one.
///
/// # Only for a session that is RUNNING, and only on the desktop
///
/// A session that is `dead` has nothing to stop, and one whose liveness
/// is `unknown` is precisely the case where signalling would be a guess
/// -- so neither gets the affordance, and `unknown` says why rather than
/// rendering nothing. `claude_stop_session` is `Class::Local`, so the
/// phone is behind `IS_MOBILE_BUILD` with a sentence in its place.
function StopSession({
  session: s,
  detail: d,
}: {
  session: ClaudeSession;
  detail: ClaudeSessionDetail;
}) {
  // Every hook unconditionally, before any early return.
  const [proposal, setProposal] = useState<ClaudeStopProposal | null>(null);
  const [busy, setBusy] = useState(false);

  // The DETAIL's liveness, not the row's: the row's was derived on the
  // list's poll and is up to ten seconds old, and this section is about
  // a process that may have exited in that window.
  const live = d.liveness;

  const propose = () => {
    setBusy(true);
    void claudeProposeStop([s.session_id]).then(
      (rows) => {
        setBusy(false);
        setProposal(rows[0] ?? null);
      },
      (e: unknown) => {
        setBusy(false);
        toast.error("Could not look at this session", { description: errorMessage(e) });
      },
    );
  };

  const stop = () => {
    setBusy(true);
    // The SESSION ID, never the pid shown above. Rust re-derives the pid
    // on that call and refuses if the start times disagree -- the number
    // on screen is stale the instant it is rendered.
    void claudeStopSession(s.session_id).then(
      (out) => {
        setBusy(false);
        setProposal(null);
        toast.success(
          out.signal === "terminated"
            ? `Session stopped — it took SIGTERM after ${Math.round(out.waited_ms / 100) / 10}s, so it wrote its transcript`
            : `Session killed — it did not exit within the grace period, so SIGKILL followed and no end record was written`,
        );
      },
      (e: unknown) =>
        // NAMES the refusal. The pid-reuse refusal in particular is the
        // most useful thing this feature can say, and a generic "could
        // not stop" would throw it away.
        toast.error("Nothing was signalled", { description: errorMessage(e) }),
    );
  };

  if (IS_MOBILE_BUILD) {
    if (live.state !== "running") return null;
    return (
      <section className="rounded-md border border-[#30363d] bg-[#161b22] p-3">
        <h3 className="text-xs font-semibold text-[#e6edf3]">Stopping this session</h3>
        <p className="mt-1.5 text-xs text-[#8b949e]">
          A session can only be stopped from the Mac it is running on, so this is not available
          here.
        </p>
      </section>
    );
  }

  if (live.state === "dead") return null;

  if (live.state === "unknown") {
    return (
      <section className="rounded-md border border-[#30363d] bg-[#161b22] p-3">
        <h3 className="text-xs font-semibold text-[#e6edf3]">Stopping this session</h3>
        {/* Says why rather than showing nothing: "could not tell" is the
            one state in which signalling would be a guess, and a user who
            expected the button needs to know it was withheld deliberately
            rather than missing. */}
        <p className="mt-1.5 text-xs text-[#8b949e]">
          Whether this session is running could not be confirmed, so nothing can be signalled
          without guessing at which process is meant. {live.why}
        </p>
      </section>
    );
  }

  const ev = proposal?.evidence;
  const uptime = uptimeLabel(ev?.uptime_secs ?? null);

  return (
    <section className="rounded-md border border-[#30363d] bg-[#161b22] p-3">
      <h3 className="text-xs font-semibold text-[#e6edf3]">Stopping this session</h3>
      <p className="mt-1.5 text-xs text-[#8b949e]">
        This session is running as pid {live.pid}. Headstate does not recommend stopping it — the
        figures above describe it, and the decision is yours.
      </p>

      {proposal === null ? (
        <button
          type="button"
          disabled={busy}
          onClick={propose}
          className="tap-target mt-3 rounded-md border border-[#30363d] bg-[#21262d] px-2 py-1 text-xs text-[#e6edf3] hover:bg-[#30363d] disabled:opacity-60"
        >
          {busy ? "Looking…" : "Review stopping it"}
        </button>
      ) : proposal.action !== "proposed" ? (
        <>
          {/* A REFUSAL, shown as one. The pid-reuse case is the reason
              this feature re-derives the pid, and hiding the refusal
              would leave the user with a button that appeared to do
              nothing. */}
          <p className="mt-3 text-xs text-[#f85149]">{proposal.why}</p>
          <button
            type="button"
            onClick={() => setProposal(null)}
            className="tap-target mt-3 rounded-md border border-[#30363d] bg-[#21262d] px-2 py-1 text-xs text-[#e6edf3] hover:bg-[#30363d]"
          >
            Close
          </button>
        </>
      ) : (
        <>
          <dl className="mt-3 space-y-1.5 text-xs">
            <Field label="Process">
              <span className="font-mono">pid {proposal.pid}</span>
              {/* Advisory, and labelled as such: `status` is STORED by
                  the session and a killed one never corrects it. */}
              {ev?.status ? (
                <span className="ml-1.5 rounded-full bg-[#21262d] px-2 py-0.5 text-[11px] text-[#8b949e]">
                  it last published {ev.status}
                </span>
              ) : null}
            </Field>
            <Field label="Running for">
              {uptime ?? "could not be read"}
            </Field>
            {/* Absent is not zero: `null` is "no compaction record
                exists for this session", which is not "it never
                compacted" (#1065). */}
            <Field label="Auto-compactions">
              {ev?.auto_compactions === null || ev?.auto_compactions === undefined
                ? "no compaction record — not the same as none"
                : ev.auto_compactions.toLocaleString()}
            </Field>
          </dl>

          {/* WHAT IT LAST SAID, above the button. The issue's
              requirement, and the reason this is a proposal rather than
              a confirmation dialog: a user asked to end something must
              be shown what they are ending. */}
          <div className="mt-3">
            <h4 className="text-xs font-semibold text-[#e6edf3]">What it last said</h4>
            {ev?.last_turn ? (
              <p className="mt-1 whitespace-pre-wrap break-words rounded-md bg-[#0d1117] p-2 text-xs text-[#c9d1d9]">
                {ev.last_turn}
              </p>
            ) : (
              <p className="mt-1 text-xs text-[#8b949e]">
                Its transcript could not be read, so there is nothing to show here — which is not
                the same as the session having said nothing.
              </p>
            )}
          </div>

          <p className="mt-3 text-xs text-[#8b949e]">
            Stopping sends SIGTERM first, so the session can write its transcript and record that
            it ended. Only if it has not exited after a few seconds does SIGKILL follow, and that
            leaves no end record.
          </p>

          <div className="mt-3 flex flex-wrap gap-2">
            <button
              type="button"
              disabled={busy}
              onClick={stop}
              className="tap-target rounded-md border border-[#30363d] bg-[#21262d] px-2 py-1 text-xs text-[#e6edf3] hover:bg-[#30363d] disabled:opacity-60"
            >
              {busy ? "Stopping…" : "Stop this session"}
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() => setProposal(null)}
              className="tap-target rounded-md border border-[#30363d] bg-[#21262d] px-2 py-1 text-xs text-[#8b949e] hover:bg-[#30363d] disabled:opacity-60"
            >
              Leave it running
            </button>
          </div>
        </>
      )}
    </section>
  );
}

function TranscriptPreview({ detail: d }: { detail: ClaudeSessionDetail }) {
  const [open, setOpen] = useState(false);
  // `transcript_state`, never `cwd_state` (#919, and
  // `ClaudeSessionDetail`'s own doc): 0% of transcripts are gone against
  // 83% of cwds. Both live on the detail since #985.
  const refusal = revealRefusal(d.transcript_path, d.transcript_state);
  const { data, isError, error, isLoading } = useClaudeTranscriptTail(
    d.transcript_path,
    open && refusal === null,
  );

  return (
    <section className="rounded-md border border-[#30363d] bg-[#161b22] p-3">
      <h3 className="text-xs font-semibold text-[#e6edf3]">What it was doing</h3>
      {/* The refusal is NAMED, with the tri-state's three distinct
          wordings rather than one shared shrug -- `revealRefusal`'s doc
          argues at length why collapsing `gone` and `unknown` destroys
          the point of the third state. Reused rather than re-worded, so
          this pane and the Reveal transcript button cannot come to
          disagree about the same file.

          Prefixed with what THIS control cannot do, because on the
          desktop the disabled Reveal button states the same refusal a few
          lines up, and two identical sentences side by side read as two
          separate failures -- the rule `ClaudeCodePage`'s own error arm
          and `WorktreeJump` both follow. The refusal clause itself stays
          verbatim, so the distinction between `gone` and `unknown`
          survives the prefix. */}
      {refusal !== null ? (
        <p className="mt-2 text-xs text-[#8b949e]">
          There is nothing to read here: {refusal}.
        </p>
      ) : !open ? (
        <>
          <p className="mt-2 text-xs text-[#8b949e]">
            The last few exchanges, to check this is the session you meant before resuming it.
          </p>
          <button
            type="button"
            onClick={() => setOpen(true)}
            className="tap-target mt-3 flex items-center gap-1.5 rounded-md border border-[#30363d] bg-[#21262d] px-2 py-1 text-xs text-[#e6edf3] hover:bg-[#30363d]"
          >
            <Terminal className="h-3 w-3" aria-hidden="true" />
            Read the transcript
          </button>
        </>
      ) : isError ? (
        // BEFORE the empty arm (#846): `data` is undefined on a rejection
        // exactly as it is before the first read.
        <p className="mt-2 text-xs text-[#8b949e]">
          Could not read its transcript
          {errorMessage(error) ? ` (${errorMessage(error)})` : ""}. This is not the same as the
          session having said nothing.
        </p>
      ) : isLoading || data === undefined ? (
        <p className="mt-2 text-xs text-[#8b949e]">Reading its transcript…</p>
      ) : data.messages.length === 0 ? (
        // Read, and there was no conversation in the window. The counts
        // say which kind of nothing it was, because "300 machinery
        // records" and "an empty file" are different facts.
        <p className="mt-2 text-xs text-[#8b949e]">
          {data.non_conversation_records > 0 || data.unparseable_records > 0
            ? `No conversation in the last ${formatKb(data.bytes_read)} — ${data.non_conversation_records.toLocaleString()} bookkeeping record${data.non_conversation_records === 1 ? "" : "s"} and ${data.unparseable_records.toLocaleString()} that could not be read.`
            : "Its transcript holds no conversation to show."}
        </p>
      ) : (
        <>
          {/* The cap, STATED, and this is where it matters most: a reader
              who cannot tell a short conversation from a truncated one has
              been told something false by omission. #910's own words asked
              for "a 'showing the last N lines of a large file' label", and
              the 39 real files over 1 MB are where it binds. */}
          <p className="mt-2 text-xs text-[#8b949e]">
            {data.truncated
              ? `The last ${data.messages.length.toLocaleString()} message${data.messages.length === 1 ? "" : "s"}, from the final ${formatKb(data.bytes_read)} of a ${formatKb(data.file_bytes)} transcript. Earlier exchanges are not shown.`
              : `All ${data.messages.length.toLocaleString()} message${data.messages.length === 1 ? "" : "s"} in this transcript.`}
            {data.non_conversation_records > 0
              ? ` ${data.non_conversation_records.toLocaleString()} bookkeeping record${data.non_conversation_records === 1 ? "" : "s"} in that window are not conversation and are not shown.`
              : ""}
          </p>
          <ol className="mt-3 space-y-2">
            {data.messages.map((m, i) => (
              <li
                // The index is the key on purpose: transcript records
                // carry no stable id this reads, and two identical
                // messages in a row are a real thing a session does. The
                // list is never reordered or filtered, so the index IS the
                // identity here.
                key={i}
                className="rounded border border-[#30363d] bg-[#0d1117] p-2"
              >
                <div className="flex flex-wrap items-center gap-x-2 text-[11px] text-[#8b949e]">
                  <span className="font-semibold text-[#e6edf3]">
                    {m.role === "assistant" ? "Claude" : "You"}
                  </span>
                  {m.model ? <span>{m.model}</span> : null}
                </div>
                <div className="mt-1 space-y-1">
                  {m.blocks.length === 0 ? (
                    <p className="text-xs text-[#6e7681]">(nothing in this message)</p>
                  ) : (
                    m.blocks.map((b, j) => <PreviewBlock key={j} block={b} />)
                  )}
                </div>
              </li>
            ))}
          </ol>
        </>
      )}
    </section>
  );
}

/// Bytes as KB or MB, whichever reads better.
///
/// The figures here span 256 KB windows and 76 MB files, and "78,586 KB"
/// is not a sentence anybody reads.
function formatKb(bytes: number): string {
  return bytes >= 1024 * 1024
    ? `${(bytes / (1024 * 1024)).toFixed(1)} MB`
    : `${Math.round(bytes / 1024).toLocaleString()} KB`;
}

/// One content block of a previewed message.
///
/// Five kinds, five renderings, because they answer different questions:
/// text is what was said, a tool call is what was DONE, and a tool result
/// is usually far too long to show whole. Flattening them into prose is
/// how 12,903 of 13,425 assistant messages would have rendered as nothing
/// (they stop on `tool_use`).
function PreviewBlock({ block: b }: { block: ClaudePreviewBlock }) {
  switch (b.kind) {
    case "text":
      return (
        <p className="whitespace-pre-wrap break-words text-xs text-[#e6edf3]">
          {b.text}
          {b.truncated ? <span className="text-[#8b949e]"> … (clipped)</span> : null}
        </p>
      );
    case "thinking":
      // Dimmed rather than hidden: 195 of 1,500 sampled blocks are
      // thinking, and a reader scanning for "what was it doing" wants it
      // out of the way but not gone.
      return (
        <p className="whitespace-pre-wrap break-words text-xs italic text-[#6e7681]">
          {b.text}
          {b.truncated ? " … (clipped)" : ""}
        </p>
      );
    case "tool_use":
      // The NAME, not the arguments: "Read" tells the reader what the
      // session was doing and a 40 KB argument blob does not.
      return (
        <p className="text-xs text-[#8b949e]">
          Ran <span className="font-mono text-[#e6edf3]">{b.name}</span>
        </p>
      );
    case "tool_result":
      return (
        <pre className="max-h-32 overflow-auto whitespace-pre-wrap break-words rounded bg-[#161b22] p-1.5 text-[11px] text-[#8b949e]">
          {b.text || "(no output)"}
          {b.truncated ? "\n… (clipped)" : ""}
        </pre>
      );
    case "other":
      // NAMED, not dropped. Claude Code owns this format, and a pane that
      // silently omitted a future block kind would show an exchange with
      // an invisible hole in it.
      return (
        <p className="text-xs text-[#6e7681]">
          A <span className="font-mono">{b.block_type}</span> block, which this version of
          Headstate does not know how to show.
        </p>
      );
  }
}

/// A reveal button that is DISABLED with a reason rather than absent
/// (#919).
///
/// # Why not hide it
///
/// Most recorded cwds no longer exist (measured at the time: ~83%, and
/// higher when re-measured -- a historical observation, not a live fact
/// (#969)), so a button that is simply absent on a gone path is absent on
/// the common case -- and a reader cannot tell "this app has no such action" from
/// "this particular path is gone". Worse is the version that renders
/// enabled and does nothing: revealing a deleted directory on macOS
/// silently opens the user's home folder, which looks like the app
/// misfired.
///
/// So the button is always rendered (on the desktop), and when it cannot
/// work it is `disabled` with the reason in its `title` and in visible
/// text beside it. A disabled control with a stated reason is the only
/// one of the three that answers the question the reader actually has.
///
/// # Three refusal reasons, not one
///
/// `gone` and `unknown` get DIFFERENT wording, and that is the whole of
/// the absent-is-not-zero rule here:
///
/// - `gone` -- the path is definitely not there. Nothing to reveal;
///   expect it to stay that way.
/// - `unknown` -- the check itself failed, so the path may well be
///   there. The remedy is to fix whatever blocked the check, and saying
///   "gone" would send the user looking for work that was never lost.
/// - `not-recorded` -- we never knew the path. Distinct again: there is
///   no missing file, only a fact we do not hold.
///
/// A single shared "unavailable" string would collapse all three into
/// the shrug the tri-state exists to prevent.
function RevealButton({
  label,
  path,
  state,
  what,
  onReveal,
}: {
  label: string;
  /// The path to reveal. `null` is its own refusal reason and is not
  /// folded into `state`: a row can carry a path whose check failed, and
  /// a row can carry no path at all.
  path: string | null;
  state: CwdState;
  /// Names the thing in the failure toast, so "Could not reveal the
  /// transcript" is distinguishable from the directory's message.
  what: string;
  onReveal: (path: string, what: string) => void;
}) {
  const refusal = revealRefusal(path, state);
  // Narrowed rather than cast. `revealRefusal` returns non-null for
  // every null path, so `path as string` under `refusal === null` would
  // have been sound -- but only because of a fact stated in another
  // function, and a cast asks the reader to take that on trust. This
  // makes the compiler check it instead.
  const revealable = refusal === null && path !== null ? path : null;
  return (
    <div className="flex items-center gap-1.5">
      <button
        type="button"
        // `disabled` and not merely styled: a click that reaches
        // `claude_reveal_path` with a gone path produces a Finder window
        // on the home directory, which is the silent-nothing failure
        // this whole component exists to replace.
        disabled={revealable === null}
        // The reason on hover as well as beside the button. Belt and
        // braces on purpose: the visible text is what a keyboard or
        // screen-reader user gets, the title is what a mouse user
        // reaching for a greyed control looks for.
        title={refusal ?? undefined}
        onClick={revealable === null ? undefined : () => onReveal(revealable, what)}
        className={
          revealable !== null
            ? "tap-target flex items-center gap-1.5 rounded-md border border-[#30363d] bg-[#21262d] px-2 py-1 text-xs text-[#e6edf3] hover:bg-[#30363d]"
            : "tap-target flex cursor-not-allowed items-center gap-1.5 rounded-md border border-[#30363d] bg-[#161b22] px-2 py-1 text-xs text-[#6e7681]"
        }
      >
        <FolderOpen className="h-3 w-3" aria-hidden="true" />
        {label}
      </button>
      {/* The reason is VISIBLE, not only a tooltip. A greyed button whose
          explanation is hover-only is unreadable on a touch screen and
          invisible to a screen reader. */}
      {refusal !== null ? <span className="text-[11px] text-[#8b949e]">{refusal}</span> : null}
    </div>
  );
}

/// Why a reveal cannot happen, or `null` when it can.
///
/// NOT exported: the three distinct strings are asserted through the
/// rendered DOM by `revealing a path that may be gone`, which is the
/// level a reader of the UI cares about, and exporting a helper nothing
/// imports is what `yarn knip` exists to catch.
function revealRefusal(path: string | null, state: CwdState): string | null {
  // Checked before the state, because a row with no path has nothing for
  // the state to be about. A hook-sourced session Headstate saw start
  // before any transcript import ran is exactly this case.
  if (path === null || path === "") return "no path recorded";
  switch (state.state) {
    case "exists":
      return null;
    case "gone":
      return "the path no longer exists";
    case "unknown":
      // NAMES the error. "Could not check" with nothing to act on is
      // barely better than "gone"; the point of the third state is that
      // its remedy is different, and the user needs the reason to apply
      // it.
      return `could not check whether it exists (${state.why})`;
    case "not-recorded":
      return "no path recorded";
  }
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-wrap gap-x-2">
      <dt className="shrink-0 text-[#8b949e]">{label}</dt>
      <dd className="min-w-0 text-[#e6edf3]">{children}</dd>
    </div>
  );
}

/// The resume command, its caveat, and nothing that hides either (#918).
///
/// # Why the `cd` is not optional
///
/// `claude --resume <id>` works from any directory -- verified -- and
/// adopts the **invoking** one. So a bare command on the clipboard
/// resurrects the session pointed at whatever tree the terminal happened
/// to be in, which is worse than failing because it looks like it
/// worked: the session arrives with all its context and starts editing
/// the wrong repository.
///
/// # Why a running session is offered something different
///
/// `claude --help` says resuming a session that is already running
/// starts a COPY of it. So a live session gets the id to copy and an
/// explanation, not a Resume button whose label would promise something
/// it does not do.
///
/// And a session whose liveness is `unknown` gets Resume with the
/// caveat rather than either the confident button or nothing: it is
/// probably over -- that is what 1,400 imported rows are -- but we did
/// not establish it, so the label must not imply we did.
function Resume({
  session: s,
  detail: d,
  onCopy,
  onLaunch,
  terminalConfigured,
}: {
  session: ClaudeSession;
  /// The command itself, built on the backend against the cwd's state.
  /// From the DETAIL since #985: it restated the `cwd` a second time on
  /// every one of 1,474 rows to be read on one, which was 25.6% of the
  /// payload -- the single largest field.
  detail: ClaudeSessionDetail;
  onCopy: (value: string, what: string) => void;
  /// Open the resume command in the configured terminal (#1126).
  ///
  /// Takes the id and cwd rather than the built command: Rust rebuilds
  /// it, so this can never become "run this text in a terminal".
  onLaunch: (sessionId: string, cwd: string | null) => void;
  /// Whether a terminal is configured, which decides what the primary
  /// button does and whether a separate Copy is offered beside it.
  terminalConfigured: boolean;
}) {
  if (s.liveness.state === "running") {
    return (
      <section className="rounded-md border border-[#3fb950]/40 bg-[#3fb950]/5 p-3">
        <h3 className="text-xs font-semibold text-[#3fb950]">This session is running</h3>
        <p className="mt-1 text-xs text-[#8b949e]">
          Resuming a session that is already running starts a second copy of it, so there is
          nothing to resurrect here. Copy the id if you want to find it in a terminal.
        </p>
        <button
          type="button"
          onClick={() => onCopy(s.session_id, "Session id")}
          className="tap-target mt-2 rounded-md border border-[#30363d] bg-[#21262d] px-2 py-1 text-xs text-[#e6edf3] hover:bg-[#30363d]"
        >
          Copy session id
        </button>
      </section>
    );
  }

  const anchored = d.resume.anchored;
  return (
    <section
      className={`rounded-md border p-3 ${
        // ANCHORED is the primary presentation. A command that carries
        // its own `cd` lands where the work was; one that does not is
        // offered more quietly, because 84% of rows are in that state
        // and it must read as normal rather than as broken.
        anchored
          ? "border-[#1f6feb]/50 bg-[#1f6feb]/5"
          : "border-[#30363d] bg-[#161b22]"
      }`}
    >
      <h3 className="flex items-center gap-1.5 text-xs font-semibold text-[#e6edf3]">
        <Terminal className="h-3.5 w-3.5" aria-hidden="true" />
        Resume this session
      </h3>
      {/* The command is SHOWN, not only copied. A user who can read the
          line before pasting it can see the `cd` -- or see that there
          is none, which is the whole point of the caveat below. */}
      <pre className="mt-2 overflow-x-auto rounded bg-[#0d1117] px-2 py-1.5 font-mono text-[11px] text-[#e6edf3]">
        {d.resume.command}
      </pre>
      {/* Never collapsed into the button's label, and never hidden
          behind a tooltip: this is the sentence that stops the command
          landing in the wrong tree. */}
      {d.resume.caveat ? (
        <p className="mt-2 text-xs text-[#d29922]">{d.resume.caveat}</p>
      ) : null}
      {s.liveness.state === "unknown" ? (
        <p className="mt-1 text-xs text-[#8b949e]">
          It could not be established whether this session is running, so it may already be
          open somewhere. Resuming it then starts a second copy.
        </p>
      ) : null}
      <div className="mt-2 flex flex-wrap items-center gap-2">
        <button
          type="button"
          // Launches when a terminal is configured, copies otherwise
          // (#1126). One button either way: a permanent second button
          // that most users can never use is the thing the setting
          // exists to avoid.
          onClick={() =>
            terminalConfigured
              ? onLaunch(s.session_id, s.cwd ?? null)
              : onCopy(
                  d.resume.command,
                  anchored ? "Resume command" : "Resume command (no directory)",
                )
          }
          className={`tap-target rounded-md px-2 py-1 text-xs ${
            anchored
              ? "bg-[#1f6feb] text-white hover:bg-[#388bfd]"
              : "border border-[#30363d] bg-[#21262d] text-[#e6edf3] hover:bg-[#30363d]"
          }`}
        >
          {terminalConfigured
            ? anchored
              ? "Resume in terminal"
              : "Resume anyway"
            : anchored
              ? "Copy resume command"
              : "Copy anyway"}
        </button>
        {/* Copy stays reachable whenever the button no longer does it.
            The configured terminal is one user's choice of one tool,
            and the raw string is what you need to paste elsewhere,
            read before running, or hand to someone else. */}
        {terminalConfigured ? (
          <button
            type="button"
            onClick={() =>
              onCopy(
                d.resume.command,
                anchored ? "Resume command" : "Resume command (no directory)",
              )
            }
            className="tap-target rounded-md border border-[#30363d] px-2 py-1 text-xs text-[#8b949e] hover:bg-[#21262d] hover:text-[#e6edf3]"
          >
            Copy
          </button>
        ) : null}
      </div>
      {/* What happens next, which differs by whether a terminal is set.
          The old sentence -- "Headstate does not open one for you" --
          was true for every build until #1126 and would now be a
          statement the app contradicts the moment the button is
          pressed. `claudify_command`'s reasoning still holds for the
          DEFAULT: nothing is guessed, and this opens only what the user
          configured. */}
      <p className="mt-1.5 text-[11px] text-[#8b949e]">
        {terminalConfigured
          ? "Opens in the terminal you configured in Settings."
          : "Paste it into your own terminal — Headstate opens one only if you configure it in Settings."}
      </p>
    </section>
  );
}

/// The pull requests one session produced (#1132).
///
/// Absent entirely when there are none, rather than an empty heading:
/// most sessions open no pull request, and a permanent "Pull requests:
/// none" would be noise on almost every row.
function SessionPullRequests({ detail }: { detail: ClaudeSessionDetail }) {
  const prs = detail.pull_requests ?? [];
  if (prs.length === 0) return null;
  return (
    <div className="mt-3">
      <p className="text-xs font-semibold text-[#e6edf3]">
        Pull request{prs.length === 1 ? "" : "s"}
      </p>
      <ul className="mt-1 space-y-0.5">
        {prs.map((pr) => (
          <li key={`${pr.repo}#${pr.number}`} className="text-xs">
            <ExternalLink href={pr.url} className="text-[#58a6ff] hover:underline">
              {pr.repo}#{pr.number}
            </ExternalLink>
          </li>
        ))}
      </ul>
    </div>
  );
}

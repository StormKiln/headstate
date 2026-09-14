import { useMemo, useState } from "react";
import { Bot, Circle, FolderOpen, GitBranch, RefreshCw, Search, Terminal } from "lucide-react";
import { toast } from "sonner";
import type {
  ClaudePreviewBlock,
  ClaudeSession,
  CwdState,
  Liveness,
} from "@/types/pr";
import {
  useClaudeSessionUsage,
  useClaudeSessions,
  useClaudeTranscriptTail,
  useWorktrees,
} from "@/api/hooks";
import { claudeRevealPath } from "@/api/tauri";
import { current } from "@/lib/ariaCurrent";
import { copyText } from "@/lib/clipboard";
import { IS_MOBILE_BUILD } from "@/lib/target";
import { relativeTime } from "@/lib/time";
import { useIsMobile } from "@/lib/useIsMobile";
import { pathBasename, safetyReason, sessionWorktree } from "@/lib/worktrees";
import { type ClaudeSessionFilter, useFilters } from "@/store/filters";
import { QueryError, errorMessage } from "./QueryError";

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
  const query = useFilters((f) => f.claudeQuery);
  const filter = useFilters((f) => f.claudeFilter);

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
    const chipped = all.filter((s) => matchesClaudeFilter(s, filter));
    const hits = q
      ? chipped.filter((s) =>
          [s.name, s.cwd, s.git_branch, s.session_id].some((f) =>
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
  }, [all, query, filter]);

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
    return {
      all: all.length,
      resumable: all.filter((s) => matchesClaudeFilter(s, "resumable")).length,
      gone: all.filter((s) => matchesClaudeFilter(s, "gone")).length,
      running: all.filter((s) => matchesClaudeFilter(s, "running")).length,
      ended: all.filter((s) => matchesClaudeFilter(s, "ended")).length,
    };
  }, [all]);

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
  const selected = useFilters((f) => f.claudeSelected);
  const selectSession = useFilters((f) => f.selectClaudeSession);
  // Local, not in the store: unlike the query and the selection nothing
  // outside this component reads it, and it is a statement about how much
  // of ONE rendering of the list has been asked for.
  const [showAll, setShowAll] = useState(false);

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

  const capped = showAll ? matched.ordered : matched.ordered.slice(0, RENDER_CAP);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="shrink-0 border-b border-[#30363d] p-3">
        <label className="flex items-center gap-2 rounded-md border border-[#30363d] bg-[#0d1117] px-2">
          <Search className="h-3.5 w-3.5 shrink-0 text-[#8b949e]" aria-hidden="true" />
          <input
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search title, directory, branch or id"
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
          capped.map((s) => (
            <SessionEntry
              key={s.session_id}
              session={s}
              now={now}
              active={s.session_id === selected}
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

function SessionEntry({
  session: s,
  now,
  active,
  onSelect,
}: {
  session: ClaudeSession;
  /// The poll's timestamp. See the page's doc comment: never
  /// `Date.now()`.
  now: number;
  active: boolean;
  onSelect: () => void;
}) {
  const note = cwdNote(s.cwd_state);
  return (
    <button
      type="button"
      onClick={onSelect}
      aria-current={current(active)}
      className={`mb-1 flex w-full flex-col items-start gap-0.5 rounded px-2 py-1.5 text-left ${
        active ? "bg-[#1f6feb] text-white" : "text-[#e6edf3] hover:bg-[#161b22]"
      }`}
    >
      <span className="flex w-full items-center gap-1.5">
        <Bot className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
        {/* `aiTitle`, or the id. NOT a fabricated name: the two
            transcripts in 1,438 with no title get their id, which is at
            least true and is also the resume handle. */}
        <span className="min-w-0 flex-1 truncate text-xs font-medium">
          {s.name ?? s.session_id}
        </span>
      </span>
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
      <span
        className={`w-full truncate font-mono text-[11px] ${
          active ? "text-white" : "text-[#8b949e]"
        }`}
      >
        {s.cwd ?? "no directory recorded"}
        {note ? ` · ${note}` : ""}
      </span>
    </button>
  );
}

/// One session: where it ran, whether it is alive, and how to get it
/// back.
function SessionDetail({
  session: s,
  now,
}: {
  session: ClaudeSession;
  now: number;
}) {
  const copy = (value: string, what: string) => {
    void copyText(value).then((failure) =>
      failure === null
        ? toast.success(`${what} copied to the clipboard`)
        : toast.error(`Could not copy the ${what.toLowerCase()}`, { description: failure }),
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
      <SessionBody session={s} now={now} copy={copy} reveal={reveal} />
      {/* Both BELOW "Where it ran" and above the worktree jump, which is
          the order the questions are asked in: what is this, how much was
          it, what was it saying, and where do I go next. The preview is
          last of the two because it is the one that costs a read. */}
      <SessionUsage session={s} />
      <TranscriptPreview session={s} />
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

/// The session detail's own fields, split from [`SessionDetail`] so the
/// worktree section can sit beside them without this function growing a
/// second concern.
function SessionBody({
  session: s,
  now,
  copy,
  reveal,
}: {
  session: ClaudeSession;
  now: number;
  copy: (value: string, what: string) => void;
  reveal: (path: string, what: string) => void;
}) {
  // A fragment, not a wrapper: `SessionDetail` owns the column gap so
  // that `WorktreeJump` is spaced from these sections by the same rule
  // they are spaced from each other.
  return (
    <>
      <div>
        <h2 className="text-sm font-semibold text-[#e6edf3]">{s.name ?? s.session_id}</h2>
        <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-[#8b949e]">
          <LivenessBadge liveness={s.liveness} />
          {s.last_activity_at ? (
            <span>Last active {relativeTime(s.last_activity_at, new Date(now))}</span>
          ) : (
            <span>No recorded activity</span>
          )}
          <span>Started {relativeTime(s.first_seen_at, new Date(now))}</span>
        </div>
        {/* The REASON, for the two states that have one. A "not running"
            established by an orphaned registry entry is a crash and says
            so; a "could not tell" says what stopped us. Hiding either
            leaves the user with a verdict and no grounds. */}
        {s.liveness.state !== "running" ? (
          <p className="mt-1.5 text-xs text-[#8b949e]">{s.liveness.why}</p>
        ) : null}
      </div>

      <Resume session={s} onCopy={copy} />

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
          {s.claude_version ? <Field label="Claude">{s.claude_version}</Field> : null}
          <Field label="Session id">
            <span className="break-all font-mono">{s.session_id}</span>
          </Field>
          {/* `runs: 0` is the whole imported corpus, and saying so is
              what distinguishes "we never watched this process" from
              "we watched it and it ended" -- which is also the
              difference between two liveness answers. */}
          <Field label="Observed runs">
            {s.runs === 0
              ? "none — this session was read from its transcript, not watched while it ran"
              : s.runs.toLocaleString()}
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
              path={s.transcript_path}
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
              state={s.transcript_state}
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
function SessionUsage({ session: s }: { session: ClaudeSession }) {
  // The transcript's OWN state, never the cwd's (#919): 1,213 of 1,461
  // rows have a dead cwd and a live transcript, so a reading gated on the
  // cwd would be absent on almost every row.
  const readable = s.transcript_path !== null && s.transcript_state.state !== "gone";
  const { data, isError, error, isLoading } = useClaudeSessionUsage(
    readable ? s.transcript_path : null,
  );

  return (
    <section className="rounded-md border border-[#30363d] bg-[#161b22] p-3">
      <h3 className="text-xs font-semibold text-[#e6edf3]">How much work it did</h3>
      {s.transcript_path === null ? (
        <p className="mt-2 text-xs text-[#8b949e]">
          No transcript was recorded for this session, so there is nothing to read this from.
        </p>
      ) : s.transcript_state.state === "gone" ? (
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
          {/* The cap, STATED. Without this the reader cannot tell a
              complete sum from one that stopped 8 MB in, which is the #846
              defect with a number on it. It binds on the 16 real files over
              10 MB and on nothing else, so this line is almost never
              drawn -- which is exactly why it must be there when it is. */}
          {data.truncated ? (
            <p className="mt-2 text-xs text-[#d29922]">
              These are floors, not totals: the transcript is{" "}
              {formatMb(data.file_bytes)} and only its first {formatMb(data.bytes_read)} were
              read. Reading it whole would hang this pane.
            </p>
          ) : null}
        </>
      )}
    </section>
  );
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
/// The primary action stays the clipboard copy, for the reason
/// `claudify_command` records: macOS has no default-terminal concept. This
/// pane is for deciding, not for doing.
function TranscriptPreview({ session: s }: { session: ClaudeSession }) {
  const [open, setOpen] = useState(false);
  // `transcript_state`, never `cwd_state` (#919, and `ClaudeSession`'s own
  // doc): 0% of transcripts are gone against 83% of cwds.
  const refusal = revealRefusal(s.transcript_path, s.transcript_state);
  const { data, isError, error, isLoading } = useClaudeTranscriptTail(
    s.transcript_path,
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
  onCopy,
}: {
  session: ClaudeSession;
  onCopy: (value: string, what: string) => void;
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

  const anchored = s.resume.anchored;
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
        {s.resume.command}
      </pre>
      {/* Never collapsed into the button's label, and never hidden
          behind a tooltip: this is the sentence that stops the command
          landing in the wrong tree. */}
      {s.resume.caveat ? (
        <p className="mt-2 text-xs text-[#d29922]">{s.resume.caveat}</p>
      ) : null}
      {s.liveness.state === "unknown" ? (
        <p className="mt-1 text-xs text-[#8b949e]">
          It could not be established whether this session is running, so it may already be
          open somewhere. Resuming it then starts a second copy.
        </p>
      ) : null}
      <button
        type="button"
        onClick={() =>
          onCopy(s.resume.command, anchored ? "Resume command" : "Resume command (no directory)")
        }
        className={`tap-target mt-2 rounded-md px-2 py-1 text-xs ${
          anchored
            ? "bg-[#1f6feb] text-white hover:bg-[#388bfd]"
            : "border border-[#30363d] bg-[#21262d] text-[#e6edf3] hover:bg-[#30363d]"
        }`}
      >
        {anchored ? "Copy resume command" : "Copy anyway"}
      </button>
      {/* No terminal is spawned, and that is a decision rather than a
          gap -- `claudify_command` records it: macOS has no
          default-terminal concept, so the app would have to guess, while
          the clipboard works everywhere and lands the user in their own
          shell. */}
      <p className="mt-1.5 text-[11px] text-[#8b949e]">
        Paste it into your own terminal — Headstate does not open one for you.
      </p>
    </section>
  );
}

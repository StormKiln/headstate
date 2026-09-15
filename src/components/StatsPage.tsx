import { useState } from "react";
import {
  type StatsScope,
  scopeIsLoadable,
  useCycleTrend,
  useHistory,
  useMergedDetail,
  usePeriods,
  useScopedCounts,
  useStatsBoard,
  useStatsReviewers,
  useStatsSeries,
  useStatsTree,
} from "../api/hooks";
import { classifyFailedDays, namedDaysText, unmeasuredMessage } from "../lib/stats";
import { useActiveFilters } from "../store/filters";
import type { ShortSlice, Unmeasured } from "../types/pr";
import { QueryError, errorMessage } from "./QueryError";
import { ActivityChart } from "./stats/ActivityChart";
import { CycleTime } from "./stats/CycleTime";
import { DeltaCards } from "./stats/DeltaCards";
import { HelpButton } from "./HelpButton";
import { InsightCards } from "./stats/InsightCards";
import { Leaderboards } from "./stats/Leaderboard";
import { Outliers } from "./stats/Outliers";
import { RepoTable } from "./stats/RepoTable";
import { GroupFigures, PersonFigures, ScopeCounts } from "./stats/ScopeSummary";
import { SkeletonChart, SkeletonRow } from "./stats/Skeleton";

/// The windows a scope page offers are `ActivityChart`'s `RANGES`, and the
/// chart's own buttons are the ONE control that sets them.
///
/// This file used to declare the same three numbers and render a second,
/// markup-identical group from them in the header row -- two groups on one
/// screen, both `aria-pressed`, both calling this page's `setDays`, with
/// nothing to tell a user or a screen reader which was which. The comment
/// introducing that constant asked for "one control rather than two that
/// can disagree about what 'this period' means"; the header group was
/// removed to make that true, which also makes the scoped page agree with
/// the unscoped one below it (#980).
///
/// `days` stays lifted here rather than moving into the chart: `ScopeCounts`
/// takes it and the series query is keyed on it, so the counts and the chart
/// would otherwise fall out of step -- exactly the disagreement above.
/// 30 is the default because it is the shortest window in which a monthly
/// cadence of work is visible at all, and it is what the unscoped page
/// defaulted to.

/// Which half of a scope the page is showing.
///
/// Two views, not two pages: they are the same measurement partitioned, so a
/// switch between them costs nothing -- the board is already loaded and
/// `row_for` / `others` split it. That is why this is local state and not a
/// persisted filter.
type Half = "mine" | "others";

/// The PR Stats view: two views on every scope, and the leaderboards.
///
/// # Progressive rendering, extended rather than inherited
///
/// The page before this ran three independent queries, each rendering the
/// moment IT landed rather than behind one combined gate, because they
/// differed enough in cost that a single gate wasted most of the wait
/// (periods ~1.6s, the daily series ~3.7s, the merged sample ~3.7s).
///
/// That property is kept and it matters MORE here, which #826 says
/// explicitly: an "Others" view over an organisation has more parts and more
/// variance than the account-wide page did. The parts now are
///
///   - two scoped counts, merged and opened (count-only, fastest);
///   - the daily series (count-only, ~1.4-1.5s per ten days MEASURED);
///   - the board (per-PR nodes across every slice -- seconds on a busy org,
///     and the only part whose cost scales with how much work happened).
///
/// Each renders as it arrives. A single gate would hide two sub-second
/// answers behind the one that is inherently slow.
///
/// # Nothing loads until clicked
///
/// `enabled` is threaded into all three hooks from one place: whether a scope
/// is actually selected. Arriving at the view with nothing clicked costs
/// nothing beyond the sidebar's own 2 points, which is
/// `hooks.ts:712-717`'s discovery/measurement split. The old "Measure
/// button" pattern is gone -- #796 removed the last one and
/// `SystemHealthPage.tsx:1763-1788` argues against re-adding one -- so the
/// gate is the selection, and the sidebar row is the click that opens it.
///
/// # The account-wide page and the scoped pages BOTH live here
///
/// Two pages, routed by one condition: `UnscopedStats` when nothing is
/// selected or "Everything" is, `ScopedStats` otherwise. #826's reopening
/// requires the account-wide view be reachable WITHOUT choosing a scope
/// first, and the reason is measured rather than a preference -- see
/// `UnscopedStats` for the 893-against-317 figures. The scoped pages are
/// good and unchanged; what was wrong was treating one as a replacement for
/// the other.
export function StatsPage() {
  const filters = useActiveFilters();

  // The selection the sidebar wrote, as one object. The three keys ARE one
  // selection (`setStatsScope`), so they are read together and passed
  // together rather than threaded as three arguments that could drift apart.
  const scope: StatsScope | undefined = filters.statsScopeKind
    ? {
        kind: filters.statsScopeKind,
        value: filters.statsScopeValue,
        subject: filters.statsSubject,
      }
    : undefined;

  // The account-wide page, on BOTH of the two ways to ask for it: the
  // "Everything" row, and no selection at all.
  //
  // Making it the default is what restores the capability #829 removed. The
  // scoped page's empty state ("Pick something to measure") was a correct
  // thing to show when the account-wide page did not exist, but with it back
  // there is a better answer to "I have not chosen yet" than a prompt: the
  // question the user most likely has, already answered. A zero-click
  // overview is the specific thing that was lost, and an "Everything" row
  // the user must find and click would only have halved the regression.
  //
  // `kind === "all"` rather than a fourth store field. `Filters.statsScopeKind`
  // has had an `"all"` variant since #825 ("the widest scope was deliberately
  // clicked") with nothing selecting it; this is the row that does, so the
  // sidebar highlight, the store and the page agree without a new axis to
  // keep in sync.
  if (!scope || scope.kind === "all") return <UnscopedStats />;
  return <ScopedStats scope={scope} />;
}

/// A scope page: the headline counts, the chart, and the two views.
function ScopedStats({ scope }: { scope: StatsScope }) {
  const [days, setDays] = useState(30);
  const [half, setHalf] = useState<Half>("mine");
  const loadable = scopeIsLoadable(scope);

  const counts = useScopedCounts(scope, days, loadable);
  const seriesQ = useStatsSeries(scope, days, loadable);
  // Merged, not opened: the board's measures are about work DELIVERED, and
  // a leaderboard of opened pull requests would rank people on intake. The
  // opened count still appears in the headline figures, where it is the
  // intake half of the pair.
  const boardQ = useStatsBoard(scope, "merged", days, loadable);

  // The roster for the reviews-GIVEN board, read off the tree the sidebar
  // already loaded rather than fetched again. `useStatsTree` is keyed
  // `["stats-tree"]` with a five-minute staleTime, so this is the SAME cached
  // answer the sidebar is rendering -- no request, and no chance of the board
  // disagreeing with the Members rows beside it.
  //
  // Only an ORG scope has a roster. A repository, Personal and Everything
  // have no membership to enumerate, so the reviewer board is absent there
  // rather than empty -- which is the honest shape: "nobody reviewed" and
  // "nothing enumerated the reviewers" must not render as the same chart.
  const tree = useStatsTree(true).data;
  // The org the roster comes from, held rather than re-found, so the logins
  // and the truncation flag below are read off ONE object. Finding it twice
  // would let a re-render between the two reads pair a complete flag with a
  // truncated list, which is exactly the disagreement #851 is about.
  const scopeOrg =
    scope.kind === "org"
      ? tree?.orgs.find((o) => o.login === scope.value)
      : undefined;
  const reviewerLogins = (scopeOrg?.members ?? []).map((m) => m.login);
  // #851: the roster is capped at `tree::PAGE` (100) and `membersTotal`
  // keeps telling the truth above it, so this is the board's own
  // `members_truncated()`. Computed here rather than in the component
  // because the component is handed LOGINS, not the tree, and a list of 100
  // strings cannot say whether a 101st existed.
  const reviewersTruncated =
    !!scopeOrg && scopeOrg.members.length < scopeOrg.membersTotal;
  const reviewersQ = useStatsReviewers(scope, days, reviewerLogins, loadable);

  const board = boardQ.data;
  const series = seriesQ.data;

  // A half-written selection: a kind with no value, which `scopeIsLoadable`
  // refuses so it cannot reach a command and come back as "scope org needs a
  // value" -- an error about an internal contract shown to a user who only
  // clicked a row.
  //
  // No longer the "nothing clicked yet" state: that now routes to
  // `UnscopedStats`, which answers the question rather than asking for one.
  // This branch survives because the store can still hold a kind without a
  // value, and a page that rendered nothing for it would look broken.
  if (!loadable) {
    return (
      <div className="rounded-md border border-[#30363d] px-4 py-12 text-center">
        <p className="text-sm font-semibold text-[#e6edf3]">
          Pick something to measure
        </p>
        <p className="mx-auto mt-2 max-w-md text-sm text-[#8b949e]">
          Choose an organization, a repository or a person in the sidebar -- or
          "Everything" for your account-wide figures. Nothing is measured
          until you do: a scope-wide load costs rate limit, so it waits for a
          click.
        </p>
      </div>
    );
  }

  // Every part failed. Without this branch each section shows its own error
  // and the page becomes three copies of the same message -- and the cause
  // is usually one thing (no token, no network, budget exhausted) rather
  // than three.
  const allFailed = counts.failed === 2 && seriesQ.isError && boardQ.isError;
  const retryAll = () => {
    counts.refetch();
    void seriesQ.refetch();
    void boardQ.refetch();
  };
  if (allFailed) {
    return (
      <QueryError
        title="Could not load statistics for this scope"
        message={errorMessage(boardQ.error)}
        onRetry={retryAll}
      />
    );
  }

  const scopeLabel = describeScope(scope);
  const subject = scope.subject;
  // Who "Mine" is about. A Members row sets a subject and keeps the org
  // scope, so on that selection "Mine" is that COLLEAGUE rather than the
  // viewer -- which is the question the row asks ("this person, in this
  // org"). `board.viewer` travels with the board so this split cannot be
  // made against a login from a different account.
  const mineLogin = subject ?? board?.viewer;
  const mineRow = board && mineLogin ? board.rows.find((r) => r.login === mineLogin) : undefined;
  const otherRows = board && mineLogin ? board.rows.filter((r) => r.login !== mineLogin) : [];

  // Why the board is partial, assembled from whichever channels applied.
  // All three are reported, because they fail for different reasons and a
  // reader deciding whether to trust a ranking needs to know which.
  const caveat = board ? partialityCaveat(board) : undefined;

  // Some days missing, or none measured at all (#1045). Classified against
  // `days` -- the window the chart ASKED for -- rather than against
  // `points.length`, which a total failure would leave at zero and make the
  // two agree on "nothing is missing".
  const failedDays = classifyFailedDays(series?.failedDays ?? [], days);

  return (
    <div className="flex flex-col gap-3">
      {/* The scope has to be named explicitly or a reader will mistake it
          for the narrower one beside it. The range buttons that used to sit
          opposite this label are gone -- the chart below owns them (#980) --
          but the label and its help stay. */}
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-1 text-xs text-[#8b949e]">
          <span>{scopeLabel}</span>
          <HelpButton topic="stats-sample" />
        </div>
      </div>

      {/* The headline counts land first and are the cheapest part. They are
          scope-wide rather than per-view, because "how much happened here"
          is the same question whichever half you are reading. */}
      {counts.merged || counts.opened || counts.failed > 0 ? (
        <ScopeCounts
          merged={counts.merged}
          opened={counts.opened}
          days={days}
          failed={counts.failed}
        />
      ) : (
        <SkeletonRow count={2} cols="sm:grid-cols-2" />
      )}

      {series && failedDays.kind === "total" ? (
        /* NO day was measured, so there is no chart to annotate (#1045).
           The old branch fell through to the warning below and enumerated
           all 30 dates -- which told a reader nothing they could not see
           from the empty chart, while burying the fact that actually
           matters: the measurement did not complete. A total failure is an
           error state and gets the error panel, including the retry that a
           chart-shaped warning has nowhere to put.

           The series resolved, so `seriesQ.error` is empty and the cause
           has to come from the query's own channels. `refusedFields`
           distinguishes a SAML refusal (which a retry will not fix) from
           an unanswered document (which it usually will), and that is the
           difference between advice a user can act on and a shrug. */
        <BudgetAwareError
          count={failedDays.count}
          refusedFields={series.refusedFields}
          unmeasured={series.unmeasured}
          onRetry={() => void seriesQ.refetch()}
        />
      ) : series ? (
        <>
          <ActivityChart
            // The scoped series carries the same three fields
            // `HistoryPoint` does, so it renders through the SAME chart --
            // #826 asks that the existing components be reused rather than
            // a second charting idiom introduced.
            points={series.points}
            days={days}
            onDaysChange={setDays}
          />
          {/* Named days, not a count. A chart of 30 days missing 2 is still
              the most informative thing available, provided it says which 2
              -- and a missing day rendered as zero would draw a trough that
              reads as a quiet Tuesday.

              BOUNDED as of #1045. That reasoning holds for a handful and
              stops holding for a wall: past `NAMED_DAYS` the rest are
              counted, so the sentence still says how much is missing
              without becoming a paragraph nobody reads. */}
          {failedDays.kind === "partial" && (
            <p className="text-xs text-[#d29922]">
              {failedDays.count} day{failedDays.count === 1 ? "" : "s"} could
              not be measured and {failedDays.count === 1 ? "is" : "are"}{" "}
              absent from the chart rather than drawn as zero:{" "}
              {namedDaysText(failedDays.named, failedDays.rest)}.
            </p>
          )}
        </>
      ) : seriesQ.isError ? (
        <QueryError
          title="Could not load the activity chart"
          message={errorMessage(seriesQ.error)}
          onRetry={() => void seriesQ.refetch()}
        />
      ) : (
        <SkeletonChart
          title="Pull request activity"
          hint="Opened and merged per day"
        />
      )}

      {/* The two views. Rendered as a switch rather than two pages because
          they are one measurement partitioned -- switching costs nothing,
          since the board is already loaded. */}
      <div
        className="flex gap-1 border-b border-[#30363d]"
        role="tablist"
        aria-label="Which half of this scope"
      >
        {(
          [
            ["mine", subject ? subject : "Mine"],
            ["others", "Others"],
          ] as const
        ).map(([id, label]) => (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={half === id}
            onClick={() => setHalf(id as Half)}
            className={`-mb-px border-b-2 px-3 py-1.5 text-sm ${
              half === id
                ? "border-[#1f6feb] text-[#e6edf3]"
                : "border-transparent text-[#8b949e] hover:text-[#e6edf3]"
            }`}
          >
            {label}
          </button>
        ))}
      </div>

      {board ? (
        <>
          {/* The partiality caveat sits ABOVE both views rather than inside
              the leaderboards, because it qualifies every figure drawn from
              the board -- Mine's four cards, the cycle-time distribution, the
              outliers and the repo shares included, not just the rankings.

              It was inside `Leaderboards` first, which left the Mine tab
              saying "at least 12" with nothing anywhere on screen to say WHY
              it was a floor. A reader cannot act on a prefix alone. */}
          {!board.complete && caveat ? (
            <div className="rounded-md border border-[#d29922]/40 bg-[#d29922]/10 px-3 py-2 text-xs text-[#d29922]">
              This board is incomplete, so every figure below is a floor rather
              than a total. {caveat}
            </div>
          ) : null}
          {half === "mine" ? (
            <>
              <PersonFigures
                row={mineRow}
                who={subject ? subject : "You"}
                partial={!board.complete}
              />
              {/* Cycle time, the outliers and the repo table -- the three
                  sections the unscoped page had and #826 keeps for Mine.
                  Drawn from the SAME board as the figures above, so they
                  cannot disagree with them; the unscoped page fetched them
                  from a separate `get_merged_detail` sample, which is why it
                  needed a page-level "from a sample of recent merged pull
                  requests" caveat that a scope page does not.

                  Rendered only when there is a row: with no activity there is
                  no distribution, and three empty cards under "No activity"
                  would be three more ways to say the same thing. */}
              {mineRow ? (
                <>
                  <CycleTime hours={mineRow.cycleTimeHours} prs={mineRow.prs} />
                  <Outliers
                    // The outliers are the SCOPE's, not this person's, and
                    // that is deliberate on the Mine view too: "the slowest
                    // pull request here" is the useful question, and
                    // narrowing it to one author on a single-author scope
                    // would produce the identical list under a narrower
                    // claim. The rows name their author, so whose they are
                    // is never in doubt.
                    slowest={board.slowest}
                    largest={board.largest}
                    slowestBy={(pr) => pr.cycleTimeHours}
                    hint={
                      board.complete
                        ? "across everyone in this scope and window"
                        : "across the part of this scope that could be measured"
                    }
                  />
                  <RepoTable
                    repos={board.repoCounts}
                    // NOT `sampleSize`. These shares are of the whole
                    // window, not of a fixed recent sample, so the
                    // component's default wording would understate a
                    // complete measurement -- and when it is not complete
                    // the caveat above the leaderboards says why.
                    hint={
                      board.complete
                        ? `share of all ${board.total.toLocaleString()} merged in this window`
                        : `share of the ${board.retrieved.toLocaleString()} of ${board.total.toLocaleString()} merged that could be measured`
                    }
                  />
                </>
              ) : null}
            </>
          ) : (
            <>
              <GroupFigures rows={otherRows} partial={!board.complete} />
              {/* Ranked over EVERYONE including the viewer, not over
                  `otherRows`. A leaderboard that silently excluded the
                  reader would put whoever is second in first place, which
                  is a wrong ranking rather than a filtered one -- and the
                  reader is the one person who can tell it is wrong. */}
              {/* No `caveat` here: the page-level banner above already
                  carries it, and two copies of the same warning reads as two
                  different problems. `complete` is still passed, because the
                  component's own short reminder on the rankings is where a
                  reader's eye actually is when they read a name off a
                  board. */}
              <Leaderboards
                rows={board.rows}
                complete={board.complete}
                // The reviews-GIVEN board travels as its OWN query rather than
                // as a field on the rows, because it is a different search
                // over a different population -- the rows are authors in the
                // window, and a reviewer need not have authored anything. So
                // it lands independently and the rankings drawn from the board
                // do not wait on it, which is this page's progressive rule
                // applied to one more part.
                //
                // `undefined` while pending, which the component renders as a
                // loading board rather than an empty one. A reviewer board
                // that printed "no reviews in this window" for the second it
                // was in flight would be a claim, and it is the claim this
                // account's real data makes TRUE -- so a reader could not tell
                // the transient from the answer.
                reviewers={reviewersQ.data}
                reviewersPending={reviewersQ.isPending && reviewerLogins.length > 0}
                reviewersError={reviewersQ.isError}
                // Absent, not empty, when nothing enumerated a roster. Only an
                // org scope has members; on a repository or Personal scope
                // there is nobody to ask about, and an empty chart there would
                // say "nobody reviewed" on the strength of never having
                // looked.
                reviewersAvailable={reviewerLogins.length > 0}
                // #851: whether the roster the board ranks was itself cut
                // short. The sidebar already says "Showing N of M members"
                // two columns away (`StatsSidebar.tsx`), while the board
                // said only "this organization's N listed members" -- which
                // reads as the whole org.
                //
                // Read off the same `OrgTree` the logins came from, so the
                // flag and the list cannot disagree: if the tree says the
                // membership was truncated, the logins below it ARE the
                // truncated set.
                reviewersTruncated={reviewersTruncated}
              />
            </>
          )}
        </>
      ) : boardQ.isError ? (
        <QueryError
          title="Could not load this scope's people"
          message={errorMessage(boardQ.error)}
          onRetry={() => void boardQ.refetch()}
        />
      ) : (
        <SkeletonRow count={4} cols="sm:grid-cols-2 lg:grid-cols-4" />
      )}
    </div>
  );
}

/// The account-wide Stats page: "how am I doing, across everything".
///
/// # Why this exists beside the scoped pages (#826, reopened)
///
/// #829 deleted it as "superseded" by the scoped board. It was not, and the
/// reason is a different question rather than a narrower one. A scope page
/// answers "how is THIS organisation / repository / person doing", which
/// requires choosing one first; this answers "how am I doing, across
/// everything" with no selection at all. The zero-click overview became a
/// two-click drill-down, and no issue asked for that.
///
/// The scoped pages are good and unchanged. What was wrong was treating one
/// as a replacement for the other.
///
/// # `All repos` is NOT account-wide, and the gap is measured
///
/// This is the part that makes the removal a correctness problem rather than
/// a taste one. The queries behind this page carry `author:@me` and NO
/// repository qualifier (`github/query.rs:241-258`), which is the only shape
/// that spans every organisation the viewer contributes to, owned or not.
///
/// MEASURED live 2026-09-11, 30-day window ending yesterday, one aliased
/// document at cost 1:
///
/// | Query | Merged PRs |
/// |---|---|
/// | `author:@me` (this page) | **893** |
/// | `author:@me user:pktstorm` (Personal / All repos) | **317** |
/// | `author:@me org:FNX-Labs` | 494 |
/// | `author:@me org:Stohic` | 82 |
///
/// So the nearest scoped equivalent shows **35%** of the viewer's activity,
/// and no single sidebar row covers the other 576 pull requests -- they are
/// in organisations the viewer contributes to without owning, which on this
/// account is most of the work. Presenting the scoped page as a replacement
/// lost two thirds of the number with nothing on screen to say so, which is
/// the exact failure mode this feature's every other rule exists to prevent.
///
/// # Three independent queries, each rendering as IT lands
///
/// The property #829 kept for the scope pages and which applies here
/// unchanged: periods ~1.6s, the daily series ~3.7s, the merged sample
/// ~3.7s. Blocking on the slowest left the fast numbers finished and
/// invisible. Each section keeps its own footprint while loading, so nothing
/// jumps as the later queries arrive.
///
/// # It is a SAMPLE, and says so once
///
/// `useMergedDetail` reads the most recent 100 merged pull requests
/// (`github/query.rs:175-195`), so the insight cards and repo shares are of a
/// fixed recent sample rather than of a window. That caveat governs every
/// figure below it and is stated once at the top rather than on each card --
/// and it is precisely why `RepoTable` takes `sampleSize` here and a `hint`
/// on a scope page: two honest claims about two different populations, which
/// is the reason both pages exist.
function UnscopedStats() {
  const [days, setDays] = useState(30);
  const periodsQ = usePeriods();
  const historyQ = useHistory(days);
  const detailQ = useMergedDetail();
  const { data: cycleTrend } = useCycleTrend();
  const { data: periods } = periodsQ;
  const { data: history } = historyQ;
  const { data: detail } = detailQ;

  // Every section gates on truthy data, so without an explicit error branch
  // a REJECTED query is indistinguishable from a pending one and its
  // skeleton pulses forever. Nothing else covers this: `poll-error` is
  // emitted only by the PR poll loop, so the AuthGate banner structurally
  // cannot reach these three commands.
  const allFailed = periodsQ.isError && historyQ.isError && detailQ.isError;
  const retryAll = () => {
    void periodsQ.refetch();
    void historyQ.refetch();
    void detailQ.refetch();
  };

  if (allFailed) {
    return (
      <QueryError
        title="Could not load your statistics"
        message={errorMessage(periodsQ.error)}
        onRetry={retryAll}
      />
    );
  }

  // A brand-new user, or one back from holiday, otherwise met four cards
  // reading 0 and "--", plus "over 0 merged", plus "No activity in this
  // period", plus "No merged pull requests in this sample" -- four
  // uncoordinated fragments where one sentence is clearer. Tolerates
  // `detail` being absent, since the two queries land independently and a
  // flicker would be worse than the fragments.
  const nothingYet =
    periods !== undefined &&
    periods.week_current === 0 &&
    periods.month_current === 0 &&
    periods.opened_week_current === 0 &&
    (detail === undefined || detail.sample_size === 0);

  if (nothingYet) {
    return (
      <div className="rounded-md border border-[#30363d] px-4 py-12 text-center">
        <p className="text-sm font-semibold text-[#e6edf3]">
          No merged pull requests yet
        </p>
        <p className="mx-auto mt-2 max-w-md text-sm text-[#8b949e]">
          Statistics appear here once you have merged some pull requests.
          Headstate counts only pull requests you opened. Pick an organization
          or a person in the sidebar to measure somebody else's.
        </p>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-3">
      {/* One line for what is being measured AND the caveat that governs
          every figure below: these are drawn from a SAMPLE, and p90 is the
          maximum on a small one. If the sample caveat can be made in only
          one place, it is at the top of the view rather than on a card.

          The scope half is named explicitly -- "across every organization"
          -- because that is the property a reader cannot otherwise see and
          it is the one that distinguishes this page from `Personal` /
          `All repos`, which is 317 of these 893 pull requests. A page whose
          scope is invisible is one a reader will mistake for the narrower
          one beside it. */}
      <div className="flex items-center gap-1 text-xs text-[#8b949e]">
        <span>
          Your pull requests across every organization, from a sample of
          recent merges
        </span>
        <HelpButton topic="stats-sample" />
      </div>
      {periods ? (
        <DeltaCards periods={periods} />
      ) : periodsQ.isError ? (
        <QueryError
          title="Could not load the headline figures"
          message={errorMessage(periodsQ.error)}
          onRetry={() => void periodsQ.refetch()}
        />
      ) : (
        <SkeletonRow count={4} cols="sm:grid-cols-2 lg:grid-cols-4" />
      )}

      {history ? (
        <ActivityChart
          points={history.points}
          days={days}
          onDaysChange={setDays}
        />
      ) : historyQ.isError ? (
        <QueryError
          title="Could not load the activity chart"
          message={errorMessage(historyQ.error)}
          onRetry={() => void historyQ.refetch()}
        />
      ) : (
        <SkeletonChart
          title="Pull request activity"
          hint="Opened and merged per day"
        />
      )}

      {detail ? (
        <>
          <InsightCards detail={detail} trend={cycleTrend} />
          {/* `slowestBy` is REQUIRED by `Outliers` rather than defaulted,
              and this is the call site that proves why: `MergedPr` spells
              the field `cycle_time_hours` where `BoardPr` spells it
              `cycleTimeHours`. A default matching either one would hand the
              other caller 0 for every row and draw a silent list of zeroes.
              The component was widened for these two callers in #829 and its
              doc comment names this page as one of them -- so the widening
              was right and the deletion of the caller was not. */}
          <Outliers
            slowest={detail.slowest}
            largest={detail.largest}
            slowestBy={(pr) => pr.cycle_time_hours}
          />
          {/* `sampleSize`, NOT a `hint`. These shares are of the last N
              merged pull requests rather than of a window, and the component
              renders "share of the last 100 merged" from it -- the honest
              claim for this page, where a scope page's is about a whole
              window. */}
          <RepoTable repos={detail.repo_counts} sampleSize={detail.sample_size} />
        </>
      ) : detailQ.isError ? (
        <QueryError
          title="Could not load the merged-PR sample"
          message={errorMessage(detailQ.error)}
          onRetry={() => void detailQ.refetch()}
        />
      ) : (
        <SkeletonRow count={3} cols="md:grid-cols-3" />
      )}
    </div>
  );
}

/// One line naming what is being measured.
///
/// Spelled out because every figure on the page is relative to it, and a
/// page that silently changed scope when the sidebar was clicked would
/// present one organisation's numbers under another's -- which nothing else
/// on screen would contradict.
export function describeScope(scope: StatsScope): string {
  const where =
    scope.kind === "repo"
      ? scope.value
      : scope.kind === "org"
        ? `everything in ${scope.value}`
        : scope.kind === "user"
          ? `${scope.value}'s own repositories`
          : "everything this token can see";
  // The subject, when there is one, KEEPS the scope -- "this person, in this
  // org" is the question a Members row asks, so both halves are named.
  return scope.subject ? `${scope.subject}, in ${where}` : String(where);
}

/// Why a board is partial, in words a reader can act on.
///
/// Assembled from all three channels rather than reporting the first, and
/// that is the point of their being separate fields: they fail for different
/// reasons, and "some slices were short" and "GitHub refused fields" suggest
/// different things to do about it. Returns `undefined` for a complete board
/// so the caller has nothing to render.
export function partialityCaveat(board: {
  complete: boolean;
  total: number;
  retrieved: number;
  // The named type rather than an inline shape, so a field added to it on
  // the Rust side reaches this function's reader rather than being silently
  // absent from a structural duplicate.
  truncatedSlices: ShortSlice[];
  refusedFields: number;
  /// #1004. Optional so a caller holding an older payload -- a `stats_cache`
  /// row written before this shipped -- still type-checks and simply reads
  /// as not accumulating, rather than rendering `undefined of 2,942`.
  accumulated?: number;
  accumulating?: boolean;
}): string | undefined {
  if (board.complete) return undefined;
  const parts: string[] = [];
  // What is STORED, not what this one load fetched (#1004). The reporter's
  // complaint is not the shortfall itself -- it is that "1523 of 2942 could
  // not be retrieved" reads identically whether the next load will help or
  // not. So the sentence says how many are held, how many remain, and that
  // it improves.
  const accumulated = board.accumulating ? (board.accumulated ?? 0) : undefined;
  if (accumulated !== undefined && accumulated > 0 && accumulated < board.total) {
    parts.push(
      `${accumulated.toLocaleString()} of ${board.total.toLocaleString()} pull requests have been collected so far and ${(
        board.total - accumulated
      ).toLocaleString()} are still to come -- loading this scope again adds to them`,
    );
  } else if (board.retrieved < board.total) {
    // The SIZE of the gap, not just its existence. A reader deciding whether
    // a top-five is trustworthy needs to know whether four pull requests are
    // missing or four hundred.
    parts.push(
      `${(board.total - board.retrieved).toLocaleString()} of ${board.total.toLocaleString()} pull requests could not be retrieved`,
    );
  }
  if (board.truncatedSlices.length > 0) {
    parts.push(
      `${board.truncatedSlices.length} date range${
        board.truncatedSlices.length === 1 ? "" : "s"
      } came back short`,
    );
  }
  if (board.refusedFields > 0) {
    // Scope first, SSO second (#840): the two causes are indistinguishable
    // from the response (see `tree.rs`'s `readable` doc) and only one of
    // them is the reader's to fix, so the cheap self-serve fix is named
    // before the one that may need an administrator.
    parts.push(
      `GitHub refused ${board.refusedFields} field${
        board.refusedFields === 1 ? "" : "s"
      } -- the token may be missing the read:org scope (\`gh auth refresh -s read:org\`), or this organization may use SAML single sign-on and need the token authorized for it`,
    );
  }
  // A board can be incomplete with none of the above: an irreducible slice
  // is over the 1,000-result cap before any request is made, which the Rust
  // side folds into `complete` directly. Saying so generically beats saying
  // nothing, which would leave "These rankings are incomplete." with no
  // reason attached.
  if (parts.length === 0) {
    parts.push("part of this window holds more pull requests than GitHub will return");
  }
  return `${parts.join("; ")}.`;
}

/// The total-failure panel, which must not offer a retry that cannot work.
///
/// Split out of `StatsPage` because the decision is a real one -- three causes
/// with different advice, only one of which a retry helps -- and an inline
/// ternary is where the previous version quietly said the wrong thing to the
/// user reporting #1050: a budget-exhausted load matched the "GitHub did not
/// answer" branch and was handed a button that could not succeed.
function BudgetAwareError({
  count,
  refusedFields,
  unmeasured,
  onRetry,
}: {
  count: number;
  refusedFields: number;
  unmeasured: Unmeasured | undefined;
  onRetry: () => void;
}) {
  const { message, canRetry } = unmeasuredMessage(count, refusedFields, unmeasured);
  return (
    <QueryError
      title="Could not measure activity for this scope"
      message={message}
      onRetry={canRetry ? onRetry : undefined}
    />
  );
}

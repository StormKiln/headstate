import { useMemo } from "react";
import { useRepoUpstreams, useWorktrees } from "@/api/hooks";
import { PartialScanNotice } from "@/components/PartialScanNotice";
import { QueryError, errorMessage } from "@/components/QueryError";
import { repoCurrency, repoOverviewRows } from "@/lib/repoOverview";
import type { RepoOverviewRow } from "@/lib/repoOverview";
import { upstreamReasonAged, upstreamToneAged } from "@/lib/worktrees";
import { UpdateAllButton } from "@/components/UpdateAllButton";

/// The All Repositories overview: one row per repository, each saying how
/// its main checkout stands against the ref it tracks (#1011).
///
/// # This table does not fetch, and that is the decision (#1021)
///
/// Every verdict here is computed from refs already on disk. The table
/// reads them on mount and never goes to the network, which means a green
/// "up to date" can be WRONG -- and the measurement says how often.
///
/// On the reporting machine, only one of the repositories in the scan
/// root is fresh by the app's own one-hour threshold. Thirteen have never
/// been fetched at all; fifteen are over 30 days stale and five over a
/// year. Sampled against `git ls-remote`, half of the eight checked would
/// have rendered a green "up to date" while actually behind -- verified
/// concretely on one repository sitting two commits behind its true
/// remote tip.
///
/// The obvious fix is to fetch. It is refused, and `scan.rs` records the
/// two reasons the SCAN already refuses it (#788): cost, and hanging up
/// to the full 30s `GIT_TIMEOUT` on an unreachable remote. Fetching on
/// mount would be a network call to every remote in the scan root because
/// somebody opened a view, and thirteen of those remotes have never been
/// contacted from this machine at all -- several are third-party clones
/// that may not be reachable. MEASURED, `git fetch --dry-run` averages
/// 1.98s per repository here, which is roughly 75s serial against a view
/// whose sibling scan lists in under a second.
///
/// So neither: the table reads locally, and QUALIFIES every verdict by
/// the age of the refs behind it. `upstreamReasonAged` and
/// `upstreamToneAged` are used rather than the plain forms, and they
/// exist precisely because #788 established that an unqualified green "up
/// to date" on this exact claim is the defect. On the reporting machine
/// that greys almost every row -- which is the honest picture, not a
/// regression. Per-repository `fetch_refs` is already the remedy, and a
/// row refreshes its own verdict the moment its refs are.
///
/// # It adds no scan (#1029), and one cheap query per row (#1042)
///
/// `useWorktrees` is the same query the Worktrees view and both sidebars
/// read, under the same `queryKey`. The rows are a projection over it, so
/// the LISTING costs nothing additional when the table and the sidebar
/// are both on screen, and cannot drift from the row it summarises.
///
/// The VERDICTS are not a projection, and the claim that they were is
/// what #1042 is. This file used to say the table "adds no command, no
/// query and no git invocation" because `upstream` was supposedly
/// "already on the wire". It never was: that field is written in exactly
/// one place on the Rust side, inside `classify`, which the walk runs
/// only behind a `with_safety` flag that every production caller passes
/// `false` for -- the deep path that would have set it is `#[cfg(test)]`.
/// So the Status column was a permanent skeleton, and nothing appeared in
/// the logs because there was no failing call; there was no call.
///
/// `useRepoUpstreams` is the correction, and it keeps the property the
/// old claim was actually protecting. It classifies the MAIN CHECKOUT
/// ONLY -- the one worktree this table has a row for -- rather than every
/// worktree of every repository, which would be ~295 of them on the
/// reporting machine to render 38 rows, and it is one query PER
/// REPOSITORY so a slow repository cannot hold the others on skeletons.
/// It still does not fetch.
///
/// Not polled, deliberately. The two pollers in this app read cheap local
/// state answering questions that change on their own; this data changes
/// when the user acts or when a remote moves, and the second is invisible
/// without the network call this table is not making. Polling a
/// multi-second scan to observe changes it cannot see is cost with no
/// information behind it.
export function AllRepositoriesTable() {
  // `isError` and `refetch` as well as the data (#846) -- and
  // `unreadable`, which is the whole of #1028's requirement now that the
  // reporting form exists.
  const { data, unreadable, isLoading, isError, error, refetch } = useWorktrees();

  const scanned = useMemo(() => repoOverviewRows(data ?? []), [data]);

  // The verdicts, which the scan does NOT carry (#1042).
  //
  // This component's own doc used to say the table "adds no command, no
  // query and no git invocation", on the reading that `upstream` was
  // already on the wire from the scan. It was not, and the Status column
  // was an indefinite skeleton for it: `Worktree::upstream` is written
  // only inside the Rust `classify`, which the walk runs behind a flag
  // every production caller passes `false` for, and the path that would
  // have set it is test-only. So the field was `None` for every row,
  // always, and no amount of waiting was ever going to change it.
  //
  // What the claim was RIGHT about is the cost, and that is preserved:
  // this classifies the MAIN CHECKOUT only, one query per repository, and
  // it does not fetch. Not `useWorktreeSafety`, which would classify
  // every worktree of every repository -- ~295 of them on the reporting
  // machine, to render 38 rows.
  //
  // Keyed by repository PATH, which is the row's identity and unique
  // across scan roots where `name` is not.
  //
  // A BARE repository is excluded (#1142): it has no working tree, so
  // `classify_main_checkout` would be asked to judge a checkout that is
  // not there. `Safety::MainCheckout` means "this is the main checkout,
  // protect it", which is a claim about something a mirror does not
  // have -- and an answer computed about nothing is exactly the
  // confident wrong verdict this table's other guards exist to prevent.
  const paths = useMemo(() => scanned.filter((r) => !r.bare).map((r) => r.path), [scanned]);
  const { upstreams } = useRepoUpstreams(paths);

  // The resolved verdict laid over the row, and `?? row.upstream`
  // underneath it rather than `?? null`: the scan's own value is what
  // this falls back to, so if a future change ever does populate it the
  // table reads it rather than ignoring it. An absent entry stays
  // whatever the scan said, which today is `null` -- the PENDING
  // skeleton, correct for exactly as long as the query is in flight and
  // no longer, which is the whole of #1042.
  //
  // NOT memoised, deliberately. `upstreams` is rebuilt on every render by
  // design (the hook says why), so a `useMemo` over it would recompute
  // every render anyway while adding a dependency array to get wrong --
  // and the work is one spread per repository over the scan root, 38 on
  // the reporting machine. `repoCurrency` below follows for the same
  // reason: it is one pass over those same rows.
  const rows = scanned.map((row) => ({
    ...row,
    upstream: upstreams.get(row.path) ?? row.upstream,
  }));
  const currency = repoCurrency(rows);

  // The error arm FIRST, before the empty arm (#846). With `data`
  // undefined on a rejection `rows` is `[]`, so an empty-state check
  // placed above this would claim the scan found nothing -- which is
  // `RepoPickerSidebar`'s exact defect, whose copy sent a user to fix
  // settings that were never wrong.
  if (isError) {
    return (
      <QueryError
        title="Could not scan for repositories."
        message={errorMessage(error)}
        onRetry={() => void refetch()}
      />
    );
  }

  if (isLoading) {
    return (
      <p className="px-4 py-8 text-center text-sm text-[#8b949e]">Scanning for repositories…</p>
    );
  }

  if (rows.length === 0) {
    return (
      <div className="px-4 py-8 text-center text-sm text-[#8b949e]">
        {/* The diagnosis is hedged where `RepoPickerSidebar`'s was not,
            and `PartialScanNotice` above may already be saying why. */}
        <p>No repositories found in the scanned folders.</p>
      </div>
    );
  }

  return (
    <div>
      {/* ABOVE the table, not instead of it: the repositories that did
          read are real and worth showing. No retry -- an unreadable path
          is unreadable for a reason a second identical walk will not
          change (#951, #1028). */}
      <PartialScanNotice
        unreadable={unreadable}
        consequence="this table is a floor rather than a complete census"
      />
      {/* A VISIBLE heading, not only the caption below (#1015).
          The caption is `sr-only` and names the table for a screen
          reader; a sighted user landing on this view sees a bare table
          with no statement of what it covers. "All Repositories" is the
          name the view was asked for, and it is also what makes the
          table addressable -- a test, or a person, can ask for it by
          name rather than by its first row. */}
      <h2 className="text-sm font-semibold text-[#e6edf3]">All Repositories</h2>
      <RepoCurrencySummary currency={currency} />
      {/* The button belongs HERE, above the rows it acts on, and it is
          handed the same `unreadable` the notice above renders (#1025).
          `rows.length` rather than a literal: the label names the number
          it can actually see when the scan fell short, and a measured
          figure rendered as a string is correct on the day it is written
          and describes someone else's machine from then on (#969).

          What it is NOT handed is the table's verdicts. Every precondition
          is re-derived inside the command at the moment of acting, per
          repository -- this table's currency column is minutes old by the
          time the button is pressed, and it is a reason to OFFER the
          action, never the basis for performing it. */}
      <UpdateAllButton count={rows.length} unreadable={unreadable} />
      <table className="w-full border-collapse text-sm">
        <caption className="sr-only">
          Every repository found in the scanned folders, and how its main checkout stands against
          the ref it tracks.
        </caption>
        <thead>
          <tr className="border-b border-[#30363d] text-left text-xs text-[#8b949e]">
            <th scope="col" className="px-4 py-2 font-normal">
              Repository
            </th>
            <th scope="col" className="px-4 py-2 font-normal">
              Branch
            </th>
            <th scope="col" className="px-4 py-2 font-normal">
              Compared against
            </th>
            <th scope="col" className="px-4 py-2 font-normal">
              Status
            </th>
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <RepoRow key={row.path} row={row} />
          ))}
        </tbody>
      </table>
    </div>
  );
}

/// The ratio, with its denominator stated (#1027).
///
/// "N of M up to date" over a set where some were never compared is a
/// number that cannot be true, so the denominator here is the repositories
/// that HAVE an upstream, and the rows outside that population are
/// counted separately rather than folded in or silently dropped.
///
/// Every number is computed from live data. None is a literal -- a
/// measured figure rendered as a string is correct on the day it is
/// written and describes someone else's machine from then on (#969).
function RepoCurrencySummary({
  currency,
}: {
  currency: { current: number; compared: number; total: number };
}) {
  const uncompared = currency.total - currency.compared;
  return (
    <p className="border-b border-[#30363d] px-4 py-2 text-xs text-[#8b949e]">
      {currency.current} of {currency.compared} repositories with an upstream are up to date with
      it, as of each one&rsquo;s last fetch.
      {uncompared > 0 ? (
        <>
          {" "}
          {uncompared} {uncompared === 1 ? "repository has" : "repositories have"} no upstream to
          compare against and {uncompared === 1 ? "is" : "are"} not counted here.
        </>
      ) : null}
    </p>
  );
}

/// One repository.
///
/// The Worktrees first row renders seven things and this summarises three
/// of them, because the SAFETY verdict on that row carries no information
/// (#1015): `classify` returns `Safety::MainCheckout` for the main
/// checkout before it looks at `git status`, so a column sourced from it
/// would print the same sentence on every row. The load-bearing field is
/// `upstream`.
function RepoRow({ row }: { row: RepoOverviewRow }) {
  return (
    <tr className="border-b border-[#21262d]">
      <td className="px-4 py-2 font-mono text-[#e6edf3]">{row.name}</td>
      <td className="px-4 py-2 text-xs text-[#8b949e]">
        {/* A detached HEAD has no branch NAME. `Upstream::Detached`
            already says so in the status cell, so this says only that
            there is nothing to name. */}
        {row.branch ?? <span className="italic">detached</span>}
      </td>
      <td className="px-4 py-2 font-mono text-xs text-[#8b949e]">
        {/* Never the string `origin/main` when the field is absent. That
            hardcode is wrong for over 10% of the repositories on the
            reporting machine, and inventing it here would be a confident
            answer about which branch was compared (#1026). */}
        {row.defaultRef ?? <span className="font-sans italic">could not resolve</span>}
      </td>
      <td className="px-4 py-2">
        <UpstreamCell row={row} />
      </td>
    </tr>
  );
}

/// The verdict, or the absence of one (#1027).
///
/// `null` is the case this codebase keeps getting wrong. It means "not
/// computed yet", not "current", and it is rendered as a PENDING skeleton
/// -- the same distinction the Worktrees page draws between
/// `Safety::Pending` ("not checked yet", a skeleton) and `Safety::Unknown`
/// ("checked, could not decide", a failure). Collapsing them is what made
/// every unclassified row claim its check had failed; rendering `null` as
/// a verdict would do the inverse and worse, claiming every repository is
/// up to date before anything was measured.
///
/// Every other state renders `Upstream`'s OWN prose through
/// `upstreamReasonAged`. No new vocabulary: that function already handles
/// all seven states and is already what the first row shows, so the table
/// and the row it summarises cannot word the same fact differently.
function UpstreamCell({ row }: { row: RepoOverviewRow }) {
  if (row.upstream === null) {
    return (
      <>
        <span
          aria-hidden="true"
          className="inline-block h-3 w-40 rounded bg-[#30363d] align-middle motion-safe:animate-pulse"
        />
        <span className="sr-only">Checking</span>
      </>
    );
  }
  // `scannedAt` is NOT passed, so both helpers use their `new Date()`
  // default. That is correct here and not an oversight: `WorktreesPage`
  // pins an anchor from the query's own `dataUpdatedAt` because its rows
  // re-render continuously beside a header reading the same age, and the
  // two must not drift. This cell has no such neighbour, and the age it
  // shows is coarse -- hours and days.
  return (
    <span className={upstreamToneAged(row.upstream, row.fetchedAt)}>
      {upstreamReasonAged(row.upstream, row.fetchedAt)}
    </span>
  );
}

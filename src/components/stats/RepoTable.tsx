import { Card } from "@/components/ui/card";
import { useFilters } from "@/store/filters";
import type { RepoCount } from "@/types/pr";

/// Merged-PR distribution across repositories.
///
/// Clicking a row scopes the app to that repo and switches to the list.
/// "Which of my many repos is this happening in" is the whole reason this
/// table exists, so it has to be a way in, not just a readout.
export function RepoTable({
  repos,
  sampleSize,
  hint,
}: {
  repos: RepoCount[];
  /// How many merged pull requests the figures are drawn from, when they are
  /// drawn from a fixed recent SAMPLE. Omit on a scope page, where the
  /// population is the whole window rather than a sample -- see `hint`.
  sampleSize?: number;
  /// What the shares are shares OF, in the caller's words, overriding the
  /// sample wording.
  ///
  /// The unscoped page's figures come from the last N merged pull requests,
  /// and that caveat is load-bearing there: the bars "are the most visually
  /// assertive element on the page and had the weakest footing". A scope
  /// page's come from the whole window, so repeating "share of recent merges"
  /// there would understate a complete measurement -- and on a PARTIAL one
  /// the caller knows why it is partial, which this component does not.
  hint?: string;
}) {
  // `setView`, which this did not call -- the whole of the dead click
  // (#852). See the `onClick` below.
  const { setFilter, setView } = useFilters();
  const total = repos.reduce((sum, r) => sum + r.merged, 0);

  return (
    <Card className="px-4">
      <div className="text-sm font-semibold">Merged by repository</div>
      {/* The bars are the most visually assertive element on the page and
          had the weakest footing: they are shares of a SAMPLE of recent
          merges, not of all time. The delta and insight cards already say
          so; this was the holdout. */}
      <div className="text-xs text-[#8b949e]">
        {hint ??
          (sampleSize
            ? `share of the last ${sampleSize} merged`
            : "share of recent merges")}
      </div>
      {repos.length === 0 ? (
        <div className="py-8 text-center text-sm text-[#8b949e]">
          No merged pull requests in this sample.
        </div>
      ) : (
        <div className="mt-3 flex flex-col gap-1">
          {repos.map((r) => {
            const pct = total === 0 ? 0 : Math.round((r.merged / total) * 100);
            return (
              <button
                key={r.repo}
                type="button"
                // A DEAD CLICK before this (#852). It called `setFilter`
                // and `setPanel("list")` and never `setView`, so nothing
                // navigated: the user stayed on PR Stats, and since
                // `setFilter` writes into `filtersByView[s.view]` the repo
                // landed in `pr-stats`' own filter set -- which `StatsPage`
                // does not read. Clicking the "Merged by repository" bars
                // did nothing at all, while the doc comment above promised
                // "Clicking a row scopes the app to that repo and switches
                // to the list… it has to be a way in, not just a readout."
                //
                // `setView` FIRST, then `setFilter`, and the order is
                // load-bearing: `setFilter` writes to whichever view is
                // active when it runs, so the reverse order would put the
                // repo in `pr-stats`' slot again -- the same bug with an
                // extra call. `setView` also clears the selection and the
                // working set, which is correct here: a set assembled on
                // one view means nothing on another.
                //
                // `setPanel` is GONE rather than reordered. It was setting
                // `"list"`, already the only value My PRs reads, so the
                // call was a no-op dressed as navigation -- and `panel` has
                // been removed from the store (see `store/filters.ts`),
                // since "a `panel` nobody routes on would be a silent
                // no-op" was the rule it was already breaking.
                onClick={() => {
                  setView("my-prs");
                  setFilter("repo", r.repo);
                }}
                className="flex items-center gap-3 rounded px-2 py-1.5 text-sm hover:bg-[#161b22]"
              >
                {/* `min-w-0 flex-1` rather than `w-56 shrink-0`, which
                    is the pattern `Outliers.tsx` uses one directory over
                    for the same row shape (#942): the long field
                    absorbs the slack and truncates, the short ones are
                    bare `shrink-0`.

                    `w-56` was 224px of a 356px row minimum in a 326px
                    card at 390px, so the row overflowed by 30px and the
                    bar -- the only column that could give -- was
                    squeezed to ZERO width for every repo. A table whose
                    doc comment calls itself "a way in, not just a
                    readout" cannot answer "which repo is this happening
                    in" with five identical zero-width bars, and because
                    `Card` is `overflow-hidden` the clipped `%` column
                    was silently gone rather than scrollable.

                    `title` is added with the truncation: the repo name
                    is what the row is ABOUT, and the desktop kept its
                    full width, so the phone needs the name recoverable
                    rather than merely shortened. */}
                <span className="min-w-0 flex-1 truncate text-left" title={r.repo}>
                  {r.repo}
                </span>
                <span
                  // `basis-16 shrink-0`, not `flex-1`: beside a
                  // `flex-1` name the bar would split the slack evenly
                  // and both would be starved at phone width. 64px is
                  // the floor at which the share comparison still
                  // reads, and this is the column that must never
                  // return to zero.
                  className="relative h-1.5 basis-16 shrink-0 overflow-hidden rounded bg-[#21262d]"
                >
                  <span
                    className="absolute inset-y-0 left-0 rounded bg-[#3fb950]"
                    style={{ width: `${pct}%` }}
                  />
                </span>
                {/* Bare `shrink-0`: both figures size themselves, per
                    `Outliers`. `tabular-nums` still keeps the counts
                    aligned down the column. */}
                <span className="shrink-0 text-right tabular-nums">{r.merged}</span>
                <span className="shrink-0 text-right text-xs text-[#8b949e]">{pct}%</span>
              </button>
            );
          })}
        </div>
      )}
    </Card>
  );
}

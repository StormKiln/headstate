import type { Upstream, WorktreeRepo } from "@/types/pr";

/// One row of the All Repositories table: a repository, and how its main
/// checkout stands against the ref it tracks (#1015).
///
/// # Why this is a projection and not a scan
///
/// `useWorktrees` delivers `WorktreeRepo[]` under `queryKey: ["worktrees"]`,
/// each repo already carrying `fetched_at`, `default_ref`, and a
/// `worktrees` array whose `is_main` entry is the row this summarises. So
/// the LISTING adds no command, no query and no git invocation (#1029).
/// MEASURED on the reporting machine, the local reads behind it are 73ms
/// mean per repository and 2.77s serial for the whole scan root; the table
/// inherits that cost rather than paying it a second time.
///
/// # `upstream` is the exception, and assuming otherwise was #1042
///
/// This block used to claim that the `is_main` entry "already has
/// `upstream` populated", quoting `Worktree::upstream`'s own doc about
/// being "computed for EVERY worktree". That doc describes `classify`,
/// and the walk behind `useWorktrees` does not run it: `collect_inner`
/// calls `classify` only when `with_safety` is true, and the only
/// production entry point, `scan_dirs_fast_reporting`, passes `false`.
/// The deep path that would have populated it is `#[cfg(test)]`.
///
/// So `main.upstream` below is `null` on every row that comes out of the
/// scan, permanently, and the Status column was an indefinite skeleton
/// for it. The verdicts now come from `useRepoUpstreams`, which
/// classifies the main checkout per repository and lays its answer over
/// this projection. The rest of the row is still projected, and still
/// cannot drift from the Worktrees first row it summarises.
export interface RepoOverviewRow {
  /// Directory name, the table's leftmost cell and its sort key.
  name: string;
  /// Absolute path to the main checkout. The row's identity -- unique
  /// across the scan roots, where `name` is not (two scan roots may each
  /// hold an `api`).
  path: string;
  /// The local branch the main checkout is on, or `null` for a detached
  /// HEAD.
  ///
  /// Null rather than the word "detached": this is the branch NAME, and
  /// a detached checkout has none. The prose for that state comes from
  /// `Upstream::Detached` through `upstreamReasonAged`, which already
  /// says it, so spelling it here too would be a second vocabulary for
  /// one fact.
  branch: string | null;
  /// The ref the verdict is measured against -- `origin/main`,
  /// `origin/master`, a bare local name -- or `null` when it could not be
  /// resolved (#1026).
  ///
  /// Carried from `Repo::default_ref`, never assumed. `origin/main` is
  /// the wrong answer for over 10% of the repositories on the reporting
  /// machine.
  defaultRef: string | null;
  /// How the main checkout stands against its upstream, or `null` for
  /// "not computed yet".
  ///
  /// `null` is PENDING, not current (#1027). `Worktree::upstream` is an
  /// `Option` for a reason and this preserves it: a table rendering it as
  /// "up to date" would claim every repository is current before anything
  /// was measured, which is the inverse of the `Safety::Pending` versus
  /// `Safety::Unknown` distinction the Worktrees page already makes.
  upstream: Upstream | null;
  /// When this repository's refs were last fetched, RFC 3339, or `null`
  /// for never fetched / unreadable.
  ///
  /// Feeds `upstreamReasonAged` and `upstreamToneAged` so the verdict
  /// carries its own age. Read from `Repo::fetched_at` rather than
  /// re-`stat`ing `FETCH_HEAD`, which would be a second definition of
  /// freshness (#1021).
  fetchedAt: string | null;
}

/// The main checkout's row, or `undefined` when the scan has not listed
/// one.
///
/// `is_main` rather than "the first worktree": `sort_for_sidebar` orders
/// the repositories, not the worktrees within one, and an orphaned
/// gitdir's synthetic entry is explicitly `is_main: false`. Picking by
/// position would make that orphan the repository's own checkout.
function mainCheckout(repo: WorktreeRepo) {
  return repo.worktrees.find((w) => w.is_main);
}

/// Project the worktree scan into one row per repository (#1029).
///
/// # What the row carries, and what it deliberately does not
///
/// The Worktrees first row renders seven things (#1015). This summarises
/// the ones that carry information about currency and drops the ones that
/// do not:
///
/// **The safety verdict is dropped, and that is the finding.** `classify`
/// returns `Safety::MainCheckout` for the main checkout BEFORE it looks at
/// `git status`, so on this row the verdict is the constant "the
/// repository's main checkout" -- the same string for all 38 rows on the
/// reporting machine. `WorktreesPage.tsx` says so about its own button.
/// A column sourced from it would be a column of one repeated sentence.
///
/// **The load-bearing field is `upstream`**, and it is kept whole. It is a
/// seven-state enum, not a boolean, and `upstream` is itself an Option on
/// top of that -- eight answers, of which exactly one is "up to date".
///
/// Size and the two action buttons belong to the row, not to a summary of
/// it, so they are not projected.
///
/// # Repositories with no main checkout
///
/// Dropped, not rendered with a blank verdict. The only producer of one is
/// the orphaned-gitdir branch, whose synthetic worktree is `is_main:
/// false` because there is no repository left to run git in -- it has no
/// upstream, no default ref and no branch, so every cell of its row would
/// be empty. It belongs to the Worktrees view, which can act on it; a
/// currency table has nothing to say about it.
///
/// Order is the scan's, which `sort_for_sidebar` has already made stable.
/// Re-sorting here would be a second ordering for the same list, and the
/// sidebar beside this table uses the first one.
export function repoOverviewRows(repos: readonly WorktreeRepo[]): RepoOverviewRow[] {
  const rows: RepoOverviewRow[] = [];
  for (const repo of repos) {
    const main = mainCheckout(repo);
    if (!main) continue;
    rows.push({
      name: repo.name,
      path: repo.path,
      // `""` is what the porcelain parser leaves for a detached HEAD, and
      // it is normalised to null HERE rather than at every reader.
      branch: main.branch === "" ? null : main.branch,
      // `?? null` on an OPTIONAL FIELD, not on a value: the type marks
      // `default_ref` optional so fixtures need not enumerate it, and
      // `undefined` and `null` both mean "not resolved". This is not the
      // `?? 0` coercion #1027 forbids -- that one invents a measurement,
      // this one folds two spellings of "absent" into one.
      defaultRef: repo.default_ref ?? null,
      // Preserved as-is, INCLUDING null. See `RepoOverviewRow.upstream`.
      upstream: main.upstream,
      fetchedAt: repo.fetched_at ?? null,
    });
  }
  return rows;
}

/// How many repositories are up to date, and how many were actually
/// compared (#1027).
///
/// # Why this is a ratio with a stated denominator and not a count
///
/// "34 of 38 up to date" over a set where six were never compared is a
/// number that cannot be true. Six of 38 repositories on the reporting
/// machine are `Untracked` -- local-only branches with no upstream to
/// stand against -- and `Upstream::Untracked`'s own doc refuses the
/// reading a bare count would give it: *"Normal, not an error -- and
/// distinctly not 'up to date', which is what a bare zero would imply."*
///
/// So `compared` is the denominator, and it EXCLUDES every state where no
/// comparison happened. A caller that renders `current` over `compared`
/// is stating a ratio about the repositories that have an upstream, which
/// is the only population the question applies to.
///
/// `total` is carried alongside so the caller can say how many rows the
/// ratio does not cover, rather than leaving the reader to subtract.
///
/// # The states, and which side of the line each falls
///
/// Compared, because a remote ref was read and the answer is a number:
/// `current`, `ahead`, `behind`, `diverged`.
///
/// Not compared, because no remote ref was consulted at all:
/// - `untracked` -- there is no upstream. Not a failure, not currency.
/// - `detached` -- no branch, so no `@{u}` to ask about.
/// - `unknown` -- git was asked and could not answer. This is the
///   `caches/mod.rs:550` case: *"An idle time we could not read is not
///   evidence that anything is disposable."*
/// - `null` -- not computed yet. The scan is still running.
///
/// # No `?? 0`, anywhere
///
/// #967 is this bug in the shape this table is most exposed to: a missing
/// `%(ahead-behind:)` became `.unwrap_or(0)` at two call sites, and the
/// row rendered "Not merged — 0 commits not on the default branch" beside
/// a delete checkbox. The counts became `Option<u64>` so the coercion
/// would have to be written deliberately. Nothing here writes it.
export interface RepoCurrency {
  /// Repositories whose upstream comparison says `current`.
  ///
  /// The age of the refs behind that verdict is NOT considered here, and
  /// that is deliberate: the qualification is per-row and visible, made by
  /// `upstreamToneAged` on the verdict itself. Folding staleness into this
  /// count would make it a different and unstated question -- and on the
  /// reporting machine, where 37 of 38 rows rest on non-fresh refs, it
  /// would read as "1 up to date" for a set that is mostly fine.
  current: number;
  /// Repositories where a comparison actually happened. The denominator.
  compared: number;
  /// Every row, compared or not. Never a denominator for `current`.
  total: number;
}

export function repoCurrency(rows: readonly RepoOverviewRow[]): RepoCurrency {
  let current = 0;
  let compared = 0;
  for (const row of rows) {
    // `null` first and by identity, not by a truthiness check that would
    // also swallow a future zero-valued state.
    if (row.upstream === null) continue;
    switch (row.upstream.kind) {
      case "current":
        current += 1;
        compared += 1;
        break;
      case "ahead":
      case "behind":
      case "diverged":
        compared += 1;
        break;
      // `untracked`, `detached` and `unknown` fall through counting
      // NEITHER. Listed explicitly rather than left to a default, so
      // adding an eighth state to `Upstream` is a compile error here
      // rather than a silent assignment to one side of the ratio.
      case "untracked":
      case "detached":
      case "unknown":
        break;
    }
  }
  return { current, compared, total: rows.length };
}

/// Where a row sits when the table is ordered by currency (#1027).
///
/// # Unknowns are not ranked, they are grouped
///
/// *"If the table sorts by currency, the states with no comparison have
/// no position on that axis; they go in their own group rather than being
/// given an implied zero."*
///
/// An implied zero is the whole defect: it would sort the six local-only
/// repositories in among the up-to-date ones, at the exact position that
/// reads as "nothing to do here". So there are three bands, and the
/// uncompared band is last -- after the answers, not interleaved with
/// them.
///
/// Within the "needs attention" band the rows keep the scan's order rather
/// than being ranked by commit count: a repository 1 commit behind and one
/// 40 behind are both simply behind, and ordering by magnitude would put a
/// number the refs may understate (`behind` is only ever an
/// under-statement when refs are stale) in charge of the row order.
export function currencyRank(upstream: Upstream | null): number {
  if (upstream === null) return 2;
  switch (upstream.kind) {
    case "behind":
    case "diverged":
      return 0;
    case "ahead":
    case "current":
      return 1;
    case "untracked":
    case "detached":
    case "unknown":
      return 2;
  }
}

/// The rows ordered by currency, uncompared ones last (#1027).
///
/// Lives here rather than beside the table for the reason `lib/repos.ts`
/// gives about `repoCounts`: a component exporting a pure helper is a
/// layering smell, and `react-refresh/only-export-components` makes it a
/// lint warning besides. The table keeps the scan's order by default and
/// a host view can offer this as a sort.
export function sortByCurrency(rows: readonly RepoOverviewRow[]): RepoOverviewRow[] {
  // A copy: `rows` comes from a `useMemo` over query data, and sorting in
  // place would mutate the cache React hands back to every other reader.
  return [...rows].sort((a, b) => currencyRank(a.upstream) - currencyRank(b.upstream));
}

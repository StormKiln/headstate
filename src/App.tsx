import { emit } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Menu } from "lucide-react";
import { lazy, Suspense, useEffect, useRef, useState } from "react";
import {
  usePullRequests,
  useRefreshFromGesture,
  useRefreshRequested,
  useReviewing,
  useReviewingCount,
  useViewCadence,
  useTruncation,
  useIncomplete,
  useReviewShortfall,
  usePollError,
  useUpdateRunOutcome,
  useUpdateRunResume,
  useUiPrefs,
} from "./api/hooks";
import { useReviewingDiag } from "./api/diag";
import { useScrollReset } from "./lib/scrollReset";
import { FilterBar } from "./components/FilterBar";
import { NudgeWizard } from "./components/NudgeWizard";
import { PrioritiesStrip } from "./components/PrioritiesStrip";
import { ReadyStrip } from "./components/ReadyStrip";
import { CourtStrip } from "./components/CourtStrip";
import { PrDetailView } from "./components/PrDetailView";
import { BulkBar } from "./components/BulkBar";
import { PrList } from "./components/PrList";
import { ReviewChips } from "./components/ReviewChips";
import { TriageChips } from "./components/TriageChips";
import { WorktreeSidebar } from "./components/WorktreeSidebar";
import { ArtifactsPage } from "./components/ArtifactsPage";
import { ArtifactSidebar } from "./components/ArtifactSidebar";
import { PackagesPage } from "./components/PackagesPage";
import { ClaudeMdPage } from "./components/ClaudeMdPage";
import { ClaudeCodePage } from "./components/ClaudeCodePage";
import { ClaudeCodeSidebar } from "./components/ClaudeCodeSidebar";
import { RepoPickerSidebar } from "./components/RepoPickerSidebar";
import { RepositoriesPage } from "./components/RepositoriesPage";
import { DockerPage } from "./components/DockerPage";
import { DockerSidebar } from "./components/DockerSidebar";
import { BranchesPage } from "./components/BranchesPage";
import { WorktreesPage } from "./components/WorktreesPage";
import { QueryError, errorMessage } from "./components/QueryError";
import { RepoSidebar } from "./components/RepoSidebar";
import { StatsSidebar } from "./components/StatsSidebar";
import { StatusBar } from "./components/StatusBar";
import { SystemHealthSidebar } from "./components/SystemHealthSidebar";
import { ConnectionBanner } from "./components/ConnectionBanner";
import { StaleRibbon } from "./components/StaleRibbon";
import { IS_DESKTOP_BUILD, IS_MOBILE_BUILD } from "./lib/target";
import { usePullToRefresh } from "./lib/usePullToRefresh";
import { PullIndicator } from "./components/PullIndicator";
import { Sheet, SheetContent, SheetTitle } from "./components/ui/sheet";
import { applyFilters, hasActiveFilters, sortPrs } from "./lib/derive";
import { shortcutFor } from "./lib/shortcuts";
import { activeRowCursor, nextCursor, type RowCursorTarget } from "./lib/rowCursor";
import { useIsMobile } from "./lib/useIsMobile";
import { relativeSeconds } from "./lib/time";
import { MOBILE_HIDDEN_VIEWS, useActiveFilters, useFilters } from "./store/filters";

/// The chart-carrying views, split off the launch chunk (#838, #921).
///
/// # An update from #921, which added the third one
///
/// This comment said "the two heavy views" and "the three chunks" until
/// the Claude Code overview joined them. Measured on the same
/// `VITE_TARGET=mobile yarn build`, and the shape of the output CHANGED
/// rather than simply gaining a row:
///
/// ```text
/// ClaudeOverviewPage    28.88 kB   recharts mentions: 2
/// StatsPage             45.72 kB                      1
/// SystemHealthPage      53.26 kB                      0
/// chart                339.96 kB                     15   <- NEW, shared
/// index (launch)       968.41 kB                      0   <- the claim
/// ```
///
/// With two lazy consumers of `ui/chart` the bundler hoists the charting
/// library into its OWN shared chunk rather than inlining it into one
/// route. So the `15 / 0 / 0` figure quoted below is now `15` in `chart`
/// and `0` in the launch chunk, which is the property that was ever
/// load-bearing -- and the library is fetched once for both routes rather
/// than twice.
///
/// The regression, measured by making the new route eager on purpose:
/// the launch chunk goes **968.41 kB -> 1,337.75 kB** (+369 kB, +38%) and
/// its `grep -c recharts` goes **0 -> 16**, with the shared `chart` chunk
/// disappearing entirely into the launch path. `App.lazy.test.tsx` fails
/// on that source shape, naming the route.
///
/// # Measured, before and after
///
/// The frontend shipped as ONE chunk with no dynamic imports at all, so a
/// launch parsed every view before painting the PR list -- on a tray app
/// whose value proposition is a fast badge. With the two routes split:
///
/// | | launch chunk | time to React's first commit |
/// |---|---|---|
/// | before | 1,378,820 B | 33.1 ms median |
/// | after | 945,919 B | 24.8 ms median |
///
/// A 432,901-byte (31%) drop on the launch path and ~8ms off the median,
/// which is about 25%. Headless Chrome, HTTP cache disabled, one browser,
/// three warm-up loads discarded, 21 loads per side interleaved A/B so
/// machine drift hits both equally; a second independent run of the same
/// harness agreed (33.5 / 26.0 mean). FCP did NOT move -- 36ms to 32ms,
/// inside the noise -- and that is expected rather than disappointing:
/// `index.html` paints its own styled shell before any module evaluates,
/// so FCP never saw the bundle. The number that moves is the one a user
/// waits on, which is when the list appears.
///
/// # What is actually in the split chunks
///
/// `recharts` (9.3 MB on disk) now lands in the StatsPage chunk and is
/// absent from the launch chunk -- verified on the build output, not
/// inferred: `grep -c recharts` over the three chunks gives 15 / 0 / 0.
///
/// One correction to #838's own description while I was here. It names
/// four recharts importers; only ONE actually imports it. `ui/chart.tsx`
/// does, and `stats/ActivityChart` imports both it and recharts directly.
/// `stats/Leaderboard` and `SystemHealthPage` only MENTION recharts, in
/// comments explaining why each draws its own SVG instead
/// (`Leaderboard.tsx:59-65`, `SystemHealthPage.tsx:173`). The split is
/// still right -- ActivityChart lives behind the Stats route, which is
/// where the library went -- but the health page's 51 kB chunk is its own
/// code rather than a charting library, and a reader comparing the chunk
/// sizes against the issue would otherwise find them inexplicable.
///
/// # The phone gets more out of this than the desktop
///
/// Before #838 the StatsPage code sat in the one chunk every phone launch
/// parsed. It is now behind the route boundary, so a phone fetches the
/// 382 kB chunk -- recharts included -- only when the user opens Stats,
/// and a launch that never goes there never pays for it.
///
/// This paragraph said something stronger until #863: that the companion
/// could not reach the Stats route AT ALL, so the chunk was never fetched
/// rather than merely fetched late. That followed from `pr-stats` being in
/// `MOBILE_HIDDEN_VIEWS`, and #863 emptied that set. The split is what
/// makes shipping Stats to the phone cheap -- on-demand instead of on the
/// launch path -- but "never fetched" is no longer true, and the deferral
/// now matters MORE on the phone than it did when it was a guarantee.
///
/// System Health is 51 kB and carries no charting library, per the
/// correction above.
///
/// Verified on the mobile build rather than assumed: `VITE_TARGET=mobile
/// yarn build` produces the same three chunks.
///
/// # Why the ROUTE boundary and not the chart components
///
/// `React.lazy` needs a component boundary already gated behind a user
/// action, and these two are: both are reached only by clicking a view.
/// Splitting lower down -- lazying `ActivityChart` inside a synchronously
/// loaded `StatsPage` -- would leave the page's own code on the launch
/// path (381 kB of the 433 kB moved), and would put a Suspense boundary
/// in the middle of a layout that deliberately renders its sections as
/// each query lands (`StatsPage.tsx:12-22`). The route boundary changes
/// no component's internals at all.
///
/// # Why the SIDEBARS are not lazy
///
/// Neither `StatsSidebar` nor `SystemHealthSidebar` imports charting code
/// (verified by grep). Lazying them would add two more Suspense
/// boundaries to move almost nothing, and `SystemHealthSidebar` is
/// imported BY `SystemHealthPage` anyway (`SystemHealthPage.tsx:47` reads
/// `healthPagesFor` from it), so splitting it would only duplicate it.
///
/// # Why not `manualChunks`
///
/// `vite.config.ts` still has no `rollupOptions`, and does not need one:
/// the route boundary is a real boundary in the import graph, so the
/// bundler derives the split from the code. A `manualChunks` function
/// would be a second, hand-maintained description of the same fact, and
/// one that goes stale silently when an import moves. `src/App.lazy.test.tsx`
/// guards the property instead.
///
/// # `lucide-react` tree-shakes; checked, not assumed
///
/// #838 also asked whether lucide's 44 MB on disk reaches the bundle. It
/// does not. The package ships 2,057 icon modules; `src/` imports 52
/// distinct icons, whose own modules contain 109 `<path>` elements between
/// them, and the built chunks hold 118 SVG path literals -- the 109 plus a
/// handful from the app's hand-drawn SVG. If the set had shipped the count
/// would be in the thousands. Nothing to do here, and worth recording so
/// the 44 MB does not get re-investigated.
const StatsPage = lazy(() =>
  import("./components/StatsPage").then((m) => ({ default: m.StatsPage })),
);
const SystemHealthPage = lazy(() =>
  import("./components/SystemHealthPage").then((m) => ({
    default: m.SystemHealthPage,
  })),
);
/// The THIRD chart-carrying route (#921), and the reason the comment above
/// says "two" no longer holds.
///
/// `ClaudeOverviewPage` reaches `recharts` through `stats/SessionsChart` ->
/// `ui/chart`, so it belongs behind the same boundary for the same measured
/// reason: the launch chunk is 31% smaller with the charting library off
/// it, and the median time to React's first commit ~8ms faster.
///
/// The trap #921 names, restated because it is what makes this line easy to
/// get wrong: `App.lazy.test.tsx` reads `App.tsx?raw` and checks the SOURCE
/// SHAPE of this table, so a new charting route omitted from its `it.each`
/// list sails through CI while silently regressing the bundle. There is no
/// bundle-size gate to catch it -- `vite.config.ts` has no `manualChunks`,
/// by documented choice. The entry was added in the same change as this
/// line, and the claim was verified on the build output rather than
/// inferred: `VITE_TARGET=mobile yarn build`, then `grep -c recharts` over
/// each chunk.
///
/// Note that the SESSIONS page (#917) is deliberately NOT lazy while this
/// one is. They are different destinations behind one view id, and the
/// split is the point: the page you open to get work back after a crash
/// must not wait on a chunk fetch, and the page you open to look at charts
/// can.
const ClaudeOverviewPage = lazy(() =>
  import("./components/ClaudeOverviewPage").then((m) => ({
    default: m.ClaudeOverviewPage,
  })),
);
/// The FOURTH chart-carrying route (#1075).
///
/// `ClaudePluginsPage` reaches `recharts` through `stats/PluginCallsChart`
/// -> `ui/chart`, the same path the three above reach it by, so it belongs
/// behind the same boundary for the same reason. Its entry in
/// `App.lazy.test.tsx`'s `LAZY_ROUTES` was added in the same change as this
/// line -- that table is the only gate, for the reason stated above.
const ClaudePluginsPage = lazy(() =>
  import("./components/ClaudePluginsPage").then((m) => ({
    default: m.ClaudePluginsPage,
  })),
);

/// What fills a lazy view's frame while its chunk arrives.
///
/// Deliberately NOT a spinner, and deliberately not the stats skeleton
/// either. A spinner on a local webview flashes for a frame and reads as
/// jank; the stats skeleton is chart-shaped, so showing it for a chunk
/// fetch would claim a layout that the System Health page does not have.
///
/// An empty frame of the right height is what is left: the page's own
/// loading states take over the instant its module evaluates, and they
/// are the ones that know what shape the content is.
function ViewLoading() {
  return <div className="min-h-40" aria-busy="true" />;
}

/// The assembled app shell. `AuthGate` already wraps this component once in
/// `main.tsx` -- it is not repeated here, so there is exactly one
/// `get_auth_state` query and one `usePollError` subscription (and
/// therefore one error banner) per window.
export default function App() {
  const {
    data: prs = [],
    isLoading,
    isError,
    error,
    refetch,
    dataUpdatedAt,
  } = usePullRequests();
  const filters = useActiveFilters();
  const {
    view: storedView,
    selectedPr,
    selectPr,
    applyPreset,
    // Which of the Claude Code view's two pages is showing. Read here
    // rather than inside a page because the SPLIT is the route: sessions
    // and overview get different wrappers and only one of them is lazy.
    claudePage,
  } = useFilters();
  const isMobile = useIsMobile();
  // A view the companion does not ship falls back to the default one.
  //
  // This replaces the `panel === "stats"` downgrade that stood here
  // until #794. Stats was a `panel` value then; it is the `pr-stats`
  // VIEW now, and `view` persists across launches exactly as `panel`
  // did -- so a desktop closed on PR Stats would otherwise open a phone
  // on a page the phone does not offer, with no switcher entry to leave
  // it by since that entry is hidden too.
  //
  // Derived rather than written back to the store, deliberately. The
  // phone and the desktop can share a persisted store (UI prefs sync,
  // and a user may restore a backup), so CORRECTING the stored value
  // would silently move the desktop off PR Stats as well. What the
  // phone cannot show, the phone declines to show; what the desktop
  // stored stays stored.
  //
  // On the BUILD, not the viewport, per `lib/target.ts`: a desktop user
  // who drags their window under 768px keeps the page.
  // The CAPABILITY fall-through, alongside the build-time one (#921).
  //
  // `ViewSwitcher` refuses to offer `claude-code` while
  // `claude_integrations_enabled` is off (#916), overriding even the
  // current-view escape hatch, exactly as the build-time set does -- and
  // its comment justifies that by saying there is no page behind the
  // entry. Giving the view a real route makes that claim false unless the
  // ROUTE applies the same predicate, which is the three-call-site
  // disagreement `MOBILE_HIDDEN_VIEWS` exists to prevent.
  //
  // Derived, never written back, for the reason the mobile case states
  // just above: the stored value is the user's, and a desktop that later
  // turns the capability on should land back where it was.
  //
  // `prefs` is undefined while `get_ui_prefs` is in flight AND if it
  // rejects, so this falls back in both cases. That is the fail-CLOSED
  // direction and the right one: the alternative is rendering a page for a
  // capability we could not confirm is on. It is also what `ViewSwitcher`
  // already does with the same value, so the two agree in the uncertain
  // case as well as the settled ones.
  const { prefs } = useUiPrefs();
  const claudeCodeOff = !prefs?.claude_integrations_enabled;
  const view =
    (IS_MOBILE_BUILD && MOBILE_HIDDEN_VIEWS.has(storedView)) ||
    (storedView === "claude-code" && claudeCodeOff)
      ? "my-prs"
      : storedView;
  // The sidebar is a sheet on the phone, opened from a button in the
  // header. Any navigation closes it: the point of picking a repo is
  // to look at it, and a sheet still covering the list would hide the
  // very thing that was picked. "Open" is therefore recorded AGAINST
  // the place it was opened from, so moving anywhere else makes it
  // closed by derivation rather than by an effect that runs a render
  // late.
  //
  // `panel` is gone from this key with the axis itself (#852). It had been
  // contributing a constant since #326 removed the Builds page -- nothing
  // ever set anything but `"list"` -- so every value of this key carried
  // the same literal, and dropping it changes no behaviour. Kept in mind
  // rather than silently: this key's job is to name every axis that
  // changes WHERE you are, so a reader should know why one of them left.
  const navKey = `${view}|${filters.repo ?? ""}`;
  const [navOpenedAt, setNavOpenedAt] = useState<string | null>(null);
  const navOpen = navOpenedAt === navKey;
  const setNavOpen = (open: boolean) => setNavOpenedAt(open ? navKey : null);
  // The main panel is the scroll container for every view, so the reset
  // hangs off it rather than off each page.
  const mainRef = useRef<HTMLElement>(null);
  // Pull-to-refresh, mobile build only. The desktop has `r` and the
  // tray's "Refresh now"; a phone has neither, and the poll loop that
  // would otherwise correct a stale list runs on the DESKTOP. Guarded
  // on the build rather than the viewport: a narrow desktop window
  // still has the keyboard, and attaching touch handlers to its scroll
  // container would be the same category error #598 fixed.
  //
  // `refreshNow()` directly, for the same reason `useRefreshRequested`
  // does it: invalidating `["prs"]` re-reads the SQLite snapshot the
  // poll loop just wrote, so the user would see the rows they were
  // already looking at. Pull to refresh has to mean "ask GitHub now".
  const refreshFromGesture = useRefreshFromGesture();
  const pull = usePullToRefresh(mainRef, refreshFromGesture, IS_MOBILE_BUILD);
  // Every axis that changes WHAT is rendered, and nothing that merely
  // changes the data within it. A poll tick refreshing the same list
  // must not scroll the user away from what they are reading.
  // `panel` dropped here too (#852), and for the same reason as `navKey`
  // above: it had been a constant in this string since #326, so it could
  // never have been one of the axes this key exists to watch.
  useScrollReset(
    mainRef,
    `${view}|${filters.repo ?? ""}|${selectedPr ? `${selectedPr.repo}#${selectedPr.number}` : ""}`,
  );

  // The tray's "Refresh now" menu item only emits `refresh-requested`; this
  // is what actually makes it do anything (see the hook's own comment).
  useRefreshRequested();
  useViewCadence(view);
  const truncatedTotal = useTruncation();
  const refusedFields = useIncomplete();
  const reviewShortfall = useReviewShortfall();
  // At the APP level, not in the wizard: the run outlives the modal
  // that started it, and the user is expected to be elsewhere by the
  // time it finishes (#495).
  useUpdateRunOutcome();
  // And the catch-up for a run whose outcome arrived while the app
  // was not listening -- which on a phone is any run it slept
  // through, since a suspended app holds no event stream.
  useUpdateRunResume(filters.repo);
  const pollError = usePollError();
  // The LIST only where it is rendered. The badge below uses a count
  // query instead, so Docker and Worktrees no longer fetch 100 pull
  // requests to display a number.
  // `isLoading` is taken from the query that feeds the CURRENT view.
  // Only the authored query's was used, and it has already resolved by
  // the time anyone reaches To review -- so switching there showed an
  // empty list with no indication anything was happening, for as long
  // as the request took.
  const reviewingQuery = useReviewing(view === "to-review");
  const {
    data: reviewing = [],
    isLoading: reviewingLoading,
    isError: reviewingError,
    error: reviewingErr,
    refetch: refetchReviewing,
    isRefreshing: reviewingRefreshing,
    isFromCache: reviewingFromCache,
    staleSecs: reviewingStaleSecs,
  } = reviewingQuery;
  // DIAGNOSTIC LOGGING (Settings > diagnostic log).
  useReviewingDiag({
    enabled: view === "to-review",
    status: reviewingQuery.status,
    fetchStatus: reviewingQuery.fetchStatus,
    count: reviewingQuery.data?.length,
  });
  const { data: reviewingCount = 0 } = useReviewingCount();

  // The app had no keyboard affordances at all. These three need no
  // backend change: `refresh-requested` already exists and the window
  // already hides to the tray on close.

  // Splash dismissal deliberately does NOT live here. `App` only mounts
  // when auth succeeds, so dismissing on `isSuccess` left every
  // unauthenticated machine showing the splash forever -- see AuthGate,
  // which owns it now and lifts it on any settled auth result.

  // Sorting was moved out of PrList in M3 -- it renders exactly the order
  // it's handed, so the sort dropdown in FilterBar is inert unless this
  // call site applies it.
  // The list the active view operates on. Everything downstream --
  // sidebar counts, filters, the strip -- reads this rather than `prs`,
  // so the two views share every component instead of duplicating them.
  const source = view === "to-review" ? reviewing : prs;
  const visible = sortPrs(applyFilters(source, filters), filters.sort);

  // A cursor past the end of a newly-filtered list points at nothing.
  // Clamping here rather than in the key handler means it is correct for
  // rendering too, not just for the next key press.
  const { cursor, setCursor } = useFilters();
  useEffect(() => {
    // Only while My PRs owns the cursor (#953). Another view's list is
    // a different length, and clamping its cursor against `visible` --
    // which is the PR list whatever view is on screen -- would drag a
    // sessions cursor down to the number of pull requests, or to `null`
    // when there are none. The owning view does its own clamping;
    // `nextCursor` is the shared rule.
    if (activeRowCursor() !== null) return;
    if (cursor !== null && cursor >= visible.length) {
      setCursor(visible.length > 0 ? visible.length - 1 : null);
    }
  }, [cursor, visible.length, setCursor]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const action = shortcutFor(e);
      if (!action) return;
      e.preventDefault();
      if (action === "onRefresh") {
        void emit("refresh-requested", null);
      } else if (action === "onHide") {
        // Desktop only. `core:window:*` is not in the phone's
        // capabilities, so this rejects there -- unhandled, because
        // `void` drops it -- and "hide to the tray" is meaningless on a
        // platform with no tray. Guarded on the build target rather
        // than the viewport: a narrow desktop window still has a tray.
        //
        // The other shortcuts are left bound. A phone has no hardware
        // keyboard, but an iPad with one reaches them, and j/k/Enter/x
        // all do something sensible there -- it is only this branch
        // that cannot work.
        if (IS_DESKTOP_BUILD) void getCurrentWindow().hide();
      } else if (action === "onFocusSearch") {
        // The VISIBLE search box, not the document's first (#953).
        //
        // This was a bare `querySelector('input[type="search"]')`, a
        // global first-match. It worked on the sessions page only by
        // coincidence -- that page happens to have exactly one -- and
        // nothing asserted it, so a second search input anywhere would
        // have silently stolen `/` from whichever page came later in the
        // DOM.
        //
        // `offsetParent` is the test rather than a view lookup, because
        // the hidden inputs here are hidden by CSS: the phone's two-pane
        // views keep the list mounted under a `hidden` class so scrolling
        // back keeps its position, and Settings' sections are all
        // rendered with only one unhidden. An element inside any of those
        // has a null `offsetParent`, which is exactly "not on screen".
        //
        // Falls back to the first match when nothing is visible, so the
        // behaviour is never worse than it was.
        const boxes = [...document.querySelectorAll<HTMLInputElement>('input[type="search"]')];
        const el = boxes.find((b) => b.offsetParent !== null) ?? boxes[0];
        el?.focus();
        el?.select();
      } else {
        // The list the cursor walks, decided HERE rather than baked in
        // (#953).
        //
        // This branch used to read `visible` -- a `PullRequest[]` -- and
        // index into it with `selectPr` and a `repo#number` key, which is
        // why `j`/`k`/`Enter`/`x` worked on exactly one list of ten and
        // silently moved a PR cursor behind the other nine.
        //
        // A view that has claimed the cursor answers first; My PRs is the
        // fallback and is expressed in the same vocabulary, so there is
        // one code path rather than a special case for the original list.
        // `activeRowCursor()` is CALLED here, at keypress time, not
        // captured at mount -- the constraint #953 states explicitly,
        // and the same one the `visible` dependency below encodes.
        const registered = activeRowCursor();
        const target: RowCursorTarget = registered ?? {
          rows: () => visible.length,
          open: (i) => {
            const pr = visible[i];
            if (pr) selectPr({ repo: pr.repo, number: pr.number });
          },
          toggle: (i) => {
            const pr = visible[i];
            if (pr) useFilters.getState().toggleChecked(`${pr.repo}#${pr.number}`);
          },
        };
        const rows = target.rows();
        if (rows === 0) return;
        const { cursor, setCursor } = useFilters.getState();
        if (action === "onNext" || action === "onPrev") {
          // Clamped, not wrapped: wrapping from the bottom back to the
          // top silently moves the eye across the whole screen. The rule
          // lives in `nextCursor` now, so it is stated once and tested
          // without a DOM.
          setCursor(nextCursor(cursor, rows, action === "onNext" ? "next" : "prev"));
        } else if (cursor !== null && cursor < rows) {
          if (action === "onOpen") {
            target.open(cursor);
          } else if (action === "onToggleSelect") {
            // Optional by design: a list with no bulk action leaves
            // `toggle` out and `x` does nothing there, rather than the
            // view inventing a selection it cannot act on.
            target.toggle?.(cursor);
          }
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // `visible` is a real dependency, not noise: the handler indexes
    // into it, so a listener bound to a stale list would move the
    // cursor through rows that are no longer on screen. Re-binding one
    // window listener per filter change is cheap; a wrong cursor is not.
  }, [selectPr, visible]);

  // The priorities strip is scoped to the selected repo, matching the page
  // it sits on: on `octocat/hello-world` you want that repo's blocked PRs,
  // not a list dominated by nine other repos you are not looking at. The
  // dashboard is the whole-account view, so its strip spans every repo.
  //
  // Note this scopes by REPO only, not by the rest of the filters. Something
  // blocked on you stays blocked whether or not you happen to be filtering
  // by label, so a label filter must not hide it -- but a repo selection is
  // a change of page, and the strip should follow.
  const scopedForStrip = filters.repo
    ? source.filter((pr) => pr.repo === filters.repo)
    : source;

  // Scoped the SAME WAY as `scopedForStrip`. The court strip counts
  // both lists together, so passing a repo-scoped authored list beside
  // an account-wide review queue produced a sentence with two different
  // scopes in it: "36 needs you · 18 waiting on others · of 13 open",
  // where 36 and 18 spanned every repo and 13 was one repo. That reads
  // as an arithmetic bug because it is one.
  const scopedReviewing = filters.repo
    ? reviewing.filter((pr) => pr.repo === filters.repo)
    : reviewing;

  // `repo` is navigation, not a filter (see the store's `reset`), so it
  // does not count -- an empty repo page should still explain itself.

  // FilterBar still sees the *unfiltered* `prs`: its label menu should offer
  // every label present across all open PRs, not shrink to only the labels
  // that survive whatever filter is already active, which would make some
  // combinations unreachable.
  const sidebar =
    view === "system-health" ? (
      // The ONLY view with no repository axis: it describes the
      // machine, so there is nothing for a repo list to pick between.
      // Rendering one of the repo sidebars here would show a column of
      // repositories whose every row is inert, or -- on a machine with
      // no scanned checkouts -- an empty picker under a heading, which
      // reads as a page that failed to load.
      //
      // What the column holds instead is the machine's own classes
      // (#687): CPU, Memory, Disk, Network, Power. Navigation within
      // the thing the view is about, which is what every other sidebar
      // in the app holds -- and the natural occupant of a column that
      // was previously the view switcher alone.
      <SystemHealthSidebar viewCounts={{ "to-review": reviewingCount }} />
    ) : view === "claude-code" ? (
      // NOT one of the repository sidebars, and not a fall-through
      // either. A Claude Code session is not scoped to a repository:
      // 665 distinct working directories over 1,461 sessions, mostly
      // deleted agent worktrees, 83% of them no longer on disk. A repo
      // picker here would be a column of inert rows -- the same
      // reasoning the `system-health` branch above states.
      //
      // What it holds instead is the view's own two pages, exactly as
      // `SystemHealthSidebar` holds the machine's classes.
      <ClaudeCodeSidebar viewCounts={{ "to-review": reviewingCount }} />
    ) : view === "packages" || view === "claude-md" || view === "repositories" ? (
      // The SAME list, not a second one (#1030). `RepoPickerSidebar` is
      // already "a plain repository list, for views whose only axis is
      // 'which repo'", and the browser's only axis is which repo -- the
      // position INSIDE it is store state (#1034), not a second sidebar
      // concern.
      //
      // A copy would be a second set of the five empty arms, and #854 is
      // the shipped evidence that a second consumer of this data gets
      // them wrong: `RepoPickerSidebar` ITSELF was the missed consumer
      // when #846 fixed this defect on four surfaces. Copying it to bolt
      // a file tree on would recreate that bug with the new copy as the
      // one that drifts -- and it would re-ship #846 with a component
      // whose empty copy is a diagnosis naming the user's settings.
      //
      // `allLabel` for the Repositories view ALONE (#1043). All three
      // views render something when no repository is selected, but only
      // this one renders a page there -- the All Repositories overview,
      // which answers "which of my repositories is behind its remote"
      // before any repository is chosen. Packages and CLAUDE.md render a
      // one-line prompt, and a permanently selected row leading to a
      // prompt would announce an empty view as somewhere the user is.
      //
      // The label matches the overview's own visible heading exactly.
      // Two spellings of one destination is how a sidebar and the page it
      // leads to start disagreeing.
      <RepoPickerSidebar
        reviewingCount={reviewingCount}
        allLabel={view === "repositories" ? "All Repositories" : undefined}
      />
    ) : view === "artifacts" ? (
      <ArtifactSidebar reviewingCount={reviewingCount} />
    ) : view === "docker" ? (
      <DockerSidebar viewCounts={{ "to-review": reviewingCount }} />
    ) : view === "worktrees" || view === "branches" ? (
      // Same repository list: Branches acts on the same checkouts
      // Worktrees does, so a second sidebar would be the same rows
      // under a different name.
      <WorktreeSidebar viewCounts={{ "to-review": reviewingCount }} />
    ) : view === "pr-stats" ? (
      // PR Stats has its OWN sidebar now (#825), and is no longer a
      // fall-through. It inherited `RepoSidebar` in #794 as a deliberate
      // placeholder whose rows were "continuity and a future scope hook,
      // not a live filter" -- `ViewSwitcher`'s doc comment said so, and
      // said it was worth revisiting "if PR Stats is ever scoped per
      // repo, at which point these rows stop being decoration". This is
      // that point.
      //
      // The column it replaces listed repositories where the viewer has
      // an OPEN PR (`repoCounts(prs)`), which cannot hold an
      // organisation or a person -- so #823's second audience ("how is
      // my team doing?") had nowhere to be asked from. This one is
      // sourced from GitHub and consults nothing on disk.
      <StatsSidebar viewCounts={{ "to-review": reviewingCount }} />
    ) : (
      // My PRs, and any future view that falls through. The repo rows
      // are a live filter here -- this is the view they were always
      // about.
      <RepoSidebar prs={source} viewCounts={{ "to-review": reviewingCount }} />
    );

  return (
    <div className="flex h-dvh flex-col bg-[#0d1117] text-[#e6edf3] px-safe">
      {/* Above everything, including the header: it says which
          desktop the whole screen is describing. Renders nothing on
          the desktop itself. */}
      <ConnectionBanner updatedAt={dataUpdatedAt} />
      {/* Below the banner and above everything else: the banner says
          which desktop, this says the rows underneath may be old. The
          banner alone was not enough -- it is one line that scrolls out
          of mind, and `ConnectionBanner` was the only component in the
          app reading the connection state at all. */}
      <StaleRibbon />
      <div className="flex min-h-0 flex-1">
      {isMobile ? (
        // The same sidebar component, in a sheet. Its own `w-64` and
        // right border are for sitting beside the list; here it fills
        // the sheet instead. Overridden from outside rather than by a
        // prop on five sidebars, so the desktop render of each is
        // byte-for-byte what it was.
        <Sheet open={navOpen} onOpenChange={setNavOpen}>
          <SheetContent
            side="left"
            showCloseButton={false}
            className="w-72 gap-0 border-[#30363d] bg-[#0d1117] p-0 text-[#e6edf3] [&>nav]:min-h-0 [&>nav]:w-full [&>nav]:flex-1 [&>nav]:border-r-0"
          >
            <SheetTitle className="sr-only">Navigation</SheetTitle>
            {sidebar}
          </SheetContent>
        </Sheet>
      ) : (
        sidebar
      )}
      <main ref={mainRef} className="relative flex-1 overflow-auto">
        {/* Absolutely positioned inside the scroll container, so it
            needs the container to be a positioning context. Renders
            nothing at rest. */}
        <PullIndicator state={pull} />
        {/* NO safe-area padding here, deliberately. `ConnectionBanner`
            is above this and already carries `pt-safe`, so it is what
            clears the status bar; adding the inset again here applied it
            TWICE and left a notch-height gap between the banner and the
            header -- about a tenth of the screen on an iPhone.

            The inset belongs to whatever is top-most, and on the phone
            that is always the banner: it renders for every state
            including "unpaired", and returns null only on the desktop,
            where the inset is zero anyway. */}
        <header className="sticky top-0 z-20 flex items-center gap-2 border-b border-[#30363d] bg-[#0d1117] px-4 py-3">
          {isMobile ? (
            <button
              type="button"
              onClick={() => setNavOpen(true)}
              aria-label="Open navigation"
              className="tap-target -ml-1 flex items-center justify-center rounded hover:bg-[#161b22]"
            >
              <Menu className="h-4 w-4" aria-hidden="true" />
            </button>
          ) : null}
          {/* View selection lives in the sidebar's switcher rather than
              as a per-page tab row here: the sidebar is already where you
              choose what you are looking at, and a tab row repeated above
              every page competed with it. Since #794 that is true of PR
              Stats too -- it was the last destination reached any other
              way. */}
          <h1 className="text-sm font-semibold">
            {view === "to-review"
              ? "Pull requests to review"
              : view === "system-health"
                ? "System health"
              : view === "claude-md"
                ? "CLAUDE.md"
              // Matching the switcher entry exactly, per #794's finding:
              // a header naming the page something other than the menu
              // item that opened it is how a user doubts they are where
              // they meant to be.
              : view === "claude-code"
                ? "Claude Code"
              : view === "packages"
                ? "Package updates"
              : view === "artifacts"
                ? "Build artifacts"
              : view === "docker"
                ? "Docker images"
                : view === "worktrees"
                  ? "Worktrees"
                : view === "branches"
                  ? "Branches"
                // Matching the switcher entry exactly, per #794's
                // finding: a header naming the page something other than
                // the menu item that opened it is how a user doubts they
                // are where they meant to be.
                : view === "repositories"
                  ? "Repositories"
                  // "PR Stats", matching the switcher entry exactly
                  // (#794). The header naming the page something other
                  // than the menu item that opened it is how a user
                  // doubts they are where they meant to be -- and this
                  // said "Stats" while that now says "PR Stats".
                : view === "pr-stats"
                  ? "PR Stats"
                  : "Pull requests"}
          </h1>
          <div className="ml-auto">
            {/* My pull requests ONLY. The wizard composes a nudge for
                pull requests YOU authored, so it means nothing on
                Docker or Worktrees (local state) and nothing on To
                review (other people's work). The previous condition
                excluded only Worktrees, so it appeared on all three. */}
            {view === "my-prs" ? (
              // scopedRepo skips the wizard's "which repositories?" step:
              // selecting a repo in the sidebar already answers it.
              <NudgeWizard prs={source} scopedRepo={filters.repo} />
            ) : null}
          </div>
        </header>

        {/* Local-state views never render a PR detail: a pull request
            selected earlier in My PRs would otherwise take over the
            page, and none of Worktrees, Branches or System health has
            any notion of a selected PR to go back to. System health
            least of all -- it is about the machine, and nothing on it
            can be reached from a pull request.

            PR Stats joins them (#794), and for a reason the others do
            not share: it IS about pull requests, just not about one of
            them. A whole-account summary with a single PR's detail
            rendered over it is not a page anybody asked for. `setView`
            clears `selectedPr`, so the switcher path could not reach
            this anyway -- but "could not reach it today" is the wrong
            thing for the route to rely on, since the detail branch is
            FIRST in this chain and therefore wins over every view
            branch below it. */}
        {selectedPr &&
        view !== "worktrees" &&
        view !== "branches" &&
        view !== "pr-stats" &&
        // Claude Code joins them (#921), for the reason the others
        // share: it is local state about this machine's sessions, with
        // no notion of a selected pull request to go back to. `setView`
        // clears `selectedPr`, so the switcher path cannot reach that
        // today -- but this branch is FIRST in the chain and therefore
        // wins over every view branch below, which is exactly what the
        // `pr-stats` comment says not to rely on.
        view !== "claude-code" &&
        view !== "system-health" ? (
          <div className="p-4">
            <PrDetailView
              repo={selectedPr.repo}
              number={selectedPr.number}
              onBack={() => selectPr(null)}
            />
          </div>
        ) : view === "claude-md" ? (
          <ClaudeMdPage />
        ) : view === "claude-code" ? (
          // This branch is what makes #916's registered view id a real
          // route: until it existed, `claude-code` fell through to My PRs
          // by the documented fall-through at the end of this chain, so
          // the switcher offered a destination that rendered the pull
          // request list.
          //
          // BOTH pages are routed here now. #921 landed the overview
          // alone and said so, because `ClaudeCodeSidebar` already
          // offered both rows and picking "Sessions" fell back to the
          // overview until #917 arrived. It has: this is that change, so
          // the placeholder is retired and the sidebar's two rows now
          // reach two different pages.
          //
          // The split is on `claudePage`, not on a second view id. One
          // view with two destinations is what `ClaudePage` and the
          // sidebar were built around, and a second entry in `ALL_VIEWS`
          // would make the persisted `view` and the sidebar's selection
          // two sources of truth for one position.
          //
          // They are wrapped differently ON PURPOSE, which is why this is
          // not one shared wrapper with a swapped child:
          //
          // Sessions owns its own padding -- its banners sit flush with
          // the pane beneath them, and on the phone it mounts the session
          // list itself (#939), which sits flush with its own search box.
          // It is also NOT lazy. #838's boundary is for the views that
          // pull in a charting library; this one imports nothing heavier
          // than `lucide-react` icons the launch chunk already has, and
          // `ClaudeCodeSidebar` imports `ClaudeSessionColumn` from it on
          // every Claude Code render anyway, so a lazy boundary here would
          // split a chunk the sidebar has already fetched.
          claudePage === "sessions" ? (
            <ClaudeCodePage />
          ) : claudePage === "plugins" ? (
            // Its own arm rather than a second fall-through. The overview
            // is the `else`, so a page added to `ClaudePage` and to
            // `CLAUDE_PAGES` but not here compiles cleanly, highlights
            // its own sidebar row, and renders the overview -- a
            // navigation that silently goes nowhere. #1038 is the same
            // defect one layer up: a component and its host landed in
            // different PRs and nothing rendered the component.
            //
            // The page owns its own padding, so no wrapper here.
            // `Suspense` because it reaches `recharts` through
            // `stats/PluginCallsChart`.
            <Suspense fallback={<ViewLoading />}>
              <ClaudePluginsPage />
            </Suspense>
          ) : (
            <div className="p-4">
              {/* Suspense because the page is a lazy chunk: it reaches
                  `recharts` through `stats/SessionsChart`, and #838's
                  boundary is the route. INSIDE the padded wrapper so the
                  frame it reserves is the same box the page will occupy --
                  outside it the fallback would be unpadded and the content
                  would shift sideways as the chunk landed. */}
              <Suspense fallback={<ViewLoading />}>
                <ClaudeOverviewPage />
              </Suspense>
            </div>
          )
        ) : view === "packages" ? (
          <PackagesPage />
        ) : view === "artifacts" ? (
          // No `p-4` wrapper: ArtifactsPage owns its own padding, since
          // its header row has to sit flush with the list beneath it.
          <ArtifactsPage />
        ) : view === "docker" ? (
          <div className="p-4">
            <DockerPage />
          </div>
        ) : view === "worktrees" ? (
          <div className="p-4">
            <WorktreesPage />
          </div>
        ) : view === "branches" ? (
          <div className="p-4">
            <BranchesPage />
          </div>
        ) : view === "repositories" ? (
          // A real route rather than the documented fall-through at the
          // end of this chain (#1023). #916's lesson, stated on the
          // `claude-code` branch above: a registered view id with no arm
          // here is a switcher entry that silently renders the pull
          // request list.
          //
          // NOT lazy. #838's boundary is for the views that pull in a
          // charting library; this page imports nothing heavier than
          // `lucide-react` icons the launch chunk already has, so a lazy
          // boundary would split a chunk for no weight.
          <div className="p-4">
            <RepositoriesPage />
          </div>
        ) : view === "system-health" ? (
          // No `FilterBar` and no strips, deliberately. Every control in
          // that bar narrows a list of pull requests, and this page has
          // none: rendering it here would put a search box, a sort menu
          // and two label pickers above a description of the CPU.
          <div className="p-4">
            {/* Suspense because the page is now a lazy chunk (#838). The
                boundary is INSIDE the padded wrapper so the frame it
                reserves is the same box the page will occupy -- outside
                it, the fallback would be unpadded and the content would
                shift sideways as the chunk landed. */}
            <Suspense fallback={<ViewLoading />}>
              <SystemHealthPage />
            </Suspense>
          </div>
        ) : view === "pr-stats" ? (
          <div className="p-4">
            {/* No priorities strip here: PR Stats is a read-only summary
                of the whole account, and the strip is a triage surface
                that belongs beside the list it acts on. Its cards already
                surface what needs attention, and each one clicks through
                to the list.

                Keyed on `view` since #794, not on `panel`. It sits after
                the other view branches for the same reason it sat after
                `system-health` before: this chain is ordered, and a
                branch that tested a DIFFERENT axis had to come last or
                it would have swallowed every view whose panel happened
                to be "stats". That hazard is gone now that one axis
                decides. */}
            {/* Suspense because the page is now a lazy chunk (#838); see
                the `SystemHealthPage` branch above for why the boundary
                sits inside the padded wrapper. */}
            <Suspense fallback={<ViewLoading />}>
              <StatsPage />
            </Suspense>
          </div>
        ) : (
          <div className="p-4">
            {/* Only for My PRs: the strip means "blocked on YOU as
                author", and someone else's red CI is not yours to fix. The
                review view gets its own attention rule below. */}
            {/* Answers "is anything on fire?" before the filter
                toolbar does anything. `PrioritiesStrip` still follows
                with the WHY for each blocked pull request -- this says
                whether to look at all, that says what to look at. */}
            {view === "my-prs" ? (
              <CourtStrip
                authored={scopedForStrip}
                reviewing={scopedReviewing}
                onSelect={(court) =>
                  applyPreset(
                    court === "mine"
                      ? { needsAttentionOnly: true }
                      : { awaitingReviewOnly: true },
                  )
                }
              />
            ) : null}
            {view === "my-prs" ? (
              <PrioritiesStrip
                prs={scopedForStrip}
                onOpen={(pr) => selectPr({ repo: pr.repo, number: pr.number })}
              />
            ) : null}
            {/* The review queue's counterpart to the attention strip:
                what a reviewer can pick up right now. Scoped to the
                sidebar selection for the same reason -- on one
                repository you want that repository's work, not a list
                dominated by nine others. */}
            {view === "to-review" ? (
              <ReadyStrip
                prs={scopedForStrip}
                onOpen={(pr) => selectPr({ repo: pr.repo, number: pr.number })}
              />
            ) : null}
            {/* Counts come from the same predicates the chips apply, so a
                chip can never open a list that disagrees with its number.
                Scoped to the sidebar selection like the strip above. */}
            {view === "my-prs" ? <TriageChips prs={scopedForStrip} /> : null}
            {view === "to-review" ? <ReviewChips prs={scopedForStrip} /> : null}
            {/* GitHub answered with usable data and a complaint that it
                could not compute all of it. The list is real but short,
                and saying so beats hiding it -- or, as v3.2.5 did,
                discarding the data and showing nothing at all. */}
            {refusedFields > 0 ? (
              <p className="mb-3 rounded-md border border-[#d29922]/40 bg-[#d29922]/5 px-4 py-2 text-xs text-[#d29922]">
                GitHub could not compute {refusedFields} field
                {refusedFields === 1 ? "" : "s"} on the last refresh, so some pull
                requests may be missing details or absent. It usually recovers on
                the next one.
              </p>
            ) : null}
            {/* The 100 -> 50 fallback returns a SHORT list, and this is
                the only thing that says so. Without it the panel shows
                50 pull requests under a sidebar badge reading 62, with
                nothing to explain the gap -- which is what "the numbers
                are off" was describing. */}
            {view === "to-review" && reviewShortfall > 0 ? (
              <p className="mb-3 rounded-md border border-[#d29922]/40 bg-[#d29922]/5 px-4 py-2 text-xs text-[#d29922]">
                {reviewShortfall} pull request{reviewShortfall === 1 ? " is" : "s are"}{" "}
                missing from this list — GitHub could not answer the full query, so
                it was retried for fewer. Refreshing usually returns the rest.
              </p>
            ) : null}
            {/* The other half of the reported complaint: "no indication
                that it is blocked". The list now paints from the cache
                immediately, so without this the user would be looking
                at stale data with nothing to say it was being
                refreshed. */}
            {view === "to-review" && reviewingRefreshing && reviewingFromCache ? (
              // Amber, and it names the age, when the snapshot is past
              // the freshness window (#742). Such a snapshot used to be
              // thrown away, which reached the view as an empty list --
              // "nothing awaits your review", stated confidently, for
              // as long as the live fetch took. Showing the old rows
              // and saying how old they are beats asserting there are
              // none. Inside the window it stays the quiet grey note:
              // a snapshot seconds old needs no warning.
              <p
                className={
                  reviewingStaleSecs === null
                    ? "mb-3 rounded-md border border-[#30363d] bg-[#161b22] px-4 py-2 text-xs text-[#8b949e]"
                    : "mb-3 rounded-md border border-[#d29922]/30 bg-[#d29922]/10 px-4 py-2 text-xs text-[#d29922]"
                }
              >
                {reviewingStaleSecs === null
                  ? "Showing the last saved list — checking GitHub for changes…"
                  : `Showing a saved list from ${relativeSeconds(reviewingStaleSecs)} — checking GitHub for changes…`}
              </p>
            ) : null}
            <FilterBar prs={source} />
            {/* Fed the UNFILTERED list on purpose: selection is keyed by
                repo#number, so narrowing a filter after selecting must
                not shrink the batch out from under the user. */}
            {view === "my-prs" ? <BulkBar prs={source} /> : null}
            {(view === "to-review" ? reviewingLoading : isLoading) ? (
              // `get_cached` returns `[]` both for "never polled" and for
              // "authenticated, first poll (~3s) still in flight" -- an
              // empty PrList would misreport the latter as "no pull
              // requests match these filters" when no filters are even
              // active. Gating on isLoading keeps a cold start visibly
              // loading instead of falsely claiming zero matches.
              <div className="rounded-md border border-[#30363d] px-4 py-12 text-center text-sm text-[#8b949e]">
                Loading pull requests…
              </div>
            ) : (view === "to-review" ? reviewingError : isError) ? (
              // The same reasoning one step further. A REJECTED query also
              // leaves `prs` at its `[]` default, so without this branch the
              // list renders "0 Open -- no pull requests match these
              // filters": a confident answer to a question the app could not
              // answer. `poll-error` does not cover this -- that banner is
              // emitted by the background loop, and a failure here means the
              // foreground fetch itself never produced data.
              <QueryError
                title={
                  view === "to-review"
                    ? "Could not load the pull requests awaiting your review"
                    : "Could not load your pull requests"
                }
                // The failing query's OWN error and retry. Reporting the
                // authored query's here would show a stale message and
                // a retry that refetches the wrong list.
                message={errorMessage(view === "to-review" ? reviewingErr : error)}
                onRetry={() =>
                  void (view === "to-review" ? refetchReviewing() : refetch())
                }
              />
            ) : (
              <PrList
                prs={visible}
                hasFilters={hasActiveFilters(filters)}
                total={view === "my-prs" ? (truncatedTotal ?? undefined) : undefined}
                // `source`, not `visible`: the truncation marker compares
                // against GitHub's unfiltered count, so the number beside
                // it has to be unfiltered too (#745).
                fetched={source.length}
                onOpen={(pr) => selectPr({ repo: pr.repo, number: pr.number })}
                canWrite={view === "my-prs"}
                selectable={view === "my-prs"}
                // A poll failure with a SUCCESSFUL but empty cache read
                // is the fresh-install case: `isError` above covers a
                // rejected query, and this covers "the query returned
                // the empty snapshot because no poll has ever landed".
                unreachable={pollError !== null && source.length === 0}
              />
            )}
          </div>
        )}
      </main>
      </div>
      {/* Pinned below both the sidebar and the list, so it reads as the
          window's status rather than the list's. */}
      <StatusBar updatedAt={dataUpdatedAt} />
    </div>
  );
}

import { create } from "zustand";
import { persist } from "zustand/middleware";
import type { Filters } from "../lib/derive";

/// zustand holds UI state only. Server data lives in TanStack Query and is
/// never duplicated here.

/// Every top-level view, in sidebar order.
///
/// `view` and `panel` are separate axes because "which view am I in" and
/// "which sub-page of it" were once one enum, which is what made the
/// sidebar highlight logic awkward -- `reviewing` and `dashboard` were
/// peers of `list` despite being different kinds of thing. (The stats half
/// of that story has come full circle: #794 made it a view again, but as a
/// peer of `my-prs` rather than of `list`, which is the distinction the
/// split was about.)
///
/// The single source of truth: `View` is derived from it, and the
/// migration's completeness test iterates it rather than repeating the
/// names. A hardcoded second list is one that gets edited to match
/// whatever the code does and stops checking anything.
export const ALL_VIEWS = [
  // FIRST, per #823. Promoted out of `panel` in #794 -- it was a sub-page
  // of My PRs pinned to the sidebar's bottom -- and #794 then placed it
  // here in the middle, which is where #823 asked for it to stop being.
  //
  // The v5.13.0 rebuild is the reason: the page answers "how am I doing"
  // and "how is my team doing" across every organisation and member, not
  // just this machine's checkouts. That is a thing someone opens the app
  // TO SEE, so it leads rather than sitting ninth of ten.
  //
  // This list's order is only cosmetic -- `View` is derived from it and
  // nothing else reads the sequence. `ViewSwitcher`'s own `VIEWS` array
  // is what the menu renders, so the two are kept in the same order
  // deliberately: a reader comparing them should not have to wonder which
  // one the user sees.
  "pr-stats",
  "my-prs",
  "to-review",
  "worktrees",
  "branches",
  "docker",
  "artifacts",
  "packages",
  "claude-md",
  // Gated by the `claude_integrations_enabled` capability rather than by
  // `hidden_views` (#916). Registered here regardless of the switch, because
  // `View` is the type every route and preference is keyed on -- a view that
  // existed only while enabled would make a persisted `view` unparseable the
  // moment someone switched the integrations off.
  "claude-code",
  // Last, deliberately: the only entry that is not about pull requests.
  "system-health",
] as const;

export type View = (typeof ALL_VIEWS)[number];

/// Views the mobile companion does not offer, whatever is persisted.
///
/// A BUILD-time set, not a viewport one, for the reason `lib/target.ts`
/// gives: "the companion does not offer this" is a statement about which
/// app this is, and hiding by width takes a page away from a desktop
/// user who dragged their window narrow.
///
/// Why this exists at all: until #794, Stats was a `panel` value, and
/// `App.tsx` kept it off the phone by downgrading a stored
/// `panel === "stats"` to `"list"` on every render. Promoting it to a
/// `View` makes that downgrade dead code -- a persisted `view` is read by
/// the switcher, the header and the body route, and patching one of them
/// leaves the other two offering a page the phone cannot show. One set,
/// read everywhere a view is offered or routed.
///
/// EMPTY as of #863: the companion now offers every view the desktop
/// does. `pr-stats` was the only member, hidden because the companion's
/// first release scoped it out and #794 moved it between desktop surfaces
/// without revisiting that.
///
/// Kept as a mechanism rather than deleted, and that is the deliberate
/// choice here. The set is what makes the three-call-site bug described
/// above impossible: `App.tsx`, and `ViewSwitcher`'s collapsed button and
/// its menu all consult it, so hiding a future view is one line here
/// instead of three patches that can disagree. Deleting it would return
/// the next such decision to the shape #794 had to fix. An empty set
/// costs one allocation and `has()` returning false.
///
/// If you are adding a member: it must be a statement about what the
/// COMPANION cannot do, not about screen width -- see `lib/target.ts`.
/// And note the local-machine views (worktrees, docker, artifacts,
/// packages, claude-md) are the companion's whole purpose and are not
/// candidates.
export const MOBILE_HIDDEN_VIEWS: ReadonlySet<View> = new Set<View>();

/// The System Health sub-pages, in sidebar order (#687).
///
/// "overview" is the landing page and stays exactly what it was: the
/// panels people open the view for. The rest are DRILL-DOWNS from it,
/// each answering the "why" a panel can only raise -- a panel says
/// memory is at 88%, the Memory page says which processes.
///
/// A separate axis from `panel` rather than three more values in it.
/// `panel` is Docker's images-versus-builds switch (it was the My PRs
/// list-versus-stats switch too, until #794 made Stats a view); widening
/// it would mean every consumer of `panel` had to know about pages that
/// only exist inside one view, and a health page persisted there would
/// decide what Docker shows. Views that do not have sub-pages should not
/// have to name these.
export const ALL_HEALTH_PAGES = [
  "overview",
  "cpu",
  "memory",
  "disk",
  // Only reachable on a machine with a discoverable GPU (#717). It is
  // in the union unconditionally because the union is the set of pages
  // that EXIST; whether one is offered is a fact about the machine, and
  // `SystemHealthSidebar` filters it out where `gpus` is empty -- the
  // same rule that decides whether the overview draws a GPU panel at
  // all. A page in the union that is not offered is fine; a page
  // offered that renders nothing is not.
  "gpu",
  "network",
  // Battery, thermal and uptime together, and last. None of the three
  // has enough of its own to carry a page: battery is two numbers with
  // no history behind them, thermal is a single coarse label the
  // platform publishes, and uptime is one figure. What they share is
  // that they describe the machine's CONDITION rather than its work,
  // which is a real grouping and not a leftovers drawer -- and it is
  // the drawer test that decided it, since a page per figure would be
  // three sidebar rows leading to one stat each.
  "power",
] as const;

export type HealthPage = (typeof ALL_HEALTH_PAGES)[number];

interface FilterStore {
  /// Filters are PER VIEW: a repo selected in My PRs must not leak into
  /// Worktrees, which has an entirely different repo list.
  filtersByView: Record<View, Filters>;
  view: View;
  /// `panel` is GONE as of #852, and the axis went with it.
  ///
  /// It was Docker's images-versus-builds switch. #326 removed the Builds
  /// PAGE ("its data was diagnostic rather than actionable -- a log with no
  /// button on it") and #794 moved Stats out to its own view, which left
  /// the axis holding exactly one reachable value. `"builds"` was never set
  /// by any component and `DockerPage` never read `panel` at all, so every
  /// remaining write was `setPanel("list")` -- a no-op.
  ///
  /// The comment that stood here stated the rule this was breaking: "a
  /// value still in the union is one a component can set, and a `panel`
  /// nobody routes on would be a silent no-op." It said that about
  /// `"stats"` while `"builds"` sat beside it in the same condition, and
  /// two components were dressing the no-op as navigation: `DockerSidebar`
  /// rendered one permanently-active button whose only effect was
  /// `setPanel("list")`, and `RepoTable` called it INSTEAD of `setView`,
  /// making its documented navigation a dead click.
  ///
  /// Removed rather than kept for a hypothetical second Docker sub-page:
  /// the union is what a component can set, so an axis nothing routes on
  /// is a trap for the next caller, and re-adding one later is a smaller
  /// change than the two bugs this one caused. The v4 migration below drops
  /// a persisted value.
  setFilter: <K extends keyof Filters>(key: K, value: Filters[K]) => void;
  applyPreset: (filters: Filters) => void;
  /// Pick a PR Stats scope, and optionally a person within it (#825).
  ///
  /// One action rather than three `setFilter` calls, because the three keys
  /// are ONE selection. Written separately they would render three times
  /// and pass through states that are not selections at all -- a subject
  /// with the previous scope still set, which is a question about the wrong
  /// organisation. The `repo` precedent in `setFilter` is the same idea at
  /// one key: a navigation write is atomic with its consequences.
  ///
  /// `subject` is `undefined` for "the viewer", which is what every row
  /// except a Members row selects -- so clicking a repository after
  /// clicking a person CLEARS the person rather than silently keeping them
  /// scoped to a new repository. That is the behaviour the caller gets by
  /// default, which is why it is a required parameter rather than an
  /// optional one: a forgotten argument would leave a stale subject, and
  /// the leaderboard would quietly be about one person.
  setStatsScope: (
    kind: Filters["statsScopeKind"],
    value: string | undefined,
    subject: string | undefined,
  ) => void;
  setView: (view: View) => void;
  /// Which System Health page is open (#687).
  ///
  /// Deliberately NOT persisted, unlike `view` and `panel`. Those
  /// restore what you were working on; this is a drill-down taken to
  /// answer one question, and relaunching straight onto "Memory" would
  /// skip the overview -- the page that says whether there is anything
  /// to drill into today. The landing page has to be the landing page
  /// on launch, or it stops being one.
  healthPage: HealthPage;
  setHealthPage: (page: HealthPage) => void;
  /// How tightly PR rows pack.
  ///
  /// A global preference rather than per-view: it is about the user's
  /// eyes and screen, not about which list they happen to be reading.
  density: "comfortable" | "dense";
  setDensity: (density: "comfortable" | "dense") => void;
  /// The PR the detail view is showing, or null for the list.
  ///
  /// Deliberately NOT persisted: reopening the app on a detail page for a
  /// PR that has since merged is worse than landing on the list.
  selectedPr: { repo: string; number: number } | null;
  selectPr: (pr: { repo: string; number: number } | null) => void;
  /// Rows checked for a bulk action, keyed `repo#number`.
  ///
  /// Keyed rather than held as a list of PRs so selection is independent
  /// of the filtered list: narrowing a filter after selecting must not
  /// silently drop rows from the batch, which the issue calls out as the
  /// first requirement. Not persisted -- a selection is a working set for
  /// one sitting, and restoring it against PRs that may have merged
  /// would be worse than starting empty.
  checked: string[];
  toggleChecked: (key: string) => void;
  /// The last row toggled by a plain click, for shift-click ranges.
  ///
  /// Deliberately NOT persisted and cleared with the view, like
  /// `checked`: an anchor is meaningful only against the list currently
  /// on screen, and restoring one from a previous session would extend a
  /// range from a row the user never touched.
  anchor: string | null;
  /// Which visible row the keyboard cursor is on, or null for none.
  ///
  /// An INDEX into the visible list rather than a PR key, because the
  /// list the user is arrowing through is the filtered, sorted one --
  /// and a key would silently point at a row that filtering has removed.
  /// Cleared with the view for the same reason as `checked`.
  cursor: number | null;
  setCursor: (cursor: number | null) => void;
  setAnchor: (key: string | null) => void;
  setChecked: (keys: string[]) => void;
  clearChecked: () => void;
  reset: () => void;
}

const EMPTY_FILTERS: Record<View, Filters> = {
  "my-prs": {},
  "to-review": {},
  worktrees: {},
  branches: {},
  docker: {},
  artifacts: {},
  packages: {},
  "claude-md": {}, "claude-code": {},
  // PR Stats holds its own scope selection (#825): `statsScopeKind`,
  // `statsScopeValue` and `statsSubject`, written by `StatsSidebar`
  // through `setStatsScope`. Empty here like every other view -- the entry
  // is what makes `filtersByView` TOTAL over `View`, and
  // `useActiveFilters` reads `[view]` on every render, so a missing key is
  // the undefined-crash this record exists to prevent.
  //
  // Between #794 and #825 this held a `repo` written by the inherited
  // `RepoSidebar` and read by nothing. The keys changed; the reason the
  // entry exists did not.
  "pr-stats": {},
  // System Health has an entry like every other view even though it
  // has no filters to hold. `filtersByView` must be TOTAL over `View`
  // -- `useActiveFilters` reads `[view]` and every consumer reads
  // `.repo`/`.sort` off the result -- so a view omitted here is the
  // undefined-crash this record exists to prevent, not a saving.
  "system-health": {},
};

/// Filters and view survive a relaunch.
///
/// Every launch previously dropped the user on "All repositories, no
/// filters, sort newest", discarding whatever they were looking at. The
/// state is a flat bag of primitives and string arrays, so `persist` needs
/// no custom serialization.
///
/// `query` is deliberately NOT persisted: a search box that comes back
/// pre-filled with yesterday's text renders a filtered list that looks
/// like an empty one -- the same class of confusion as the old empty
/// state, and harder to diagnose because the cause is offscreen history.
/// The localStorage key this store persists under.
///
/// Exported so the error boundary's reset clears the SAME key this writes.
/// A hardcoded string in both places is one rename away from a reset
/// button that silently clears nothing -- and the crash it exists to
/// escape came from persisted state, so a no-op reset would loop forever.
export const PERSIST_KEY = "headstate-filters";

export const useFilters = create<FilterStore>()(
  persist(
    (set) => ({
      filtersByView: { ...EMPTY_FILTERS },
      view: "my-prs",
      density: "comfortable",
      setDensity: (density) => set({ density }),
      setFilter: (key, value) =>
        set((s) => ({
          filtersByView: {
            ...s.filtersByView,
            [s.view]: { ...s.filtersByView[s.view], [key]: value },
          },
          // Choosing a REPO is navigation -- it is how the sidebar
          // changes page -- so it must leave the detail view. Selecting
          // a repository and still staring at one pull request from a
          // different one meant clicking "Back to list" every time, and
          // the sidebar appeared to do nothing.
          //
          // Only `repo`. The other keys narrow the list you are looking
          // at, and closing the detail view on a label filter would
          // throw away what the user is reading.
          ...(key === "repo" ? { selectedPr: null } : {}),
        })),
      setStatsScope: (kind, value, subject) =>
        set((s) => ({
          filtersByView: {
            ...s.filtersByView,
            [s.view]: {
              ...s.filtersByView[s.view],
              statsScopeKind: kind,
              statsScopeValue: value,
              statsSubject: subject,
            },
          },
        })),
      // Preset navigation replaces the filter set wholesale, so a click
      // never inherits a filter the user forgot was active and shows a
      // count that doesn't match the list it opens.
      //
      // FILTERS AND PANEL ONLY. `density` and `setDensity` used to sit in
      // this object -- an indentation slip that read as two more top-level
      // store members (#806). It reset the user's density to "comfortable"
      // on every preset click, silently and with nothing on screen to
      // explain it, and rebuilt `setDensity` as a side effect of a filter
      // update, so the action had a second definition to keep in sync and
      // an identity that changed under anything holding a reference.
      //
      // The rule this encodes: a preset is about WHICH PRs you are looking
      // at, and density is about the user's eyes and screen -- so anything
      // a preset writes here has to be part of "which PRs", or it is
      // clobbering a preference the user set deliberately. The store test
      // asserts density outlives a preset, because nothing did and that is
      // precisely why this survived.
      // The `panel: "list"` reset here is gone with the axis (#852). It
      // existed to drop a preset user back out of the stats sub-page, and
      // that destination has been a view since #794 -- so by the time the
      // axis was removed this was writing the only value anything read.
      applyPreset: (filters) =>
        set((s) => ({
          filtersByView: { ...s.filtersByView, [s.view]: filters },
        })),
      // Selection clears with the view: a working set assembled on My
      // PRs means nothing on the review list, and carrying it across
      // would let a later batch act on rows the user cannot see.
      setView: (view) =>
        set({
          view,
          selectedPr: null,
          checked: [],
          anchor: null,
          cursor: null,
          // Leaving System Health and coming back lands on the
          // overview, for the same reason it is not persisted: the
          // drill-down answered a question that is now behind you, and
          // returning to a detail page skips the one that says whether
          // there is a new question worth asking.
          healthPage: "overview",
        }),
      healthPage: "overview",
      setHealthPage: (healthPage) => set({ healthPage }),
      selectedPr: null,
      selectPr: (selectedPr) => set({ selectedPr }),
      checked: [],
      anchor: null,
      cursor: null,
      setCursor: (cursor) => set({ cursor }),
      setAnchor: (anchor) => set({ anchor }),
      toggleChecked: (key) =>
        set((s) => ({
          checked: s.checked.includes(key)
            ? s.checked.filter((k) => k !== key)
            : [...s.checked, key],
        })),
      setChecked: (checked) => set({ checked }),
      clearChecked: () => set({ checked: [] }),
      // `repo` is sidebar NAVIGATION, not a filter chip -- it decides
      // which page you are on, scopes the priorities strip, and
      // pre-answers the wizard's repo step. Clearing it navigated the user
      // off the repo they were looking at, which is not what "Clear
      // filters" says it does.
      reset: () =>
        set((s) => {
          const current = s.filtersByView[s.view];
          return {
            filtersByView: {
              ...s.filtersByView,
              [s.view]: current.repo ? { repo: current.repo } : {},
            },
          };
        }),
    }),
    {
      name: PERSIST_KEY,
      // Bumped when the persisted SHAPE changes. Without this, a store
      // saved by v1 -- which had a flat `filters` and a `view` enum
      // conflating view with panel -- rehydrates straight into the new
      // shape, leaving `filtersByView` undefined and crashing on first
      // render. Tests never caught it because they always start empty.
      //
      // v3 (#794): Stats stopped being a `panel` value and became the
      // `pr-stats` VIEW. A store written by v2 can hold
      // `panel: "stats"`, which now routes nowhere -- such a user would
      // land on the PR list with no sign their Stats page had moved.
      //
      // v4 (#852): `panel` is gone entirely. A v3 store carries
      // `panel: "list"`, and `merge` below spreads the persisted object
      // over the current one -- so without this the field would be put
      // back onto a store whose type no longer has it, leaving a key
      // nothing reads and `partialize` re-persisting it forever. Dropping
      // it here is what actually ends the axis rather than just hiding it
      // from the type.
      version: 4,
      migrate: (persisted: unknown, from: number) => {
        // Run in ORDER and fall through, rather than one branch per
        // starting version. A v1 store that sat unopened across every
        // change has to go v1 -> v2 -> v3 -> v4; a chain of
        // `if (from === n)` arms would apply one and skip the others,
        // which is exactly the black-window class of bug the comment
        // below this is about.
        let state = persisted ?? {};
        if (from < 2) {
          // v1 -> v2: lift the single filter set into the active view,
          // and split `view` into view + panel. An unrecognised value
          // falls back to the defaults rather than propagating a bad
          // state.
          const old = state as { filters?: Filters; view?: string };
          // "dashboard" was v1's name for the stats page. v2 mapped it
          // onto `panel: "stats"`; since #794 the destination is the
          // view itself, so the hop through `panel` is gone and this
          // lands where v3 would have put it anyway.
          const view: View =
            old.view === "dashboard"
              ? "pr-stats"
              : old.view === "reviewing"
                ? "to-review"
                : "my-prs";
          state = {
            // The old flat filters belong to the view they were
            // filtering. "dashboard" had none of its own -- it showed
            // the whole account -- but `pr-stats` keeps the repo
            // sidebar, so carrying them there is what the user had.
            filtersByView: { ...EMPTY_FILTERS, [view]: old.filters ?? {} },
            view,
            panel: "list",
          };
        }
        if (from < 3) {
          // v2 -> v3: a stored `panel: "stats"` meant "My PRs, showing
          // the stats page". That destination is now a view, so move
          // the user THERE and reset `panel` to the only value My PRs
          // and Docker still share. Left as "stats" it would be a value
          // no route reads, and the user would silently lose the page
          // they closed the app on.
          const old = state as { panel?: string; view?: string };
          if (old.panel === "stats") {
            state = {
              ...old,
              // Only from My PRs. `panel` is shared with Docker, and a
              // Docker user cannot have set "stats" -- but a store hand-
              // edited or written by a build mid-rename could, and
              // teleporting someone off Docker would be worse than
              // dropping a value that was never reachable there.
              ...(old.view === "my-prs" || old.view === undefined
                ? { view: "pr-stats" }
                : {}),
              panel: "list",
            };
          }
        }
        if (from < 4) {
          // v3 -> v4: drop `panel` (#852). The axis is gone, so a stored
          // value is a key nothing reads -- and `merge` below spreads the
          // persisted object over the current store, so leaving it would
          // put it back on every launch and `partialize` would re-persist
          // it forever.
          //
          // The two arms above deliberately still WRITE `panel`: they
          // describe the shapes v2 and v3 actually had, and rewriting
          // history to pretend the field never existed would make the
          // "stats" rescue above unreadable. This arm is where it leaves,
          // which is also why the chain has to fall through rather than
          // branch -- a v1 store must reach here too.
          //
          // `delete` on a COPY, not `panel: undefined`. An explicit
          // `undefined` is still an own property, and `merge`'s spread
          // would write it over the default -- so the key would survive
          // with a worse value than before. A copy rather than mutating
          // `persisted`, which the caller still owns.
          //
          // `delete` rather than a rest destructure because the binding a
          // rest pattern leaves behind is unused by construction, and this
          // project's lint rules correctly refuse an unused variable
          // whatever it is named.
          const rest = { ...(state as Record<string, unknown>) };
          delete rest.panel;
          state = rest;
        }
        return state as never;
      },
      // Stored state is REPLACED into the store, not merged, so adding a
      // view to the `View` union silently breaks every existing install:
      // the persisted `filtersByView` has no key for it, and reading
      // `.sort` off undefined takes down the entire app with a black
      // window. This happened for real when `worktrees` and `docker`
      // were added -- both were already version 2, so the migration
      // above returned the old shape untouched.
      //
      // Merging per-view against EMPTY_FILTERS makes the store complete
      // by construction, for every view that exists now or later.
      merge: (persisted, current) => {
        const p = (persisted ?? {}) as Partial<FilterStore>;
        return {
          ...current,
          ...p,
          filtersByView: { ...EMPTY_FILTERS, ...(p.filtersByView ?? {}) },
        };
      },
      partialize: (s) => ({
        // `query` is dropped per view for the same reason as before: a
        // search box restored with yesterday's text renders a filtered
        // list that looks like an empty one.
        filtersByView: Object.fromEntries(
          Object.entries(s.filtersByView).map(([k, f]) => [k, { ...f, query: undefined }]),
        ) as Record<View, Filters>,
        view: s.view,
        // `panel` is gone (#852). Left here it would re-persist the field
        // the v4 migration drops, so the next launch would read it back
        // and the axis would survive its own removal.
        density: s.density,
      }),
    },
  ),
);

/// Shared empty-filter object, so the fallback below is reference-stable.
const NO_FILTERS: Filters = Object.freeze({});

/// The active view's filters.
///
/// A selector rather than a stored field, so there is exactly one source
/// of truth and no chance of the two drifting apart.
export function useActiveFilters(): Filters {
  // NO_FILTERS is a module constant, not an inline `?? {}`. zustand
  // compares selector results by reference, so returning a fresh `{}`
  // each call makes every read look like a change and spins
  // useSyncExternalStore into an infinite re-render -- a worse failure
  // than the crash this guards against. Caught by the test below.
  //
  // The guard itself is real, not defensive noise. `filtersByView` is
  // rehydrated from disk, and `persist` REPLACES this object rather than
  // merging it -- so a store written before a view existed comes back
  // without that view's key. Every consumer reads `.sort` / `.repo` off
  // this value, so returning undefined crashes the whole tree.
  return useFilters((s) => s.filtersByView[s.view] ?? NO_FILTERS);
}

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
  // nothing else reads the sequence.
  //
  // It used to say the two lists were kept in the same order deliberately,
  // "so a reader comparing them should not have to wonder which one the
  // user sees". That stopped being true in #1017: `ViewSwitcher`'s `VIEWS`
  // is grouped now, and the menu renders group by group, so the order a
  // user sees is `GROUPS` first and position within a group second. The
  // two lists still happen to read alike, but only by coincidence, and a
  // comment claiming otherwise is one a reader would rely on.
  //
  // What IS still true, and is the part worth keeping: `VIEWS` is what
  // the menu renders and this list only derives the type. When they
  // disagree about membership that is a bug -- `readme.views.test.ts` and
  // `ViewSwitcher.test.tsx` both assert every `ALL_VIEWS` id reaches
  // `VIEWS` exactly once. When they disagree about ORDER, that is now
  // expected.
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
  // The repository browser (#1023, epic #1011). Beside `worktrees` and
  // `branches` because it is the third view over the same checkouts --
  // the grouped menu files all three under "Repos" -- and it is the only
  // one of the three that answers "what is IN this repository" rather
  // than "what state is it in".
  //
  // Registered here unconditionally, like every other view: `View` is
  // the type every route and preference is keyed on, and `filtersByView`
  // is TOTAL over it. A view added to `ViewSwitcher`'s array and not to
  // this one would be a menu entry with no bucket for its repo selection
  // -- the undefined-crash `EMPTY_FILTERS` exists to prevent.
  "repositories",
  // Last, deliberately: the only entry that is not about pull requests.
  "system-health",
] as const;

export type View = (typeof ALL_VIEWS)[number];

/// The name a view goes by in the UI.
///
/// A `Record<View, string>` rather than a ternary chain with a default
/// arm, because the default arm is a silent failure: a view added to
/// `ALL_VIEWS` without a label here rendered as "Pull requests", which
/// is the same header the PR list shows. #916's finding, in a second
/// place -- a registered view id with no arm reads as a page that
/// quietly went somewhere else. Being total, this is a compile error.
///
/// These are the HEADER names, carried over verbatim from the ternary
/// chain this replaced. Four of them do not match the switcher entry
/// that opens the page -- "My pull requests" opens a page headed "Pull
/// requests", "To review" one headed "Pull requests to review",
/// "Docker" one headed "Docker images", "Artifacts" one headed "Build
/// artifacts" -- even though the chain's own comments claimed three
/// times that they matched exactly (#794). Reconciling them changes
/// four visible headers, so it is filed separately rather than smuggled
/// into a refactor; `viewLabel.switcher.test.ts` pins the pairs that DO
/// match so the gap cannot widen unnoticed.
const VIEW_LABELS: Record<View, string> = {
  // The MENU wording, which is what the user clicked (#1185). Four of
  // these used to differ from the switcher entry -- "My pull requests"
  // opened a page headed "Pull requests", which is also the generic
  // fallback the old default arm produced, so the page specifically
  // about your own PRs carried the least specific name in the app.
  //
  // #794's rule is that the header should match the menu item that
  // opened it. `ViewSwitcher` now READS this table rather than keeping
  // its own, so there is one place to change and nothing left to drift.
  "my-prs": "My pull requests",
  "to-review": "To review",
  "pr-stats": "PR Stats",
  "claude-md": "CLAUDE.md",
  packages: "Package updates",
  artifacts: "Artifacts",
  docker: "Docker",
  worktrees: "Worktrees",
  branches: "Branches",
  "claude-code": "Claude Code",
  repositories: "Repositories",
  "system-health": "System health",
};

export function viewLabel(view: View): string {
  return VIEW_LABELS[view];
}

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

/// The Claude Code view's two pages (#921).
///
/// A separate axis from `healthPage`, for the reason that one gives for
/// being separate from `panel`: a page that only exists inside one view
/// must not be nameable from another. Widening `HealthPage` to hold
/// `"sessions"` would mean every consumer of it -- `healthPagesFor`, the
/// sidebar's filter, the phone's card list -- had to know about a page
/// System Health does not have.
///
/// `"sessions"` is FIRST and is the default, unlike System Health's
/// `"overview"`. The list is what a user opens this view to do -- get a
/// crashed session back -- and #917's route comment makes the same point
/// about why the list is not behind a lazy boundary: the moment you need
/// it is the worst moment to wait. The overview is where you go to see
/// the shape of things, which is a second question rather than the first.
/// A plain union, not an `as const` array like `ALL_HEALTH_PAGES`.
///
/// That one is an array because `healthPagesFor` FILTERS it at runtime --
/// a machine with no discoverable GPU is offered no GPU page. Every
/// Claude Code page is offered unconditionally, so nothing ever iterates
/// the set: the sidebar's own `CLAUDE_PAGES` carries the labels and icons
/// and is the only list there is. An array here would be a second
/// declaration of the same names with nothing reading it, which is what
/// `yarn knip` objects to and it is right.
export type ClaudePage = "sessions" | "overview" | "plugins";

/// Which subset of the Claude Code session list is showing (#949).
///
/// # Why these five and not a set of independent toggles
///
/// Because two of them are the same axis. `cwd_state` is a TRI-state --
/// `exists` / `gone` / `unknown` -- so "resumable" and "gone" are two of its
/// three values and cannot both be on. `liveness` is a second, genuinely
/// independent axis (measured: all 1,474 transcripts exist while 1,295
/// directories do not), and "running" and "crashed" are two of its values.
/// Crossing them would be a matrix of twenty states, nineteen of which
/// nobody asks for.
///
/// One of five, therefore, and `"all"` is the default rather than a sixth
/// chip that means "no chip": a mode with a name is one the reader can see
/// they are in.
///
/// # `unknown` is swept into neither
///
/// `"resumable"` and `"gone"` are the overview's own two predicates, which
/// `matchesClaudeFilter` states and `overview.rs` owns the originals of: not
/// running plus `cwd_state === "exists"`, and not running plus
/// `cwd_state === "gone"`. So a session whose directory could not be checked
/// appears under neither -- which is what the overview's tiles already do
/// (it counts them "as neither"). Folding `unknown` in with `gone` would
/// tell the user a directory is missing when the check itself is what
/// failed, and `revealRefusal` gives the four states four different
/// sentences precisely so that cannot happen.
///
/// Matching the tiles exactly is load-bearing as of #948, which makes those
/// tiles navigate here: a tile reading 179 that opens a list of 181 rows is
/// a tile that lied about where it went.
///
/// # Why `"ended"` and not `"crashed"`
///
/// #949 asks for a Crashed chip and calls a crash "the single most
/// actionable state in the whole feature", which is right. It is not
/// available as a client-side predicate, and the reason is worth stating
/// rather than shipping the chip anyway.
///
/// A crash is one particular way of being `Liveness::Dead`: the live
/// registry lists a pid that is no longer running, which means the process
/// did not report its own `SessionEnd`. The registry file outlives a
/// SIGKILL, which is what makes that inference sound -- and `liveness.rs`
/// draws it. But the DISTINCTION crosses the wire only inside
/// `liveness.why`, as the sentence "…so this session ended without shutting
/// down". There is no flag.
///
/// So a Crashed chip would have to substring-match a sentence owned by
/// Rust. That is a second description of a rule that already has one, in a
/// language whose compiler cannot see the first -- and the failure is
/// silent and total: a reworded sentence leaves the chip matching zero rows
/// while every test that mocks its own fixtures stays green. `ClaudeCodePage`
/// makes exactly this argument about stating the ordering rule once, and
/// `notify.rs` makes it about thresholds.
///
/// `"ended"` is the honest structured neighbour: `liveness.state === "dead"`,
/// which is every session that has finished, crash or clean. It is a real
/// narrowing of 1,474 rows and it promises only what it can check. A
/// crashed chip wants a structured field on `Liveness::Dead` -- see the PR
/// body, which proposes one rather than smuggling it in here.
export type ClaudeSessionFilter = "all" | "resumable" | "gone" | "running" | "ended";

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
  /// Which Claude Code page is open (#921, reordered by #939).
  ///
  /// Not persisted, for the same reason `healthPage` is not: this is a
  /// position within a view rather than a preference, and the landing
  /// page has to be the landing page on launch.
  ///
  /// The DEFAULT is `"overview"` as of #939, which reverses #921's
  /// choice. #921 argued the sessions list is what the view is opened
  /// for, so relaunching onto the overview "would put a chart between the
  /// user and the list they came for". Using it showed the premise wrong
  /// on both halves: the overview is not only a chart -- it carries the
  /// resurrection tiles and the resumable list, which IS what a user
  /// acts on -- and since #939 the sessions list is no longer behind the
  /// overview at all. It lives in the sidebar, on screen whichever page
  /// is selected, so landing on the overview costs nothing the old
  /// default was protecting.
  ///
  /// No migration is needed for the change of default, and that is worth
  /// stating rather than assuming: `partialize` has never listed this key,
  /// so no install has one on disk. And `"sessions"` is still a member of
  /// `ClaudePage`, so even a hand-edited store carrying it would spread
  /// through `merge` into a value the sidebar can render -- the change
  /// moves which page you LAND on, not which pages exist.
  claudePage: ClaudePage;
  setClaudePage: (page: ClaudePage) => void;
  /// The Claude Code session list's search text (#939).
  ///
  /// In the store rather than `useState` inside `ClaudeCodePage` because
  /// the box and the rows it filters moved into `ClaudeCodeSidebar`,
  /// which is a SIBLING of the page rather than its child -- the two
  /// components that need this value have no common ancestor below
  /// `App`. This is where cross-component view state already lives, next
  /// to `claudePage`, which those same two components already share for
  /// the same reason.
  ///
  /// Not persisted, deliberately: `partialize` already strips every
  /// per-view `query` because "a search box restored with yesterday's
  /// text renders a filtered list that looks like an empty one". This is
  /// the same box over a longer list, so it gets the same treatment.
  claudeQuery: string;
  setClaudeQuery: (query: string) => void;
  /// Which of the session list's five states is showing (#949).
  ///
  /// The two axes a user actually acts on -- whether the directory still
  /// exists, and whether the session crashed -- were on every row and
  /// readable only as prose. Search covered four TEXT fields, so on a
  /// corpus where 87.9% of directories are gone the user paged through
  /// 1,295 rows of archaeology to reach the 179 that can be resumed into
  /// place.
  ///
  /// Beside `claudeQuery` rather than in `filtersByView` for exactly the
  /// reason `claudeQuery` gives: the chips and the rows they filter both
  /// live in `ClaudeSessionColumn`, which is a SIBLING of the page, and
  /// `filtersByView` is keyed by `View` -- one bucket for the whole of
  /// Claude Code, where the session list is only one of its pages. Putting
  /// a session-list predicate there would file it under the same key the
  /// PR filters use a different shape of.
  ///
  /// Not persisted, like `claudeQuery` and for the same reason: a list
  /// restored under yesterday's chip is a short list that looks like an
  /// empty one. It also resets with the view, below.
  claudeFilter: ClaudeSessionFilter;
  setClaudeFilter: (filter: ClaudeSessionFilter) => void;
  /// Whether subagent sessions are shown (#1002).
  ///
  /// # Why a toggle beside the chips rather than a sixth chip
  ///
  /// The five chips are ONE axis by construction -- the comment on
  /// `ClaudeSessionFilter` sets out why: `cwd_state` is a tri-state and
  /// `liveness` is a second, and crossing them would be a matrix of twenty
  /// states nobody asks for. Subagent-ness is a THIRD axis and a genuinely
  /// independent one: "the running subagents" and "the subagents whose
  /// directory is gone" are both real questions, and folding this in as a
  /// sixth mutually-exclusive chip would make them unaskable.
  ///
  /// So it is a boolean crossed with the chip, which is the one shape that
  /// costs no new states: every chip means the same thing it did, over a
  /// list that either includes the machinery or does not.
  ///
  /// Default `false`. 391 of 1,524 measured rows are subagents, and with
  /// one live session the rows below it were that session's own machinery
  /// rather than the user's past work -- which is what #1002 reports.
  /// Hidden, never deleted: they stay searchable the moment this is on,
  /// and they remain real, resumable sessions.
  ///
  /// Not persisted, like `claudeQuery` and `claudeFilter` above and for
  /// the same reason: a list restored under yesterday's control is a short
  /// list that looks like an empty one.
  claudeShowSubagents: boolean;
  setClaudeShowSubagents: (show: boolean) => void;
  /// Jump from the overview to the session list, filtered (#948).
  ///
  /// ONE action rather than a `setClaudePage` call followed by a
  /// `setClaudeFilter` call, and the reason is #920's. That jump -- the
  /// "Show in Worktrees" button -- had to call `setView` BEFORE `setFilter`
  /// because `setFilter` writes into `filtersByView[state.view]`, the
  /// CURRENT view, so the natural-reading order filed the value under the
  /// page being left and the destination opened on its default. A test
  /// asserting only the view would not have noticed; the one asserting both
  /// caught it.
  ///
  /// `claudeFilter` is a flat field, so the same bug is not available here
  /// -- which is exactly why this is one action. An invariant that holds
  /// only because of how a field happens to be stored is one a later change
  /// to the storage silently breaks, and the call sites would all still
  /// compile. A single `set` makes the pairing structural: there is no
  /// order for a caller to get wrong, and the two values land in one render
  /// rather than two.
  ///
  /// Clears the search text. A chip and a leftover query intersect, so a
  /// tile reading 179 would land on a list of however many of those 179
  /// also match yesterday's search -- a number that matches the tile the
  /// user just pressed only by luck. The tile's figure is a promise about
  /// what the next screen shows.
  showClaudeSessions: (filter: ClaudeSessionFilter) => void;
  /// The Claude Code session whose detail the main panel shows, or
  /// undefined for none (#939).
  ///
  /// Store state for the same reason as `claudeQuery`: the rows that set
  /// it are in the sidebar and the detail that reads it is in the page.
  ///
  /// Holds the session ID and not the session, so a rescan cannot leave a
  /// stale COPY of a row on screen. The consequence is that the id may
  /// name a session the current list no longer contains -- a transcript
  /// deleted between polls -- which is why `ClaudeCodePage` resolves it
  /// by lookup against the live list on every render. A lookup that
  /// misses renders the choose-a-session prompt; it can never render a
  /// detail pane built out of remembered fields.
  ///
  /// Not persisted: relaunching onto the detail of a session whose
  /// transcript has since been deleted is worse than landing on the list,
  /// which is the reason `selectedPr` gives for the same choice.
  claudeSelected: string | undefined;
  selectClaudeSession: (id: string | undefined) => void;
  /// Where inside the selected repository the browser is (#1034).
  ///
  /// Repository-relative, `""` for the root, and it names a DIRECTORY --
  /// `repoFile` below is the file being read within it, so "which folder
  /// am I in" and "which file am I reading" stay two questions rather
  /// than one string the reader has to disambiguate.
  ///
  /// # In the store rather than `useState`
  ///
  /// `claudeQuery`'s reason exactly: `RepoPickerSidebar` and the file
  /// panel are SIBLINGS under `App` with no common ancestor below it, and
  /// the sidebar's selection has to reset the panel's path. This is where
  /// cross-component view state already lives, beside `claudePage`.
  ///
  /// # NOT in `filtersByView`
  ///
  /// `claudeFilter`'s reason, and it is the same shape: `filtersByView`
  /// is keyed by `View` and holds `Filters`, whose keys narrow a list.
  /// A path is not a filter; it is navigation. Filing it there would put
  /// it under the same key the repo selection uses a different shape of,
  /// and `activeFilterCount` would have to learn to exclude it the way it
  /// already excludes `repo`.
  ///
  /// # NOT persisted
  ///
  /// Absent from `partialize`, and no migration is needed -- worth
  /// stating rather than assuming, per `claudePage`'s comment about a key
  /// `partialize` has never listed: no install has one on disk.
  ///
  /// The `query` argument, applied to a path, and it lands harder. A
  /// search box restored with yesterday's text "renders a filtered list
  /// that looks like an empty one"; a PATH restored from yesterday can
  /// point at a directory that no longer exists -- on this machine ~100
  /// sibling agent worktrees are created and destroyed continuously -- and
  /// the user did not ask to go there, so the empty listing reads as a
  /// broken repository rather than as a stale restore.
  ///
  /// The repository SELECTION stays persisted, because it already is and
  /// it is a genuine preference. The position inside it is not.
  ///
  /// Do not add this to `partialize` later without deciding what a
  /// vanished path renders as.
  repoPath: string;
  setRepoPath: (path: string) => void;
  /// The file the browser is reading, repository-relative, or undefined
  /// for "showing the directory listing" (#1034).
  ///
  /// A second field rather than a mode flag on `repoPath`, so descending
  /// and reading cannot contradict each other: opening a file keeps the
  /// directory it was opened from, which is what "back to the listing"
  /// needs and what the GitHub code view shows in its breadcrumb.
  ///
  /// Not persisted, for `repoPath`'s reason and one more of its own,
  /// which `selectedPr` already states about itself: relaunching onto the
  /// detail of something that has since been deleted is worse than
  /// landing on the list.
  repoFile: string | undefined;
  setRepoFile: (path: string | undefined) => void;
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
  "claude-md": {},
  "claude-code": {},
  // Repositories holds a `repo`, written by the SAME `RepoPickerSidebar`
  // that Packages and CLAUDE.md use (#1030). The position INSIDE the
  // repository is `repoPath` below, which is deliberately not here --
  // see its comment for why a path is navigation rather than a filter.
  repositories: {},
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
          //
          // The browser's position goes with it (#1034), and for the
          // same reason at one more remove: selecting a DIFFERENT
          // repository while three directories deep must not carry
          // `src/components/` into a repository that has no such path,
          // and the file being read certainly does not survive it.
          //
          // Reset INSIDE the setter, not at each caller. `filters.ts:378`
          // records a bug of exactly this shape -- "Show in Worktrees"
          // had to call `setView` BEFORE `setFilter` because `setFilter`
          // writes into `filtersByView[state.view]` -- and every caller
          // doing its own reset is how that ordering hazard spreads.
          // There is no order for a caller to get wrong if there is
          // nothing for a caller to do.
          ...(key === "repo" ? { selectedPr: null, repoPath: "", repoFile: undefined } : {}),
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
          // Same rule, and since #939 the same default. Leaving Claude
          // Code and coming back lands on the OVERVIEW: it carries the
          // resurrection tiles and the resumable list, which is what the
          // returning user acts on, and the sessions list is in the
          // sidebar either way so returning here loses nothing. #921 had
          // this reset to "sessions" on the premise that the overview was
          // a chart standing between the user and the list; #939 moved the
          // list out from behind it, so the premise is gone.
          //
          // The search text and the selected session go with it, for the
          // reason the block above gives for `checked` and `anchor`: a
          // query typed against this machine's sessions means nothing on
          // the review list, and a session id selected before leaving
          // would restore a detail pane the user did not ask for.
          claudePage: "overview",
          claudeQuery: "",
          // And the chip (#949), for the reason the sentence above gives
          // about the query: a filter narrowed on this machine's sessions
          // means nothing on the review list, and coming back to a list
          // showing 179 of 1,474 rows under a chip the user set last week
          // is the short-list-that-looks-empty failure with a control
          // instead of a search box behind it.
          claudeFilter: "all",
          // And the subagent toggle (#1002), for the same reason: coming
          // back to a list that silently includes 391 rows of machinery
          // because it was switched on last week is the same
          // stale-control failure in the other direction.
          claudeShowSubagents: false,
          claudeSelected: undefined,
          // And the browser's position (#1034), for the reason this
          // block gives throughout: a position inside one view means
          // nothing on another, and coming back to a file panel opened
          // last week is the stale-restore failure the non-persistence
          // above is about, arriving by a different route.
          repoPath: "",
          repoFile: undefined,
        }),
      healthPage: "overview",
      setHealthPage: (healthPage) => set({ healthPage }),
      // `"overview"` here and in `setView` above, which must agree: one is
      // the value on a cold launch and the other the value on re-entry,
      // and two different landing pages for the same view would be a
      // position that depends on how you got there.
      claudePage: "overview",
      setClaudePage: (claudePage) => set({ claudePage }),
      claudeQuery: "",
      setClaudeQuery: (claudeQuery) => set({ claudeQuery }),
      claudeFilter: "all",
      setClaudeFilter: (claudeFilter) => set({ claudeFilter }),
      claudeShowSubagents: false,
      setClaudeShowSubagents: (claudeShowSubagents) => set({ claudeShowSubagents }),
      // Page, filter and query in one `set`, so the list cannot render for
      // a frame under the old chip and so no caller can order the three
      // wrongly. `claudeSelected` goes too: arriving on a filtered list
      // with a detail pane already open for a session the chip may have
      // just excluded would be a pane describing a row that is not there.
      showClaudeSessions: (claudeFilter) =>
        set({
          claudePage: "sessions",
          claudeFilter,
          claudeQuery: "",
          // The overview's tiles count the user's OWN sessions, so a tile
          // that opened a list including subagents would show more rows
          // than the figure the user just pressed. #948's rule: a tile's
          // number is a promise about what the next screen shows.
          claudeShowSubagents: false,
          claudeSelected: undefined,
        }),
      claudeSelected: undefined,
      selectClaudeSession: (claudeSelected) => set({ claudeSelected }),
      repoPath: "",
      // Descending or going up CLEARS the file being read, in one `set`
      // rather than two calls a caller has to order (#1034). The pairing
      // is structural for the reason `showClaudeSessions` gives about its
      // own: an invariant that holds only because callers remember is one
      // a later call site silently breaks, and every call site would
      // still compile.
      setRepoPath: (repoPath) => set({ repoPath, repoFile: undefined }),
      repoFile: undefined,
      // Opening a file does NOT move `repoPath`: the directory it was
      // opened from is what "back to the listing" returns to, and what a
      // breadcrumb names.
      setRepoFile: (repoFile) => set({ repoFile }),
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
        // `repoPath` and `repoFile` are DELIBERATELY absent (#1034), and
        // the absence is the decision rather than an omission. A path
        // restored from yesterday can point at a directory that no longer
        // exists, and the user did not ask to go there -- so the empty
        // listing reads as a broken repository rather than as a stale
        // restore. The repository SELECTION is still persisted, inside
        // `filtersByView`, because that is a preference and this is a
        // position. No migration is needed: `partialize` has never listed
        // either key, so no install has one on disk.
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

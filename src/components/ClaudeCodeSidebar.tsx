import { Blocks, Bot, Gauge, ListTree } from "lucide-react";
import { useIsMobile } from "../lib/useIsMobile";
import { type ClaudePage, type View, useFilters } from "../store/filters";
import { ClaudeSessionColumn } from "./ClaudeCodePage";
import { ViewSwitcher } from "./ViewSwitcher";

/// Every Claude Code page, in sidebar order.
///
/// Exported so the sidebar and any other renderer of the same list cannot
/// drift into offering different pages -- the failure `HEALTH_PAGES` exists
/// to prevent, and #675 fixed for `VIEWS`.
///
/// `"overview"` is first because it is the landing page
/// (#939). #921 put `"sessions"` first and argued for it: "the view is
/// opened to get a crashed session back, and the overview answers a second
/// question rather than the first." Using the view disproved both halves.
/// The overview is not a chart standing in the way of that answer -- it
/// carries the resurrection tiles and the resumable list, which is the
/// thing a user acts on, so it answers the FIRST question. And the
/// sessions list is no longer behind it: since #939 the list renders in
/// this column, under the `Sessions` row, a glance away whichever page is
/// selected. "Sessions first" was buying immediate access to something
/// that is no longer hidden, at the price of hiding the tiles.
///
/// The store's `claudePage` default and its `setView` reset both say
/// `"overview"` to match. Three places, one decision: were they to
/// disagree, where you land would depend on how you arrived.
///
/// This is the ONLY list of the pages -- `ClaudePage` in the store is a
/// plain union rather than an `as const` array, precisely so there is no
/// second declaration of the same two names for this one to drift from.
export const CLAUDE_PAGES: {
  id: ClaudePage;
  label: string;
  Icon: typeof Bot;
}[] = [
  { id: "overview", label: "Overview", Icon: Gauge },
  { id: "sessions", label: "Sessions", Icon: ListTree },
  // Last: it answers a question asked occasionally ("am I getting value
  // out of what I installed?") rather than the one the view is opened
  // for. Its scan is also the most expensive thing behind any of these
  // pages, and putting it above Sessions would make arriving here cost a
  // corpus read nobody asked for.
  { id: "plugins", label: "Plugins", Icon: Blocks },
];

/// The Claude Code view's own sidebar (#917, #921, #939).
///
/// It has NO repository axis, and none of the repository sidebars fits. A
/// session is not scoped to a repository: 665 distinct working directories
/// over 1,461 sessions on the development machine, mostly deleted agent
/// worktrees, and 83% of them no longer on disk. A repository picker here
/// would be a column whose every row is inert -- the same reasoning
/// `App.tsx` gives for `system-health` declining one, where "an empty
/// picker under a heading reads as a page that failed to load".
///
/// So the column holds the view switcher, the view's own two pages, and --
/// since #939 -- the session list that the `Sessions` page is about.
///
/// # Why the search box and the session rows are here now
///
/// They used to be a `w-96` column inside the main panel, which meant the
/// main panel carried its own nested navigation: a search box over 200
/// rows that choose what the pane beside them shows, sitting immediately
/// right of the real sidebar. Two columns of navigation, one of them
/// dressed as content. Moved here, the sidebar holds navigation and the
/// main panel holds one thing -- the selected session's detail, or the
/// overview.
///
/// # The phone objection, and how it is answered rather than dropped
///
/// This comment used to say the search box was "deliberately NOT here",
/// because "on a phone this column is a sheet that closes on navigation,
/// which would take the search with it". That objection is correct and
/// still stands: `App.tsx` renders this component inside a `Sheet` below
/// `MOBILE_BREAKPOINT`, and `navOpen` there is derived from the view and
/// repo it was opened at, so the sheet is dismissed as soon as you
/// navigate. A search box reachable only from behind a hamburger, which
/// closes when you tap a result, is not a usable search box.
///
/// So on the phone the list is NOT in this column. `ClaudeSessionColumn` has
/// two mount points and `useIsMobile()` picks between them:
///
/// | width | where the list renders | why |
/// |---|---|---|
/// | desktop | here, under `Sessions` | this column is persistent, so the box stays put while the detail changes beside it |
/// | phone | the main panel, as the list SCREEN | the sheet closes on navigation, so a list inside it could not survive being used |
///
/// The phone therefore keeps exactly the two-screen behaviour it had
/// before #939: `ClaudeCodePage` shows the list until a session is picked
/// and the detail afterwards, with a back link. Nothing about the narrow
/// layout changed; what changed is that the wide layout stopped imitating
/// it. `HEALTH_PAGES` is the precedent for one list with two renderings --
/// there the phone redraws the sidebar's rows as cards on the overview,
/// for the same underlying reason: a column the phone cannot keep open is
/// not a place to put something the thumb needs.
///
/// One list, one component, at both mount points. A second copy for the
/// phone would drift from this one the first time either was touched,
/// which is the rule `useIsMobile`'s own doc comment states.
///
/// The list renders only while `sessions` is the open page, not under both
/// rows: it is that page's content, and 200 session rows drawn beneath a
/// selected `Overview` would have the column answering a question the user
/// has navigated away from.
export function ClaudeCodeSidebar({
  viewCounts,
}: {
  viewCounts?: Partial<Record<View, number>>;
}) {
  const { claudePage, setClaudePage } = useFilters();
  // `useIsMobile` rather than `IS_MOBILE_BUILD`: the question is whether
  // this column is a sheet right now, which is a question about WIDTH, so
  // a desktop user who drags the window under the breakpoint gets the
  // phone's arrangement too -- they are behind the same hamburger.
  const isMobile = useIsMobile();

  return (
    <nav className="flex w-64 shrink-0 flex-col border-r border-[#30363d] p-3">
      <ViewSwitcher counts={viewCounts} />
      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto">
        {/* A heading, because these rows are a different KIND of thing
            from the switcher above them: that one changes which view you
            are in, these move within one view. Two unlabelled stacks of
            buttons would read as one list whose first entry happens to
            look different. `SystemHealthSidebar` states the same rule. */}
        <div className="shrink-0 px-3 pb-1 pt-2 text-xs font-semibold uppercase tracking-wide text-[#8b949e]">
          <Bot className="mr-1.5 inline h-3 w-3" aria-hidden="true" />
          Claude Code
        </div>
        <ul className="flex shrink-0 flex-col">
          {CLAUDE_PAGES.map(({ id, label, Icon }) => (
            <li key={id}>
              <button
                type="button"
                onClick={() => setClaudePage(id)}
                // `aria-current="page"` rather than `aria-pressed`: these
                // are navigation, not toggles, and a screen reader
                // announcing "pressed" for the page you are already on
                // describes a control that did something rather than a
                // location you are at.
                aria-current={id === claudePage ? "page" : undefined}
                className={`flex w-full items-center gap-2 rounded px-3 py-2 text-sm ${
                  id === claudePage
                    ? "bg-[#1f6feb] text-white"
                    : "text-[#e6edf3] hover:bg-[#161b22]"
                }`}
              >
                <Icon className="h-4 w-4 shrink-0" aria-hidden="true" />
                <span className="truncate">{label}</span>
              </button>
            </li>
          ))}
        </ul>
        {/* `-mx-3` so the search box and the rows span the column's full
            width instead of sitting inside the nav's padding: at this
            point the list IS the column's content rather than an item in
            it, and the border above it reads as the edge of a section. */}
        {claudePage === "sessions" && !isMobile ? (
          <div className="-mx-3 mt-2 flex min-h-0 flex-1 flex-col border-t border-[#30363d]">
            <ClaudeSessionColumn />
          </div>
        ) : null}
      </div>
    </nav>
  );
}

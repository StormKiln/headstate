import { Bot, Gauge, ListTree } from "lucide-react";
import { type ClaudePage, type View, useFilters } from "../store/filters";
import { ViewSwitcher } from "./ViewSwitcher";

/// Every Claude Code page, in sidebar order.
///
/// Exported so the sidebar and any other renderer of the same list cannot
/// drift into offering different pages -- the failure `HEALTH_PAGES` exists
/// to prevent, and #675 fixed for `VIEWS`.
///
/// Two entries, and `"sessions"` is first because it is the landing page:
/// the view is opened to get a crashed session back, and the overview
/// answers a second question rather than the first. The store's
/// `claudePage` default says the same thing, and its `setView` reset
/// returns here on every re-entry.
///
/// This is the ONLY list of the pages -- `ClaudePage` in the store is a
/// plain union rather than an `as const` array, precisely so there is no
/// second declaration of the same two names for this one to drift from.
export const CLAUDE_PAGES: {
  id: ClaudePage;
  label: string;
  Icon: typeof Bot;
}[] = [
  { id: "sessions", label: "Sessions", Icon: ListTree },
  { id: "overview", label: "Overview", Icon: Gauge },
];

/// The Claude Code view's own sidebar (#917, #921).
///
/// It has NO repository axis, and none of the repository sidebars fits. A
/// session is not scoped to a repository: 665 distinct working directories
/// over 1,461 sessions on the development machine, mostly deleted agent
/// worktrees, and 83% of them no longer on disk. A repository picker here
/// would be a column whose every row is inert -- the same reasoning
/// `App.tsx` gives for `system-health` declining one, where "an empty
/// picker under a heading reads as a page that failed to load".
///
/// So the column holds the view switcher and the view's own two pages,
/// which is exactly `SystemHealthSidebar`'s shape: navigation within the
/// thing the page is about, which is what every sidebar in this app holds.
///
/// The list's search box is deliberately NOT here. It filters the session
/// list, so it belongs above the rows it acts on -- and on a phone this
/// column is a sheet that closes on navigation, which would take the
/// search with it.
export function ClaudeCodeSidebar({
  viewCounts,
}: {
  viewCounts?: Partial<Record<View, number>>;
}) {
  const { claudePage, setClaudePage } = useFilters();

  return (
    <nav className="flex w-64 shrink-0 flex-col border-r border-[#30363d] p-3">
      <ViewSwitcher counts={viewCounts} />
      <div className="min-h-0 flex-1 overflow-y-auto">
        {/* A heading, because these rows are a different KIND of thing
            from the switcher above them: that one changes which view you
            are in, these move within one view. Two unlabelled stacks of
            buttons would read as one list whose first entry happens to
            look different. `SystemHealthSidebar` states the same rule. */}
        <div className="px-3 pb-1 pt-2 text-xs font-semibold uppercase tracking-wide text-[#8b949e]">
          <Bot className="mr-1.5 inline h-3 w-3" aria-hidden="true" />
          Claude Code
        </div>
        <ul className="flex flex-col">
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
      </div>
    </nav>
  );
}

import { Bot } from "lucide-react";
import { type View } from "../store/filters";
import { ViewSwitcher } from "./ViewSwitcher";

/// The Claude Code view's own sidebar (#917).
///
/// It has NO axis, and none of the repository sidebars fits.
///
/// `RepoPickerSidebar` and `RepoSidebar` both list repositories, and a
/// session is not scoped to one: 662 distinct working directories over
/// 1,438 sessions, most of them deleted agent worktrees, and 84% of them
/// no longer on disk. Rendering a repository picker beside this list
/// would offer a column whose every row is inert -- the same reason
/// `SystemHealthPage` declines one, stated in `App.tsx`: "an empty picker
/// under a heading reads as a page that failed to load".
///
/// So it holds the `ViewSwitcher` and a heading naming what the column is
/// showing, which is exactly `DockerSidebar`'s shape and for the same
/// reason: with one destination there is nothing to navigate between, and
/// the honest element is a heading rather than a button whose only effect
/// is to set the state it is already in.
///
/// The search box is deliberately NOT here. It filters the list, so it
/// belongs above the list -- putting it in this column would separate the
/// control from the rows it acts on, and on a phone this column is a
/// sheet that closes on navigation, which would take the search with it.
export function ClaudeCodeSidebar({
  viewCounts,
}: {
  viewCounts?: Partial<Record<View, number>>;
}) {
  return (
    <nav className="flex w-64 shrink-0 flex-col border-r border-[#30363d] p-3">
      <ViewSwitcher counts={viewCounts} />
      <div className="min-h-0 flex-1 overflow-y-auto">
        {/* A HEADING, not a button, per `DockerSidebar` and
            `SystemHealthSidebar`: `aria-current="page"` is for
            navigation and `aria-pressed` for toggles, and this is
            neither -- it says where you are. */}
        <div className="flex w-full items-center gap-2 rounded px-3 py-2 text-sm text-[#e6edf3]">
          <Bot className="h-4 w-4 shrink-0" aria-hidden="true" />
          Sessions
        </div>
      </div>
    </nav>
  );
}

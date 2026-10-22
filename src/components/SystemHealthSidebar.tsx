import { Activity } from "lucide-react";
import { type View, useFilters } from "../store/filters";
import { healthPagesFor } from "./healthPages";
import { ViewSwitcher } from "./ViewSwitcher";
import { useSystemHealth } from "@/api/hooks";

/// The System Health view's own sidebar (#687).
///
/// # Why this column exists at all
///
/// The view deliberately has no repository list: #664 established that
/// it describes the MACHINE, so a repo picker beside it would be a
/// control that changes nothing. That left the column holding only the
/// view switcher. The class list is the natural occupant -- it is
/// navigation within the thing the page is about, which is exactly what
/// every other sidebar in the app holds.
///
/// # Overview is a row, not a back button
///
/// It sits at the top of the same list as the others rather than being
/// reachable only by a "back" affordance, because it is a peer: the
/// landing page, and the one that answers "is anything wrong" fastest.
/// A back button would make it feel like somewhere you leave, when it is
/// the place people arrive.
///
/// Desktop only. At phone width `App` shows no persistent sidebar, and
/// this list appears on the overview itself instead -- see
/// `SystemHealthPage`, which is where that decision is written down.
export function SystemHealthSidebar({
  viewCounts,
}: {
  viewCounts?: Partial<Record<View, number>>;
}) {
  const { healthPage, setHealthPage } = useFilters();
  // `false`: this reads the cache that `SystemHealthPage` -- always
  // mounted beside this column -- is already filling. A second enabled
  // observer here would be a second poller on the same key, and the
  // sidebar has no reason to drive a measurement of its own. Before the
  // first sample lands there is no GPU to offer, which is correct: the
  // row appears when the machine has answered, not before.
  const gpuCount = useSystemHealth(false).data?.gpus.length ?? 0;
  const pages = healthPagesFor(gpuCount);

  return (
    <nav className="flex w-64 shrink-0 flex-col border-r border-[#30363d] p-3">
      <ViewSwitcher counts={viewCounts} />
      <div className="min-h-0 flex-1 overflow-y-auto">
        {/* A heading, because these rows are a different KIND of thing
            from the switcher above them: that one changes which view you
            are in, these move within one view. Two unlabelled stacks of
            buttons would read as one list where the first entry happens
            to look different. */}
        <div className="px-3 pb-1 pt-2 text-xs font-semibold uppercase tracking-wide text-[#8b949e]">
          <Activity className="mr-1.5 inline h-3 w-3" aria-hidden="true" />
          This machine
        </div>
        <ul className="flex flex-col">
          {pages.map(({ id, label, Icon }) => (
            <li key={id}>
              <button
                type="button"
                onClick={() => setHealthPage(id)}
                // `aria-current="page"` rather than `aria-pressed`: these
                // are navigation, not toggles, and a screen reader
                // announcing "pressed" for the page you are already on
                // describes a control that did something rather than a
                // location you are at.
                aria-current={id === healthPage ? "page" : undefined}
                className={`flex w-full items-center gap-2 rounded px-3 py-2 text-sm ${
                  id === healthPage
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

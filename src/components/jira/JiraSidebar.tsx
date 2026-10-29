import type { View } from "@/store/filters";
import { ViewSwitcher } from "../ViewSwitcher";
import { JiraBrowser } from "./JiraBrowser";

/// My Jira issues' sidebar: the issue tree, under the view menu (#2038).
export function JiraSidebar({ viewCounts }: { viewCounts?: Partial<Record<View, number>> }) {
  return (
    <nav className="flex w-64 shrink-0 flex-col border-r border-[#30363d] p-3">
      <ViewSwitcher counts={viewCounts} />
      <div className="min-h-0 flex-1 overflow-y-auto">
        <JiraBrowser />
      </div>
    </nav>
  );
}

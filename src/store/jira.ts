import { create } from "zustand";

/// Which issues My Jira issues lists (#2038).
///
/// `linked`: named by one of your pull requests or worktrees. `assigned`:
/// assigned to you and not done, including work with no pull request yet.
/// `all` is both.
export type JiraFilter = "all" | "linked" | "assigned";

interface JiraViewState {
  selected: string | null;
  filter: JiraFilter;
  select: (key: string | null) => void;
  setFilter: (filter: JiraFilter) => void;
}

/// The page's selection, shared by its sidebar and main panel. Not
/// persisted: a ticket selected yesterday may be closed today.
export const useJiraView = create<JiraViewState>()((set) => ({
  selected: null,
  filter: "all",
  select: (selected) => set({ selected }),
  setFilter: (filter) => set({ filter }),
}));

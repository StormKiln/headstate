import { create } from "zustand";
import { persist } from "zustand/middleware";

export type SourceSelection = "github" | "gitlab" | "both";

type State = {
  selection: SourceSelection;
  repoKey: string | null;
  query: string;
  setSelection: (selection: SourceSelection) => void;
  setRepoKey: (repoKey: string | null) => void;
  setQuery: (query: string) => void;
};

export const useSourceSelection = create<State>()(
  persist(
    (set) => ({
      selection: "github", repoKey: null, query: "",
      setSelection: (selection) => set({ selection, repoKey: null, query: "" }),
      setRepoKey: (repoKey) => set({ repoKey }),
      setQuery: (query) => set({ query }),
    }),
    {
      name: "headstate-source-selection",
      partialize: ({ selection }) => ({ selection }),
      merge: (stored, current) => {
        const value = (stored as Partial<State> | null)?.selection;
        return { ...current, selection: value === "gitlab" || value === "both" ? value : "github" };
      },
    },
  ),
);

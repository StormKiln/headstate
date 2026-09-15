import { beforeEach, describe, expect, it } from "vitest";
import { ALL_VIEWS, PERSIST_KEY, useFilters, type View } from "./filters";
import type { Filters } from "../lib/derive";

/// The repository browser's position, and the three rules it has (#1034).
///
/// Derived from `ALL_VIEWS` rather than written out, for the reason
/// `readme.views.test.ts` gives about its own map: a hand-written second
/// list is one that gets edited to match whatever the code does and stops
/// checking anything.
const EMPTY: Record<View, Filters> = Object.fromEntries(
  ALL_VIEWS.map((v) => [v, {}]),
) as Record<View, Filters>;

beforeEach(() => {
  useFilters.setState({
    view: "repositories",
    filtersByView: { ...EMPTY },
    repoPath: "",
    repoFile: undefined,
    selectedPr: null,
  });
});

describe("the browsed path", () => {
  it("moves with setRepoPath and clears the file being read", () => {
    useFilters.getState().setRepoFile("src/main.rs");
    useFilters.getState().setRepoPath("src/components");
    expect(useFilters.getState().repoPath).toBe("src/components");
    // One `set`, so there is no order for a caller to get wrong and the
    // two values land in one render rather than two.
    expect(useFilters.getState().repoFile).toBeUndefined();
  });

  it("does not move the directory when a file is opened", () => {
    useFilters.getState().setRepoPath("src");
    useFilters.getState().setRepoFile("src/main.rs");
    expect(useFilters.getState().repoPath).toBe("src");
    expect(useFilters.getState().repoFile).toBe("src/main.rs");
  });

  /// The reset that #1034 asks for, and the one rule that has to live in
  /// the SETTER: selecting a different repository while three directories
  /// deep must not carry `src/components/` into a repository that has no
  /// such path.
  it("resets when a different repository is selected", () => {
    useFilters.getState().setRepoPath("src/components");
    useFilters.getState().setRepoFile("src/components/App.tsx");
    useFilters.getState().setFilter("repo", "/code/other");
    expect(useFilters.getState().repoPath).toBe("");
    expect(useFilters.getState().repoFile).toBeUndefined();
    // And the selection itself landed, so this is not passing because
    // `setFilter` did nothing.
    expect(useFilters.getState().filtersByView.repositories.repo).toBe("/code/other");
  });

  it("resets when the repository is cleared back to all repositories", () => {
    useFilters.getState().setRepoPath("src");
    useFilters.getState().setFilter("repo", undefined);
    expect(useFilters.getState().repoPath).toBe("");
  });

  /// Only `repo`. The other keys narrow a list rather than navigate, and
  /// throwing away the user's position on a label filter would be the
  /// same over-reach `setFilter`'s own comment refuses for `selectedPr`.
  it("survives a filter that is not a repository selection", () => {
    useFilters.getState().setRepoPath("src");
    useFilters.getState().setFilter("query", "main");
    expect(useFilters.getState().repoPath).toBe("src");
  });

  it("resets when the view changes", () => {
    useFilters.getState().setRepoPath("src/components");
    useFilters.getState().setRepoFile("src/components/App.tsx");
    useFilters.getState().setView("worktrees");
    expect(useFilters.getState().repoPath).toBe("");
    expect(useFilters.getState().repoFile).toBeUndefined();
  });
});

describe("the browsed path is deliberately not persisted", () => {
  /// The decision, asserted rather than assumed. A path restored from
  /// yesterday can point at a directory that no longer exists -- ~100
  /// sibling agent worktrees are created and destroyed continuously on
  /// this machine -- and the user did not ask to go there, so the empty
  /// listing reads as a broken repository rather than as a stale restore.
  ///
  /// Read out of `localStorage` rather than out of `partialize`, because
  /// what matters is what reaches DISK: a `partialize` that was correct
  /// and a second write path that was not would still persist it.
  it("writes neither key to storage", () => {
    useFilters.getState().setRepoPath("src/components");
    useFilters.getState().setRepoFile("src/components/App.tsx");
    const raw = localStorage.getItem(PERSIST_KEY);
    expect(raw, "the store must have persisted something at all").toBeTruthy();
    const stored = JSON.parse(raw as string) as { state: Record<string, unknown> };
    expect(Object.keys(stored.state)).not.toContain("repoPath");
    expect(Object.keys(stored.state)).not.toContain("repoFile");
    // The repository SELECTION is still persisted, because that is a
    // preference and this is a position -- so this is not passing
    // because nothing at all is stored.
    expect(Object.keys(stored.state)).toContain("filtersByView");
  });

  /// `merge` REPLACES rather than merges, so a store written before this
  /// field existed comes back without it. It gets its default for free by
  /// not being persisted -- worth asserting rather than assuming, since
  /// the black-window crash this store's comments describe came from
  /// exactly this shape.
  it("defaults to the root when a persisted store has never heard of it", () => {
    useFilters.setState({ repoPath: "src", repoFile: "src/main.rs" });
    const merged = useFilters.persist.getOptions().merge?.(
      { view: "repositories", filtersByView: { ...EMPTY } },
      useFilters.getState(),
    ) as { repoPath: string; repoFile: string | undefined };
    expect(merged.repoPath).toBe("src");
    expect(merged.repoFile).toBe("src/main.rs");
  });
});

describe("the Repositories view is registered", () => {
  /// #1023. `filtersByView` must be TOTAL over `View` -- every consumer
  /// reads `.repo` off the result of `useActiveFilters` -- so a view in
  /// the union with no bucket here is the undefined-crash `EMPTY_FILTERS`
  /// exists to prevent.
  it("has a filter bucket like every other view", () => {
    expect([...ALL_VIEWS]).toContain("repositories");
    expect(useFilters.getState().filtersByView.repositories).toEqual({});
  });
});

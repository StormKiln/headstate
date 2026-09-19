import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Artifact } from "@/types/pr";
import { stubViewport } from "@/test-utils";

const state = vi.hoisted(() => ({
  /// A previously stored scan, for the cold-start tests (#1152).
  cached: undefined as
    | { data: { path: string; size_bytes: number | null; repo_path: string }[]; ageSecs: number; stale: boolean }
    | undefined,
  /// Scan progress and unreadable roots, for the streaming tests (#1151).
  scanProgress: undefined as { roots_done: number; roots_total: number; found: number } | undefined,
  failedRoots: undefined as { root: string; why: string }[] | undefined,
  artifacts: [] as Artifact[],
  loading: false,
  // #846: a REJECTED scan, which the `= []` default made
  // indistinguishable from a clean machine.
  failed: false,
  venvsFailed: false,
  venvs: [] as unknown[],
  sizes: new Map<string, number>(),
  ages: new Map<string, number>(),
  pending: 0,
  total: 0,
  /// #956: how many size batches REJECTED. A rejection leaves `pending`
  /// and never comes back, so `pending === 0` alone reads as "fully
  /// measured".
  sizesFailed: 0,
}));

// Typed so a test can resolve with real outcomes: a bare
// `Promise.resolve([])` infers `never[]`, which rejects every fixture.
const removeFn = vi.hoisted(() =>
  vi.fn<(paths: string[]) => Promise<{ path: string; error: string | null }[]>>(() =>
    Promise.resolve([]),
  ),
);
// The explicit retry the `retry: false` on `useArtifacts` is paired with
// (#846). `useStatsBoard` states the rule: no silent retries, but only
// because the view has one the user can see.
const refetchFn = vi.hoisted(() => vi.fn());
const refetchVenvsFn = vi.hoisted(() => vi.fn());

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

vi.mock("../api/hooks", () => ({
  // Undefined by default: the first-run state, which every other
  // assertion in this file assumes (#1152). A cached result would paint
  // a dated list instead of the live one.
  useCachedScan: () => state.cached,
  // No progress and no failed roots by default: the settled state, which
  // every other assertion in this file assumes (#1151).
  useArtifactScanProgress: () => state.scanProgress,
  useArtifactScanFailures: () => state.failedRoots,
  // The page renders VenvSection, which reads this since #1151.
  useVenvWalkProgress: () => undefined,
  useRemoveArtifacts: () => removeFn,
  // The page renders VenvSection, which has its own hooks. Stubbed to
  // empty here rather than exercised: that component has its own test
  // file, and duplicating its fixtures would make both harder to change.
  useVenvs: () => ({
    data: state.venvs ?? [],
    isLoading: false,
    isError: state.venvsFailed,
    error: "venv scan refused",
    refetch: refetchVenvsFn,
  }),
  useVenvSizes: () => ({ sizes: new Map(), idle: new Map(), measuring: false }),
  useRemoveVenvs: () => vi.fn(),
  // `VenvSection` reads the staleness threshold from here since #957. `0`
  // is what every install stores and what `staleVenvDays` reads as "use
  // the default", so this section behaves exactly as it did before.
  useUiPrefs: () => ({ prefs: { stale_venv_days: 0 }, set: vi.fn() }),
  // The page renders CleanupLog on "Everything"; it has its own test
  // file, so this is stubbed empty rather than exercised here.
  useCleanupLog: () => ({ entries: [], isLoading: false, run: () => Promise.resolve([]) }),
  useArtifacts: () => ({
    data: state.artifacts,
    isLoading: state.loading,
    isError: state.failed,
    error: "scan refused: permission denied",
    refetch: refetchFn,
  }),
  useArtifactSizes: () => ({
    sizes: state.sizes,
    ages: state.ages,
    pending: state.pending,
    total: state.total,
    failed: state.sizesFailed,
  }),
}));

import { ArtifactsPage } from "./ArtifactsPage";

/// The hoisted `state` is shared across every describe in this file, and
/// only some of them reset each field. `sizesFailed` defaults to the
/// healthy value after EVERY test so a failure fixture cannot leak the
/// "sizes are incomplete" advisory into an unrelated assertion.
afterEach(() => {
  state.sizesFailed = 0;
});

const art = (over: Partial<Artifact> = {}): Artifact => ({
  path: "/code/repo/target",
  kind: "cargo_target",
  repo_path: "/code/repo",
  size_bytes: null,
  ...over,
});

describe("ArtifactsPage on a phone", () => {
  afterEach(() => stubViewport(null));

  const rowFor = (path: string) =>
    screen.getByLabelText(`Select ${path}`).closest("li") as HTMLElement;

  /// Every fact the desktop row states -- kind, path, age, size, how to
  /// rebuild -- is still stated, on two lines instead of one. A row
  /// that dropped the size to fit would hide the number the view
  /// exists to show.
  it("stacks each row so nothing is pushed off the edge", () => {
    stubViewport(390);
    state.artifacts = [art({ path: "/code/repo/target" })];
    state.sizes = new Map([["/code/repo/target", 1024]]);
    state.ages = new Map([["/code/repo/target", 86_400 * 3]]);
    render(<ArtifactsPage />);
    const row = rowFor("/code/repo/target");
    expect(row.className).toContain("flex-col");
    expect(within(row).getByText("/code/repo/target")).toBeTruthy();
    expect(within(row).getByText("1.0 KB")).toBeTruthy();
    expect(within(row).getByText("3 days ago")).toBeTruthy();
    expect(within(row).getByText("cargo build")).toBeTruthy();
    expect(within(row).getByText("target")).toBeTruthy();
    // The checkbox still selects, so the remove flow is unchanged.
    fireEvent.click(within(row).getByRole("checkbox"));
    expect(screen.getByRole("button", { name: /remove 1 /i })).toBeTruthy();
  });

  it("keeps the desktop row on one line", () => {
    stubViewport(1400);
    state.artifacts = [art({ path: "/code/repo/target" })];
    state.sizes = new Map([["/code/repo/target", 1024]]);
    render(<ArtifactsPage />);
    const row = rowFor("/code/repo/target");
    expect(row.className).not.toContain("flex-col");
    // Checkbox, kind, path, rebuild hint, age, size: siblings of one row.
    expect(within(row).getByText("1.0 KB").parentElement).toBe(row);
    expect(within(row).getByText("/code/repo/target").parentElement).toBe(row);
  });
});

describe("ArtifactsPage", () => {
  it("says so when there is nothing to show", () => {
    state.artifacts = [];
    render(<ArtifactsPage />);
    expect(screen.getByText(/No build output found/)).toBeTruthy();
  });

  /// Every row names what puts it back. "You can delete this" is only
  /// actionable next to the command that restores it, and that pairing
  /// is the whole safety argument for the view.
  it("names the command that regenerates each kind", () => {
    state.artifacts = [
      art(),
      art({ path: "/code/repo/node_modules", kind: "node_modules" }),
      art({ path: "/code/repo/.terraform", kind: "terraform" }),
    ];
    render(<ArtifactsPage />);
    expect(screen.getByText("cargo build")).toBeTruthy();
    expect(screen.getByText("npm install")).toBeTruthy();
    expect(screen.getByText("terraform init")).toBeTruthy();
  });

  /// The total is over a PARTIAL set until every batch answers, and
  /// claiming a finished number while measurement runs is the kind of
  /// quiet wrongness that makes a figure untrustworthy everywhere else.
  it("says the total is a lower bound while measuring", () => {
    state.artifacts = [art()];
    state.sizes = new Map([["/code/repo/target", 1_000_000_000]]);
    state.pending = 2;
    state.total = 5;
    render(<ArtifactsPage />);
    expect(screen.getByText(/at least/)).toBeTruthy();
    expect(screen.getByText(/measuring — 3 of 5/)).toBeTruthy();
  });

  it("drops the lower-bound wording once every batch has answered", () => {
    state.artifacts = [art()];
    state.sizes = new Map([["/code/repo/target", 1_000_000_000]]);
    state.pending = 0;
    state.total = 5;
    render(<ArtifactsPage />);
    expect(screen.queryByText(/at least/)).toBeNull();
    expect(screen.queryByText(/measuring/)).toBeNull();
  });

  /// #956. A REJECTED batch drops out of `pending` and never returns, so
  /// the old `pending > 0` test treated it as having answered: the total
  /// lost its "at least" while still being summed over a partial set, and
  /// the Remove button gained a byte figure understating what the click
  /// deletes.
  ///
  /// The qualifier must survive a failure exactly as it survives a
  /// pending batch -- and unlike a pending batch this one will not clear,
  /// so the page also says WHY the number is a floor.
  it("keeps the total qualified and off the remove button after a failed size batch", () => {
    // TWO rows: the "Remove all" button appears only for a group of more
    // than one, which is the case this figure actually labels.
    state.artifacts = [art(), art({ path: "/code/repo/node_modules", kind: "node_modules" })];
    state.sizes = new Map([["/code/repo/target", 1_000_000_000]]);
    state.ages = new Map([
      ["/code/repo/target", 60 * 60 * 24 * 30],
      ["/code/repo/node_modules", 60 * 60 * 24 * 30],
    ]);
    // Every batch has stopped fetching, and one of them rejected.
    state.pending = 0;
    state.total = 5;
    state.sizesFailed = 1;
    render(<ArtifactsPage />);

    expect(screen.getByText(/at least/)).toBeTruthy();
    // And it says what is missing, which "at least" alone cannot.
    expect(screen.getByText(/could not be measured/)).toBeTruthy();

    // The destructive button carries the count but NOT a byte figure.
    const btn = screen.getByRole("button", { name: /Remove all/ });
    expect(btn.textContent).toContain("Remove all 2");
    expect(btn.textContent).not.toMatch(/\bGB\b|\bMB\b|\bB\b·/);
    expect(btn.textContent).not.toContain("·");

    // The list is NOT blanked: the scan succeeded and the rows are real.
    expect(screen.getByText("/code/repo/target")).toBeTruthy();
  });

  /// The confirmation is the last place the figure is read before the
  /// click, so a partial measurement must say so there too (#956).
  it("states the freed bytes as a floor in the dialog when sizes are incomplete", () => {
    state.artifacts = [art(), art({ path: "/code/repo/node_modules", kind: "node_modules" })];
    state.sizes = new Map([["/code/repo/target", 1_000_000_000]]);
    state.ages = new Map([
      ["/code/repo/target", 60 * 60 * 24 * 30],
      ["/code/repo/node_modules", 60 * 60 * 24 * 30],
    ]);
    state.pending = 0;
    state.total = 5;
    state.sizesFailed = 1;
    render(<ArtifactsPage />);

    fireEvent.click(screen.getByRole("button", { name: /Remove all/ }));
    expect(screen.getByText(/This frees at least/)).toBeTruthy();
    expect(screen.getByText(/real figure is larger/)).toBeTruthy();
  });

  /// A build writing into a directory does not show up in `git status`,
  /// because build output is gitignored. The mtime is the only signal
  /// there is, so it must be surfaced rather than folded away.
  it("flags a directory that was written to recently", () => {
    state.artifacts = [art()];
    state.ages = new Map([["/code/repo/target", 60]]);
    state.sizes = new Map();
    state.pending = 0;
    render(<ArtifactsPage />);
    expect(screen.getByText("written recently")).toBeTruthy();
  });

  /// The reported gap: the list showed paths and sizes but never WHEN.
  /// Size cannot rank these -- every node_modules is ~1.4 GB -- so age
  /// is the only thing that says which are safe to remove.
  it("shows how long ago each directory was written", () => {
    state.artifacts = [art()];
    state.ages = new Map([["/code/repo/target", 60 * 60 * 24 * 270]]);
    state.sizes = new Map([["/code/repo/target", 1_400_000_000]]);
    state.pending = 0;
    render(<ArtifactsPage />);
    expect(screen.getByText("9 months ago")).toBeTruthy();
  });

  /// An unknown age must render a placeholder, never "just now" -- the
  /// same rule the size column follows for "not measured yet". Reading
  /// unknown as brand-new would hide the oldest directories.
  it("does not claim an unmeasured directory was written just now", () => {
    state.artifacts = [art()];
    state.ages = new Map();
    state.sizes = new Map();
    state.pending = 1;
    render(<ArtifactsPage />);
    expect(screen.queryByText("just now")).toBeNull();
  });

  it("can sort by age, oldest first", () => {
    const old = { ...art(), path: "/code/repo/old" };
    const fresh = { ...art(), path: "/code/repo/fresh" };
    state.artifacts = [fresh, old];
    state.ages = new Map([
      ["/code/repo/fresh", 60 * 60 * 24],
      ["/code/repo/old", 60 * 60 * 24 * 300],
    ]);
    // Equal sizes, so only the age ordering can produce a difference --
    // which is the real case this exists for.
    state.sizes = new Map([
      ["/code/repo/fresh", 1_400_000_000],
      ["/code/repo/old", 1_400_000_000],
    ]);
    state.pending = 0;
    render(<ArtifactsPage />);
    fireEvent.change(screen.getByLabelText("Sort artifacts"), {
      target: { value: "age" },
    });
    const shown = screen.getAllByText(/\/code\/repo\/(old|fresh)/);
    expect(shown[0].textContent).toContain("/code/repo/old");
  });

  it("does not flag a directory nobody has touched", () => {
    state.artifacts = [art()];
    state.ages = new Map([["/code/repo/target", 60 * 60 * 24 * 30]]);
    render(<ArtifactsPage />);
    expect(screen.queryByText("written recently")).toBeNull();
  });

  /// An unmeasured row must render a PLACEHOLDER, never "0 B".
  ///
  /// This is the half a sort test cannot catch: zero and unmeasured
  /// order identically at the bottom, so only the rendered cell
  /// distinguishes them. Showing 0 B for a directory nobody has measured
  /// is a number the user would act on -- and the 61 GB target on the
  /// machine that prompted this feature reads 0 B for its first minute.
  it("shows a placeholder, not 0 B, for an unmeasured row", () => {
    state.artifacts = [art({ path: "/code/a/target" })];
    state.sizes = new Map();
    state.ages = new Map();
    state.pending = 1;
    state.total = 1;
    const { container } = render(<ArtifactsPage />);
    expect(screen.queryByText("0 B")).toBeNull();
    expect(container.querySelector(".animate-pulse, [class*=animate-pulse]")).toBeTruthy();
  });

  /// Ordering, which is the other half. Note a zero-sort produces the
  /// SAME order as sorting unmeasured last -- zero is the minimum, so
  /// both land at the bottom. The rendered placeholder above is what
  /// actually separates the two; this pins that the largest measured
  /// directory leads, which is the view's whole point.
  it("sorts unmeasured rows last rather than as zero", () => {
    // Three rows, and the UNMEASURED one sorts alphabetically FIRST.
    // With only two rows a zero-sort produces the same order by
    // accident, so this needs a measured row on each side of it to
    // distinguish "last" from "as zero".
    state.artifacts = [
      art({ path: "/code/a-unmeasured/target" }),
      art({ path: "/code/b-small/target" }),
      art({ path: "/code/c-large/target" }),
    ];
    state.sizes = new Map([
      ["/code/b-small/target", 1_000_000],
      ["/code/c-large/target", 9_000_000_000],
    ]);
    state.ages = new Map();
    state.pending = 0;
    render(<ArtifactsPage />);

    const paths = screen
      .getAllByText(/\/code\/[abc][^/]*\/target/)
      .map((el) => el.textContent);
    expect(paths).toEqual([
      "/code/c-large/target",
      "/code/b-small/target",
      "/code/a-unmeasured/target",
    ]);
  });
});

describe("ArtifactsPage removal", () => {
  beforeEach(() => {
    removeFn.mockClear();
    state.ages = new Map();
    state.pending = 0;
  });
  /// Nothing is offered until something is chosen: a destructive action
  /// that is always on screen is one click from being an accident.
  it("offers no removal until a row is selected", () => {
    state.artifacts = [art()];
    state.sizes = new Map([["/code/repo/target", 1_000]]);
    state.ages = new Map();
    state.pending = 0;
    render(<ArtifactsPage />);
    expect(screen.queryByRole("button", { name: /^Remove/ })).toBeNull();
  });

  it("names the count and the size before the dialog", () => {
    state.artifacts = [art()];
    state.sizes = new Map([["/code/repo/target", 2_000_000_000]]);
    render(<ArtifactsPage />);
    fireEvent.click(screen.getByRole("checkbox", { name: /Select \/code\/repo\/target/ }));
    expect(screen.getByRole("button", { name: /^Remove 1 ·/ })).toBeTruthy();
  });

  /// The dialog states the loss in the terms that matter. For build
  /// output the honest answer is that the cost is TIME, which is exactly
  /// what separates this from removing a worktree.
  it("says the cost is a rebuild, not lost work", () => {
    state.artifacts = [art()];
    state.sizes = new Map([["/code/repo/target", 1_000]]);
    render(<ArtifactsPage />);
    fireEvent.click(screen.getByRole("checkbox", { name: /Select/ }));
    fireEvent.click(screen.getByRole("button", { name: /^Remove 1/ }));
    expect(screen.getByText(/not lost work/)).toBeTruthy();
  });

  /// A directory a build is writing to will be refused by the backend.
  /// Saying so BEFORE the click is the difference between a guard the
  /// user understands and one that looks like a malfunction.
  it("warns when a selected directory was written to recently", () => {
    state.artifacts = [art()];
    state.sizes = new Map([["/code/repo/target", 1_000]]);
    state.ages = new Map([["/code/repo/target", 30]]);
    render(<ArtifactsPage />);
    fireEvent.click(screen.getByRole("checkbox", { name: /Select/ }));
    fireEvent.click(screen.getByRole("button", { name: /^Remove 1/ }));
    expect(screen.getByText(/may have a build running/)).toBeTruthy();
  });

  it("removes the selected paths on confirm", () => {
    state.artifacts = [art()];
    state.sizes = new Map([["/code/repo/target", 1_000]]);
    state.ages = new Map();
    render(<ArtifactsPage />);
    fireEvent.click(screen.getByRole("checkbox", { name: /Select/ }));
    fireEvent.click(screen.getByRole("button", { name: /^Remove 1/ }));
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(removeFn).toHaveBeenCalledWith(["/code/repo/target"]);
  });

  it("removes nothing on cancel", () => {
    state.artifacts = [art()];
    state.sizes = new Map([["/code/repo/target", 1_000]]);
    render(<ArtifactsPage />);
    fireEvent.click(screen.getByRole("checkbox", { name: /Select/ }));
    fireEvent.click(screen.getByRole("button", { name: /^Remove 1/ }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(removeFn).not.toHaveBeenCalled();
  });

  /// With 178 rows an unnamed checkbox is 178 identical controls to a
  /// screen reader.
  it("names each checkbox by its path", () => {
    state.artifacts = [art({ path: "/code/a/target" }), art({ path: "/code/b/target" })];
    render(<ArtifactsPage />);
    expect(screen.getByRole("checkbox", { name: "Select /code/a/target" })).toBeTruthy();
    expect(screen.getByRole("checkbox", { name: "Select /code/b/target" })).toBeTruthy();
  });
});

describe("ArtifactsPage bulk removal and grouping", () => {
  /// The bulk button must EXCLUDE anything a build may be writing to.
  /// Those are refused at delete time anyway, so including them would
  /// only produce a failure report nobody asked for -- and the count in
  /// the label would promise more than the click delivers.
  it("leaves recently-written directories out of the bulk selection", () => {
    state.artifacts = [
      art({ path: "/code/a/target" }),
      art({ path: "/code/b/target" }),
      art({ path: "/code/busy/target" }),
    ];
    state.sizes = new Map();
    state.ages = new Map([["/code/busy/target", 30]]);
    state.pending = 0;
    render(<ArtifactsPage />);

    fireEvent.click(screen.getByRole("button", { name: /Remove all 2/ }));
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(removeFn).toHaveBeenCalledWith(["/code/a/target", "/code/b/target"]);
  });

  it("does not offer bulk removal for a single directory", () => {
    state.artifacts = [art()];
    state.ages = new Map();
    render(<ArtifactsPage />);
    expect(screen.queryByRole("button", { name: /Remove all/ })).toBeNull();
  });
});

describe("selection during removal", () => {
  /// A blanket `setChecked(new Set())` after the await discarded
  /// anything ticked while the removal was in flight -- a long window on
  /// a 100k-file node_modules, with no sign it had happened.
  it("keeps a selection made while the removal was running", async () => {
    const a = { ...art(), path: "/code/repo/a" };
    const b = { ...art(), path: "/code/repo/b" };
    state.artifacts = [a, b];
    state.sizes = new Map([
      ["/code/repo/a", 1_000],
      ["/code/repo/b", 2_000],
    ]);
    state.ages = new Map([
      ["/code/repo/a", 60 * 60 * 24 * 30],
      ["/code/repo/b", 60 * 60 * 24 * 30],
    ]);
    state.pending = 0;

    // Hold the removal open so a second selection lands mid-flight.
    let settle: (v: { path: string; error: string | null }[]) => void = () => {};
    removeFn.mockImplementationOnce(
      () => new Promise((res) => { settle = res; }),
    );

    render(<ArtifactsPage />);
    fireEvent.click(screen.getByLabelText("Select /code/repo/a"));
    fireEvent.click(screen.getByRole("button", { name: /Remove 1/ }));
    fireEvent.click(screen.getByRole("button", { name: /^Remove$/ }));

    // Mid-removal, the user ticks another row. Asserted, so a change
    // that disables checkboxes while busy fails here loudly instead of
    // making this test silently vacuous.
    expect(removeFn).toHaveBeenCalled();
    fireEvent.click(screen.getByLabelText("Select /code/repo/b"));
    expect((screen.getByLabelText("Select /code/repo/b") as HTMLInputElement).checked).toBe(true);

    // Inside `act`, so the state update the resolution triggers is
    // flushed before the assertion. Without it the post-settle render
    // had not happened and the test passed against a gutted fix.
    await act(async () => {
      settle([{ path: "/code/repo/a", error: null }]);
    });

    // Wait for a state that only exists AFTER the clear has run: `a` is
    // unticked. Waiting on `b` instead would pass instantly -- it was
    // already ticked before settling -- which is how this test survived
    // gutting the fix.
    await waitFor(() =>
      expect((screen.getByLabelText("Select /code/repo/a") as HTMLInputElement).checked).toBe(false),
    );
    // And the mid-flight selection survived it.
    expect((screen.getByLabelText("Select /code/repo/b") as HTMLInputElement).checked).toBe(true);
  });

  /// A row that FAILED to remove is the one still needing attention;
  /// unticking it makes the user find it again.
  it("keeps the selection for a row that could not be removed", async () => {
    const a = { ...art(), path: "/code/repo/a" };
    state.artifacts = [a];
    state.sizes = new Map([["/code/repo/a", 1_000]]);
    state.ages = new Map([["/code/repo/a", 60 * 60 * 24 * 30]]);
    state.pending = 0;
    removeFn.mockResolvedValueOnce([
      { path: "/code/repo/a", error: "a build is writing there" },
    ]);

    render(<ArtifactsPage />);
    fireEvent.click(screen.getByLabelText("Select /code/repo/a"));
    fireEvent.click(screen.getByRole("button", { name: /Remove 1/ }));
    fireEvent.click(screen.getByRole("button", { name: /^Remove$/ }));

    // Wait for the removal to SETTLE first. Without this the assertion
    // passes before anything happened -- the box was already ticked --
    // which is a vacuous test that survives deleting the fix.
    await waitFor(() => expect(removeFn).toHaveBeenCalled());
    await waitFor(() =>
      expect(screen.getByRole("button", { name: /Remove 1/ })).toBeTruthy(),
    );
    expect((screen.getByLabelText("Select /code/repo/a") as HTMLInputElement).checked).toBe(true);
  });

  /// #722: the date column showed a bare relative time, and the user
  /// could not tell whether it meant CREATED or LAST WRITTEN.
  ///
  /// The distinction decides the action: a directory created months ago
  /// but written to this morning is in active use; one created this
  /// morning and untouched since is not. The value is the newest mtime
  /// anywhere inside the tree, which is the more useful of the two --
  /// asserted here so a future edit cannot quietly relabel it.
  it("says the date is the last write, not the creation", () => {
    state.artifacts = [art({ path: "/code/repo/a" })];
    state.ages = new Map([["/code/repo/a", 7200]]);
    state.pending = 0;

    render(<ArtifactsPage />);
    const cell = screen.getByTitle(/Last written/i);
    expect(cell).toBeTruthy();
    expect(cell.getAttribute("title")).toMatch(/not when the directory itself was created/i);
  });

  /// "Oldest" invited exactly the wrong reading. The ordering is by last
  /// write, and the label now says so.
  it("names the sort by what it actually orders on", () => {
    state.artifacts = [art({ path: "/code/repo/a" })];
    state.pending = 0;
    render(<ArtifactsPage />);
    expect(screen.getByRole("option", { name: /Least recently written/i })).toBeTruthy();
  });
});

/// #846: a failed scan must not read as a clean machine.
///
/// `QueryError`'s own doc comment diagnoses this exact defect -- "A
/// rejected query left `data` at its `[]` default, and the empty-list
/// copy then told the user 'no pull requests match these filters' -- a
/// confident, wrong answer to a question the app could not actually
/// answer. An error has to look like an error." This page still carried
/// the pre-fix idiom.
describe("ArtifactsPage when the scan fails", () => {
  beforeEach(() => {
    refetchFn.mockClear();
    refetchVenvsFn.mockClear();
    state.artifacts = [];
    state.venvs = [];
    state.failed = false;
    state.venvsFailed = false;
    state.loading = false;
    state.sizes = new Map();
    state.ages = new Map();
    state.pending = 0;
  });
  afterEach(() => stubViewport(null));

  /// The assertion that matters is the NEGATIVE one. The page used to
  /// fall through to "No build output found in the scanned directories":
  /// a disk-cleanup tool reporting a clean machine when it could not
  /// look, with nothing red and nothing to retry -- so the failure was
  /// not merely unreported, it was actively reassuring.
  it("does not claim the scanned directories are clean", () => {
    state.failed = true;
    render(<ArtifactsPage />);
    expect(screen.queryByText(/no build output found/i)).toBeNull();
    expect(screen.getByRole("alert")).toBeTruthy();
    expect(screen.getByText(/could not scan for build output/i)).toBeTruthy();
  });

  /// The rejection's own words, which a generic "something went wrong"
  /// would throw away -- a permission-denied root is a different remedy
  /// from a missing directory.
  it("reports the scan's own refusal", () => {
    state.failed = true;
    render(<ArtifactsPage />);
    expect(screen.getByText(/permission denied/i)).toBeTruthy();
  });

  /// The pairing that makes `retry: false` acceptable. Without it the
  /// page had no retry affordance at all, and the hook's default
  /// `retry: 3` meant three silent walks of the whole code tree first.
  it("offers a retry the user can see", () => {
    state.failed = true;
    render(<ArtifactsPage />);
    fireEvent.click(screen.getByRole("button", { name: /try again/i }));
    expect(refetchFn).toHaveBeenCalled();
  });

  /// Says the total is ABSENT, not zero. "Could not scan" invites reading
  /// the last number the user saw as still true, and this page's entire
  /// claim is a total.
  it("says nothing was measured rather than letting a total stand", () => {
    state.failed = true;
    render(<ArtifactsPage />);
    expect(screen.getByText(/not a report that your directories are clean/i)).toBeTruthy();
  });

  /// The error arm has to come BEFORE the empty-state arm, because with
  /// `data = []` on a rejection the empty arm is reached first. This
  /// pins the ordering by giving the page the exact state that used to
  /// take the wrong branch: failed scan, no artifacts, no virtualenvs.
  it("prefers the error to the empty state when both would apply", () => {
    state.failed = true;
    state.artifacts = [];
    state.venvs = [];
    render(<ArtifactsPage />);
    expect(screen.queryByText(/found in the scanned directories/i)).toBeNull();
    expect(screen.getByRole("alert")).toBeTruthy();
  });

  /// The artifact failure is a PANEL, not a page (#846).
  ///
  /// An early return would take the virtualenv section with it, replacing
  /// one silent loss with another: 78 removable virtualenvs hidden behind
  /// a failure about build output. The page's own empty state reasons the
  /// same way -- "an empty artifact list beside 78 virtualenvs is not an
  /// empty page".
  it("still shows the virtualenvs a healthy scan found", () => {
    state.failed = true;
    state.venvs = [
      { path: "/cache/p-AAAAAAAA-py3.13", project: "p", state: "orphaned", source: null },
    ];
    render(<ArtifactsPage />);
    expect(screen.getByText(/could not scan for build output/i)).toBeTruthy();
    expect(screen.getByText(/poetry virtualenvs/i)).toBeTruthy();
  });

  /// A count and a total beside an error would state two things at once
  /// and let the eye take the reassuring one. A failed scan has no count,
  /// so the toolbar goes rather than printing "0 directories · 0 B".
  it("prints no count or total beside the failure", () => {
    state.failed = true;
    render(<ArtifactsPage />);
    expect(screen.queryByText(/0 directories/i)).toBeNull();
    expect(screen.queryByRole("combobox", { name: /sort artifacts/i })).toBeNull();
  });
});

/// #817's advisory-displacement bug, which survived here (#852).
///
/// The rule (`WorktreesPage:1696`): "Ordering is what actually settles it.
/// With the group after Remove, nothing upstream of Remove changes width
/// when the button appears… by construction rather than by tuning." A
/// self-scheduling, variable-width advisory sat UPSTREAM of both Remove
/// buttons here, which relied on `ml-auto` -- the remedy #817 explicitly
/// rejected -- and the desktop container has no `flex-wrap` and no
/// `overflow`, so when the advisory appeared mid-scan Remove was pushed off
/// the right edge.
///
/// Asserted with `compareDocumentPosition` rather than on the parent alone.
/// A parent-only assertion is exactly what let #817 ship green: the two
/// elements were correctly not siblings and the button still moved.
describe("ArtifactsPage toolbar ordering", () => {
  beforeEach(() => {
    state.artifacts = [];
    state.venvs = [];
    state.failed = false;
    state.venvsFailed = false;
    state.loading = false;
    state.sizes = new Map();
    state.ages = new Map();
    state.pending = 0;
    state.total = 0;
  });
  afterEach(() => stubViewport(null));

  const threeRemovable = () => {
    state.artifacts = [
      art({ path: "/code/a/target" }),
      art({ path: "/code/b/target" }),
      art({ path: "/code/c/target" }),
    ];
    state.sizes = new Map([
      ["/code/a/target", 1_000],
      ["/code/b/target", 2_000],
      ["/code/c/target", 3_000],
    ]);
  };

  it("puts the measuring advisory AFTER the remove button, not before it", () => {
    threeRemovable();
    state.pending = 2;
    state.total = 5;
    render(<ArtifactsPage />);
    const remove = screen.getByRole("button", { name: /remove all 3/i });
    const advisory = screen.getByText(/measuring — 3 of 5 repositories/i);
    expect(
      remove.compareDocumentPosition(advisory) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  /// The structural claim, stated as the thing that can actually go wrong:
  /// NO element whose presence depends on the scan may sit upstream of
  /// Remove.
  ///
  /// Counted by POSITION, not compared by text. The total beside the count
  /// legitimately rewords with `pending` ("5.9 KB" becomes "at least 5.9
  /// KB") and so does the button's own label; neither is displacement, and a
  /// text comparison would fail on both while missing the real defect.
  ///
  /// Both directions are checked in one test because #817's remedy is
  /// ORDERING, and ordering is only demonstrated by a pair: an assertion
  /// that the advisory follows Remove while the scan runs, AND that nothing
  /// new appeared ahead of Remove between the two states. Either alone
  /// passes against an arrangement that still moves the button -- which is
  /// how #817's own structural test shipped green.
  it("never puts a scan-dependent element upstream of remove", () => {
    const around = (label: RegExp) => {
      const remove = screen.getByRole("button", { name: label });
      const bar = remove.closest("div") as HTMLElement;
      const upstream = [...bar.children].filter(
        (el) => el.compareDocumentPosition(remove) & Node.DOCUMENT_POSITION_FOLLOWING,
      );
      return { remove, upstream: upstream.length };
    };

    threeRemovable();
    state.pending = 0;
    state.total = 5;
    const { unmount } = render(<ArtifactsPage />);
    const quiet = around(/remove all 3/i).upstream;
    unmount();

    state.pending = 2;
    render(<ArtifactsPage />);
    const advisory = screen.getByText(/measuring — 3 of 5/i);
    const { remove, upstream } = around(/remove all 3/i);
    // The advisory is on screen, and DOWNSTREAM of the button.
    expect(
      remove.compareDocumentPosition(advisory) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    // And nothing arrived ahead of it between the two states.
    expect(upstream).toBe(quiet);
  });

  /// `ml-auto` is GONE, which is the mechanism #817 rejected: it made each
  /// button's position a function of everything upstream of it, so an
  /// advisory consuming the free space pushed the button left.
  it("does not position the remove button with an auto margin", () => {
    threeRemovable();
    render(<ArtifactsPage />);
    const remove = screen.getByRole("button", { name: /remove all 3/i });
    expect(remove.className).not.toContain("ml-auto");
  });

  /// The two buttons differ in width a lot -- "Remove all 47 · 112 GB"
  /// against "Remove 3 · 8 GB" -- and they swap on the first checkbox tick,
  /// so ticking a row moved the button the user was about to aim at. A
  /// fixed-width cell holds the position: `WorktreesPage`'s row reached the
  /// same shape for the same reason ("A row's layout should not depend on
  /// which action it currently offers").
  it("keeps the action in one fixed cell across the button swap", () => {
    threeRemovable();
    stubViewport(1400);
    render(<ArtifactsPage />);
    const before = screen.getByRole("button", { name: /remove all 3/i });
    const cell = before.parentElement as HTMLElement;
    expect(cell.className).toMatch(/\bw-48\b/);

    fireEvent.click(screen.getByLabelText("Select /code/a/target"));
    const after = screen.getByRole("button", { name: /remove 1 ·/i });
    // The SAME cell, so the button cannot move sideways when it changes.
    expect(after.parentElement).toBe(cell);
  });

  /// #852's live-region half, on this page. `StatusBar`: "A live region has
  /// to exist before the text appears or the first announcement is missed --
  /// the one that matters most, since it is the one saying work started."
  /// The `<span>` carrying `aria-live` was created by the render that first
  /// gave it text, so the "measuring" announcement was never heard.
  it("mounts the measuring live region before there is anything to announce", () => {
    threeRemovable();
    state.pending = 0;
    state.total = 5;
    const { container } = render(<ArtifactsPage />);
    const region = container.querySelector('[aria-live="polite"]');
    expect(region).toBeTruthy();
    expect(region?.textContent).toBe("");
  });

  it("announces through that same region once measuring starts", () => {
    threeRemovable();
    state.pending = 2;
    state.total = 5;
    const { container } = render(<ArtifactsPage />);
    const regions = [...container.querySelectorAll('[aria-live="polite"]')];
    expect(regions.some((r) => /measuring — 3 of 5/.test(r.textContent ?? ""))).toBe(true);
  });
});

/// Progress and unreadable roots (#1151).
describe("ArtifactsPage while the scan runs", () => {
  beforeEach(() => {
    state.loading = true;
    state.scanProgress = undefined;
    state.failedRoots = undefined;
  });

  it("says only that it is looking before the first event", () => {
    // "0 of 0 folders" reads as a scan that found nothing to do, which
    // is a different claim from "we have not heard back yet".
    render(<ArtifactsPage />);
    expect(screen.getByText(/Looking for build output…/)).toBeTruthy();
    expect(screen.queryByText(/of 0 folders/)).toBeNull();
  });

  it("says how far along it is, with a denominator", () => {
    // #754 and #830 were both "indefinite load" reports. The number
    // that answers "is it stuck" is how many roots are left, not how
    // many directories have gone by.
    state.scanProgress = { roots_done: 3, roots_total: 4, found: 142 };
    render(<ArtifactsPage />);
    expect(screen.getByText(/3 of 4 folders, 142 found/)).toBeTruthy();
  });

  it("gets the singular right for one folder", () => {
    state.scanProgress = { roots_done: 1, roots_total: 1, found: 7 };
    render(<ArtifactsPage />);
    expect(screen.getByText(/1 of 1 folder,/)).toBeTruthy();
  });
});

describe("ArtifactsPage and roots it could not read", () => {
  beforeEach(() => {
    state.loading = false;
    state.scanProgress = undefined;
    state.failedRoots = undefined;
  });

  it("says nothing when every root was readable", () => {
    render(<ArtifactsPage />);
    expect(screen.queryByText(/could not be read/)).toBeNull();
  });

  it("names an unreadable root and why, rather than omitting it", () => {
    // THE rule (#846). A root the scan could not read contributes
    // nothing, and an empty list for a configured directory reads as an
    // answer about that directory.
    state.failedRoots = [{ root: "/Volumes/code", why: "Permission denied (os error 13)" }];
    render(<ArtifactsPage />);
    expect(screen.getByText(/1 configured folder could not be read/)).toBeTruthy();
    expect(screen.getByText(/\/Volumes\/code — Permission denied/)).toBeTruthy();
  });

  it("keeps showing the artifacts that WERE found", () => {
    // A warning about one unreadable path must not hide the real
    // results from the roots that worked -- that replaces one silent
    // loss with another.
    state.failedRoots = [{ root: "/Volumes/code", why: "Permission denied" }];
    state.artifacts = [art({ path: "/code/proj/target" })];
    render(<ArtifactsPage />);
    expect(screen.getByText(/could not be read/)).toBeTruthy();
    expect(screen.getByText(/\/code\/proj\/target/)).toBeTruthy();
  });
});

/// The cold start: a dated previous scan instead of a blank page (#1152).
describe("ArtifactsPage while the scan is running", () => {
  const cachedRows = [
    { path: "/code/proj/target", size_bytes: 10 * 1024 ** 3, repo_path: "/code/proj" },
    { path: "/code/other/target", size_bytes: 2 * 1024 ** 3, repo_path: "/code/other" },
  ];

  beforeEach(() => {
    state.loading = true;
    // No live result yet -- the state this whole describe is about.
    state.artifacts = [];
    state.cached = undefined;
  });

  it("says it is looking when nothing has ever been scanned", () => {
    // A first run has no previous result, and a cache MISS is not an
    // empty scan -- painting "no build output found" here is the #742
    // conflation.
    render(<ArtifactsPage />);
    expect(screen.getByText(/Looking for build output/)).toBeTruthy();
  });

  it("paints the previous scan instead of a blank page", () => {
    // ~1.5 s to discover and ~56 s to size, every cold start. Those
    // directories were there this morning, which is strictly more than
    // nothing.
    state.cached = { data: cachedRows, ageSecs: 45 * 60, stale: false };
    render(<ArtifactsPage />);
    expect(screen.getByText("/code/proj/target")).toBeTruthy();
    expect(screen.queryByText(/Looking for build output/)).toBeNull();
  });

  it("says how old it is, every time", () => {
    // The age is not a caveat on an exceptional path -- it is the whole
    // claim. A user who deleted 40 GB this morning and sees it listed
    // undated would reasonably conclude the deletion failed.
    state.cached = { data: cachedRows, ageSecs: 45 * 60, stale: false };
    render(<ArtifactsPage />);
    expect(screen.getByText(/Showing the last scan, from 45 minutes ago/)).toBeTruthy();
    expect(screen.getByText(/Rescanning now/)).toBeTruthy();
  });

  it("warns harder when the scan is past the staleness window", () => {
    state.cached = { data: cachedRows, ageSecs: 9 * 3600, stale: true };
    render(<ArtifactsPage />);
    expect(screen.getByText(/9 hours ago/)).toBeTruthy();
    expect(screen.getByText(/old enough that it may have changed/)).toBeTruthy();
  });

  it("offers NO removal controls on a cached result", () => {
    // THE rule `branches/cache.rs` states: a stale verdict must never
    // authorise a destructive action. Structural -- this view renders
    // no controls at all -- rather than a disabled attribute someone
    // can later re-enable.
    state.cached = { data: cachedRows, ageSecs: 60, stale: false };
    render(<ArtifactsPage />);
    expect(screen.queryByRole("button", { name: /remove|delete/i })).toBeNull();
    expect(screen.queryByRole("checkbox")).toBeNull();
  });

  it("falls back to the scanning state when the cached scan is empty", () => {
    // An empty cached list is indistinguishable from "we found
    // nothing", and painting it during a live scan would claim a result
    // the app does not have yet.
    state.cached = { data: [], ageSecs: 60, stale: false };
    render(<ArtifactsPage />);
    expect(screen.getByText(/Looking for build output/)).toBeTruthy();
  });
});

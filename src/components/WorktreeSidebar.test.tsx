import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { stubViewport } from "@/test-utils";

const repos = vi.hoisted(() => vi.fn<() => unknown>(() => []));
// #846: the scan's STATE, not just its data. On a failure `data` is
// `undefined` -- which is also what it is before the scan runs -- so the
// column could not distinguish "we have not looked" from "we looked and
// could not" from "we looked and there is nothing".
// `unreadable` since #951: the scan can come back SHORT without
// rejecting, which is a FOURTH state -- "we looked, and could not read
// all of where we looked" -- and every count in this column is then a
// floor rather than a total.
const scan = vi.hoisted(() => ({
  loading: false,
  failed: false,
  unreadable: [] as string[],
}));
const refetchFn = vi.hoisted(() => vi.fn());

vi.mock("../api/hooks", () => ({
  useWorktrees: () => ({
    data: repos(),
    isLoading: scan.loading,
    isError: scan.failed,
    refetch: refetchFn,
    unreadable: scan.unreadable,
  }),
}));
vi.mock("./ViewSwitcher", () => ({ ViewSwitcher: () => null }));

import { WorktreeSidebar } from "./WorktreeSidebar";
import { useFilters } from "../store/filters";
import { ORPHAN_FILTER } from "../lib/worktrees";

/// The complete per-view filter map, which `setState` REPLACES rather than
/// merges -- so a partial object here leaves views without a key and every
/// consumer reads `.repo` off undefined. Declared once rather than inlined at
/// each call, which is what the `beforeEach` below was already doing
/// by hand.
const EMPTY = {
  "my-prs": {},
  "to-review": {},
  worktrees: {},
  branches: {},
  docker: {},
  artifacts: {},
  packages: {},
  "claude-md": {}, "claude-code": {},
  "pr-stats": {},
  "system-health": {},
} as const;

/// One report entry, in the `<path>: <why>` shape the Rust side sends.
/// Synthetic per `CONTRIBUTING.md`: the real ones name a real machine.
const UNREADABLE = "/code/broken: fatal: not a repository";

afterEach(() => stubViewport(null));

/// The pinned Stats row is gone from this sidebar too (#794) -- same
/// reason as `DockerSidebar`: it only existed to set `view` and `panel`
/// together, and PR Stats is a view the switcher reaches. `ViewSwitcher`
/// is mocked away in this file, so these assertions see only the repo
/// rows.
///
/// Both widths stay, because the removed row was gated by VIEWPORT and
/// that is the rule #598 forbids. The two widths agreeing is the
/// assertion.
describe("WorktreeSidebar at either width", () => {
  it("shows the repositories at a phone width, with nothing pinned below", () => {
    stubViewport(390);
    repos.mockReturnValue([repo("busy", 3)]);
    render(<WorktreeSidebar />);
    expect(screen.queryByRole("button", { name: /^stats$/i })).toBeNull();
    expect(screen.getByText("busy")).toBeTruthy();
    expect(screen.getByText("All repositories")).toBeTruthy();
  });

  it("shows the same rows at the desktop width", () => {
    stubViewport(1400);
    repos.mockReturnValue([repo("busy", 3)]);
    render(<WorktreeSidebar />);
    expect(screen.queryByRole("button", { name: /^stats$/i })).toBeNull();
    expect(screen.getByText("busy")).toBeTruthy();
    expect(screen.getByText("All repositories")).toBeTruthy();
  });
});

/// `worktrees` includes the MAIN checkout, so a repo with only main has
/// nothing anyone would remove.
///
/// `is_main` is set on the first entry, which the real scanner always
/// does. The fixture omitted it before, and the count was computed as
/// `n - 1` -- so the two agreed by coincidence rather than because the
/// fixture was right. An ORPHANED repo has no main at all, which is
/// what broke that arithmetic.
const repo = (name: string, worktreeCount: number) => ({
  identity: null,
  name,
  path: `/code/${name}`,
  worktrees: Array.from({ length: worktreeCount }, (_, i) => ({
    path: `/w/${name}/${i}`,
    is_main: i === 0,
    safety: { kind: "safe" as const },
  })),
});

/// A repository record for an ORPHAN: one worktree, no main checkout.
const orphanRepo = (name: string) => ({
  identity: null,
  name,
  path: `/code/${name}`,
  worktrees: [
    { path: `/code/${name}`, is_main: false, safety: { kind: "orphaned" as const } },
  ],
});

beforeEach(() => {
  repos.mockReturnValue([]);
  // #846. Leaked either way these are confusing: a stale `failed` replaces
  // the column's head with an error, and a stale `loading` silences the
  // "No repositories found" diagnosis every other test expects.
  scan.loading = false;
  scan.failed = false;
  // Same reasoning, for the same reason (#951): a leaked `unreadable`
  // turns every count in every other test into a floor and puts a banner
  // above the rows they assert on.
  scan.unreadable = [];
  refetchFn.mockClear();
  useFilters.setState({
    filtersByView: { "my-prs": {}, "to-review": {}, worktrees: {},
  branches: {}, docker: {}, artifacts: {}, packages: {}, "claude-md": {}, "claude-code": {}, "pr-stats": {}, "system-health": {} },
    view: "worktrees",
  } as never);
});

describe("WorktreeSidebar", () => {
  it("hides repositories whose only checkout is main", () => {
    repos.mockReturnValue([repo("busy", 3), repo("empty", 1), repo("bare", 0)]);
    render(<WorktreeSidebar />);
    expect(screen.getByText("busy")).toBeTruthy();
    expect(screen.queryByText("empty")).toBeNull();
    expect(screen.queryByText("bare")).toBeNull();
  });

  /// The total is built from the same `removable` rule, so hiding rows
  /// cannot change it. A total that moved when rows were hidden would
  /// be a different number pretending to be the same one.
  it("counts the same total whether or not rows are hidden", () => {
    repos.mockReturnValue([repo("busy", 3), repo("empty", 1)]);
    render(<WorktreeSidebar />);
    // 3 worktrees minus main = 2. The empty repo contributes 0, so the
    // total matches the one visible row -- scoped to the All row, since
    // both render the same digit.
    const all = screen.getByText("All repositories").closest("button");
    expect(all?.textContent).toContain("2");
  });

  /// A blank list reads as a failed scan, which is a different and more
  /// alarming thing than "everything is tidy".
  it("says so when every repository is empty, rather than rendering blank", () => {
    repos.mockReturnValue([repo("a", 1), repo("b", 1)]);
    render(<WorktreeSidebar />);
    expect(screen.getByText(/No worktrees in any scanned repository/i)).toBeTruthy();
  });

  /// Distinct message: the scan found nothing at all, which points at
  /// configuration rather than at a tidy machine.
  it("keeps the no-repositories message distinct", () => {
    repos.mockReturnValue([]);
    render(<WorktreeSidebar />);
    expect(screen.getByText(/No repositories found/i)).toBeTruthy();
    expect(screen.queryByText(/No worktrees in any scanned/i)).toBeNull();
  });

  /// Reported: orphans were invisible in the sidebar. `n - 1` assumed
  /// every repository has a main checkout -- an orphan has one entry
  /// and no main, so it counted as zero and the row was hidden.
  describe("orphans", () => {
    it("gives them their own section rather than a repo row", () => {
      repos.mockReturnValue([repo("busy", 3), orphanRepo("veil-coh")]);
      render(<WorktreeSidebar />);
      expect(screen.getByText("Orphaned")).toBeTruthy();
      // NOT listed among the repositories: an orphan is not one.
      expect(screen.queryByText("veil-coh")).toBeNull();
    });

    it("counts them, and does not fold them into the repository total", () => {
      repos.mockReturnValue([
        repo("busy", 3),
        orphanRepo("a"),
        orphanRepo("b"),
      ]);
      render(<WorktreeSidebar />);
      const orphan = screen.getByText("Orphaned").closest("button");
      expect(orphan?.textContent).toContain("2");
    });

    /// A permanent empty heading trains the eye to skip it.
    it("is absent entirely when there are none", () => {
      repos.mockReturnValue([repo("busy", 3)]);
      render(<WorktreeSidebar />);
      expect(screen.queryByText("Orphaned")).toBeNull();
    });

    /// The reported symptom behind the 120-vs-123 mismatch: the
    /// sidebar's `n - 1` undercounted by exactly the number of
    /// repositories with no main checkout.
    it("counts a repository by what is not its main checkout", () => {
      repos.mockReturnValue([repo("busy", 3), orphanRepo("orphan")]);
      render(<WorktreeSidebar />);
      const all = screen.getByText("All repositories").closest("button");
      // 2 removable in `busy`, plus the 1 orphan = 3.
      expect(all?.textContent).toContain("3");
    });

    /// #951: an orphan count over an incomplete walk.
    ///
    /// QUALIFIED, not suppressed. Each orphan is a POSITIVE finding --
    /// `orphan_gitdir` reads the worktree's own `.git` file and checks
    /// the repository it names is gone, consulting nothing else -- so a
    /// short walk cannot invent an orphan, only miss one. The error has
    /// one direction, and "at least" is the honest word for it. Hiding
    /// the section instead would hide orphans that are real and on disk,
    /// which is the entire reason this page exists.
    it("says at least, rather than an exact count, on a partial scan", () => {
      repos.mockReturnValue([orphanRepo("a"), orphanRepo("b")]);
      scan.unreadable = [UNREADABLE];
      render(<WorktreeSidebar />);
      const orphan = screen.getByText("Orphaned").closest("button");
      // The count SURVIVES -- the two orphans are real -- and is marked
      // as a floor.
      expect(orphan?.textContent).toContain("2");
      expect(orphan?.textContent).toContain("\u2265");
    });

    /// And an exact count when the walk finished, or the qualifier is
    /// permanent furniture and stops meaning anything.
    it("gives an exact count when the scan read everything", () => {
      repos.mockReturnValue([orphanRepo("a"), orphanRepo("b")]);
      render(<WorktreeSidebar />);
      const orphan = screen.getByText("Orphaned").closest("button");
      expect(orphan?.textContent).toContain("2");
      expect(orphan?.textContent).not.toContain("\u2265");
    });

    /// The shortfall is REPORTED, with the reason, above the counts it
    /// qualifies -- not left in a log nobody reads, which is the
    /// argument `caches/mod.rs` makes about `truncated`.
    it("names what it could not read", () => {
      repos.mockReturnValue([orphanRepo("a")]);
      scan.unreadable = [UNREADABLE];
      render(<WorktreeSidebar />);
      expect(screen.getByText(UNREADABLE)).toBeTruthy();
      expect(screen.getByText(/floors rather than totals/i)).toBeTruthy();
    });
  });

  /// #846: this column had ZERO loading or error handling.
  ///
  /// On a failed `useWorktrees`, `repos` is `undefined`, so both guards at
  /// the bottom of the list fail (`repos !== undefined && …` and
  /// `repos?.length === 0`), `allCount` reduces over `[]`, and the column
  /// rendered exactly one thing: "All repositories  0". A failure as a
  /// confident zero, in the one place the user looks to ask where the disk
  /// went. `RepoPickerSidebar` -- the plain list this is a decorated
  /// version of -- has always distinguished the two.
  describe("when the scan fails", () => {
    /// The defect in one assertion: the count was a confident zero.
    ///
    /// An em dash, never a number, which is the rule the size cells on
    /// the worktree page already follow -- zero is an ANSWER and a failed
    /// scan has none.
    it("shows no count rather than a confident zero", () => {
      scan.failed = true;
      repos.mockReturnValue(undefined);
      render(<WorktreeSidebar />);
      const all = screen.getByText("All repositories").closest("button");
      expect(all?.textContent).toContain("—");
      expect(all?.textContent).not.toContain("0");
    });

    it("says the scan failed, and offers a retry", () => {
      scan.failed = true;
      repos.mockReturnValue(undefined);
      render(<WorktreeSidebar />);
      expect(screen.getByText(/could not scan for worktrees/i)).toBeTruthy();
      fireEvent.click(screen.getByRole("button", { name: /try again/i }));
      expect(refetchFn).toHaveBeenCalled();
    });

    /// "No repositories found. Check the scanned directories in Settings"
    /// is a DIAGNOSIS -- it points at the user's configuration. On a
    /// failure it would send someone to fix something that is not broken,
    /// which is the rule `RepoPickerSidebar` states. The `repos?.length
    /// === 0` chain is false on `undefined`, so this was already right;
    /// the test pins it against a future change to that guard.
    it("does not send the user to Settings over a failure", () => {
      scan.failed = true;
      repos.mockReturnValue(undefined);
      render(<WorktreeSidebar />);
      expect(screen.queryByText(/check the scanned directories/i)).toBeNull();
    });
  });

  /// #852: four of seven sidebars conveyed selection by BACKGROUND COLOUR
  /// ALONE, with zero `aria-current`.
  ///
  /// `StatsSidebar` already states the rule: "`aria-current` rather than
  /// only a colour: the selection is navigation state, and a screen reader
  /// reading a list of repository names has no other way to know which one
  /// is open."
  describe("selection is not conveyed by colour alone", () => {
    it("marks the selected repository as current", () => {
      repos.mockReturnValue([repo("busy", 3), repo("other", 2)]);
      useFilters.setState({
        filtersByView: { ...EMPTY, worktrees: { repo: "/code/busy" } },
        view: "worktrees",
      } as never);
      render(<WorktreeSidebar />);
      expect(
        screen.getByText("busy").closest("button")?.getAttribute("aria-current"),
      ).toBe("true");
      // And only that one. A second `aria-current` would announce two
      // locations at once.
      expect(
        screen.getByText("other").closest("button")?.getAttribute("aria-current"),
      ).toBeNull();
    });

    it("marks All repositories as current when nothing is scoped", () => {
      repos.mockReturnValue([repo("busy", 3)]);
      render(<WorktreeSidebar />);
      expect(
        screen.getByText("All repositories").closest("button")?.getAttribute("aria-current"),
      ).toBe("true");
    });

    /// The Orphaned row most of all: its only other distinguishing mark is
    /// amber TEXT, so a reader who cannot see colour had nothing at all.
    it("marks the Orphaned row as current when it is selected", () => {
      repos.mockReturnValue([repo("busy", 3), orphanRepo("veil-coh")]);
      useFilters.setState({
        filtersByView: { ...EMPTY, worktrees: { repo: ORPHAN_FILTER } },
        view: "worktrees",
      } as never);
      render(<WorktreeSidebar />);
      expect(
        screen.getByText("Orphaned").closest("button")?.getAttribute("aria-current"),
      ).toBe("true");
    });

    /// `undefined`, never `"false"`. Absence is how "not current" is
    /// spelled, and `aria-current="false"` is announced by some readers --
    /// so a row that is merely not selected would say so out loud.
    it("omits the attribute on unselected rows rather than setting it false", () => {
      repos.mockReturnValue([repo("busy", 3)]);
      render(<WorktreeSidebar />);
      const rows = screen.getAllByRole("button");
      expect(rows.some((r) => r.getAttribute("aria-current") === "false")).toBe(false);
    });
  });

  /// A holding message, not a diagnosis -- the other half of the same rule.
  describe("while the scan is running", () => {
    it("says it is looking rather than naming a number or a fault", () => {
      scan.loading = true;
      repos.mockReturnValue(undefined);
      render(<WorktreeSidebar />);
      expect(screen.getByText(/looking for repositories/i)).toBeTruthy();
      expect(screen.queryByText(/no repositories found/i)).toBeNull();
      const all = screen.getByText("All repositories").closest("button");
      expect(all?.textContent).toContain("—");
    });
  });
});

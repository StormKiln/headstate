import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import type { Upstream, Worktree, WorktreeRepo } from "@/types/pr";

/// The hook is mocked rather than the Tauri boundary, because the
/// property under test is what the TABLE does with a scan -- including
/// the `unreadable` half, which only the hook's wrapper shape carries.
const useWorktrees = vi.hoisted(() => vi.fn());
/// The verdicts, which the SCAN does not carry (#1042). Mocked
/// separately from `useWorktrees` because they arrive separately: the
/// walk lists repositories and never classifies them, and this hook is
/// the second pass that fills the Status column one repository at a
/// time. Keeping them as two mocks is what lets a test put a row in the
/// scan and leave its verdict outstanding, which is the state the whole
/// column was stuck in.
const useRepoUpstreams = vi.hoisted(() => vi.fn());
/// `UpdateAllButton` mounts inside this table (#1012) and reads three
/// more hooks from the same module, so the mock must carry them or the
/// whole table fails to render. Stubbed rather than given behaviour: the
/// button's own conduct is `UpdateAllButton.test.tsx`'s subject, and
/// these tests are about what the TABLE does with a scan.
vi.mock("@/api/hooks", () => ({
  useWorktrees,
  useRepoUpstreams,
  useUpdateAllRepositories: () => vi.fn(),
  useCancelUpdateAll: () => vi.fn(),
  useUpdateAllProgress: () => null,
}));

import { AllRepositoriesTable } from "./AllRepositoriesTable";

const wt = (over: Partial<Worktree> = {}): Worktree =>
  ({
    path: "/code/a",
    branch: "main",
    head: "abc",
    size_bytes: null,
    safety: { kind: "main_checkout" },
    is_main: true,
    merged_at: null,
    upstream: { kind: "current" } as Upstream,
    last_commit: null,
    ...over,
  }) as Worktree;

const repo = (over: Partial<WorktreeRepo> = {}): WorktreeRepo => ({
  identity: null,
  name: "a",
  path: "/code/a",
  worktrees: [wt()],
  fetched_at: null,
  default_ref: "origin/main",
  ...over,
});

/// A fetch inside the freshness threshold, so an unqualified verdict is
/// the honest one and the age note is correctly absent.
const justNow = () => new Date(Date.now() - 60_000).toISOString();

const scan = (over: Record<string, unknown> = {}) => ({
  data: [repo()],
  unreadable: [] as readonly string[],
  isLoading: false,
  isError: false,
  error: null,
  refetch: vi.fn(),
  ...over,
});

/// What `useRepoUpstreams` returns for a given path-to-verdict map.
const verdicts = (entries: Record<string, Upstream> = {}) => ({
  upstreams: new Map<string, Upstream>(Object.entries(entries)),
  pending: 0,
  failed: 0,
  total: Object.keys(entries).length,
});

beforeEach(() => {
  useWorktrees.mockReset();
  // No verdicts unless a test supplies them, so a test that does not
  // care about the Status column gets the honest default: the scan
  // listed the repository and its verdict has not landed.
  useRepoUpstreams.mockReset();
  useRepoUpstreams.mockReturnValue(verdicts());
});

describe("AllRepositoriesTable", () => {
  /// #1015: the safety verdict on the main checkout is a CONSTANT --
  /// `classify` returns `Safety::MainCheckout` before it looks at `git
  /// status` -- so a column sourced from it would print one sentence on
  /// every row. The table must not carry it.
  it("does not render the main checkout's constant safety verdict", () => {
    useWorktrees.mockReturnValue(scan());
    render(<AllRepositoriesTable />);
    expect(screen.queryByText(/the repository's main checkout/i)).toBeNull();
  });

  /// #1021: the verdict is qualified by the age of the refs behind it.
  /// A year-old fetch says so, beside the verdict, in grey.
  it("qualifies an up-to-date verdict with the age of the refs", () => {
    const old = new Date(Date.now() - 400 * 24 * 3600_000).toISOString();
    useWorktrees.mockReturnValue(scan({ data: [repo({ fetched_at: old })] }));
    render(<AllRepositoriesTable />);
    const cell = screen.getByText(/up to date with upstream/);
    expect(cell.textContent).toMatch(/as of \d+d ago/);
    // Grey, not green: an unverified up-to-date is not a verified one.
    expect(cell.className).toContain("#8b949e");
  });

  /// The never-fetched case, which `refAgeShort` deliberately does not
  /// abbreviate.
  it("says never fetched rather than inventing an age", () => {
    useWorktrees.mockReturnValue(scan({ data: [repo({ fetched_at: null })] }));
    render(<AllRepositoriesTable />);
    expect(screen.getByText(/never fetched/)).toBeTruthy();
  });

  /// Paired with the two above: fresh refs make "up to date" simply
  /// true, and the table adds no hedge it has not earned.
  it("adds no age caveat when the refs are fresh", () => {
    useWorktrees.mockReturnValue(scan({ data: [repo({ fetched_at: justNow() })] }));
    render(<AllRepositoriesTable />);
    const cell = screen.getByText(/up to date with upstream/);
    expect(cell.textContent).not.toMatch(/as of/);
    expect(cell.textContent).not.toMatch(/never fetched/);
    // Green, because this one was actually verified.
    expect(cell.className).toContain("#3fb950");
  });

  /// #1027, THE sabotage-proved test. `Untracked` is not "up to date",
  /// and rendering it as such is this codebase's characteristic
  /// absent-read-as-success bug.
  it("renders an untracked repository as local only, never as up to date", () => {
    useWorktrees.mockReturnValue(
      scan({ data: [repo({ worktrees: [wt({ upstream: { kind: "untracked" } })] })] }),
    );
    render(<AllRepositoriesTable />);
    expect(screen.getByText(/no upstream — local only/)).toBeTruthy();
    expect(screen.queryByText(/up to date with upstream/)).toBeNull();
  });

  /// #1027: `null` is "not computed yet". Rendering it as a verdict
  /// would claim every repository is current before anything was
  /// measured.
  it("renders a pending verdict as a skeleton rather than a claim", () => {
    useWorktrees.mockReturnValue(
      scan({ data: [repo({ worktrees: [wt({ upstream: null })] })] }),
    );
    render(<AllRepositoriesTable />);
    expect(screen.getByText("Checking")).toBeTruthy();
    expect(screen.queryByText(/up to date with upstream/)).toBeNull();
  });

  /// #1027: the ratio states its denominator, and the rows that were
  /// never compared are reported separately rather than folded in.
  it("states the denominator and reports the uncompared rows separately", () => {
    useWorktrees.mockReturnValue(
      scan({
        data: [
          repo({ name: "a", path: "/a" }),
          repo({ name: "b", path: "/b", worktrees: [wt({ upstream: { kind: "untracked" } })] }),
        ],
      }),
    );
    render(<AllRepositoriesTable />);
    expect(screen.getByText(/1 of 1 repositories with an upstream are up to date/)).toBeTruthy();
    expect(screen.getByText(/1 repository has no upstream to compare against/)).toBeTruthy();
  });

  /// Paired with the test above: with nothing uncompared, the summary
  /// adds no caveat. A shortfall notice that always fires is noise.
  it("adds no uncompared caveat when every repository was compared", () => {
    useWorktrees.mockReturnValue(scan());
    render(<AllRepositoriesTable />);
    expect(screen.queryByText(/no upstream to compare against/)).toBeNull();
  });

  /// #1026: the ref compared against is the repository's own.
  it("names the repository's own default ref", () => {
    useWorktrees.mockReturnValue(scan({ data: [repo({ default_ref: "origin/master" })] }));
    render(<AllRepositoriesTable />);
    expect(screen.getByText("origin/master")).toBeTruthy();
    expect(screen.queryByText("origin/main")).toBeNull();
  });

  /// #1026: an unresolved ref says so. It does not become `origin/main`.
  it("says a default ref could not be resolved rather than guessing", () => {
    useWorktrees.mockReturnValue(scan({ data: [repo({ default_ref: null })] }));
    render(<AllRepositoriesTable />);
    expect(screen.getByText(/could not resolve/)).toBeTruthy();
    expect(screen.queryByText("origin/main")).toBeNull();
  });

  /// #1028: the table is a census, so a short one must say so.
  it("reports what the scan could not read", () => {
    useWorktrees.mockReturnValue(
      scan({ unreadable: ["/code/locked: permission denied"] }),
    );
    render(<AllRepositoriesTable />);
    expect(screen.getByText(/1 path could not be read/)).toBeTruthy();
    expect(screen.getByText("/code/locked: permission denied")).toBeTruthy();
  });

  /// #1028 / #951: no retry. A second identical walk cannot read what
  /// the first could not.
  it("offers no retry on a partial scan", () => {
    useWorktrees.mockReturnValue(scan({ unreadable: ["/code/locked: denied"] }));
    render(<AllRepositoriesTable />);
    expect(screen.queryByRole("button", { name: /try again/i })).toBeNull();
  });

  /// Paired with the two above: a complete scan renders no notice.
  it("renders no partial-scan notice when everything was read", () => {
    useWorktrees.mockReturnValue(scan());
    render(<AllRepositoriesTable />);
    expect(screen.queryByText(/could not be read/)).toBeNull();
  });

  /// #846: the error arm is reached BEFORE the empty arm. With `data`
  /// undefined on a rejection the row list is empty, so an empty-state
  /// check placed first would claim the scan found nothing.
  it("renders the failure, not an empty state, when the scan rejects", () => {
    useWorktrees.mockReturnValue(
      scan({ data: undefined, isError: true, error: new Error("git not found") }),
    );
    render(<AllRepositoriesTable />);
    expect(screen.getByRole("alert").textContent).toMatch(/Could not scan for repositories/);
    expect(screen.queryByText(/No repositories found in the scanned folders/)).toBeNull();
  });

  it("renders the empty state only when the scan genuinely found nothing", () => {
    useWorktrees.mockReturnValue(scan({ data: [] }));
    render(<AllRepositoriesTable />);
    expect(screen.getByText(/No repositories found in the scanned folders/)).toBeTruthy();
  });

  /// #1042: the Status column, against the data production actually
  /// delivers.
  ///
  /// The tests above drive the column from `Worktree.upstream` on the
  /// scanned row, which is what this table was written believing it
  /// would get. It never did: that field is written only by the Rust
  /// `classify`, which the walk runs behind a flag every production
  /// caller passes `false` for. Those tests still earn their place --
  /// they pin what each VERDICT renders as -- but none of them could
  /// fail while the column was permanently blank, because all of them
  /// hand it a verdict the scan does not carry.
  ///
  /// So these start from the honest scan: `upstream: null` on every row,
  /// exactly as `list_worktrees` returns it, and the verdicts arriving
  /// separately through `useRepoUpstreams`.
  describe("the Status column, from the production shape", () => {
    /// A repository as the production scan really hands it over: listed,
    /// unclassified.
    const unclassified = (over: Partial<WorktreeRepo> = {}) =>
      repo({ worktrees: [wt({ upstream: null })], ...over });

    /// THE regression test. With a real scan and a landed verdict, the
    /// cell must show the verdict -- not the skeleton it showed forever.
    it("renders the verdict that arrives after the scan, not a skeleton", () => {
      useWorktrees.mockReturnValue(scan({ data: [unclassified()] }));
      useRepoUpstreams.mockReturnValue(
        verdicts({ "/code/a": { kind: "current" } as Upstream }),
      );
      render(<AllRepositoriesTable />);
      expect(screen.getByText(/up to date with upstream/)).toBeTruthy();
      expect(screen.queryByText("Checking")).toBeNull();
    });

    /// The skeleton is still correct while the verdict is genuinely
    /// outstanding -- for exactly that long, and no longer. Without this
    /// the fix could be "render something, anything", which is the
    /// inverse defect: a claim made before anything was measured.
    it("still shows a skeleton while a verdict is genuinely outstanding", () => {
      useWorktrees.mockReturnValue(scan({ data: [unclassified()] }));
      useRepoUpstreams.mockReturnValue(verdicts());
      render(<AllRepositoriesTable />);
      expect(screen.getByText("Checking")).toBeTruthy();
      expect(screen.queryByText(/up to date with upstream/)).toBeNull();
    });

    /// #1042's hard requirement: a repository whose classification FAILED
    /// must render its failure, never keep promising. `Upstream::Unknown`
    /// is the state for "we asked and could not answer", and the hook
    /// carries the refusal's own words into it.
    it("renders a failed classification as a failure, not as a pending row", () => {
      useWorktrees.mockReturnValue(scan({ data: [unclassified()] }));
      useRepoUpstreams.mockReturnValue(
        verdicts({
          "/code/a": { kind: "unknown", n: "could not list worktrees: git exploded" } as Upstream,
        }),
      );
      render(<AllRepositoriesTable />);
      expect(screen.queryByText("Checking")).toBeNull();
      // The refusal's OWN words, carried through rather than replaced
      // with a sentence of the UI's: "could not list worktrees" is what
      // the user needs in order to know which half failed.
      expect(screen.getByText(/upstream unknown: could not list worktrees/)).toBeTruthy();
    });

    /// Rows resolve INDEPENDENTLY. One repository whose verdict has not
    /// landed must not hold back a sibling whose has -- which is the
    /// per-repository granularity the hook exists for, observable from
    /// the outside only here.
    it("fills a resolved row while a sibling is still outstanding", () => {
      useWorktrees.mockReturnValue(
        scan({
          data: [
            unclassified({ name: "a", path: "/a" }),
            unclassified({ name: "b", path: "/b" }),
          ],
        }),
      );
      useRepoUpstreams.mockReturnValue(
        verdicts({ "/a": { kind: "behind", n: 3 } as Upstream }),
      );
      render(<AllRepositoriesTable />);
      expect(screen.getByText(/3 commits behind/)).toBeTruthy();
      // And exactly one row is still waiting, not both and not none.
      expect(screen.getAllByText("Checking")).toHaveLength(1);
    });

    /// The verdicts are keyed by PATH, which is the row's identity.
    /// `name` is not unique across scan roots -- two roots may each hold
    /// an `api` -- so a verdict keyed by name would land on the wrong
    /// row, which is the most damaging thing this column could do.
    it("matches verdicts to rows by path, not by name", () => {
      useWorktrees.mockReturnValue(
        scan({
          data: [
            unclassified({ name: "api", path: "/one/api" }),
            unclassified({ name: "api", path: "/two/api" }),
          ],
        }),
      );
      useRepoUpstreams.mockReturnValue(
        verdicts({ "/two/api": { kind: "untracked" } as Upstream }),
      );
      render(<AllRepositoriesTable />);
      expect(screen.getByText(/no upstream — local only/)).toBeTruthy();
      expect(screen.getAllByText("Checking")).toHaveLength(1);
    });

    /// The summary counts the LANDED verdicts, not the scan's blanks.
    /// Before the fix this read "0 of 0", a ratio over a population that
    /// had never been measured, and it would stay there forever.
    it("counts the landed verdicts in the currency summary", () => {
      useWorktrees.mockReturnValue(
        scan({
          data: [
            unclassified({ name: "a", path: "/a" }),
            unclassified({ name: "b", path: "/b" }),
          ],
        }),
      );
      useRepoUpstreams.mockReturnValue(
        verdicts({
          "/a": { kind: "current" } as Upstream,
          "/b": { kind: "untracked" } as Upstream,
        }),
      );
      render(<AllRepositoriesTable />);
      expect(
        screen.getByText(/1 of 1 repositories with an upstream are up to date/),
      ).toBeTruthy();
      expect(screen.getByText(/1 repository has no upstream to compare against/)).toBeTruthy();
    });

    /// The verdicts are asked for BY PATH, one query per repository. Not
    /// a single call for everything, which is what would let one slow
    /// repository hold the whole table.
    it("asks for a verdict for every scanned repository, by path", () => {
      useWorktrees.mockReturnValue(
        scan({
          data: [
            unclassified({ name: "a", path: "/a" }),
            unclassified({ name: "b", path: "/b" }),
          ],
        }),
      );
      render(<AllRepositoriesTable />);
      expect(useRepoUpstreams).toHaveBeenCalledWith(["/a", "/b"]);
    });
  });
});

import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import type { Upstream, Worktree, WorktreeRepo } from "@/types/pr";

/// The hook is mocked rather than the Tauri boundary, because the
/// property under test is what the TABLE does with a scan -- including
/// the `unreadable` half, which only the hook's wrapper shape carries.
const useWorktrees = vi.hoisted(() => vi.fn());
vi.mock("@/api/hooks", () => ({ useWorktrees }));

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

beforeEach(() => useWorktrees.mockReset());

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
});

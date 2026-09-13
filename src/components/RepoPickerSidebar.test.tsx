import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

/// #852: this column conveyed selection by BACKGROUND COLOUR ALONE, with
/// zero `aria-current` -- and had no test file at all, which is how.
///
/// `StatsSidebar` already states the rule: "`aria-current` rather than only a
/// colour: the selection is navigation state, and a screen reader reading a
/// list of repository names has no other way to know which one is open."
const repos = vi.hoisted(() => vi.fn<() => unknown>(() => []));
const scan = vi.hoisted(() => ({ loading: false, unreadable: [] as string[] }));
const selected = vi.hoisted(() => ({ repo: undefined as string | undefined }));

vi.mock("@/api/hooks", () => ({
  useWorktrees: () => ({
    data: repos(),
    isLoading: scan.loading,
    unreadable: scan.unreadable,
  }),
}));
vi.mock("./ViewSwitcher", () => ({ ViewSwitcher: () => null }));
vi.mock("@/store/filters", () => ({
  useActiveFilters: () => ({ repo: selected.repo }),
  useFilters: () => ({ setFilter: vi.fn() }),
}));

import { RepoPickerSidebar } from "./RepoPickerSidebar";

/// One report entry, in the `<path>: <why>` shape the Rust side sends.
/// Synthetic per `CONTRIBUTING.md`: the real ones name a real machine.
const UNREADABLE = "/code/broken: fatal: not a repository";

const repo = (name: string) => ({
  identity: null,
  name,
  path: `/code/${name}`,
  worktrees: [],
});

beforeEach(() => {
  repos.mockReturnValue([]);
  scan.loading = false;
  scan.unreadable = [];
  selected.repo = undefined;
});

describe("RepoPickerSidebar", () => {
  it("lists every scanned repository", () => {
    repos.mockReturnValue([repo("alpha"), repo("beta")]);
    render(<RepoPickerSidebar reviewingCount={0} />);
    expect(screen.getByText("alpha")).toBeTruthy();
    expect(screen.getByText("beta")).toBeTruthy();
  });

  /// The rule this component's own comment documents: "'No repositories
  /// found in the scanned folders' is a DIAGNOSIS, not a holding message…
  /// sends someone to fix something that is not broken." Pinned here because
  /// nothing was pinning it.
  it("says it is looking rather than diagnosing, before the scan answers", () => {
    scan.loading = true;
    render(<RepoPickerSidebar reviewingCount={0} />);
    expect(screen.getByText(/looking for repositories/i)).toBeTruthy();
    expect(screen.queryByText(/no repositories found/i)).toBeNull();
  });

  it("gives the diagnosis once the scan has answered with nothing", () => {
    repos.mockReturnValue([]);
    render(<RepoPickerSidebar reviewingCount={0} />);
    expect(screen.getByText(/no repositories found in the scanned folders/i)).toBeTruthy();
  });

  /// #951, and the second proof that issue asks for.
  ///
  /// The copy this asserts ABSENT is the one `emptyStateGuard.test.ts`
  /// names the worst in the app: "No repositories found in the scanned
  /// folders" is a diagnosis pointing at the user's settings, so a scan
  /// that could not READ those folders sent someone to fix a
  /// configuration that was never wrong. Before the fix the walk had
  /// nowhere to report the shortfall and this copy rendered regardless.
  describe("a scan that could not read everything (#951)", () => {
    it("does not blame the scanned folders when it could not read them", () => {
      repos.mockReturnValue([]);
      scan.unreadable = [UNREADABLE];
      render(<RepoPickerSidebar reviewingCount={0} />);
      expect(screen.queryByText(/no repositories found in the scanned folders/i)).toBeNull();
      expect(screen.getByText(/no repositories could be read/i)).toBeTruthy();
    });

    /// The reason, not just the fact. A count alone is unactionable: a
    /// refusal, a permission wall and a missing binary send the user to
    /// three different places, and only the message distinguishes them.
    it("names the path and the reason", () => {
      repos.mockReturnValue([]);
      scan.unreadable = [UNREADABLE];
      render(<RepoPickerSidebar reviewingCount={0} />);
      expect(screen.getByText(UNREADABLE)).toBeTruthy();
    });

    /// The repositories that DID read stay listed -- the trade
    /// `ArtifactsPage` states. A fix that blanked the list on one
    /// unreadable directory would trade a silent failure for a louder
    /// one.
    it("still lists what it did read, and says the list may be short", () => {
      repos.mockReturnValue([repo("alpha")]);
      scan.unreadable = [UNREADABLE];
      render(<RepoPickerSidebar reviewingCount={0} />);
      expect(screen.getByText("alpha")).toBeTruthy();
      expect(screen.getByText(/may not be all of them/i)).toBeTruthy();
    });

    /// And silent when there is nothing to report, or the banner becomes
    /// permanent furniture nobody reads.
    it("says nothing when the scan read everything", () => {
      repos.mockReturnValue([repo("alpha")]);
      render(<RepoPickerSidebar reviewingCount={0} />);
      expect(screen.queryByText(/could not be read/i)).toBeNull();
    });

    /// Before the scan answers there is no shortfall to report either --
    /// "we have not looked yet" must not read as "we could not look".
    it("says nothing while the scan is still running", () => {
      scan.loading = true;
      scan.unreadable = [UNREADABLE];
      render(<RepoPickerSidebar reviewingCount={0} />);
      expect(screen.getByText(/looking for repositories/i)).toBeTruthy();
      expect(screen.queryByText(/could not be read/i)).toBeNull();
    });
  });

  describe("selection is not conveyed by colour alone", () => {
    it("marks the selected repository as current", () => {
      repos.mockReturnValue([repo("alpha"), repo("beta")]);
      selected.repo = "/code/alpha";
      render(<RepoPickerSidebar reviewingCount={0} />);
      expect(
        screen.getByText("alpha").closest("button")?.getAttribute("aria-current"),
      ).toBe("true");
      // And only that one: two `aria-current` rows would announce two
      // locations at once.
      expect(
        screen.getByText("beta").closest("button")?.getAttribute("aria-current"),
      ).toBeNull();
    });

    /// `undefined`, never `"false"`: absence is how "not current" is
    /// spelled, and `aria-current="false"` is announced by some readers, so
    /// an unselected row would say so out loud.
    it("omits the attribute on unselected rows rather than setting it false", () => {
      repos.mockReturnValue([repo("alpha"), repo("beta")]);
      render(<RepoPickerSidebar reviewingCount={0} />);
      const rows = screen.getAllByRole("button");
      expect(rows.some((r) => r.getAttribute("aria-current") === "false")).toBe(false);
      // With nothing scoped, no row claims to be the current one -- this
      // column has no "All repositories" entry to fall back to.
      expect(rows.some((r) => r.getAttribute("aria-current") !== null)).toBe(false);
    });
  });
});

import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

/// #852: this column conveyed selection by BACKGROUND COLOUR ALONE, with
/// zero `aria-current` -- and had no test file at all, which is how.
///
/// `StatsSidebar` already states the rule: "`aria-current` rather than only a
/// colour: the selection is navigation state, and a screen reader reading a
/// list of repository names has no other way to know which one is open."
const repos = vi.hoisted(() => vi.fn<() => unknown>(() => []));
/// `dirs` since #952: the scan's INPUT. Every other field here is about
/// what the walk FOUND, and none of them can distinguish a walk that
/// looked in three folders and found nothing from a walk that had no
/// folders to look in -- which is the state a first-run machine without
/// `~/code` is actually in.
///
/// Defaults to one configured path, so the assertions written before
/// #952 keep reading the "looked and found nothing" arm they were
/// written for rather than silently moving to the new one.
const scan = vi.hoisted(() => ({
  loading: false,
  unreadable: [] as string[],
  dirs: ["/code"] as string[],
}));
const selected = vi.hoisted(() => ({ repo: undefined as string | undefined }));

vi.mock("@/api/hooks", () => ({
  useWorktrees: () => ({
    data: repos(),
    isLoading: scan.loading,
    unreadable: scan.unreadable,
  }),
  useWorktreeDirs: () => ({ dirs: scan.dirs, set: vi.fn() }),
}));
vi.mock("./ViewSwitcher", () => ({ ViewSwitcher: () => null }));
/// Stubbed to its identity, not rendered: the real dialog subscribes to
/// every settings query and this file mocks `@/api/hooks` down to two
/// hooks. What is under test is that the button OPENS Settings on the
/// repositories topic, which the stub reports faithfully.
vi.mock("./SettingsDialog", () => ({
  SettingsDialog: ({ initialSection }: { initialSection?: string }) => (
    <div role="dialog">Settings: {initialSection}</div>
  ),
}));
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
  scan.dirs = ["/code"];
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
    expect(screen.queryByText(/no git repositories found/i)).toBeNull();
    // And not the #952 arm either. `dirs` defaults to `[]` while its own
    // query is in flight, so an arm ordered before `isLoading` would
    // claim "no folders configured" on the first render of a machine
    // that has three -- the same "sends someone to fix something that is
    // not broken" failure, from a different direction.
    expect(screen.queryByText(/does not know where your repositories are/i)).toBeNull();
  });

  it("gives the diagnosis once the scan has answered with nothing", () => {
    repos.mockReturnValue([]);
    render(<RepoPickerSidebar reviewingCount={0} />);
    expect(screen.getByText(/no git repositories found in the folder/i)).toBeTruthy();
  });

  /// #952. "We looked and there is nothing" and "we had nowhere to look"
  /// are opposite answers, exactly as this component's own comment says of
  /// the three states it already separated -- and the second is the one a
  /// machine without `~/code` is in on first run, because
  /// `default_worktree_dirs` returns an EMPTY vector there by design.
  ///
  /// Before this, both rendered "No repositories found in the scanned
  /// folders": a claim about the user's disk, made by a scan that had
  /// never looked at it, with no remedy on screen and no mention of
  /// Settings.
  describe("nowhere to look, versus nothing there (#952)", () => {
    it("states the task rather than diagnosing a disk it never scanned", () => {
      repos.mockReturnValue([]);
      scan.dirs = [];
      render(<RepoPickerSidebar reviewingCount={0} />);
      expect(screen.getByText(/does not know where your repositories are/i)).toBeTruthy();
      // And NOT the finding. Saying both would be the contradiction the
      // issue is about.
      expect(screen.queryByText(/no git repositories found/i)).toBeNull();
    });

    /// The remedy the arm existed without. #945 fixed the field that
    /// rejected `~/code`, so a user sent to Settings can now actually type
    /// the path they have -- which is what makes pointing at it honest.
    it("offers a way to fix it, opening Settings on the repositories topic", () => {
      repos.mockReturnValue([]);
      scan.dirs = [];
      render(<RepoPickerSidebar reviewingCount={0} />);
      // Not open until asked: a dialog rendered unconditionally behind
      // every sidebar would run every settings query on views that never
      // open it.
      expect(screen.queryByRole("dialog")).toBeNull();
      fireEvent.click(screen.getByRole("button", { name: /choose folders to scan/i }));
      expect(screen.getByRole("dialog").textContent).toContain("repositories");
    });

    /// The other half, and the one that keeps the diagnosis honest: with
    /// directories configured this IS a finding about the disk, so it
    /// stays a diagnosis -- and names the paths, which is the difference
    /// between something a user can check and something they must guess.
    it("names the folders it actually scanned when there were some", () => {
      repos.mockReturnValue([]);
      scan.dirs = ["/code", "/work/src"];
      render(<RepoPickerSidebar reviewingCount={0} />);
      expect(screen.getByText(/no git repositories found in the folders/i)).toBeTruthy();
      expect(screen.getByText("/code")).toBeTruthy();
      expect(screen.getByText("/work/src")).toBeTruthy();
      expect(screen.queryByText(/does not know where your repositories are/i)).toBeNull();
    });

    /// No noise on the happy path: with repositories listed neither arm
    /// may appear, however the directories are configured. An empty-state
    /// message over a populated list is how a user concludes the list is
    /// wrong.
    it("says neither thing when repositories were found", () => {
      repos.mockReturnValue([repo("alpha")]);
      scan.dirs = [];
      render(<RepoPickerSidebar reviewingCount={0} />);
      expect(screen.getByText("alpha")).toBeTruthy();
      expect(screen.queryByText(/does not know where your repositories are/i)).toBeNull();
      expect(screen.queryByText(/no git repositories found/i)).toBeNull();
    });

    /// Arm ORDER, which is the constraint #846 and #951 both left on this
    /// component: the unreadable arm comes first. A scan that could not
    /// read its folders HAD folders, so this is belt and braces -- but the
    /// two have opposite remedies and the ordering is what guarantees the
    /// one naming the user's settings cannot win.
    it("reports what it could not read before anything about directories", () => {
      repos.mockReturnValue([]);
      scan.dirs = [];
      scan.unreadable = [UNREADABLE];
      render(<RepoPickerSidebar reviewingCount={0} />);
      expect(screen.getByText(/no repositories could be read/i)).toBeTruthy();
      expect(screen.queryByText(/does not know where your repositories are/i)).toBeNull();
    });
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
      // #952 reworded this arm ("No git repositories found in the
      // folder(s) being scanned") and gave it the directory list. Matched
      // against the LIVE copy rather than the retired sentence, which
      // would pass whatever the component rendered.
      expect(screen.queryByText(/no git repositories found/i)).toBeNull();
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

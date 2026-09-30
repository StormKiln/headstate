import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const setDirs = vi.hoisted(() => vi.fn((d: string[]) => Promise.resolve(d)));
const setInterval_ = vi.hoisted(() => vi.fn((s: number) => Promise.resolve(s)));
const dirs = vi.hoisted(() => ({ current: ["/Users/x/code"] as string[] }));

const setCleanup = vi.hoisted(() => vi.fn(() => Promise.resolve()));
/// `staleVenvDays` since #957: the setting that had no control at all,
/// and the one whose stored value every install holds as 0. `setUi` is a
/// spy so the write can be asserted -- a select that renders the right
/// options and stores nothing is the failure mode a render-only test
/// cannot see.
const uiState = vi.hoisted(() => ({ diagnosticLogging: false, staleVenvDays: 0 }));
// Typed so the written prefs can be asserted: the untyped form infers an
// empty argument tuple, and indexing it is a compile error.
const setUi = vi.hoisted(() =>
  vi.fn<(prefs: { stale_venv_days: number }) => Promise<void>>(() => Promise.resolve()),
);
const revealFn = vi.hoisted(() => vi.fn(() => Promise.resolve("/Users/x/Library/Logs/app/headstate.log")));
const cleanupPrefs = vi.hoisted(() => ({
  current: {
    enabled: false,
    mode: "preview" as const,
    artifacts: false,
    venvs: false,
    venvs_stale: false,
    branches: false,
    branches_ancestor: false,
    branches_squash: false,
    worktrees: false,
    worktrees_safe: false,
    docker: false,
    docker_dangling: false,
    max_per_run: 0,
  },
}));

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock("../api/tauri", () => ({ revealLog: revealFn }));
vi.mock("./GitLabHostPanel", () => ({ GitLabHostPanel: () => <button>Save GitLab host</button> }));
vi.mock("../api/hooks", () => ({
  // #1154. Undefined renders nothing, which is what these tests assume:
  // a failed probe must not draw "not found" for every tool.
  useToolVersions: () => ({ data: undefined, isError: false }),
  // #1127. The inventory section is collapsed and reads nothing until
  // opened, which is what these tests assume.
  useClaudeHookInventory: () => ({ data: undefined, error: null }),
  // #1130. Collapsed by default, so nothing is read until opened --
  // which is what these tests assume.
  useClaudeEffectiveSettings: () => ({ data: undefined, error: null }),
  useClaudeConfigHealth: () => ({
    data: undefined,
    error: null,
    isFetching: false,
    refetch: () => Promise.resolve(),
  }),
  // Defaults, matching the Rust side: nothing hidden, close hides.
  useUiPrefs: () => ({
    prefs: {
      hidden_views: [],
      close_hides_to_tray: true,
      diagnostic_logging: uiState.diagnosticLogging,
      stale_venv_days: uiState.staleVenvDays,
    },
    set: setUi,
  }),
  useCleanupPrefs: () => ({ prefs: cleanupPrefs.current, set: setCleanup }),
  useAutostart: () => ({ enabled: false, set: () => Promise.resolve() }),
  useRemoteEnabled: () => ({ enabled: false, set: () => Promise.resolve() }),
  useIssuePairingToken: () => () => Promise.reject("not in this test"),
  usePairedDevices: () => ({ data: [], isLoading: false, error: null }),
  useRevokePairedDevice: () => () => Promise.resolve(),
  usePollInterval: () => ({ seconds: 120, set: setInterval_ }),
  useWorktreeDirs: () => ({ dirs: dirs.current, set: setDirs }),
  // #915's panel. "Not installed" is the state a fresh machine is in, and
  // deliberately not `undefined`: an unresolved status renders a "checking"
  // line rather than any of the four answers, which would make the
  // reachability assertions below depend on a loading state.
  useClaudeHooks: () => ({
    status: { state: "not_installed" },
    isLoading: false,
    error: null,
    install: () => Promise.resolve({ command: "x", added: [], replaced: [], created_file: false }),
    reinstall: () => Promise.resolve({ command: "x", added: [], replaced: [], created_file: false }),
    uninstall: () => Promise.resolve({ removed: [], was_absent: true }),
  }),
  // Defaults, matching the Rust side: absent prefs mean everything on.
  useNotifyPrefs: () => ({
    prefs: { enabled: true, ci_failed: true, conflicted: true },
    set: () => Promise.resolve(),
  }),
}));

import { SettingsDialog } from "./SettingsDialog";
import { ALWAYS_OFFERED, VIEWS } from "./ViewSwitcher";
import { ALL_VIEWS } from "@/store/filters";

function open() {
  return render(<SettingsDialog open onOpenChange={() => {}} />);
}

it("uses the GitLab host save control instead of the scan-directory footer save", () => {
  open();
  fireEvent.click(screen.getByRole("button", { name: "GitLab" }));
  expect(screen.getByRole("button", { name: "Save GitLab host" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: /^Save$/ })).toBeNull();
});

describe("SettingsDialog", () => {
  it("shows the configured directories, one per line", () => {
    open();
    expect(screen.getByLabelText(/directories to scan/i)).toHaveProperty(
      "value",
      "/Users/x/code",
    );
  });

  it("saves trimmed, non-empty paths", async () => {
    open();
    fireEvent.change(screen.getByLabelText(/directories to scan/i), {
      target: { value: "  /a  \n\n /b \n   " },
    });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    await waitFor(() => expect(setDirs).toHaveBeenCalledWith(["/a", "/b"]));
  });

  // Unlike the interval, which only clamps, this write can be REJECTED --
  // a typo must surface rather than appearing to succeed.
  it("shows the backend's error instead of closing", async () => {
    setDirs.mockImplementationOnce(() => Promise.reject("not a directory: /nope"));
    open();
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    expect(await screen.findByRole("alert")).toHaveProperty(
      "textContent",
      "not a directory: /nope",
    );
  });

  it("changes the poll interval", () => {
    open();
    fireEvent.change(screen.getByLabelText(/check github every/i), {
      target: { value: "300" },
    });
    expect(setInterval_).toHaveBeenCalledWith(300);
  });

  // The floor is 60s on the Rust side; offering less would let the UI ask
  // for something silently clamped.
  it("offers no interval below the backend floor", () => {
    open();
    const opts = Array.from(
      screen.getByLabelText(/check github every/i).querySelectorAll("option"),
    ).map((o) => Number(o.getAttribute("value")));
    expect(Math.min(...opts)).toBeGreaterThanOrEqual(60);
  });

  /// A `<button>` nested inside a `<label>` joins that label's
  /// accessible name, so a help icon there would make the field
  /// announce as "Directories to scan for repositories About scanned
  /// directories". Caught by getByLabelText finding two matches.
  it("keeps help buttons out of field labels", () => {
    open();
    for (const label of Array.from(document.querySelectorAll("label"))) {
      expect(label.querySelector("button")).toBeNull();
    }
  });
});

describe("automatic cleanup settings", () => {
  /// The feature must present itself as what it IS. A switch that
  /// sounds like it might delete, in a build where it cannot, is the
  /// one thing worth not understating.
  it("says plainly that nothing is removed automatically", () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);
    expect(screen.getByText(/never removes anything automatically/i)).toBeTruthy();
  });

  it("hides the per-kind options until the feature is on", () => {
    cleanupPrefs.current = { ...cleanupPrefs.current, enabled: false };
    const r = render(<SettingsDialog open onOpenChange={() => {}} />);
    expect(screen.queryByLabelText(/Build artifacts/)).toBeNull();
    r.unmount();

    cleanupPrefs.current = { ...cleanupPrefs.current, enabled: true };
    render(<SettingsDialog open onOpenChange={() => {}} />);
    expect(screen.getByLabelText(/Build artifacts/)).toBeTruthy();
    expect(screen.getByLabelText(/^Virtualenvs/)).toBeTruthy();
  });

  /// #394: the opt-in must say what turning it on ASSERTS, since the
  /// distinction between orphaned and stale is the whole reason it
  /// exists as a separate switch.
  /// The stale opt-in belongs to AUTOMATIC cleanup, beside the orphan
  /// one -- not to manual removal, which it used to gate. Ticking a row
  /// and confirming a dialog is already the user's intent; unattended
  /// deletion acting on a 90-day threshold is not.
  /// #493: the categories the automatic pass can act on, each with the
  /// specific claims beneath it.
  it("offers every cleanup category", () => {
    cleanupPrefs.current = { ...cleanupPrefs.current, enabled: true };
    render(<SettingsDialog open onOpenChange={() => {}} />);
    for (const label of [
      /Build artifacts/,
      /^Virtualenvs/,
      /Merged branches/,
      /Merged worktrees/,
      /Docker images/,
    ]) {
      expect(screen.getByLabelText(label)).toBeTruthy();
    }
  });

  /// Ticking a parent ticks everything under it, in one save rather
  /// than one per checkbox.
  it("ticking a category ticks its specific options", () => {
    cleanupPrefs.current = { ...cleanupPrefs.current, enabled: true };
    render(<SettingsDialog open onOpenChange={() => {}} />);
    fireEvent.click(screen.getByLabelText(/Merged branches/));
    expect(setCleanup).toHaveBeenCalledWith(
      expect.objectContaining({
        branches: true,
        branches_ancestor: true,
        branches_squash: true,
      }),
    );
  });

  /// A user could otherwise tick a specific option under an off
  /// category and see nothing happen, because the pass reads the parent.
  it("ticking a specific option turns its category on", () => {
    cleanupPrefs.current = { ...cleanupPrefs.current, enabled: true };
    render(<SettingsDialog open onOpenChange={() => {}} />);
    fireEvent.click(screen.getByLabelText(/Merged by squash/));
    expect(setCleanup).toHaveBeenCalledWith(
      expect.objectContaining({ branches_squash: true, branches: true }),
    );
  });

  /// A parent with only some children on must not render as plain
  /// "on": that would misstate what the unattended pass will do.
  it("shows a partly-ticked category as indeterminate", () => {
    cleanupPrefs.current = {
      ...cleanupPrefs.current,
      enabled: true,
      branches: true,
      branches_ancestor: true,
      branches_squash: false,
    };
    render(<SettingsDialog open onOpenChange={() => {}} />);
    const parent = screen.getByLabelText(/Merged branches/) as HTMLInputElement;
    expect(parent.indeterminate).toBe(true);
    expect(parent.checked).toBe(false);
  });

  it("offers stale virtualenvs beside orphaned ones, under automatic cleanup", () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);
    const orphaned = screen.getByLabelText(/^Virtualenvs/);
    const stale = screen.getByLabelText(/Also stale ones/);
    expect(orphaned).toBeTruthy();
    expect(stale).toBeTruthy();
    // Same section: the stale toggle sits with the automatic-cleanup
    // choices rather than in a section of its own about manual removal.
    expect(orphaned.closest("div")?.parentElement).toBe(
      stale.closest("div")?.parentElement,
    );
  });
});

/// #957: `stale_venv_days` was the only `UiPrefs` field with no control.
///
/// Verified mechanically in the issue -- `grep -rl "stale_venv_days" src`
/// returned only the type declaration in `tauri.ts` -- while every other
/// field had a control here or a panel of its own. So it was always 0,
/// which `poll::stale_venv_days` resolves to 90, which is what the
/// frontend hardcoded: three numbers that agreed only because one of them
/// could not be changed.
describe("the virtualenv staleness threshold (#957)", () => {
  const openDialog = () =>
    render(<SettingsDialog open onOpenChange={() => {}} initialSection="cleanup" />);
  const control = () =>
    screen.getByLabelText(/Days idle before a virtualenv counts as stale/i);

  it("offers a control for it at all", () => {
    openDialog();
    expect(control()).toBeTruthy();
  });

  /// A stored 0 means "use the default", never "zero days". Rendering the
  /// raw 0 would show a threshold of zero on every existing install --
  /// the reading `UiPrefs::stale_venv_days`' own doc forbids, since "a
  /// stored 0 from a bad write must not reclassify the whole cache".
  it("shows the resolved default rather than the stored 0", () => {
    uiState.staleVenvDays = 0;
    openDialog();
    expect((control() as HTMLSelectElement).value).toBe("90");
  });

  it("shows a configured value as itself", () => {
    uiState.staleVenvDays = 180;
    openDialog();
    expect((control() as HTMLSelectElement).value).toBe("180");
    uiState.staleVenvDays = 0;
  });

  /// A select that renders correctly and writes nothing is the failure a
  /// render-only test cannot see.
  it("writes the chosen number", async () => {
    setUi.mockClear();
    openDialog();
    fireEvent.change(control(), { target: { value: "180" } });
    await waitFor(() => expect(setUi).toHaveBeenCalled());
    expect(setUi.mock.calls[0][0].stale_venv_days).toBe(180);
  });

  /// Every option must survive `poll::stale_venv_days`' clamp untouched.
  /// The whole reason this is a select rather than a number field is that
  /// the clamp cannot then rewrite what the user picked -- an option
  /// outside 30..3650 would reintroduce exactly the silent disagreement
  /// the control exists to end.
  it("offers only values the backend clamp leaves alone", () => {
    openDialog();
    const values = [...control().querySelectorAll("option")].map((o) => Number(o.value));
    expect(values.length).toBeGreaterThan(1);
    for (const v of values) {
      expect(v).toBeGreaterThanOrEqual(30);
      expect(v).toBeLessThanOrEqual(3650);
    }
  });

  /// Outside the automatic-cleanup gate, deliberately: the threshold
  /// labels rows and enables checkboxes on the Artifacts page whether or
  /// not the unattended pass is on, so hiding it behind that switch would
  /// hide the number a user is reading right now.
  it("stays reachable with automatic cleanup switched off", () => {
    cleanupPrefs.current = { ...cleanupPrefs.current, enabled: false };
    openDialog();
    expect(control()).toBeTruthy();
  });
});

describe("the diagnostic log controls", () => {
  /// The old text said "each GitHub request", which was true until the
  /// local scans were instrumented. A user with a hanging Virtualenvs
  /// page had no reason to think this setting would help.
  it("says it records local scans, not only GitHub requests", () => {
    uiState.diagnosticLogging = true;
    render(<SettingsDialog open onOpenChange={() => {}} />);
    expect(screen.getByText(/GitHub requests and local scans/)).toBeTruthy();
  });

  /// The promise is load-bearing: this log exists to be SENT to someone.
  ///
  /// It used to read "never repository names, titles, or tokens", and a
  /// test asserted exactly that -- which is how the claim survived. The
  /// logger never honoured it: #1122 found 30 unconditional sites
  /// writing a repo, a branch or a home path, and no scrubbing on the
  /// Rust side at all. The test locked in the sentence and nobody
  /// checked the sentence against the logger.
  ///
  /// So this now asserts what is TRUE: tokens and paths are stripped
  /// (`src-tauri/src/redact.rs`), and repository names are present on
  /// purpose, because the PR action lines are the audit trail README
  /// advertises.
  it("promises no token and no local path", () => {
    uiState.diagnosticLogging = true;
    render(<SettingsDialog open onOpenChange={() => {}} />);
    expect(screen.getByText(/Never your token, never a local path/)).toBeTruthy();
  });

  /// The regression that matters: someone restoring the old, false
  /// sentence. It reads well and it is what the log DOESN'T do.
  it("does not claim repository names are absent", () => {
    uiState.diagnosticLogging = true;
    render(<SettingsDialog open onOpenChange={() => {}} />);
    expect(screen.queryByText(/never repository names/i)).toBeNull();
  });

  /// Says out loud that the log names your repositories, so a user
  /// decides knowingly whether to send it.
  it("says the log records the writes you made, by repository", () => {
    uiState.diagnosticLogging = true;
    render(<SettingsDialog open onOpenChange={() => {}} />);
    expect(screen.getByText(/repository and pull request number/)).toBeTruthy();
  });

  it("can reveal the log file", async () => {
    uiState.diagnosticLogging = true;
    render(<SettingsDialog open onOpenChange={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: /show the log/i }));
    await waitFor(() => expect(revealFn).toHaveBeenCalled());
  });

  /// Nothing to reveal and nothing to explain when it is off.
  it("offers nothing while logging is disabled", () => {
    uiState.diagnosticLogging = false;
    render(<SettingsDialog open onOpenChange={() => {}} />);
    expect(screen.queryByRole("button", { name: /show the log/i })).toBeNull();
  });
});

/// #436: a notification when a PR enters the green "Ready for review"
/// panel, so it can be picked up immediately.
describe("the ready-for-review notification", () => {
  it("offers a toggle for it", () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);
    expect(screen.getByLabelText(/ready for your review/i)).toBeTruthy();
  });

  /// The wording described breakage while every notification WAS
  /// breakage. This one is good news.
  it("no longer says notifications are only about breakage", () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);
    expect(screen.queryByText(/newly breaks/)).toBeNull();
    expect(screen.getByText(/never on first launch/)).toBeTruthy();
  });
});

/// #675: the Views section carried a hand-written list of FOUR views
/// while nine existed, so `my-prs`, `branches`, `artifacts`, `packages`
/// and `claude-md` could not be hidden at all -- and nothing said so,
/// because a partial list looks exactly like a complete one.
///
/// These tests are the reason it cannot happen again: adding a view to
/// `ALL_VIEWS` without it reaching this section now fails here.
describe("the Views section", () => {
  it("offers every view that can actually be hidden", () => {
    open();
    const panel = screen.getByText("Views", { selector: "span" }).parentElement;
    expect(panel).toBeTruthy();

    // Derived from ALL_VIEWS rather than a literal: a list written out
    // here would be a THIRD copy, and would drift exactly as the one
    // in the component did.
    const expected = ALL_VIEWS.filter((id) => !ALWAYS_OFFERED.has(id));
    for (const id of expected) {
      const label = VIEWS.find((v) => v.id === id)?.label;
      expect(label, `no label for view "${id}"`).toBeTruthy();
      expect(
        within(panel as HTMLElement).getByText(label as string),
        `the Views section is missing "${label}"`,
      ).toBeTruthy();
    }
  });

  /// The count is asserted separately from the names above. Without it
  /// the test passes on a section that contains every expected view
  /// AND extras -- a stale entry for a deleted view, say, which would
  /// write an id into `hidden_views` that nothing reads.
  it("offers no view that does not exist", () => {
    open();
    const panel = screen.getByText("Views", { selector: "span" })
      .parentElement as HTMLElement;
    const boxes = within(panel).getAllByRole("checkbox");
    expect(boxes.length).toBe(ALL_VIEWS.filter((id) => !ALWAYS_OFFERED.has(id)).length);
  });

  /// `ViewSwitcher` offers `my-prs` whatever `hidden_views` says, so a
  /// checkbox for it would be a control that appears to work and
  /// silently does nothing. Absent is the honest rendering.
  it("does not offer a toggle for a view that is always shown", () => {
    open();
    const panel = screen.getByText("Views", { selector: "span" })
      .parentElement as HTMLElement;
    for (const id of ALWAYS_OFFERED) {
      const label = VIEWS.find((v) => v.id === id)?.label as string;
      expect(within(panel).queryByText(label), `"${label}" should not be togglable`).toBeNull();
    }
  });
});

/// #437: ~445 lines of continuous scroll became topics on the left and
/// one panel on the right.
describe("the settings sections", () => {
  it("offers a topic for each group", () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);
    const nav = screen.getByRole("navigation", { name: /settings sections/i });
    for (const label of ["General", "Repositories", "Notifications", "Cleanup", "Views"]) {
      expect(within(nav).getByRole("button", { name: label })).toBeTruthy();
    }
  });

  /// #485: the frame is fixed in BOTH dimensions, like macOS System
  /// Settings, so switching topics does not resize the window under
  /// the cursor.
  ///
  /// Asserted on the resolved class list because that is what the bug
  /// was: `max-w-lg` left ~20rem for controls once the 9rem topic menu
  /// took its share, and no height at all meant the dialog sized to
  /// whichever panel was showing.
  it("has a fixed frame that does not resize with the topic", () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);
    const panel = screen.getByRole("dialog");
    const cls = panel.className;

    expect(cls).toMatch(/\bh-\[32rem\]/);
    expect(cls).toMatch(/\bmax-w-3xl\b/);
    // The base DialogContent's default cap used to be `sm:max-w-sm`,
    // which tailwind-merge keyed separately from a bare `max-w-3xl` and
    // which therefore won above 640px -- so this dialog carried an
    // explicit `sm:max-w-3xl` to beat it. #1306 moved the base's cap
    // onto the callers' own key, so the bare form applies and the
    // doubled spelling is gone. Assert the cap is not merely absent but
    // REPLACED: a `max-w-3xl` sitting beside any surviving cap is the
    // bug this dialog was the first to hit.
    expect(cls).not.toMatch(/\bsm:max-w-/);
    expect(cls).not.toMatch(/(^|\s)max-w-sm(\s|$)/);
  });

  it("marks the chosen topic as current", () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);
    const nav = screen.getByRole("navigation", { name: /settings sections/i });
    const notifications = within(nav).getByRole("button", { name: "Notifications" });
    expect(notifications.getAttribute("aria-current")).toBeNull();
    fireEvent.click(notifications);
    expect(notifications.getAttribute("aria-current")).toBe("page");
  });

  /// The phone's connection banner opens Settings straight on the
  /// Phone topic: that is the only reason it was tapped.
  it("opens on the topic it is asked to", () => {
    render(<SettingsDialog open onOpenChange={() => {}} initialSection="phone" />);
    const nav = screen.getByRole("navigation", { name: /settings sections/i });
    expect(within(nav).getByRole("button", { name: "Phone" }).getAttribute("aria-current")).toBe(
      "page",
    );
    expect(within(nav).getByRole("button", { name: "General" }).getAttribute("aria-current")).toBeNull();
  });

  it("defaults to General", () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);
    const nav = screen.getByRole("navigation", { name: /settings sections/i });
    expect(within(nav).getByRole("button", { name: "General" }).getAttribute("aria-current")).toBe(
      "page",
    );
  });

  /// The constraint from the issue: a reorganisation that HIDES a
  /// control is a regression.
  ///
  /// Panels are hidden with CSS, never unmounted and never with the
  /// `hidden` attribute -- that attribute removes them from the
  /// accessibility tree, so a screen reader could not reach a setting
  /// until the right topic was clicked. Both wrong approaches were
  /// tried and both were caught by the existing tests failing to find
  /// controls by role.
  it("keeps every control reachable whichever topic is selected", () => {
    render(<SettingsDialog open onOpenChange={() => {}} />);
    // On "General" by default, yet a Notifications control is present.
    expect(screen.getByRole("checkbox", { name: /desktop notifications/i })).toBeTruthy();
    // ...and a Repositories one.
    expect(screen.getByLabelText(/directories to scan/i)).toBeTruthy();
  });
});

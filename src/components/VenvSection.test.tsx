import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Venv } from "@/types/pr";
import { stubViewport } from "@/test-utils";

// Typed so a test can resolve with real outcomes: a bare
// `Promise.resolve([])` infers `never[]`, which rejects every fixture.
const removeFn = vi.hoisted(() =>
  vi.fn<(paths: string[]) => Promise<{ path: string; error: string | null }[]>>(() =>
    Promise.resolve([]),
  ),
);
const state = vi.hoisted(() => ({
  /// Directory-walk progress, for the streaming tests (#1151).
  walk: undefined as { found: number; visited: number; max_dirs: number } | undefined,
  venvs: [] as Venv[],
  sizes: new Map<string, number>(),
  idle: new Map<string, number>(),
  measuring: false,
  pending: 0,
  total: 0,
  /// How many SIZE chunks rejected (#956). Distinct from `failed` below,
  /// which is the venv scan itself: the scan can succeed while the sizing
  /// of some chunks does not, and those two have different consequences
  /// -- one blanks the section, the other only makes the byte figures a
  /// floor.
  sizesFailed: 0,
  loading: false,
  // #846: a REJECTED scan. The sharpest case in that issue, because the
  // `isLoading` half was already fixed here for the adjacent bug and
  // `isError` was not -- so a rejection still removed the entire section.
  failed: false,
  /// The stored `stale_venv_days` (#957). The threshold is no longer a
  /// constant in this component: it is the user's setting, resolved the
  /// same way `poll::stale_venv_days` resolves it.
  ///
  /// `0` by default, which is what every existing install actually
  /// stores and what `staleVenvDays` reads as "use the default" -- so
  /// the assertions written before #957 keep running against 90 days.
  staleVenvDays: 0,
}));

// The explicit retry the `retry: false` on `useVenvs` is paired with. The
// hook's `staleTime: 30 * 60 * 1000` and `refetchOnWindowFocus: false`
// are right for data and pinned a FAILURE for half an hour, so a retry
// the user can press is the only thing that ends it.
const refetchFn = vi.hoisted(() => vi.fn());

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock("../api/hooks", () => ({
  // No walk progress by default: the settled state, which every other
  // assertion here assumes (#1151).
  useVenvWalkProgress: () => state.walk,
  useVenvs: () => ({
    data: state.venvs,
    isLoading: state.loading,
    isError: state.failed,
    error: "scan refused: operation not permitted",
    refetch: refetchFn,
  }),
  useVenvSizes: () => ({
    sizes: state.sizes,
    idle: state.idle,
    measuring: state.measuring,
    pending: state.pending,
    total: state.total,
    /// #956: how many size chunks REJECTED. A rejection stops
    /// `measuring` without answering, so `!measuring` alone reads as
    /// "fully measured".
    failed: state.sizesFailed,
  }),
  useRemoveVenvs: () => removeFn,
  // #957. Only `stale_venv_days` is read here; the rest of `UiPrefs` is
  // absent deliberately, so a component that started reading another
  // field without thinking about it fails loudly rather than seeing
  // `undefined`.
  useUiPrefs: () => ({
    prefs: { stale_venv_days: state.staleVenvDays },
    set: vi.fn(),
  }),
}));

import { VenvSection } from "./VenvSection";

const venv = (over: Partial<Venv> = {}): Venv => ({
  path: "/cache/hello-world-delivery-AAAAAAAA-py3.13",
  project: "hello-world-delivery",
  state: "orphaned",
  source: null,
  size_bytes: null,
  idle_secs: null,
  ...over,
});

beforeEach(() => {
  removeFn.mockClear();
  refetchFn.mockClear();
  state.venvs = [];
  state.loading = false;
  state.sizes = new Map();
  state.idle = new Map();
  state.measuring = false;
  // #846. Leaked, this replaces every test's section with an error panel.
  state.failed = false;
  // #956. Leaked, this suppresses every byte figure in the section and
  // adds an advisory that unrelated `role="status"` assertions would find.
  state.sizesFailed = 0;
  // #957. Leaked, a non-default threshold relabels rows other tests
  // assert the badge of, and rewrites the confirmation's sentence.
  state.staleVenvDays = 0;
});

describe("VenvSection on a phone", () => {
  afterEach(() => stubViewport(null));

  it("stacks each row, keeping project, state, source, age and size", () => {
    stubViewport(390);
    const v = venv({ source: "/code/hello-world-delivery", state: "stale" });
    state.venvs = [v];
    state.sizes = new Map([[v.path, 1024]]);
    state.idle = new Map([[v.path, 86_400 * 3]]);
    render(<VenvSection />);
    const row = screen.getByRole("checkbox").closest("li") as HTMLElement;
    expect(row.className).toContain("flex-col");
    expect(within(row).getByText("hello-world-delivery")).toBeTruthy();
    expect(within(row).getByText("stale")).toBeTruthy();
    expect(within(row).getByText("/code/hello-world-delivery")).toBeTruthy();
    expect(within(row).getByText("3 days ago")).toBeTruthy();
    expect(within(row).getByText("1.0 KB")).toBeTruthy();
    fireEvent.click(within(row).getByRole("checkbox"));
    expect(screen.getByRole("button", { name: /remove 1 /i })).toBeTruthy();
  });

  it("keeps the desktop row on one line", () => {
    stubViewport(1400);
    const v = venv();
    state.venvs = [v];
    state.sizes = new Map([[v.path, 1024]]);
    render(<VenvSection />);
    const row = screen.getByRole("checkbox").closest("li") as HTMLElement;
    expect(row.className).not.toContain("flex-col");
    expect(within(row).getByText("1.0 KB").parentElement).toBe(row);
    expect(within(row).getByText("hello-world-delivery").parentElement).toBe(row);
  });
});

describe("VenvSection", () => {
  it("renders nothing when there are no virtualenvs", () => {
    const { container } = render(<VenvSection />);
    expect(container.firstChild).toBeNull();
  });

  /// Orphaned is a FACT -- the path that made it is gone. Stale is a
  /// judgement about a project that still exists, and this view will not
  /// act on a judgement.
  /// Was "offers only orphans". A 416-day-old virtualenv is now
  /// removable without a setting: ticking the row IS the intent, and no
  /// other artifact asks twice. `live` stays refused -- its project
  /// exists and is in use, which is a fact rather than a threshold.
  it("offers orphans and stale virtualenvs, but never live ones", () => {
    state.venvs = [
      venv(),
      venv({
        path: "/cache/octo-backend-BBBBBBBB-py3.13",
        project: "octo-backend",
        state: "live",
        source: "/code/octo-backend",
      }),
    ];
    state.idle = new Map([["/cache/octo-backend-BBBBBBBB-py3.13", 416 * 86400]]);
    render(<VenvSection />);

    const boxes = screen.getAllByRole("checkbox");
    expect(boxes).toHaveLength(2);
    // The orphan.
    expect(boxes[0].hasAttribute("disabled")).toBe(false);
    // 416 days idle -> stale -> now selectable, no setting involved.
    expect(boxes[1].hasAttribute("disabled")).toBe(false);
  });

  /// The reported case: a project still on disk, untouched for over a
  /// year. It must be VISIBLE and labelled, but not removable.
  it("labels a long-idle venv as stale, not live", () => {
    state.venvs = [
      venv({
        path: "/cache/octo-backend-BBBBBBBB-py3.13",
        project: "octo-backend",
        state: "live",
        source: "/code/octo-backend",
      }),
    ];
    state.idle = new Map([["/cache/octo-backend-BBBBBBBB-py3.13", 416 * 86400]]);
    render(<VenvSection />);
    expect(screen.getByText("stale")).toBeTruthy();
  });

  it("keeps a recently used venv live", () => {
    state.venvs = [
      venv({
        path: "/cache/octocat-mcp-CCCCCCCC-py3.13",
        project: "octocat-mcp",
        state: "live",
        source: "/code/octocat-mcp",
      }),
    ];
    state.idle = new Map([["/cache/octocat-mcp-CCCCCCCC-py3.13", 3 * 3600]]);
    render(<VenvSection />);
    expect(screen.getByText("live")).toBeTruthy();
  });

  /// An orphan's path is gone, so how recently it was touched says
  /// nothing about whether anyone wants it.
  it("keeps an orphan orphaned however recently touched", () => {
    state.venvs = [venv()];
    // A VERY small idle time. Any rule that reclassifies an orphan by
    // age has to have a threshold somewhere, and a value below every
    // plausible one is what forces such a rule to show itself -- a
    // larger number sits above the threshold and passes either way.
    state.idle = new Map([[venv().path, 5]]);
    render(<VenvSection />);
    expect(screen.getByText("orphaned")).toBeTruthy();
  });

  /// ...and stays orphaned when it looks STALE by age too. Age must
  /// never reclassify an orphan in either direction: its path is gone,
  /// so the timestamp is not evidence about anything.
  it("keeps an orphan orphaned when it is also long idle", () => {
    state.venvs = [venv()];
    state.idle = new Map([[venv().path, 500 * 86400]]);
    render(<VenvSection />);
    expect(screen.getByText("orphaned")).toBeTruthy();
    expect(screen.queryByText("stale")).toBeNull();
  });

  /// The source is the EVIDENCE for the verdict -- it is what lets a
  /// user disagree with the label.
  it("names the project directory behind a live verdict", () => {
    state.venvs = [
      venv({ state: "live", source: "/code/still-here", project: "still-here" }),
    ];
    render(<VenvSection />);
    expect(screen.getByText("/code/still-here")).toBeTruthy();
  });

  it("says so when no project directory was found", () => {
    state.venvs = [venv()];
    render(<VenvSection />);
    expect(screen.getByText("no project directory found")).toBeTruthy();
  });

  it("removes the selected orphans on confirm", () => {
    state.venvs = [venv()];
    state.sizes = new Map([[venv().path, 1_000_000]]);
    render(<VenvSection />);
    fireEvent.click(screen.getByRole("checkbox", { name: /Select/ }));
    fireEvent.click(screen.getByRole("button", { name: /^Remove 1/ }));
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(removeFn).toHaveBeenCalledWith([venv().path]);
  });

  it("removes nothing on cancel", () => {
    state.venvs = [venv()];
    render(<VenvSection />);
    fireEvent.click(screen.getByRole("checkbox", { name: /Select/ }));
    fireEvent.click(screen.getByRole("button", { name: /^Remove 1/ }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(removeFn).not.toHaveBeenCalled();
  });

  /// A disabled checkbox that explains itself beats one that silently
  /// ignores clicks.
  it("says why a live venv cannot be selected", () => {
    state.venvs = [
      venv({ state: "live", source: "/code/x", project: "x" }),
    ];
    render(<VenvSection />);
    expect(
      screen.getByRole("checkbox", { name: /its project still exists/ }),
    ).toBeTruthy();
  });
});

describe("VenvSection bulk removal", () => {
  /// The reported case: 78 orphans from ONE deleted project. Ticking
  /// them individually is 78 clicks for a decision made once -- and
  /// every one is a fact rather than a judgement, so there is nothing to
  /// weigh row by row.
  it("offers one click for every orphan", () => {
    state.venvs = [
      venv({ path: "/cache/a-AAAAAAAA-py3.13" }),
      venv({ path: "/cache/b-BBBBBBBB-py3.13" }),
      venv({ path: "/cache/c-CCCCCCCC-py3.13" }),
    ];
    state.sizes = new Map([
      ["/cache/a-AAAAAAAA-py3.13", 1_000_000_000],
      ["/cache/b-BBBBBBBB-py3.13", 2_000_000_000],
      ["/cache/c-CCCCCCCC-py3.13", 3_000_000_000],
    ]);
    render(<VenvSection />);
    expect(screen.getByRole("button", { name: /Remove all 3 orphaned/ })).toBeTruthy();
  });

  /// It must select ONLY orphans. A live venv swept into a bulk action
  /// is the one outcome that would make the button untrustworthy.
  it("never sweeps a live venv into the bulk selection", () => {
    state.venvs = [
      venv({ path: "/cache/a-AAAAAAAA-py3.13" }),
      venv({
        path: "/cache/live-BBBBBBBB-py3.13",
        state: "live",
        source: "/code/live",
      }),
      venv({ path: "/cache/c-CCCCCCCC-py3.13" }),
    ];
    render(<VenvSection />);
    fireEvent.click(screen.getByRole("button", { name: /Remove all 2 orphaned/ }));
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(removeFn).toHaveBeenCalledWith([
      "/cache/a-AAAAAAAA-py3.13",
      "/cache/c-CCCCCCCC-py3.13",
    ]);
  });

  /// A single orphan needs no bulk affordance -- its own checkbox is
  /// already one click.
  it("does not offer bulk removal for a single orphan", () => {
    state.venvs = [venv()];
    render(<VenvSection />);
    expect(screen.queryByRole("button", { name: /Remove all/ })).toBeNull();
  });
});

describe("selecting a stale virtualenv", () => {
  const staleVenv = () =>
    venv({
      path: "/cache/old-project-BBBBBBBB-py3.13",
      project: "old-project",
      state: "live",
    });

  /// Settings already had "Also allow removing stale virtualenvs", and
  /// `remove_venvs` already honoured it as `policy.allow_stale`. The
  /// checkbox did not, so turning the setting on changed nothing the
  /// user could see.
  it("is selectable once the setting allows it", () => {
    state.venvs = [staleVenv()];
    state.idle = new Map([["/cache/old-project-BBBBBBBB-py3.13", 60 * 60 * 24 * 400]]);
    render(<VenvSection />);
    expect(screen.getByText("stale")).toBeTruthy();
    const box = screen.getByLabelText("Select old-project virtualenv");
    expect(box.hasAttribute("disabled")).toBe(false);
  });

  /// The gate is GONE. This asserts it stays gone: a stale virtualenv is
  /// selectable regardless of any setting, because manual removal is not
  /// where a staleness threshold needs the user's permission.
  it("is selectable regardless of any setting", () => {
    state.venvs = [staleVenv()];
    state.idle = new Map([["/cache/old-project-BBBBBBBB-py3.13", 60 * 60 * 24 * 400]]);
    render(<VenvSection />);
    const box = screen.getByLabelText("Select old-project virtualenv");
    expect(box.hasAttribute("disabled")).toBe(false);
  });

  /// A live venv is never removable, at either layer.
  it("never offers a live virtualenv, even with the setting on", () => {
    state.venvs = [venv({ path: "/cache/live-CCCCCCCC-py3.13", project: "live", state: "live" })];
    state.idle = new Map([["/cache/live-CCCCCCCC-py3.13", 60]]);
    render(<VenvSection />);
    const box = screen.getByLabelText(/live virtualenv cannot be removed/);
    expect(box.hasAttribute("disabled")).toBe(true);
  });

  /// An orphan needs no setting: its project is gone, which is a fact
  /// rather than a judgement.
  it("always offers an orphan", () => {
    state.venvs = [venv()];
    render(<VenvSection />);
    const box = screen.getByLabelText("Select hello-world-delivery virtualenv");
    expect(box.hasAttribute("disabled")).toBe(false);
  });
});

describe("selection during removal", () => {
  /// Same defect as the artifacts page: a blanket reset after the await
  /// discarded anything ticked mid-flight, and unticked rows that FAILED
  /// -- which are the ones still needing attention.
  it("keeps a selection made while the removal was running", async () => {
    state.venvs = [
      venv({ path: "/cache/a-AAAAAAAA-py3.13", project: "a" }),
      venv({ path: "/cache/b-BBBBBBBB-py3.13", project: "b" }),
    ];
    state.sizes = new Map([
      ["/cache/a-AAAAAAAA-py3.13", 1_000],
      ["/cache/b-BBBBBBBB-py3.13", 2_000],
    ]);

    let settle: (v: { path: string; error: string | null }[]) => void = () => {};
    removeFn.mockImplementationOnce(() => new Promise((res) => { settle = res; }));

    render(<VenvSection />);
    fireEvent.click(screen.getByLabelText("Select a virtualenv"));
    fireEvent.click(screen.getByRole("button", { name: /^Remove 1/ }));
    fireEvent.click(screen.getByRole("button", { name: /^Remove$/ }));
    expect(removeFn).toHaveBeenCalled();

    fireEvent.click(screen.getByLabelText("Select b virtualenv"));

    await act(async () => {
      settle([{ path: "/cache/a-AAAAAAAA-py3.13", error: null }]);
    });

    await waitFor(() =>
      expect((screen.getByLabelText("Select a virtualenv") as HTMLInputElement).checked).toBe(false),
    );
    expect((screen.getByLabelText("Select b virtualenv") as HTMLInputElement).checked).toBe(true);
  });

  it("keeps the selection for a virtualenv that could not be removed", async () => {
    state.venvs = [venv({ path: "/cache/a-AAAAAAAA-py3.13", project: "a" })];
    state.sizes = new Map([["/cache/a-AAAAAAAA-py3.13", 1_000]]);
    removeFn.mockResolvedValueOnce([
      { path: "/cache/a-AAAAAAAA-py3.13", error: "it is not an orphan" },
    ]);

    render(<VenvSection />);
    fireEvent.click(screen.getByLabelText("Select a virtualenv"));
    fireEvent.click(screen.getByRole("button", { name: /^Remove 1/ }));
    fireEvent.click(screen.getByRole("button", { name: /^Remove$/ }));

    await waitFor(() => expect(removeFn).toHaveBeenCalled());
    await waitFor(() =>
      expect(screen.getByRole("button", { name: /^Remove 1/ })).toBeTruthy(),
    );
    expect((screen.getByLabelText("Select a virtualenv") as HTMLInputElement).checked).toBe(true);
  });
});

/// #431: the virtualenv rows showed a size and no date, so "is this old
/// enough to remove" had no answer on screen. The idle time was already
/// being fetched -- it drove the stale badge -- and never displayed.
describe("age on a virtualenv row", () => {
  it("shows how long ago the virtualenv was last written", () => {
    state.venvs = [venv()];
    state.idle = new Map([
      ["/cache/hello-world-delivery-AAAAAAAA-py3.13", 60 * 60 * 24 * 270],
    ]);
    render(<VenvSection />);
    expect(screen.getByText("9 months ago")).toBeTruthy();
  });

  /// Unknown must not read as brand new -- that would hide exactly the
  /// venvs worth removing. Same rule the size column follows.
  it("does not claim an unmeasured virtualenv was written just now", () => {
    state.venvs = [venv()];
    state.idle = new Map();
    render(<VenvSection />);
    expect(screen.queryByText("just now")).toBeNull();
  });
});

/// #431: the section rendered NOTHING for the 9-40 seconds the scan
/// takes on a real machine, which is indistinguishable from having no
/// virtualenvs -- and is exactly how it was reported.
describe("while the scan is running", () => {
  it("says it is looking, rather than rendering nothing", () => {
    state.venvs = [];
    state.loading = true;
    const { container } = render(<VenvSection />);
    expect(screen.getByText(/looking for poetry virtualenvs/i)).toBeTruthy();
    expect(container.textContent).not.toBe("");
  });

  /// Once the scan has ANSWERED, an empty list really does mean none --
  /// and then saying nothing is right.
  it("renders nothing once an empty result is known", () => {
    state.venvs = [];
    state.loading = false;
    const { container } = render(<VenvSection />);
    expect(container.textContent).toBe("");
  });

  it("shows the rows once they arrive", () => {
    state.venvs = [venv()];
    state.loading = false;
    render(<VenvSection />);
    expect(screen.getByText("hello-world-delivery")).toBeTruthy();
    expect(screen.queryByText(/looking for poetry/i)).toBeNull();
  });
});

/// #421: a bare "measuring…" on a pass that took 73 seconds is
/// indistinguishable from being stuck. Sizing is chunked now, so there
/// is real progress to report.
describe("measuring progress", () => {
  it("says how many chunks have answered", () => {
    state.venvs = [venv()];
    state.measuring = true;
    state.pending = 2;
    state.total = 4;
    render(<VenvSection />);
    expect(screen.getByText(/measuring — 2 of 4/)).toBeTruthy();
  });

  it("says nothing about measuring once it is done", () => {
    state.venvs = [venv()];
    state.measuring = false;
    render(<VenvSection />);
    expect(screen.queryByText(/measuring/i)).toBeNull();
  });

  /// #956. `measuring` is false for a REJECTED chunk as much as for a
  /// finished one, so the byte figures appeared -- on two destructive
  /// buttons -- summed over a map missing the failed chunk's paths.
  ///
  /// The section must keep its rows (the venv scan succeeded; every row is
  /// real) while withholding the figures that are now a floor, and say
  /// which.
  it("withholds the byte figures and says why after a failed size chunk", () => {
    state.venvs = [
      venv(),
      venv({ path: "/cache/other-BBBBBBBB-py3.13", project: "other" }),
    ];
    // Only the first row's size landed; the chunk holding the second
    // rejected.
    state.sizes = new Map([
      ["/cache/hello-world-delivery-AAAAAAAA-py3.13", 1_000_000_000],
    ]);
    state.measuring = false;
    state.pending = 0;
    state.total = 3;
    state.sizesFailed = 1;
    render(<VenvSection />);

    // Says the sizes are short, rather than letting the figure silently
    // not appear.
    expect(screen.getByText(/could not be measured/)).toBeTruthy();
    // The bulk button keeps its count and loses its understated size.
    const btn = screen.getByRole("button", { name: /Remove all 2 orphaned/ });
    expect(btn.textContent).not.toContain("·");
    // And the rows are still there: one failure must not blank a list.
    expect(screen.getByText("other")).toBeTruthy();
  });
});

/// #747: the project walk hit its cap during ordinary use, and a
/// truncated walk yields an undersized set of live project roots --
/// exactly the condition under which a live virtualenv is misreported as
/// an orphan. The backend now withholds the verdict, reporting `unknown`
/// instead, and this is where that has to become visible.
describe("a project scan that did not finish", () => {
  it("says the answer is incomplete rather than showing a short list", () => {
    state.venvs = [venv({ state: "unknown" })];
    render(<VenvSection />);
    expect(screen.getByRole("status").textContent).toMatch(/did not finish/i);
  });

  /// The whole point of the suppression: an unchecked venv must not be
  /// offered for deletion. Its checkbox is the control that would do it.
  it("does not offer an unchecked virtualenv for removal", () => {
    state.venvs = [venv({ state: "unknown" })];
    render(<VenvSection />);
    const box = screen.getByRole("checkbox") as HTMLInputElement;
    expect(box.disabled).toBe(true);
    expect(box.getAttribute("aria-label")).toMatch(/did not finish/i);
  });

  /// An idle time must not age `unknown` into `stale`, which IS
  /// removable -- that would reintroduce the risk by the back door.
  it("does not let a long idle time make it removable", () => {
    const year = 416 * 24 * 60 * 60;
    state.venvs = [venv({ state: "unknown" })];
    state.idle = new Map([["/cache/hello-world-delivery-AAAAAAAA-py3.13", year]]);
    render(<VenvSection />);
    expect((screen.getByRole("checkbox") as HTMLInputElement).disabled).toBe(true);
    expect(screen.queryByText("stale")).toBeNull();
  });

  /// A complete scan is the ordinary case and must stay silent -- a
  /// banner on every run would train the user to ignore it.
  it("says nothing when the scan finished", () => {
    state.venvs = [venv({ state: "orphaned" })];
    render(<VenvSection />);
    expect(screen.queryByRole("status")).toBeNull();
  });
});

/// #846: the sharpest case in the issue, because it was already fixed
/// once here and the fix did not carry.
///
/// The comment on `useVenvs`' destructure above records separating
/// `isLoading` from empty, on a scan measured at 26 seconds walking 28,144
/// directories. `isError` was not separated, so a rejection still left
/// `venvs` at `[]` and the `return null` below still removed the entire
/// section -- its orphan count, its bulk-remove button, all of it. Worse
/// than the 26-second wait that comment is about, because a wait ends:
/// `staleTime: 30 * 60 * 1000` with `refetchOnWindowFocus: false` pinned
/// the failure for HALF AN HOUR with nothing re-running it.
describe("VenvSection when the scan fails", () => {
  /// The negative assertion is the defect. The section did not say
  /// anything wrong -- it ceased to exist.
  it("does not vanish", () => {
    state.failed = true;
    state.venvs = [];
    const { container } = render(<VenvSection />);
    expect(container.querySelector("section")).toBeTruthy();
    expect(screen.getByRole("alert")).toBeTruthy();
    expect(screen.getByText(/could not look for Poetry virtualenvs/i)).toBeTruthy();
  });

  /// Keeps the HEADING, unlike the empty case. A bare error panel floating
  /// under the artifact list would not say what failed; the section is how
  /// the reader knows this is about virtualenvs and not about the build
  /// output above it.
  it("still names itself, so the failure is attributable", () => {
    state.failed = true;
    render(<VenvSection />);
    expect(screen.getByText("Poetry virtualenvs")).toBeTruthy();
  });

  it("reports the scan's own refusal", () => {
    state.failed = true;
    render(<VenvSection />);
    expect(screen.getByText(/operation not permitted/i)).toBeTruthy();
  });

  /// The pairing that makes `retry: false` acceptable, and the only thing
  /// that can end a 30-minute pinned failure.
  it("offers a retry the user can press", () => {
    state.failed = true;
    render(<VenvSection />);
    fireEvent.click(screen.getByRole("button", { name: /try again/i }));
    expect(refetchFn).toHaveBeenCalled();
  });

  /// The orphan count is the number the user acts on, and its absence is
  /// not a zero. Said out loud because "could not scan" invites reading
  /// the missing count as nothing to do.
  it("says the orphan count is unknown rather than zero", () => {
    state.failed = true;
    render(<VenvSection />);
    expect(screen.getByText(/unknown — not zero/i)).toBeTruthy();
    expect(screen.queryByText(/0 orphaned/)).toBeNull();
  });

  /// Offers no removal. A bulk-remove button over a scan that refused
  /// would act on a selection assembled from nothing.
  it("offers no removal over a set it could not read", () => {
    state.failed = true;
    render(<VenvSection />);
    expect(screen.queryByRole("button", { name: /^Remove/ })).toBeNull();
    expect(screen.queryByRole("checkbox")).toBeNull();
  });

  /// The error arm must come BEFORE the `return null`, because a rejection
  /// leaves `venvs` at `[]` and that line was reached first. This gives
  /// the component exactly the state that used to take the wrong branch.
  it("prefers the error to the silent empty return", () => {
    state.failed = true;
    state.venvs = [];
    const { container } = render(<VenvSection />);
    expect(container.textContent).not.toBe("");
  });
});

/// #852: the confirmation stated a false reassurance as fact.
///
/// It read "Every one of these belongs to a project directory that no longer
/// exists" -- unconditionally, while `isRemovable` admits `stale` too. And
/// this component's own doc comment defines the difference: "An orphan is a
/// FACT -- the path that made it is gone… A stale venv is a JUDGEMENT about
/// a project that STILL EXISTS."
///
/// So for every stale row the dialog asserted the opposite of the truth, at
/// the exact moment the gate's own justification says the intent is formed:
/// "Ticking a specific row and confirming in a dialog IS the intent."
describe("VenvSection's confirmation wording", () => {
  const STALE_SECS = 90 * 24 * 60 * 60;

  const orphan = () => venv({ path: "/cache/gone-AAAA-py3.13", project: "gone" });
  const stale = () =>
    venv({ path: "/cache/here-BBBB-py3.13", project: "here", state: "live", source: "/code/here" });

  /// `stale` is a DISPLAY state derived from the idle time, so the fixture
  /// has to supply one past the threshold -- a `state: "stale"` venv with no
  /// idle time renders as `live` and cannot be selected at all.
  const ageStale = () => {
    state.idle = new Map([["/cache/here-BBBB-py3.13", STALE_SECS + 1]]);
  };

  const openWith = (rows: ReturnType<typeof venv>[]) => {
    state.venvs = rows;
    render(<VenvSection />);
    for (const r of rows) {
      fireEvent.click(screen.getByLabelText(new RegExp(`Select ${r.project} virtualenv`)));
    }
    fireEvent.click(screen.getByRole("button", { name: /^Remove \d/ }));
    return screen.getByRole("dialog");
  };

  it("does not claim a stale venv's project is gone", () => {
    ageStale();
    const dialog = openWith([stale()]);
    expect(within(dialog).queryByText(/no longer exists/i)).toBeNull();
    expect(within(dialog).getByText(/still exists/i)).toBeTruthy();
  });

  /// The judgement has to be REVIEWABLE, not merely flagged: the threshold
  /// and the cost are what let the user weigh it.
  it("names the threshold and what removing a stale venv costs", () => {
    ageStale();
    const dialog = openWith([stale()]);
    expect(within(dialog).getByText(/90 days/i)).toBeTruthy();
    expect(within(dialog).getByText(/poetry install/i)).toBeTruthy();
  });

  /// The orphan sentence is still made, unchanged, when it is true -- the
  /// fix is a split, not a retreat into vagueness.
  it("still says an orphan's project is gone", () => {
    const dialog = openWith([orphan()]);
    expect(within(dialog).getByText(/no longer exists/i)).toBeTruthy();
    expect(within(dialog).queryByText(/still exists/i)).toBeNull();
  });

  /// A MIXED selection is the case the old copy was most wrong about: it
  /// said "every one" over a set where only some qualified.
  it("says both things, each counted, for a mixed selection", () => {
    ageStale();
    const dialog = openWith([orphan(), stale()]);
    expect(within(dialog).getByText(/1 of these belong to a project directory that no longer exists/i)).toBeTruthy();
    expect(within(dialog).getByText(/1 of these belong to a project that still exists/i)).toBeTruthy();
    // And never "every one", which is the word that made it a false claim.
    expect(within(dialog).queryByText(/every one of these/i)).toBeNull();
  });

  /// #957. The threshold is now the user's setting, and the two things it
  /// governs -- the LABEL (and so the checkbox) and the SENTENCE the
  /// confirmation asserts -- have to move with it together.
  ///
  /// They did not. `VenvSection` held its own `const STALE_SECS = 90 days`
  /// while `remove_venvs` enforced `RemovalPolicy::stale_days` from
  /// `stale_venv_days`, and the dialog said "90 days" as a string literal.
  /// With the setting exposed, the two failures that produces are:
  ///
  /// 1. above 90, a row reads Stale with an enabled checkbox and the
  ///    delete is REFUSED -- a confirmed action that silently does
  ///    nothing;
  /// 2. below 90, rows the backend would delete are never offered.
  describe("the threshold is the user's setting (#957)", () => {
    /// Idle for 120 days: stale at a 90-day threshold, live at 180. One
    /// fixture, two verdicts, which is the disagreement itself.
    const DAY = 24 * 60 * 60;
    const ageDays = (days: number) => {
      state.idle = new Map([["/cache/here-BBBB-py3.13", days * DAY]]);
    };

    it("labels a row against the configured threshold, not a constant 90", () => {
      state.staleVenvDays = 180;
      ageDays(120);
      state.venvs = [stale()];
      render(<VenvSection />);
      // 120 days idle is inside a 180-day threshold, so this is LIVE --
      // and under the old hardcoded 90 it read Stale.
      expect(screen.getByText("live")).toBeTruthy();
      expect(screen.queryByText("stale")).toBeNull();
    });

    /// The half that matters most, because it is the one that ends in a
    /// refused delete: the checkbox must not offer what the backend will
    /// not remove. `isRemovable` admits `stale` and nothing else among
    /// live projects, so the label IS the gate.
    it("does not offer a row the backend would refuse to remove", () => {
      state.staleVenvDays = 180;
      ageDays(120);
      state.venvs = [stale()];
      render(<VenvSection />);
      // A non-removable row's checkbox names the REFUSAL rather than the
      // selection -- the distinction the component draws deliberately, so
      // a disabled control says why.
      const box = screen.getByLabelText(/here virtualenv cannot be removed/) as HTMLInputElement;
      expect(box.disabled).toBe(true);
    });

    /// And the other direction: lowering the threshold must OFFER rows the
    /// backend would now delete, rather than leaving them unselectable.
    it("offers a row the lowered threshold makes stale", () => {
      state.staleVenvDays = 30;
      ageDays(45);
      state.venvs = [stale()];
      render(<VenvSection />);
      expect(screen.getByText("stale")).toBeTruthy();
      const box = screen.getByLabelText(/Select here virtualenv/) as HTMLInputElement;
      expect(box.disabled).toBe(false);
    });

    /// The confirmation sentence, which was a string literal. This is the
    /// last place the figure is read before the click, so a dialog saying
    /// 90 while the app deletes on 180 is the "rubber stamp with wrong
    /// words on it" this component's own comment warns about.
    it("states the configured threshold in the confirmation, not 90", () => {
      state.staleVenvDays = 180;
      ageDays(200);
      const dialog = openWith([stale()]);
      expect(within(dialog).getByText(/180 days/i)).toBeTruthy();
      expect(within(dialog).queryByText(/90 days/i)).toBeNull();
    });

    /// A stored 0 is what EVERY existing install holds, because
    /// `UiPrefs::stale_venv_days` defaults to it. Read as a threshold it
    /// would mean "everything is stale" and reclassify every venv on every
    /// machine -- the failure `poll::stale_venv_days`' comment forbids in
    /// so many words: "a stored 0 from a bad write must not reclassify the
    /// whole cache." It must resolve to 90.
    it("reads a stored 0 as the default rather than as zero days", () => {
      state.staleVenvDays = 0;
      ageDays(45);
      state.venvs = [stale()];
      render(<VenvSection />);
      // 45 days is short of 90, so a correct reading leaves this live. A
      // literal 0 threshold would call it stale, and its checkbox would
      // go live with it.
      expect(screen.getByText("live")).toBeTruthy();
      const box = screen.getByLabelText(/here virtualenv cannot be removed/) as HTMLInputElement;
      expect(box.disabled).toBe(true);
    });

    /// And no noise on the happy path: the default threshold must keep
    /// labelling exactly what it labelled before this change. A fix that
    /// moved the 90-day boundary would be a silent reclassification of
    /// every machine's cache.
    it("still labels at 90 days when nothing is configured", () => {
      state.staleVenvDays = 0;
      ageDays(120);
      state.venvs = [stale()];
      render(<VenvSection />);
      expect(screen.getByText("stale")).toBeTruthy();
      const box = screen.getByLabelText(/Select here virtualenv/) as HTMLInputElement;
      expect(box.disabled).toBe(false);
    });
  });
});

/// Progress through the 26-second directory walk (#1151).
describe("VenvSection while the walk runs", () => {
  beforeEach(() => {
    state.loading = true;
    state.walk = undefined;
  });

  it("says only that it is looking before the first event", () => {
    // "0 directories" reads as a walk that has stalled, which is the
    // opposite of what an unstarted scan means.
    render(<VenvSection />);
    expect(screen.getByText(/Looking for Poetry virtualenvs…/)).toBeTruthy();
    expect(screen.queryByText(/0 directories/)).toBeNull();
  });

  it("reports a rising directory count, not a percentage", () => {
    // The walk discovers the tree as it goes and has no denominator, so
    // a fabricated percentage would stick or jump. This scan is
    // measured at 26 seconds over 28,144 directories.
    state.walk = { found: 41, visited: 12_400, max_dirs: 2_000_000 };
    render(<VenvSection />);
    expect(screen.getByText(/12,400 directories, 41 projects/)).toBeTruthy();
    expect(screen.queryByText(/%/)).toBeNull();
  });

  it("gets the singular right for one project", () => {
    state.walk = { found: 1, visited: 30, max_dirs: 2_000_000 };
    render(<VenvSection />);
    expect(screen.getByText(/1 project\b/)).toBeTruthy();
  });
});

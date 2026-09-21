import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  ClaudeMdAdviceCheck,
  ClaudeMdAdviceFinding,
  ClaudeMdAdviceFreshness,
  ClaudeMdAdviceReport,
  ClaudeMdAdviceResult,
} from "@/types/pr";

const copyFn = vi.hoisted(() => vi.fn(() => Promise.resolve(null as string | null)));
const toastFns = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
const refetchFn = vi.hoisted(() => vi.fn());
const state = vi.hoisted(() => ({
  data: undefined as unknown,
  isError: false,
  error: undefined as unknown,
  isFetching: false,
}));

vi.mock("../api/hooks", () => ({
  useClaudeMdAdvice: () => ({
    data: state.data,
    isError: state.isError,
    error: state.error,
    isFetching: state.isFetching,
    refetch: refetchFn,
  }),
}));
vi.mock("sonner", () => ({ toast: toastFns }));
vi.mock("../lib/clipboard", () => ({ copyText: copyFn }));

import { ClaudeMdAdvicePanel } from "./ClaudeMdAdvicePanel";
import { useFilters } from "@/store/filters";

const REPO = "/home/octocat/hello-world";

/// A filter store with a key per `View`, as `useActiveFilters` requires.
const EMPTY = {
  "my-prs": {},
  "to-review": {},
  worktrees: {},
  branches: {},
  docker: {},
  artifacts: {},
  packages: {},
  "claude-md": {},
  "claude-code": {},
  "pr-stats": {},
  repositories: {},
  "system-health": {},
} as const;

const finding = (over: Partial<ClaudeMdAdviceFinding> = {}): ClaudeMdAdviceFinding => ({
  check: "imports",
  severity: "problem",
  subject: { kind: "claudeMd", path: `${REPO}/CLAUDE.md`, scope: "repo", section: null },
  evidence: [
    { at: { kind: "file", path: `${REPO}/CLAUDE.md`, line: null }, measured: "`@./x.md`: file not found" },
  ],
  finding: "`@./x.md` in the file does not resolve: file not found",
  brief: "## the brief\nSubject: `x`\nChange only the file named above. Show me the diff and let me decide.\n",
  ...over,
});

/// The wire shape since #1293: the report WRAPPED in its freshness.
///
/// The helper builds the wrapper so every case below exercises the shape
/// the command actually returns. The panel reads `data.report` and makes
/// no currency claim of its own -- #1290 to #1292 own that -- so the
/// default freshness here is the honest one for a report that was just
/// computed.
const report = (over: Partial<ClaudeMdAdviceReport> = {}): ClaudeMdAdviceResult => ({
  report: {
    repo: REPO,
    findings: [],
    checks: [{ check: "imports", run: { state: "ran", findings: 0 } }],
    brief: "# CLAUDE.md advice\n",
    ...over,
  },
  freshness: { state: "fresh", recomputed: true },
  computedAt: "2026-01-01T00:00:00Z",
});

function open(activePath?: string) {
  const onSelectFile = vi.fn();
  render(<ClaudeMdAdvicePanel repo={REPO} activePath={activePath} onSelectFile={onSelectFile} />);
  fireEvent.click(screen.getByRole("button", { name: /show advice/i }));
  return onSelectFile;
}

beforeEach(() => {
  copyFn.mockClear();
  copyFn.mockResolvedValue(null);
  toastFns.success.mockClear();
  toastFns.error.mockClear();
  refetchFn.mockClear();
  state.data = undefined;
  state.isError = false;
  state.error = undefined;
  state.isFetching = false;
  // The advice panel renders on the `claude-md` view, which is where its
  // grouping preference is stored.
  useFilters.setState({ filtersByView: { ...EMPTY }, view: "claude-md" });
});

/// Pick a grouping through the control the user has, not by writing the
/// store: the round trip through `setFilter` is half of what is being
/// tested.
function group(label: string) {
  fireEvent.change(screen.getByLabelText("Group:"), {
    target: { value: label },
  });
}

describe("ClaudeMdAdvicePanel", () => {
  /// The panel renders the same report whichever freshness it arrived
  /// with, and claims NOTHING about currency either way (#1293).
  ///
  /// The panel is deliberately unchanged by the cache: #1290 to #1292
  /// own the advice surface and will render the freshness. What must
  /// hold in the meantime is that serving from cache did not silently
  /// change what the panel shows, AND that the panel does not start
  /// asserting currency it was never given -- an "up to date" badge
  /// added here over an `unverified` result would be the exact lie the
  /// three states exist to prevent.
  ///
  /// Every member of the union is constructed, so a member removed or
  /// renamed on the wire fails to compile here rather than silently
  /// ceasing to be handled.
  it.each<[string, ClaudeMdAdviceFreshness]>([
    ["computed now", { state: "fresh", recomputed: true }],
    ["verified current", { state: "fresh", recomputed: false }],
    ["from cache, stale", { state: "cached", stale: true }],
    ["from cache, could not verify", { state: "unverified", reason: "x: Permission denied", recomputed: false }],
  ])("renders the finding the same way when the report is %s", (_label, freshness) => {
    state.data = { ...report({ findings: [finding()] }), freshness };
    open();
    expect(screen.getByText(/does not resolve/)).toBeTruthy();
    // No currency claim, in either direction. The panel neither says
    // the report is up to date nor says it is out of date, because it
    // does not render the freshness at all yet.
    expect(screen.queryByText(/up to date|out of date|current/i)).toBeNull();
  });

  /// Collapsed by default, and nothing is claimed while closed.
  it("is collapsed by default", () => {
    state.data = report();
    render(<ClaudeMdAdvicePanel repo={REPO} activePath={undefined} onSelectFile={vi.fn()} />);
    expect(screen.queryByText(/nothing found/)).toBeNull();
    expect(screen.getByRole("button", { name: /show advice/i }).getAttribute("aria-expanded")).toBe(
      "false",
    );
  });

  /// "Not measured yet" is a skeleton. Never "no advice", and never
  /// "nothing found": a query in flight has established nothing.
  it("shows a skeleton while the report is in flight", () => {
    state.data = undefined;
    open();
    expect(screen.getByText("Checking…")).toBeTruthy();
    expect(screen.queryByText(/nothing found/)).toBeNull();
    expect(screen.queryByText(/could not/i)).toBeNull();
  });

  /// A check that could not run is stated in the producer's own words,
  /// the findings that exist are qualified as a floor, and the clean
  /// sentence is withheld (#1042: Unknown is not a pass).
  it("renders an unknown check's reason and the at-least notice, not nothing found", () => {
    state.data = report({
      checks: [
        { check: "imports", run: { state: "unknown", reason: "the repository could not be listed" } },
      ],
    });
    open();
    expect(screen.getAllByText(/the repository could not be listed/).length).toBeGreaterThan(0);
    expect(screen.getByText(/could not check/)).toBeTruthy();
    expect(screen.getByRole("alert").textContent).toContain("at least the findings");
    expect(screen.getByRole("alert").textContent).toContain("1 of 1 checks could not run");
    expect(screen.queryByText(/nothing found/)).toBeNull();
  });

  /// Only a run in which every check completed may say this.
  it("says nothing found only when every check ran and found nothing", () => {
    state.data = report();
    open();
    expect(screen.getByText(/1 check ran; nothing found\./)).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  /// The backend ranks; the panel renders in wire order. Fed OUT of rank
  /// order, the DOM must still match the wire -- re-sorting here would be
  /// a second ordering to keep in step with `Severity::rank`.
  it("renders findings in wire order without re-sorting", () => {
    state.data = report({
      findings: [
        finding({ severity: "advice", finding: "second by rank, first on the wire" }),
        finding({ severity: "problem", finding: "first by rank, second on the wire" }),
      ],
      checks: [{ check: "imports", run: { state: "ran", findings: 2 } }],
    });
    open();
    const rows = screen.getAllByRole("listitem").map((li) => li.textContent ?? "");
    const first = rows.findIndex((t) => t.includes("first on the wire"));
    const second = rows.findIndex((t) => t.includes("second on the wire"));
    expect(first).toBeGreaterThanOrEqual(0);
    expect(second).toBeGreaterThan(first);
    // And severity is stated in text, not only in colour.
    expect(screen.getByText("[advice]")).toBeTruthy();
    expect(screen.getByText("[problem]")).toBeTruthy();
  });

  /// The brief goes to the clipboard VERBATIM -- it is for an agent, and
  /// the panel never renders or rewrites it -- and a copy that could not
  /// happen says so rather than doing nothing.
  it("copies the brief verbatim and toasts the failure", async () => {
    const f = finding();
    state.data = report({
      findings: [f],
      checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
    });
    open();
    fireEvent.click(screen.getByRole("button", { name: "Copy brief" }));
    expect(copyFn).toHaveBeenCalledWith(f.brief);
    await waitFor(() => expect(toastFns.success).toHaveBeenCalled());

    copyFn.mockResolvedValue("This window has no clipboard access.");
    fireEvent.click(screen.getByRole("button", { name: "Copy brief" }));
    await waitFor(() =>
      expect(toastFns.error).toHaveBeenCalledWith(expect.stringMatching(/could not copy/i), {
        description: "This window has no clipboard access.",
      }),
    );
  });

  /// "Copy all briefs" copies the report's own combined document. The
  /// panel never concatenates briefs itself.
  it("copies the report brief for all briefs", () => {
    state.data = report({
      findings: [finding()],
      checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
      brief: "# the whole document",
    });
    open();
    fireEvent.click(screen.getByRole("button", { name: "Copy all briefs" }));
    expect(copyFn).toHaveBeenCalledWith("# the whole document");
  });

  /// A file-subject row is navigation: `aria-current` when it is the file
  /// on screen, the attribute ABSENT otherwise (never `"false"`), and a
  /// click selects the file.
  it("marks a file-subject button current only when it is the active file", () => {
    state.data = report({
      findings: [
        finding({ subject: { kind: "claudeMd", path: `${REPO}/CLAUDE.md`, scope: "repo", section: null } }),
        finding({
          subject: { kind: "claudeMd", path: `${REPO}/docs/CLAUDE.md`, scope: "repo", section: null },
        }),
      ],
      checks: [{ check: "imports", run: { state: "ran", findings: 2 } }],
    });
    const onSelectFile = open(`${REPO}/docs/CLAUDE.md`);
    const root = screen.getByRole("button", { name: "CLAUDE.md" });
    const docs = screen.getByRole("button", { name: "docs/CLAUDE.md" });
    expect(docs.getAttribute("aria-current")).toBe("true");
    expect(root.hasAttribute("aria-current")).toBe(false);
    fireEvent.click(root);
    expect(onSelectFile).toHaveBeenCalledWith(`${REPO}/CLAUDE.md`);
    // Copy is an action, not a toggle.
    for (const b of screen.getAllByRole("button", { name: "Copy brief" })) {
      expect(b.hasAttribute("aria-pressed")).toBe(false);
    }
  });

  /// Every check on the wire renders: one finding per check, and a
  /// coverage row per check whose label is visible. Built as a `Record`
  /// over the wire type, so a variant added to `ClaudeMdAdviceCheck`
  /// fails to compile here until it has a label, the same way
  /// `CHECK_LABEL` in the panel does.
  it("renders a finding and a labelled coverage row for every check", () => {
    const LABEL: Record<ClaudeMdAdviceCheck, string> = {
      imports: "imports",
      toolchain: "toolchain coverage",
      transcripts: "sessions",
      gaps: "missing subdirectory files",
      placement: "placement",
      rot: "rot",
      skills: "skills",
      shape: "content shape",
    };
    const checks = Object.keys(LABEL) as ClaudeMdAdviceCheck[];
    state.data = report({
      findings: checks.map((check) => finding({ check, finding: `a ${check} finding` })),
      checks: checks.map((check) => ({
        check,
        run: { state: "unknown", reason: `${check} could not run` },
      })),
    });
    open();
    for (const check of checks) {
      expect(screen.getByText(`a ${check} finding`)).toBeTruthy();
      expect(screen.getAllByText(LABEL[check], { exact: true }).length).toBeGreaterThan(0);
      expect(screen.getAllByText(new RegExp(`${check} could not run`)).length).toBeGreaterThan(0);
    }
    expect(screen.getByRole("alert").textContent).toContain(
      `${checks.length} of ${checks.length} checks could not run`,
    );
  });

  /// A locator with no line prints no line.
  it("prints no line number when the evidence has none", () => {
    state.data = report({
      findings: [finding()],
      checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
    });
    open();
    expect(screen.queryByText(/CLAUDE\.md:\d/)).toBeNull();
  });

  /// A rejected command is the whole run failing, which is a different
  /// claim from a run that came back short, and the one a retry can help.
  it("shows an error with a retry when the command was rejected", () => {
    state.isError = true;
    state.error = "the blocking task panicked";
    open();
    expect(screen.getByText(/Could not check these files/)).toBeTruthy();
    expect(screen.getByText("the blocking task panicked")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /try again/i }));
    expect(refetchFn).toHaveBeenCalled();
    expect(screen.queryByText(/nothing found/)).toBeNull();
  });

  /// The flat list is the DEFAULT, and it is a default rather than a
  /// value written on first render: a store key nobody chose would
  /// persist and outlive a change to what the default should be. The
  /// flat list is the backend's own ranking, the one arrangement in
  /// which a row's position means exactly one thing.
  it("defaults to the flat list without writing the store", () => {
    state.data = report({
      findings: [finding()],
      checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
    });
    open();
    expect((screen.getByLabelText("Group:") as HTMLSelectElement).value).toBe("none");
    expect(useFilters.getState().filtersByView["claude-md"].adviceGrouping).toBeUndefined();
  });

  /// THE case #1291 pins: grouping by file must not let a critical
  /// finding in a late-sorting file fall below a quiet one in a file that
  /// sorts first. Alphabetical grouping inverts these, and that is
  /// grouping silently becoming a re-ranking.
  it("does not bury a problem in zzz.md under advice in aaa.md when grouped by file", () => {
    state.data = report({
      findings: [
        finding({
          severity: "problem",
          subject: { kind: "claudeMd", path: `${REPO}/zzz.md`, scope: "repo", section: null },
          finding: "the critical one",
        }),
        finding({
          severity: "advice",
          subject: { kind: "claudeMd", path: `${REPO}/aaa.md`, scope: "repo", section: null },
          finding: "the quiet one",
        }),
      ],
      checks: [{ check: "imports", run: { state: "ran", findings: 2 } }],
    });
    open();
    group("file");
    const text = document.body.textContent ?? "";
    expect(text.indexOf("the critical one")).toBeGreaterThanOrEqual(0);
    expect(text.indexOf("the critical one")).toBeLessThan(text.indexOf("the quiet one"));
    expect(text.indexOf("zzz.md")).toBeLessThan(text.indexOf("aaa.md"));
  });

  /// Within a group the backend's order is kept exactly. Two findings
  /// about the SAME file, fed out of rank order, must still render in
  /// wire order -- the panel does not re-sort inside a group either.
  it("keeps wire order within a group", () => {
    state.data = report({
      findings: [
        finding({ severity: "advice", finding: "first on the wire" }),
        finding({ severity: "problem", finding: "second on the wire" }),
      ],
      checks: [{ check: "imports", run: { state: "ran", findings: 2 } }],
    });
    open();
    group("file");
    const text = document.body.textContent ?? "";
    expect(text.indexOf("first on the wire")).toBeLessThan(text.indexOf("second on the wire"));
  });

  /// All three `Subject` kinds appear under the file view, each labelled
  /// as what it is. Dropping a `Skill` for not being a CLAUDE.md loses
  /// the skills producer's whole output; rendering a `Directory` as a
  /// file offers a click that opens nothing.
  it("groups all three subject kinds by file, each labelled", () => {
    state.data = report({
      findings: [
        finding({ subject: { kind: "claudeMd", path: `${REPO}/CLAUDE.md`, scope: "repo", section: null }, finding: "about the file" }),
        finding({ subject: { kind: "directory", path: `${REPO}/src` }, finding: "about the directory" }),
        finding({
          subject: { kind: "skill", path: `${REPO}/.claude/skills/verify/SKILL.md`, name: "verify" },
          finding: "about the skill",
        }),
      ],
      checks: [{ check: "imports", run: { state: "ran", findings: 3 } }],
    });
    open();
    group("file");
    // Every finding is still on screen -- none went ungrouped.
    expect(screen.getByText("about the file")).toBeTruthy();
    expect(screen.getByText("about the directory")).toBeTruthy();
    expect(screen.getByText("about the skill")).toBeTruthy();
    // And each group is headed by what its subject IS.
    const headings = screen.getAllByRole("heading").map((h) => h.textContent ?? "");
    expect(headings.some((h) => h.startsWith("CLAUDE.md"))).toBe(true);
    // The trailing slash is the signal that no file exists there yet.
    expect(headings.some((h) => h.startsWith("src/"))).toBe(true);
    // A skill carries the name it is invoked with, not just its path.
    expect(headings.some((h) => h.includes("skill: verify"))).toBe(true);
  });

  /// #846 in the view organised by check: a check that could not run and
  /// a check that ran clean must not read the same. The first names its
  /// obstacle in the producer's own words; the second produces no group
  /// at all and is spoken for by the coverage sentence.
  it("distinguishes a check that could not run from one that found nothing", () => {
    state.data = report({
      findings: [finding({ check: "imports", finding: "an imports finding" })],
      checks: [
        { check: "imports", run: { state: "ran", findings: 1 } },
        { check: "rot", run: { state: "ran", findings: 0 } },
        { check: "skills", run: { state: "unknown", reason: "the skills directory could not be listed" } },
      ],
    });
    open();
    group("check");
    const headings = screen.getAllByRole("heading").map((h) => h.textContent ?? "");
    // The check that could not run has a group, and the reason sits
    // INSIDE that group rather than only in the notice at the top -- the
    // group is what the reader is looking at when they organise by
    // check, and a heading with nothing under it reads as a clean run.
    expect(headings.some((h) => h.startsWith("skills"))).toBe(true);
    const skillsGroup = screen
      .getAllByRole("heading")
      .find((h) => (h.textContent ?? "").startsWith("skills"))?.parentElement;
    expect(skillsGroup?.textContent).toContain("could not check");
    expect(skillsGroup?.textContent).toContain("the skills directory could not be listed");
    // The check that ran clean has no group -- it found nothing, which is
    // a different claim and not a failure to report.
    expect(headings.some((h) => h.startsWith("rot"))).toBe(false);
    // And the clean sentence is still withheld: this run was partial.
    expect(screen.queryByText(/nothing found/)).toBeNull();
  });

  /// A report whose ONLY content is checks that could not run still
  /// offers the by-check view, and that view still shows them. This is
  /// the report the grouping is most worth switching to, and the one
  /// where letting an empty group vanish would show a blank panel.
  it("shows unknown checks under the by-check view when there are no findings at all", () => {
    state.data = report({
      checks: [
        { check: "imports", run: { state: "unknown", reason: "imports could not run" } },
        { check: "skills", run: { state: "unknown", reason: "skills could not run" } },
      ],
    });
    open();
    group("check");
    // Each reason under its own check's heading, not only in the notice.
    for (const [check, why] of [
      ["imports", "imports could not run"],
      ["skills", "skills could not run"],
    ]) {
      const section = screen
        .getAllByRole("heading")
        .find((h) => (h.textContent ?? "").startsWith(check))?.parentElement;
      expect(section?.textContent).toContain(why);
      expect(section?.textContent).toContain("could not check");
    }
    expect(screen.queryByText(/nothing found/)).toBeNull();
  });

  /// The choice persists the way every other view preference does: into
  /// `filtersByView` under the view the panel renders on, through the
  /// generic `setFilter`. A `useState` here would be forgotten on every
  /// navigation away.
  it("persists the grouping into the per-view filter store", () => {
    state.data = report({
      findings: [finding()],
      checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
    });
    open();
    group("check");
    expect(useFilters.getState().filtersByView["claude-md"].adviceGrouping).toBe("check");
    // And it is stored per view, not leaked across them.
    expect(useFilters.getState().filtersByView["to-review"].adviceGrouping).toBeUndefined();
  });

  /// The other half of the round trip: a grouping already in the store is
  /// what the panel opens with.
  it("honours a grouping already in the store", () => {
    useFilters.setState({
      filtersByView: { ...EMPTY, "claude-md": { adviceGrouping: "file" } },
      view: "claude-md",
    });
    state.data = report({
      findings: [finding({ finding: "already grouped" })],
      checks: [{ check: "imports", run: { state: "ran", findings: 1 } }],
    });
    open();
    expect((screen.getByLabelText("Group:") as HTMLSelectElement).value).toBe("file");
    expect(screen.getAllByRole("heading").length).toBeGreaterThan(0);
    expect(screen.getByText("already grouped")).toBeTruthy();
  });

  /// Grouping is an ARRANGEMENT, not a filter: every finding the flat
  /// list shows is still shown in both groupings. A grouping that hid
  /// anything would be a filter wearing a grouping's clothes.
  it("shows every finding in all three arrangements", () => {
    const texts = ["one", "two", "three"];
    state.data = report({
      findings: [
        finding({ check: "imports", finding: "one", subject: { kind: "claudeMd", path: `${REPO}/a.md`, scope: "repo", section: null } }),
        finding({ check: "shape", finding: "two", subject: { kind: "directory", path: `${REPO}/src` } }),
        finding({ check: "skills", finding: "three", subject: { kind: "skill", path: `${REPO}/s/SKILL.md`, name: "s" } }),
      ],
      checks: [
        { check: "imports", run: { state: "ran", findings: 1 } },
        { check: "shape", run: { state: "ran", findings: 1 } },
        { check: "skills", run: { state: "ran", findings: 1 } },
      ],
    });
    open();
    for (const g of ["none", "check", "file"]) {
      group(g);
      for (const t of texts) expect(screen.getByText(t)).toBeTruthy();
    }
  });
});

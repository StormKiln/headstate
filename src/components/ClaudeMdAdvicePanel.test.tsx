import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ClaudeMdAdviceFinding, ClaudeMdAdviceReport } from "@/types/pr";

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

const REPO = "/home/octocat/hello-world";

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

const report = (over: Partial<ClaudeMdAdviceReport> = {}): ClaudeMdAdviceReport => ({
  repo: REPO,
  findings: [],
  checks: [{ check: "imports", run: { state: "ran", findings: 0 } }],
  brief: "# CLAUDE.md advice\n",
  ...over,
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
});

describe("ClaudeMdAdvicePanel", () => {
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
});

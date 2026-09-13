import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EcosystemReport, Outdated, ProjectReport } from "@/types/pr";

const state = vi.hoisted(() => ({
  repo: "/code/app" as string | undefined,
  reports: [] as ProjectReport[],
  loading: false,
}));
const markdownFn = vi.hoisted(() => vi.fn(() => Promise.resolve("# md")));
// Typed via the mock's signature rather than an unused parameter, so
// `copyFn.mock.calls[0][0]` is a string without eslint objecting to an
// argument nothing reads.
const copyFn = vi.hoisted(() =>
  vi.fn<(text: string) => Promise<string | null>>(() => Promise.resolve(null)),
);

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock("../api/hooks", () => ({
  usePackages: () => ({
    data: state.reports,
    isLoading: state.loading,
    isError: false,
    error: null,
    refetch: vi.fn(),
  }),
}));
vi.mock("../api/tauri", () => ({ packagesMarkdown: markdownFn }));
vi.mock("../lib/clipboard", () => ({ copyText: copyFn }));
vi.mock("../store/filters", () => ({ useActiveFilters: () => ({ repo: state.repo }) }));

import { PackagesPage } from "./PackagesPage";

const pkg = (name: string, bump: Outdated["bump"]): Outdated => ({
  name,
  current: "1.0.0",
  latest: "2.0.0",
  bump,
  ecosystem: "npm",
  manifest: "package.json",
});

/// One ecosystem's report, wrapped as the single unnamed project a
/// one-project repository produces.
const report = (over: Partial<EcosystemReport> = {}): ProjectReport => ({
  path: "/code/app",
  label: "",
  reports: [{ ecosystem: "npm", outdated: [], error: null, ...over }],
});

beforeEach(() => {
  state.repo = "/code/app";
  state.reports = [];
  state.loading = false;
  markdownFn.mockClear();
  copyFn.mockClear();
  copyFn.mockResolvedValue(null);
});

describe("PackagesPage", () => {
  it("asks for a repository before checking anything", () => {
    state.repo = undefined;
    render(<PackagesPage />);
    expect(screen.getByText(/Choose a repository/)).toBeTruthy();
  });

  /// The filters are the feature. "Patch only" must mean patch.
  it("filters by the size of the jump", () => {
    state.reports = [
      report({ outdated: [pkg("a", "patch"), pkg("b", "minor"), pkg("c", "major")] }),
    ];
    render(<PackagesPage />);

    // Default is patch-and-minor.
    expect(screen.getByText("a")).toBeTruthy();
    expect(screen.getByText("b")).toBeTruthy();
    expect(screen.queryByText("c")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Patch only" }));
    expect(screen.getByText("a")).toBeTruthy();
    expect(screen.queryByText("b")).toBeNull();
  });

  /// A list that silently omits what nothing could classify looks
  /// complete when it is not -- and those are precisely the packages
  /// nobody can vouch for.
  it("says how many packages the filter is hiding as uncomparable", () => {
    state.reports = [report({ outdated: [pkg("a", "patch"), pkg("odd", "unknown")] })];
    render(<PackagesPage />);
    expect(screen.queryByText("odd")).toBeNull();
    expect(screen.getByText(/1 package had versions this could not compare/)).toBeTruthy();
  });

  it("shows uncomparable packages under Everything", () => {
    state.reports = [report({ outdated: [pkg("odd", "unknown")] })];
    render(<PackagesPage />);
    fireEvent.click(screen.getByRole("button", { name: "Everything" }));
    expect(screen.getByText("odd")).toBeTruthy();
  });

  /// A tool that could not run must NEVER render as "up to date". That
  /// is the worst available answer, because nobody investigates good
  /// news.
  it("states a failed check instead of showing an empty list", () => {
    state.reports = [report({ error: "npm was not found" })];
    render(<PackagesPage />);
    expect(screen.getByText(/npm was not found/)).toBeTruthy();
    expect(screen.queryByText(/Nothing matches this filter/)).toBeNull();
  });

  /// The Cargo case specifically (#954).
  ///
  /// `error` was hardcoded `None` for Cargo, Swift and Terraform because
  /// they answer from FILES rather than a command, so there was no spawn
  /// failure to report -- and every file-read failure beneath them was
  /// therefore unrepresentable. An unreadable `Cargo.lock` dropped every
  /// crate through the missing-resolved-version branch and the page read
  /// "up to date". This pins the render for the newly-reachable message:
  /// the error line shows, and NO empty-list copy appears for it.
  it("states an unreadable Cargo lock file instead of reading as up to date", () => {
    state.reports = [
      report({
        ecosystem: "cargo",
        outdated: [],
        error: "1 file could not be read, so this list is incomplete: /r/Cargo.lock (Permission denied)",
      }),
    ];
    render(<PackagesPage />);
    expect(screen.getByText(/Cargo.lock/)).toBeTruthy();
    expect(screen.queryByText(/Nothing matches this filter/)).toBeNull();
  });

  it("says when there is genuinely nothing to do", () => {
    state.reports = [report()];
    render(<PackagesPage />);
    expect(screen.getByText(/Nothing matches this filter/)).toBeTruthy();
  });

  /// The markdown IS the deliverable of v1.
  it("copies markdown for the active filter", async () => {
    state.reports = [report({ outdated: [pkg("a", "patch")] })];
    render(<PackagesPage />);
    fireEvent.click(screen.getByRole("button", { name: "Copy markdown" }));
    await waitFor(() => expect(copyFn).toHaveBeenCalled());
    expect(markdownFn).toHaveBeenCalledWith("/code/app", state.reports, "minor");
  });

  it("reports a clipboard failure rather than claiming success", async () => {
    copyFn.mockResolvedValue("This window has no clipboard access.");
    state.reports = [report({ outdated: [pkg("a", "patch")] })];
    render(<PackagesPage />);
    fireEvent.click(screen.getByRole("button", { name: "Copy markdown" }));
    const { toast } = await import("sonner");
    await waitFor(() => expect(toast.error).toHaveBeenCalled());
  });
});

describe("PackagesPage grouping, sorting, and Claudify", () => {
  const project = (label: string, names: string[]): ProjectReport => ({
    path: `/code/app/${label}`,
    label,
    reports: [
      {
        ecosystem: "npm",
        outdated: names.map((n) => pkg(n, "patch")),
        error: null,
      },
    ],
  });

  /// A repository with several projects must say which is which: the
  /// same package at two versions in two projects is otherwise two
  /// indistinguishable rows.
  it("groups a multi-project repository by project", () => {
    state.reports = [project("frontend", ["a"]), project("backend", ["b"])];
    render(<PackagesPage />);
    expect(screen.getByRole("button", { name: /frontend/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /backend/ })).toBeTruthy();
  });

  /// ...and a single-project repo must NOT grow a heading that says
  /// nothing.
  it("adds no project heading to a single-project repository", () => {
    state.reports = [report({ outdated: [pkg("a", "patch")] })];
    render(<PackagesPage />);
    expect(screen.queryByRole("button", { name: /repository root/ })).toBeNull();
  });

  it("collapses a project group", () => {
    state.reports = [project("frontend", ["only-in-frontend"]), project("backend", ["b"])];
    render(<PackagesPage />);
    expect(screen.getByText("only-in-frontend")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /frontend/ }));
    expect(screen.queryByText("only-in-frontend")).toBeNull();
  });

  it("filters by package name", () => {
    state.reports = [report({ outdated: [pkg("react", "patch"), pkg("vite", "patch")] })];
    render(<PackagesPage />);
    fireEvent.change(screen.getByLabelText(/Filter packages by name/), {
      target: { value: "rea" },
    });
    expect(screen.getByText("react")).toBeTruthy();
    expect(screen.queryByText("vite")).toBeNull();
  });

  it("sorts by biggest jump by default, and by name on request", () => {
    // Both within the DEFAULT filter, so the ordering is what the test
    // observes rather than the filter. `zeta` is the bigger jump and
    // sorts first by size; `alpha` sorts first by name.
    state.reports = [
      report({ outdated: [pkg("zeta", "minor"), pkg("alpha", "patch")] }),
    ];
    render(<PackagesPage />);
    const names = () =>
      screen.getAllByText(/^(zeta|alpha)$/).map((el) => el.textContent);
    expect(names()[0]).toBe("zeta");

    fireEvent.change(screen.getByLabelText(/Sort updates/), { target: { value: "name" } });
    expect(names()[0]).toBe("alpha");
  });

  /// Claudify is an INSTRUCTION where Copy markdown is a REPORT. The
  /// prompt has to say what to do and, critically, to verify rather than
  /// assume -- an update applied without running the tests produces a
  /// change nobody can trust.
  it("builds a prompt that asks for verification", async () => {
    state.reports = [report({ outdated: [pkg("a", "patch")] })];
    render(<PackagesPage />);
    fireEvent.click(screen.getByRole("button", { name: "Claudify" }));
    await waitFor(() => expect(copyFn).toHaveBeenCalled());
    const prompt = copyFn.mock.calls[0][0];
    expect(prompt).toMatch(/run that project's own tests/i);
    expect(prompt).toMatch(/ACTUALLY chose/);
    expect(prompt).toMatch(/Do not update anything that is not listed/i);
  });
  /// A new ecosystem needs a label or the group header renders blank.
  /// `ECOSYSTEM_LABEL` is a total `Record<Ecosystem, string>`, so the
  /// compiler catches a MISSING one -- this pins that the label reads
  /// as a person would name it, and that a Cargo row reaches the page
  /// at all.
  it("names the Cargo ecosystem and shows its rows", () => {
    state.reports = [
      {
        path: "/code/app",
        label: "src-tauri",
        reports: [
          {
            ecosystem: "cargo",
            outdated: [
              {
                name: "tauri-plugin-log",
                current: "2.9.0",
                latest: "2.9.1",
                bump: "patch",
                ecosystem: "cargo",
                manifest: "Cargo.toml [dependencies]",
              },
            ],
            error: null,
          },
        ],
      },
    ];
    render(<PackagesPage />);
    expect(screen.getByText("Cargo")).toBeTruthy();
    expect(screen.getByText("tauri-plugin-log")).toBeTruthy();
    expect(screen.getByText(/2\.9\.0\s*→\s*2\.9\.1/)).toBeTruthy();
  });
});

/// #494: the wizard was handed every outdated package regardless of the
/// bump filter. A repository showing 122 "Patch and minor" opened a
/// modal listing all 153, and select-all selected all 153 -- so there
/// was no way to act on just what had been filtered to.
describe("the update wizard's contents", () => {
  const mixed = () => [
    report({
      outdated: [pkg("patchy", "patch"), pkg("minory", "minor"), pkg("majory", "major")],
    }),
  ];

  it("offers only what the filter is showing", async () => {
    state.reports = mixed();
    render(<PackagesPage />);

    // Default filter excludes major.
    fireEvent.click(screen.getByRole("button", { name: /update in worktree/i }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("patchy")).toBeTruthy();
    expect(within(dialog).getByText("minory")).toBeTruthy();
    expect(within(dialog).queryByText("majory")).toBeNull();
  });

  it("offers everything once the filter admits everything", async () => {
    state.reports = mixed();
    render(<PackagesPage />);
    fireEvent.click(screen.getByRole("button", { name: /everything/i }));
    fireEvent.click(screen.getByRole("button", { name: /update in worktree/i }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("majory")).toBeTruthy();
  });

  /// The count on the button and the rows in the modal describe one
  /// set. They were derived separately from the same filter, which is
  /// how they could disagree without anything looking wrong.
  it("counts on the button what the modal will list", async () => {
    state.reports = mixed();
    render(<PackagesPage />);
    expect(screen.getByText(/^2 updates$/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /update in worktree/i }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getAllByRole("checkbox")).toHaveLength(2);
  });

});

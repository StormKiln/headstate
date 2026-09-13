import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ClaudeFile, ClaudeMdScan, ImportNode } from "@/types/pr";

const copyFn = vi.hoisted(() => vi.fn(() => Promise.resolve(null as string | null)));
const state = vi.hoisted(() => ({
  repo: "/code/app" as string | undefined,
  files: [] as ClaudeFile[],
  // What the scan could not read (#972). Separate from `files` in the
  // harness because they are separate facts on the wire: a scan can return
  // files AND a shortfall, and the page has to render both.
  unreadableDirs: [] as string[],
  unreadableFiles: [] as string[],
  loading: false,
  text: "# hello" as string | undefined,
  // #846: the two queries fail independently. The scan failing means the
  // LIST is unknown; the read failing means one file could not be opened
  // while the list beside it is fine.
  failed: false,
  textFailed: false,
}));

// The explicit retries the `retry: false` on both hooks is paired with
// (#846) -- the rule `useStatsBoard` states.
const refetchFn = vi.hoisted(() => vi.fn());
const refetchTextFn = vi.hoisted(() => vi.fn());

vi.mock("../api/hooks", () => ({
  useClaudeMd: () => ({
    // A `Scan`, not a bare array (#972). The bare array was what made the
    // page unable to tell "none" from "could not look".
    data: {
      files: state.files,
      unreadable_dirs: state.unreadableDirs,
      unreadable_files: state.unreadableFiles,
      skipped_dirs: 0,
    } satisfies ClaudeMdScan,
    isLoading: state.loading,
    isError: state.failed,
    error: "could not read the repository",
    refetch: refetchFn,
  }),
  useClaudeMdText: () => ({
    data: state.text,
    isLoading: false,
    isError: state.textFailed,
    error: "no such file or directory",
    refetch: refetchTextFn,
  }),
}));
vi.mock("../store/filters", () => ({ useActiveFilters: () => ({ repo: state.repo }) }));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock("../lib/clipboard", () => ({ copyText: copyFn }));

import { ClaudeMdPage } from "./ClaudeMdPage";

const node = (over: Partial<ImportNode> = {}): ImportNode => ({
  raw: "@shared.md",
  path: "/code/app/shared.md",
  bytes: 400,
  tokens: 100,
  problem: null,
  unreadable: false,
  children: [],
  ...over,
});

const file = (over: Partial<ClaudeFile> = {}): ClaudeFile => ({
  path: "/code/app/CLAUDE.md",
  bytes: 2000,
  tokens: 500,
  total_tokens: 500,
  total_partial: false,
  imports: [],
  ...over,
});

beforeEach(() => {
  copyFn.mockClear();
  copyFn.mockResolvedValue(null);
  state.repo = "/code/app";
  state.files = [];
  state.unreadableDirs = [];
  state.unreadableFiles = [];
  state.loading = false;
  state.text = "# hello";
  state.failed = false;
  state.textFailed = false;
  refetchFn.mockClear();
  refetchTextFn.mockClear();
});

describe("ClaudeMdPage", () => {
  it("asks for a repository first", () => {
    state.repo = undefined;
    render(<ClaudeMdPage />);
    expect(screen.getByText(/Choose a repository/)).toBeTruthy();
  });

  it("says so when a repository has none", () => {
    render(<ClaudeMdPage />);
    expect(screen.getByText(/No CLAUDE.md files/)).toBeTruthy();
  });

  /// Every token figure is an ESTIMATE -- chars/4, not a tokeniser --
  /// and a number labelled "tokens" that is not measured is exactly the
  /// confidently-wrong figure this app refuses to ship.
  it("labels every token count as an estimate", () => {
    state.files = [file()];
    render(<ClaudeMdPage />);
    expect(screen.getByText(/est\. tokens/)).toBeTruthy();
  });

  /// The number that matters: a small file pulling in a large tree.
  it("states the whole-tree cost when imports add to it", () => {
    state.files = [file({ tokens: 500, total_tokens: 4000, imports: [node()] })];
    render(<ClaudeMdPage />);
    expect(screen.getByText(/4,000 est\. tokens with imports/)).toBeTruthy();
  });

  /// Two equal numbers printed side by side read as a mistake.
  it("does not repeat the total when there are no imports", () => {
    state.files = [file({ tokens: 500, total_tokens: 500 })];
    render(<ClaudeMdPage />);
    expect(screen.queryByText(/with imports/)).toBeNull();
  });

  /// A broken import must be NAMED. Dropping it makes the tree look
  /// complete when it is not.
  it("shows a broken import rather than omitting it", () => {
    state.files = [
      file({ imports: [node({ raw: "@gone.md", path: null, problem: "file not found" })] }),
    ];
    render(<ClaudeMdPage />);
    expect(screen.getByText("@gone.md")).toBeTruthy();
    expect(screen.getByText("file not found")).toBeTruthy();
  });

  /// A cycle is a bug in the user's own config, and this view is the
  /// only thing that will surface it.
  it("names a circular import", () => {
    state.files = [file({ imports: [node({ problem: "circular import" })] })];
    render(<ClaudeMdPage />);
    expect(screen.getByText("circular import")).toBeTruthy();
  });

  /// Imports are transitive, so the tree must nest rather than flatten.
  it("renders nested imports", () => {
    state.files = [
      file({ imports: [node({ raw: "@a.md", children: [node({ raw: "@leaf.md" })] })] }),
    ];
    render(<ClaudeMdPage />);
    expect(screen.getByText("@a.md")).toBeTruthy();
    expect(screen.getByText("@leaf.md")).toBeTruthy();
  });

  /// #846's exact copy must be unreachable on a scan that could not look.
  ///
  /// #846 gave this page a correct `isError` arm ordered BEFORE the empty
  /// arm. That fix is sound and untouched -- but the arm can only fire on
  /// `isError`, and `scan_claude_md` could only ever return `Ok`. So a read
  /// failure arrived as an empty list, the empty arm was reached first, and
  /// the page said "No CLAUDE.md files in this repository" about a file the
  /// user can see on disk (#972). `emptyStateGuard.test.ts` records that its
  /// own `RepoPickerSidebar` check was passing on a COMMENT rather than the
  /// behaviour, so this asserts the sentence is absent rather than that some
  /// error copy is present.
  it("does not say there are no CLAUDE.md files when the scan reports unreadable entries", () => {
    state.files = [];
    state.unreadableFiles = ["/code/app/CLAUDE.md (Permission denied)"];
    render(<ClaudeMdPage />);
    expect(screen.queryByText(/No CLAUDE.md files in this repository/)).toBeNull();
    expect(screen.getByText(/could not be read/)).toBeTruthy();
  });

  /// The path is NAMED. A count says something is wrong and nothing about
  /// where to look; a `chmod` needs the path.
  it("names the path it could not read", () => {
    state.files = [];
    state.unreadableDirs = ["/code/app/packages (Permission denied)"];
    render(<ClaudeMdPage />);
    expect(screen.getByText(/\/code\/app\/packages/)).toBeTruthy();
  });

  /// A partial scan offers NO retry, and is not reported as a failed scan.
  ///
  /// The decision #951 made for `RepoPickerSidebar` and wrote into
  /// `PartialScanNotice`: `isError` is a rejection of the whole command,
  /// while this is a walk that RAN and came back short. A second identical
  /// walk will not read what the first could not, so a "Try again" button
  /// would promise something it cannot deliver.
  it("offers no retry for a scan that ran and came back short", () => {
    state.files = [];
    state.unreadableFiles = ["/code/app/CLAUDE.md (Permission denied)"];
    render(<ClaudeMdPage />);
    expect(screen.queryByRole("button", { name: /try again/i })).toBeNull();
    expect(refetchFn).not.toHaveBeenCalled();
  });

  /// The `isError` arm still fires, and still DOES offer a retry.
  ///
  /// The new arm is a third state, not a replacement: a rejected command is
  /// a different claim from a walk that came back short, and it is the one
  /// a retry can genuinely help.
  it("still reports a rejected scan as a failure, with a retry", () => {
    state.failed = true;
    render(<ClaudeMdPage />);
    expect(screen.getByText(/Could not look for CLAUDE.md files/)).toBeTruthy();
    expect(screen.queryByText(/No CLAUDE.md files in this repository/)).toBeNull();
  });

  /// A scan that read EVERYTHING still gets to say the repository has none.
  ///
  /// The false-positive half: if the new arm swallowed the ordinary empty
  /// case, a repository that genuinely has no CLAUDE.md would look broken.
  it("still says a repository has none when the scan read everything", () => {
    state.files = [];
    state.unreadableDirs = [];
    state.unreadableFiles = [];
    render(<ClaudeMdPage />);
    expect(screen.getByText(/No CLAUDE.md files in this repository/)).toBeTruthy();
  });

  /// One unreadable file must NOT blank the list.
  ///
  /// The files that did read are real. A partial answer labelled partial
  /// beats both a silent truncation and an error page -- the rule
  /// `transcript.rs`'s `is_partial()` states, and the same trade
  /// `ArtifactsPage` makes. Rendered through `PartialScanNotice`, the
  /// component #951 added, rather than a second banner of our own.
  it("keeps the files that read and states the shortfall beside them", () => {
    state.files = [file({ path: "/code/app/CLAUDE.md" })];
    state.unreadableFiles = ["/code/app/nested/CLAUDE.md (Permission denied)"];
    render(<ClaudeMdPage />);
    expect(screen.getByText("CLAUDE.md")).toBeTruthy();
    expect(screen.getByText(/may not be all of them/)).toBeTruthy();
    // And the path, which is what a `chmod` actually needs.
    expect(screen.getByText(/nested\/CLAUDE.md/)).toBeTruthy();
  });

  /// A total missing an unmeasured import is a FLOOR, not a value.
  ///
  /// Reuses the app's existing "at least" idiom -- the one `ArtifactsPage`
  /// and `WorktreesPage` already put in front of a partially-measured size
  /// -- rather than inventing a second phrasing for the same fact.
  it("prefixes a partial token total with at least", () => {
    state.files = [
      file({
        tokens: 500,
        total_tokens: 500,
        total_partial: true,
        imports: [node({ tokens: 0, problem: "could not read: Permission denied", unreadable: true })],
      }),
    ];
    render(<ClaudeMdPage />);
    expect(screen.getByText(/at least .* with imports/)).toBeTruthy();
  });

  /// An EXACT total is not hedged.
  ///
  /// "at least" everywhere would mean nothing anywhere.
  it("does not say at least when every import was counted", () => {
    state.files = [
      file({ tokens: 500, total_tokens: 4000, total_partial: false, imports: [node()] }),
    ];
    render(<ClaudeMdPage />);
    expect(screen.queryByText(/at least/)).toBeNull();
    expect(screen.getByText(/4,000 est\. tokens with imports/)).toBeTruthy();
  });

  it("renders the selected file's content", () => {
    state.files = [file()];
    state.text = "# The rules";
    render(<ClaudeMdPage />);
    expect(screen.getByText("The rules")).toBeTruthy();
  });
});

describe("ClaudeMdPage browser", () => {
  /// The reported problem: "the file paths now are so long that it is
  /// impossible to tell what they are". A truncated absolute path eats
  /// exactly the middle segment that distinguishes one file from
  /// another.
  it("shows the path relative to the repository, not the absolute one", () => {
    state.repo = "/code/app";
    state.files = [file({ path: "/code/app/services/api/CLAUDE.md" })];
    render(<ClaudeMdPage />);
    expect(screen.getByText("services/api/")).toBeTruthy();
    expect(screen.getByText("CLAUDE.md")).toBeTruthy();
    expect(screen.queryByText("/code/app/services/api/CLAUDE.md")).toBeNull();
  });

  it("copies the relative path from the context menu", async () => {
    state.repo = "/code/app";
    state.files = [file({ path: "/code/app/services/api/CLAUDE.md" })];
    render(<ClaudeMdPage />);

    fireEvent.contextMenu(screen.getByRole("button", { name: /CLAUDE.md/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Copy relative path" }));
    await waitFor(() => expect(copyFn).toHaveBeenCalledWith("services/api/CLAUDE.md"));
  });

  it("copies the absolute path from the context menu", async () => {
    state.repo = "/code/app";
    state.files = [file({ path: "/code/app/CLAUDE.md" })];
    render(<ClaudeMdPage />);

    fireEvent.contextMenu(screen.getByRole("button", { name: /CLAUDE.md/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Copy absolute path" }));
    await waitFor(() => expect(copyFn).toHaveBeenCalledWith("/code/app/CLAUDE.md"));
  });

  /// A menu that can only be closed by choosing something is a trap.
  it("dismisses without copying", () => {
    state.repo = "/code/app";
    state.files = [file()];
    render(<ClaudeMdPage />);
    fireEvent.contextMenu(screen.getByRole("button", { name: /CLAUDE.md/ }));
    fireEvent.click(screen.getByRole("button", { name: "Close menu" }));
    expect(screen.queryByRole("menuitem")).toBeNull();
    expect(copyFn).not.toHaveBeenCalled();
  });
});

/// #846: neither query may fail silently.
///
/// This page's own doc comment stakes its design on "a wrong render costs
/// a confused reader", and it carried two of the issue's four surfaces --
/// a scan reported as "No CLAUDE.md files in this repository", and a read
/// reported as an entirely blank pane.
describe("ClaudeMdPage when a query fails", () => {
  /// The negative assertion is the defect. With `data = []` on a
  /// rejection the empty-state branch was reached first and claimed the
  /// repository had none -- so an error arm placed after it would be
  /// unreachable in exactly the case it exists for.
  it("does not claim the repository has no CLAUDE.md files", () => {
    state.failed = true;
    state.files = [];
    render(<ClaudeMdPage />);
    expect(screen.queryByText(/no CLAUDE\.md files in this repository/i)).toBeNull();
    expect(screen.getByRole("alert")).toBeTruthy();
    expect(screen.getByText(/could not look for CLAUDE\.md files/i)).toBeTruthy();
  });

  it("reports the scan's own words and offers a retry", () => {
    state.failed = true;
    render(<ClaudeMdPage />);
    expect(screen.getByText(/could not read the repository/i)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /try again/i }));
    expect(refetchFn).toHaveBeenCalled();
  });

  /// The worst single state in #846: not a wrong message but NO message.
  ///
  /// The content pane was two arms and a `null`. On a rejected read
  /// `textLoading` is false and `text` is undefined, so both arms failed
  /// and the chain ended at `null` -- the file browser on the left, the
  /// correct file highlighted blue, and an entirely empty pane on the
  /// right. No error, no retry. A file renamed between the scan and the
  /// click (`staleTime: 30_000` makes that a real window) presented as an
  /// app that had failed to draw.
  it("never renders a blank pane when the read fails", () => {
    state.files = [file({ path: "/code/app/CLAUDE.md" })];
    state.text = undefined;
    state.textFailed = true;
    render(<ClaudeMdPage />);
    expect(screen.getByRole("alert")).toBeTruthy();
    expect(screen.getByText(/could not read this file/i)).toBeTruthy();
    expect(screen.getByText(/no such file or directory/i)).toBeTruthy();
  });

  /// NAMES the file, because the list beside this pane is still correct
  /// and still highlighting a row: without the path, an error here reads
  /// as though the whole page failed rather than this one read.
  it("names the file it could not read, and keeps the list beside it", () => {
    state.files = [
      file({ path: "/code/app/CLAUDE.md" }),
      file({ path: "/code/app/services/api/CLAUDE.md" }),
    ];
    state.text = undefined;
    state.textFailed = true;
    render(<ClaudeMdPage />);
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("/code/app/CLAUDE.md");
    // Both browser rows survive: the scan worked, only the read failed.
    expect(screen.getAllByRole("button", { name: /CLAUDE\.md/ }).length).toBe(2);
  });

  it("retries only the read, not the whole scan", () => {
    state.files = [file({ path: "/code/app/CLAUDE.md" })];
    state.text = undefined;
    state.textFailed = true;
    render(<ClaudeMdPage />);
    fireEvent.click(screen.getByRole("button", { name: /try again/i }));
    expect(refetchTextFn).toHaveBeenCalled();
    expect(refetchFn).not.toHaveBeenCalled();
  });

  /// The chain's FINAL arm is a real state, not a fallback. Pinned
  /// separately from the error arm because "no text, no error" is the
  /// combination that used to reach the bare `null`.
  it("says what to do rather than rendering nothing when there is no text", () => {
    state.files = [file({ path: "/code/app/CLAUDE.md" })];
    state.text = undefined;
    state.textFailed = false;
    render(<ClaudeMdPage />);
    expect(screen.getByText(/choose a file to read it/i)).toBeTruthy();
  });
});

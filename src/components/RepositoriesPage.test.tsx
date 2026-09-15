import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { RepoFile, RepoTree } from "@/types/pr";

/// The repository browser's four outcomes, each asserted as its OWN
/// rendering (#1036).
///
/// The reason this is the bulk of the file: collapsing "we could not list
/// this directory" into "this directory is empty" is the bug #1036 exists
/// to prevent, and it is #846's exact shape -- a failed read rendering as
/// a confident empty answer. A test that only checked "something is on
/// screen" would pass against precisely that defect, so each arm asserts
/// the copy that distinguishes it AND asserts the others are absent.

const state = vi.hoisted(() => ({
  repo: "/code/app" as string | undefined,
  // The scan the page consults to notice a stale selection. `undefined`
  // is the pending-or-rejected state, which must NOT read as "gone".
  repos: [{ path: "/code/app", name: "app" }] as
    | { path: string; name: string }[]
    | undefined,
  scanFailed: false,
  // The two queries fail INDEPENDENTLY, as they do on the wire: the
  // listing failing means the directory is unknown, and the file failing
  // means one entry could not be opened while the listing beside it is
  // fine.
  tree: {
    path: "",
    entries: [
      { name: "src", path: "src", dir: true, symlink: false },
      { name: "README.md", path: "README.md", dir: false, symlink: false },
    ],
  } as RepoTree | undefined,
  treeLoading: false,
  treeFailed: false,
  file: {
    path: "README.md",
    size: 12,
    content: "# hello\n",
    truncated: false,
    binary: false,
  } as RepoFile | undefined,
  fileLoading: false,
  fileFailed: false,
  // The browser's position, which the page reads out of the store.
  repoPath: "",
  repoFile: undefined as string | undefined,
}));

const setRepoPath = vi.hoisted(() => vi.fn());
const setRepoFile = vi.hoisted(() => vi.fn());
const refetchTree = vi.hoisted(() => vi.fn());
const refetchFile = vi.hoisted(() => vi.fn());

vi.mock("../api/hooks", () => ({
  useWorktrees: () => ({
    data: state.repos,
    isError: state.scanFailed,
  }),
  useRepoTree: () => ({
    data: state.treeFailed ? undefined : state.tree,
    isLoading: state.treeLoading,
    isError: state.treeFailed,
    error: state.treeFailed ? new Error("fatal: not a git repository") : undefined,
    refetch: refetchTree,
  }),
  useRepoFile: () => ({
    data: state.fileFailed ? undefined : state.file,
    isLoading: state.fileLoading,
    isError: state.fileFailed,
    error: state.fileFailed ? new Error("No such file or directory") : undefined,
    refetch: refetchFile,
  }),
}));

vi.mock("@/store/filters", async (orig) => ({
  ...(await orig<Record<string, unknown>>()),
  useActiveFilters: () => ({ repo: state.repo }),
  useFilters: () => ({
    repoPath: state.repoPath,
    setRepoPath,
    repoFile: state.repoFile,
    setRepoFile,
  }),
}));

const { RepositoriesPage } = await import("./RepositoriesPage");

beforeEach(() => {
  state.repo = "/code/app";
  state.repos = [{ path: "/code/app", name: "app" }];
  state.scanFailed = false;
  state.tree = {
    path: "",
    entries: [
      { name: "src", path: "src", dir: true, symlink: false },
      { name: "README.md", path: "README.md", dir: false, symlink: false },
    ],
  };
  state.treeLoading = false;
  state.treeFailed = false;
  state.file = {
    path: "README.md",
    size: 12,
    content: "# hello\n",
    truncated: false,
    binary: false,
  };
  state.fileLoading = false;
  state.fileFailed = false;
  state.repoPath = "";
  state.repoFile = undefined;
  setRepoPath.mockClear();
  setRepoFile.mockClear();
  refetchTree.mockClear();
  refetchFile.mockClear();
});

describe("the repository browser's listing", () => {
  it("lists directories and files, and descends on a click", () => {
    render(<RepositoriesPage />);
    expect(screen.getByText("src")).toBeTruthy();
    expect(screen.getByText("README.md")).toBeTruthy();
    fireEvent.click(screen.getByText("src"));
    expect(setRepoPath).toHaveBeenCalledWith("src");
  });

  it("opens a file rather than descending when the row is a file", () => {
    render(<RepositoriesPage />);
    fireEvent.click(screen.getByText("README.md"));
    expect(setRepoFile).toHaveBeenCalledWith("README.md");
    // And it does NOT move the directory: the listing it was opened from
    // is what "back" returns to.
    expect(setRepoPath).not.toHaveBeenCalled();
  });

  /// Outcome 2: a listing that FAILED, and the thing it must not be.
  it("renders a failed listing as a failure, never as an empty directory", () => {
    state.treeFailed = true;
    render(<RepositoriesPage />);
    expect(screen.getByText(/could not list this directory/i)).toBeTruthy();
    // The message carries git's own words, because the three causes --
    // not a repository, a permission wall, git missing from a
    // GUI-launched app's PATH -- send the user to three different places.
    expect(screen.getByText(/not a git repository/i)).toBeTruthy();
    // #846, stated as an assertion: the empty copy must be absent.
    expect(screen.queryByText(/no tracked files/i)).toBeNull();
  });

  /// Outcome 3: a listing that SUCCEEDED and is empty. Distinct copy,
  /// and no error framing.
  it("renders a genuinely empty directory as its own answer", () => {
    state.tree = { path: "scratch", entries: [] };
    render(<RepositoriesPage />);
    expect(screen.getByText(/no tracked files/i)).toBeTruthy();
    expect(screen.queryByText(/could not list/i)).toBeNull();
  });

  it("offers a retry on a failed listing, because a vanished directory can come back", () => {
    state.treeFailed = true;
    render(<RepositoriesPage />);
    fireEvent.click(screen.getByRole("button", { name: /try again/i }));
    expect(refetchTree).toHaveBeenCalled();
  });

  /// The symlink split, which is the one part of this listing that is not
  /// a single treatment.
  ///
  /// Measured: 22 tracked symlinks across 38 repositories -- the entire
  /// population an index-based browser can display -- splitting 14 files
  /// / 6 directories / 2 broken. A single "symlinks are not followed"
  /// rule gives the DIRECTORY case a row that looks descendable and does
  /// nothing when clicked, which reads as broken. So the two are tested
  /// as two.
  it("disables a symlinked DIRECTORY row and says so on the row", () => {
    state.tree = {
      path: "",
      entries: [
        {
          name: "alias",
          path: "alias",
          dir: false,
          symlink: true,
          symlink_to_dir: true,
          target: "../shared",
        },
      ],
    };
    render(<RepositoriesPage />);
    // Said BEFORE the click, because there is no panel to say it after.
    expect(screen.getByText(/linked folder/i)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /alias/i }));
    expect(setRepoPath).not.toHaveBeenCalled();
    expect(setRepoFile).not.toHaveBeenCalled();
  });

  /// A symlinked FILE stays clickable precisely so it can explain itself
  /// in the panel, where `repo_file`'s own refusal is the explanation.
  it("leaves a symlinked FILE clickable so the panel can explain it", () => {
    state.tree = {
      path: "",
      entries: [
        {
          name: "vpc.tf",
          path: "vpc.tf",
          dir: false,
          symlink: true,
          symlink_to_dir: false,
          target: "../modules/vpc.tf",
        },
      ],
    };
    render(<RepositoriesPage />);
    fireEvent.click(screen.getByRole("button", { name: /vpc\.tf/i }));
    expect(setRepoFile).toHaveBeenCalledWith("vpc.tf");
  });

  /// Both kinds name the target. 14 of the 22 are shared Terraform
  /// module files, where what the link points at is exactly the thing the
  /// user opened the row to learn.
  it("names the target on both kinds of link", () => {
    state.tree = {
      path: "",
      entries: [
        {
          name: "alias",
          path: "alias",
          dir: false,
          symlink: true,
          symlink_to_dir: true,
          target: "../shared",
        },
        {
          name: "vpc.tf",
          path: "vpc.tf",
          dir: false,
          symlink: true,
          symlink_to_dir: false,
          target: "../modules/vpc.tf",
        },
      ],
    };
    render(<RepositoriesPage />);
    expect(screen.getByText(/→ \.\.\/shared/)).toBeTruthy();
    expect(screen.getByText(/→ \.\.\/modules\/vpc\.tf/)).toBeTruthy();
  });

  /// A broken link -- 2 of the 22 today -- is still shown, still labelled,
  /// and still names where it was pointing. That is what tells the user
  /// it is broken rather than missing.
  it("shows a broken link with its target, as a file rather than a folder", () => {
    state.tree = {
      path: "",
      entries: [
        {
          name: "dangling",
          path: "dangling",
          dir: false,
          symlink: true,
          symlink_to_dir: false,
          target: "nowhere/at/all",
        },
      ],
    };
    render(<RepositoriesPage />);
    expect(screen.getByText(/→ nowhere\/at\/all/)).toBeTruthy();
    expect(screen.queryByText(/linked folder/i)).toBeNull();
  });

  /// A link whose target could not be read still renders, with the label
  /// alone rather than a dangling arrow.
  it("renders a link with no readable target without a dangling arrow", () => {
    state.tree = {
      path: "",
      entries: [
        { name: "odd", path: "odd", dir: false, symlink: true, symlink_to_dir: false },
      ],
    };
    render(<RepositoriesPage />);
    expect(screen.getByText("link")).toBeTruthy();
    expect(screen.queryByText(/→/)).toBeNull();
  });
});

describe("the repository browser's file panel", () => {
  it("renders a text file's contents", () => {
    state.repoFile = "README.md";
    render(<RepositoriesPage />);
    expect(screen.getByText(/# hello/)).toBeTruthy();
  });

  /// Outcome 4: in the index and unreadable. Measured on the real
  /// corpus -- two tracked files whose symlink targets are gone -- so
  /// this is the first click's behaviour on some repositories.
  it("renders an unreadable file as a failure naming why, and keeps a way back", () => {
    state.repoFile = "ghost.txt";
    state.fileFailed = true;
    render(<RepositoriesPage />);
    expect(screen.getByText(/in the index but could not be read/i)).toBeTruthy();
    expect(screen.getByText(/no such file or directory/i)).toBeTruthy();
    // The LISTING is not invalidated: one unreadable entry is not
    // evidence the other 650 are wrong, so the way back is still there.
    expect(screen.getByRole("button", { name: /back to the listing/i })).toBeTruthy();
  });

  /// A binary is read perfectly well and is not text. NOT an error --
  /// the GitHub code view says exactly this about one -- and detected by
  /// NUL byte rather than by extension, which is why this `.txt` lands
  /// here at all.
  it("names a binary file rather than rendering it, and does not call it an error", () => {
    state.repoFile = "payload.txt";
    state.file = {
      path: "payload.txt",
      size: 4096,
      content: "",
      truncated: false,
      binary: true,
    };
    render(<RepositoriesPage />);
    expect(screen.getByText(/this is a binary file/i)).toBeTruthy();
    expect(screen.queryByText(/could not be read/i)).toBeNull();
    expect(screen.queryByText(/this file is empty/i)).toBeNull();
  });

  /// The third of the three read outcomes, and a real one: `.gitkeep`
  /// files appear across the corpus. It must render as neither a failure
  /// nor a binary.
  it("says an empty file is empty, distinctly from binary and from unreadable", () => {
    state.repoFile = ".gitkeep";
    state.file = {
      path: ".gitkeep",
      size: 0,
      content: "",
      truncated: false,
      binary: false,
    };
    render(<RepositoriesPage />);
    expect(screen.getByText(/this file is empty/i)).toBeTruthy();
    expect(screen.queryByText(/binary/i)).toBeNull();
    expect(screen.queryByText(/could not be read/i)).toBeNull();
  });

  /// The truncation is STATED. A window shown as if it were the whole
  /// file is worse than a refusal, and a reader who has scrolled to the
  /// bottom has already been misled.
  it("states the truncation when the bound bit, with live figures", () => {
    state.repoFile = "big.txt";
    state.file = {
      path: "big.txt",
      size: 300 * 1024,
      content: "x".repeat(256 * 1024),
      truncated: true,
      binary: false,
    };
    render(<RepositoriesPage />);
    const notice = screen.getByRole("status");
    expect(notice.textContent).toMatch(/showing the first/i);
    // Both figures, and both from THIS response rather than a constant:
    // 256 KB of 300 KB.
    expect(notice.textContent).toMatch(/256/);
    expect(notice.textContent).toMatch(/300/);
  });

  it("does not claim a truncation when the file arrived whole", () => {
    state.repoFile = "README.md";
    render(<RepositoriesPage />);
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("goes back to the listing", () => {
    state.repoFile = "README.md";
    render(<RepositoriesPage />);
    fireEvent.click(screen.getByRole("button", { name: /back to the listing/i }));
    expect(setRepoFile).toHaveBeenCalledWith(undefined);
  });
});

describe("the repository browser's selection", () => {
  it("asks for a repository when none is chosen", () => {
    state.repo = undefined;
    render(<RepositoriesPage />);
    expect(screen.getByText(/choose a repository/i)).toBeTruthy();
  });

  /// Outcome 1: the selection is stale. Re-derived against the CURRENT
  /// scan rather than trusted, per `caches/mod.rs` step 4.
  it("says the repository is no longer scanned when the scan no longer has it", () => {
    state.repo = "/code/gone";
    state.repos = [{ path: "/code/app", name: "app" }];
    render(<RepositoriesPage />);
    expect(screen.getByText(/no longer in the scanned folders/i)).toBeTruthy();
  });

  /// Absent is not zero, in the direction every other check in this
  /// codebase fails in. A scan that has not answered is not evidence
  /// that a repository is gone, and saying so would send the user to
  /// re-pick from a list that has not loaded.
  it("does not claim a repository vanished while the scan is still pending", () => {
    state.repo = "/code/app";
    state.repos = undefined;
    render(<RepositoriesPage />);
    expect(screen.queryByText(/no longer in the scanned folders/i)).toBeNull();
  });

  /// And the same for a scan that REJECTED, which is #846: a failed scan
  /// must not read as a vanished repository.
  it("does not claim a repository vanished when the scan itself failed", () => {
    state.repo = "/code/app";
    state.repos = [];
    state.scanFailed = true;
    render(<RepositoriesPage />);
    expect(screen.queryByText(/no longer in the scanned folders/i)).toBeNull();
  });

  it("names the path in the breadcrumb and navigates up from it", () => {
    state.repoPath = "src/components";
    state.tree = { path: "src/components", entries: [] };
    render(<RepositoriesPage />);
    // Every level is reachable, including the repository root.
    fireEvent.click(screen.getByRole("button", { name: "src" }));
    expect(setRepoPath).toHaveBeenCalledWith("src");
    fireEvent.click(screen.getByRole("button", { name: "app" }));
    expect(setRepoPath).toHaveBeenCalledWith("");
  });
});

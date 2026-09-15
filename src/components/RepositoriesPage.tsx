import { ChevronRight, File, Folder, Link2 } from "lucide-react";
import { useRepoFile, useRepoTree, useWorktrees } from "@/api/hooks";
import { useActiveFilters, useFilters } from "@/store/filters";
import { formatSize } from "@/lib/worktrees";
import type { RepoEntry } from "@/types/pr";
import { QueryError, errorMessage } from "./QueryError";
import { AllRepositoriesTable } from "./AllRepositoriesTable";

/// Browse one repository's files, the way GitHub's code view does
/// (#1031, #1033, #1036, epic #1011).
///
/// A directory listing, click to descend, click a file to read it. The
/// listing comes from the git INDEX rather than from the filesystem --
/// measured at 672 tracked files against 623,488 on disk in this
/// repository's own checkout -- so what the user sees is the repository
/// rather than `src-tauri/target/debug/deps`. That decision lives in
/// `src-tauri/src/repos/mod.rs`, which carries the figures.
///
/// # The four outcomes, and why none of them is a blank panel
///
/// This is the page #1036 exists for: the gap between LISTING a thing and
/// READING it, which no single command can close, and where this codebase
/// has shipped the same bug twice.
///
///  1. **The repository is no longer scanned.** Re-derived inside the
///     command against `list_worktrees`, per `caches/mod.rs` step 4 --
///     never trusted from the selection the sidebar made earlier. Renders
///     as a stale selection, because that is the remedy: pick another.
///  2. **The directory could not be listed.** git failed, and the message
///     says which of the three causes it was. Renders as a FAILURE.
///     Never an empty listing -- that is #846's exact shape.
///  3. **The directory listed and is empty.** A real, if unusual, answer.
///     Rendered as its own sentence, distinct from (2) in the way
///     `PartialScanNotice` insists the three scan outcomes are distinct.
///  4. **A file in the listing could not be read.** Measured: two tracked
///     files in that corpus do not exist on disk, both indexed symlinks
///     whose targets are gone, so the index and the filesystem already
///     disagree today. Rendered on the file panel, and the LISTING is not
///     invalidated -- one unreadable entry is not evidence the other 650
///     are wrong, which is the `PartialScanNotice` trade.
///
/// Collapsing (2) into (3) is the bug this page is written to prevent.
///
/// # Retries here are legitimate, and the scan's are not
///
/// A vanished directory CAN come back -- an agent's worktree is recreated,
/// a branch is switched back -- so re-listing it is a reasonable thing to
/// offer, and these panels offer it. What must never be offered is a
/// retry on `PartialScanNotice`'s unreadable paths: those are unreadable
/// "for a reason that a second identical walk will not change". That
/// notice lives in `RepoPickerSidebar` and this page adds nothing to it.
export function RepositoriesPage() {
  const filters = useActiveFilters();
  const repo = filters.repo;
  const { repoPath, setRepoPath, repoFile, setRepoFile } = useFilters();
  // The repository ROWS, only so this page can name the selected one and
  // notice when it is gone. The same scan the sidebar renders -- one
  // scan, one source of truth -- and `useWorktrees` is already cached, so
  // this costs a cache hit rather than a second walk.
  //
  // `isError` as well as the data (#846): a REJECTED scan must not read
  // as "your repository vanished", which would send the user to re-pick
  // from a list that itself failed to load.
  const { data: repos, isError: scanFailed } = useWorktrees();

  // No repository picked: the ALL REPOSITORIES overview (#1015, #1021).
  //
  // This is the landing state of the view rather than a prompt, because
  // "which of my repositories is behind its remote" is a question worth
  // answering before any repository is chosen -- and the table is the one
  // place it is answered. The sentence stays underneath it: the table
  // says what is stale, the sentence says what to do next, and a table
  // with no instruction reads as a dead end.
  //
  // The table qualifies every verdict by fetch age (#1021) and never
  // fetches, so landing here costs the scan that has already run rather
  // than a network call to every remote.
  if (!repo) {
    return (
      <div className="space-y-3">
        <AllRepositoriesTable />
        <p className="text-sm text-[#8b949e]">
          Choose a repository on the left to browse its files.
        </p>
      </div>
    );
  }

  // Outcome 1, rendered from the CLIENT's copy of the scan so the message
  // arrives without a round trip. The command re-derives the same fact
  // independently and rejects, which is the half that is load-bearing:
  // this check is honesty, not security, exactly as `claude_reveal_path`'s
  // existence check is annotated.
  //
  // Only once the scan has actually ANSWERED. `repos` is `undefined`
  // while pending and on a rejection, and treating either as "not in the
  // list" would tell the user their repository is gone because the scan
  // had not finished -- absent is not zero, in the direction every other
  // check in this codebase fails in.
  const known = repos?.some((r) => r.path === repo);
  if (repos && !scanFailed && !known) {
    return (
      <div role="alert" className="rounded-md border border-[#30363d] px-4 py-8 text-center">
        <p className="text-sm text-[#e6edf3]">
          This repository is no longer in the scanned folders.
        </p>
        <p className="mx-auto mt-2 max-w-lg text-sm text-[#8b949e]">
          It may have been moved or removed since the list was built. Pick
          another repository on the left.
        </p>
      </div>
    );
  }

  const name = repos?.find((r) => r.path === repo)?.name ?? repo;

  return (
    <div className="space-y-3">
      <Breadcrumb
        name={name}
        path={repoPath}
        file={repoFile}
        onNavigate={(p) => setRepoPath(p)}
      />
      {repoFile === undefined ? (
        <Listing
          repo={repo}
          path={repoPath}
          onDescend={(p) => setRepoPath(p)}
          onOpen={(p) => setRepoFile(p)}
        />
      ) : (
        <FilePanel repo={repo} path={repoFile} onBack={() => setRepoFile(undefined)} />
      )}
    </div>
  );
}

/// Where you are, and every level you can go back to.
///
/// Rendered from the PATH rather than from a history stack, so it cannot
/// disagree with what is on screen: there is one source of truth for the
/// position and it is the store field the panel reads.
function Breadcrumb({
  name,
  path,
  file,
  onNavigate,
}: {
  name: string;
  path: string;
  file: string | undefined;
  onNavigate: (path: string) => void;
}) {
  const parts = path === "" ? [] : path.split("/");
  return (
    <nav aria-label="Breadcrumb" className="flex flex-wrap items-center gap-1 text-sm">
      <button
        type="button"
        onClick={() => onNavigate("")}
        className="tap-target rounded px-1 text-[#58a6ff] hover:underline"
      >
        {name}
      </button>
      {parts.map((part, i) => (
        <span key={`${part}-${i}`} className="flex items-center gap-1">
          <ChevronRight aria-hidden className="h-3 w-3 text-[#6e7681]" />
          <button
            type="button"
            onClick={() => onNavigate(parts.slice(0, i + 1).join("/"))}
            className="tap-target rounded px-1 text-[#58a6ff] hover:underline"
          >
            {part}
          </button>
        </span>
      ))}
      {/* The file is the LAST crumb and is not a link: it is where you
          already are, and a link to the current position is a control
          that does nothing. */}
      {file !== undefined ? (
        <span className="flex items-center gap-1">
          <ChevronRight aria-hidden className="h-3 w-3 text-[#6e7681]" />
          <span className="px-1 font-mono text-[#e6edf3]">
            {file.split("/").pop()}
          </span>
        </span>
      ) : null}
    </nav>
  );
}

/// One directory level (#1031), with outcomes 2 and 3 kept apart.
function Listing({
  repo,
  path,
  onDescend,
  onOpen,
}: {
  repo: string;
  path: string;
  onDescend: (path: string) => void;
  onOpen: (path: string) => void;
}) {
  const { data, isLoading, isError, error, refetch } = useRepoTree(repo, path);

  if (isLoading) {
    return <p className="text-sm text-[#8b949e]">Listing this directory…</p>;
  }
  // Outcome 2, and it comes BEFORE the empty arm for the ordering reason
  // `ClaudeMdPage` states and `RepoPickerSidebar` restates: an arm placed
  // after the empty one is unreachable in exactly the case it exists for.
  // The two have OPPOSITE remedies -- "there is nothing here" and "we
  // could not tell" -- and only the message distinguishes the three
  // causes git can fail for.
  //
  // A retry IS offered here, and legitimately: a directory that vanished
  // because an agent removed its worktree can come back, unlike the
  // scan's unreadable paths.
  if (isError || !data) {
    return (
      <QueryError
        title="Could not list this directory."
        message={errorMessage(error)}
        onRetry={() => void refetch()}
      />
    );
  }
  // Outcome 3: git listed it and it holds no tracked files. A real
  // answer, and at a path the tree itself produced it is the signal that
  // the directory has been emptied since.
  if (data.entries.length === 0) {
    return (
      <p className="text-sm text-[#8b949e]">
        This directory has no tracked files. Only files git knows about are
        listed, so anything here is ignored or untracked.
      </p>
    );
  }

  return (
    <ul className="divide-y divide-[#30363d] rounded-md border border-[#30363d]">
      {data.entries.map((e: RepoEntry) => (
        <li key={e.path}>
          <button
            type="button"
            // A symlinked DIRECTORY is not a control; a symlinked FILE
            // is. The 22 tracked symlinks across 38 repositories split 14
            // files / 6 directories / 2 broken, and that split decides
            // this.
            //
            // A directory link has no panel to explain itself in, so a
            // single "symlinks are not followed" treatment would leave it
            // looking descendable and doing nothing when clicked -- and a
            // row that silently ignores a click reads as broken. It is
            // disabled, and the row itself carries the reason.
            //
            // A file link stays clickable precisely so it CAN explain
            // itself, in the panel. `repo_file` refuses it with "is a
            // symbolic link, which is shown but not followed" -- the
            // guard's own message, which the panel's failure arm renders
            // verbatim. The refusal is the explanation, so there is no
            // second copy of that sentence here to drift from it.
            //
            // Disabled rather than hidden: hiding a link would make
            // tracked entries silently missing from their listings.
            disabled={e.symlink && e.symlink_to_dir === true}
            onClick={() => (e.dir ? onDescend(e.path) : onOpen(e.path))}
            className="tap-target flex w-full items-center gap-2 px-3 py-2 text-left text-sm text-[#e6edf3] enabled:hover:bg-[#161b22] disabled:cursor-default disabled:text-[#8b949e]"
          >
            {e.symlink ? (
              <Link2 aria-hidden className="h-4 w-4 shrink-0 text-[#8b949e]" />
            ) : e.dir ? (
              <Folder aria-hidden className="h-4 w-4 shrink-0 text-[#58a6ff]" />
            ) : (
              <File aria-hidden className="h-4 w-4 shrink-0 text-[#8b949e]" />
            )}
            <span className="truncate font-mono">{e.name}</span>
            {/* The target, on EVERY link row. 14 of the 22 are shared
                Terraform module files, where what the link points at is
                exactly the thing the user opened the row to learn -- a
                link shown without one tells them less than the filename
                already did.

                The arrow is the spelling `ls -l` and the GitHub code view
                both use, so it needs no explaining. A link whose target
                could not be read shows the label alone rather than a
                dangling arrow. */}
            {e.symlink ? (
              <span className="ml-auto shrink-0 truncate pl-2 text-xs text-[#8b949e]">
                {e.symlink_to_dir === true ? "linked folder" : "link"}
                {e.target !== undefined ? (
                  <span className="font-mono"> → {e.target}</span>
                ) : null}
              </span>
            ) : null}
          </button>
        </li>
      ))}
    </ul>
  );
}

/// One file (#1033), with all four of its outcomes rendered distinctly.
function FilePanel({
  repo,
  path,
  onBack,
}: {
  repo: string;
  path: string;
  onBack: () => void;
}) {
  const { data, isLoading, isError, error, refetch } = useRepoFile(repo, path);

  const back = (
    <button
      type="button"
      onClick={onBack}
      className="tap-target rounded border border-[#30363d] px-2 py-1 text-xs text-[#58a6ff] hover:bg-[#161b22]"
    >
      Back to the listing
    </button>
  );

  if (isLoading) {
    return <p className="text-sm text-[#8b949e]">Reading this file…</p>;
  }
  // Outcome 4: it is in the index and could not be READ. Measured on the
  // real corpus -- two tracked files whose targets are gone -- so this is
  // the first click's behaviour on some repositories, not a hypothetical.
  //
  // The listing behind it is NOT invalidated: one unreadable entry is not
  // evidence the other 650 are wrong, which is why this renders on the
  // file panel and leaves the tree where it was.
  if (isError || !data) {
    return (
      <div className="space-y-2">
        {back}
        <QueryError
          title="This file is in the index but could not be read."
          message={errorMessage(error)}
          onRetry={() => void refetch()}
        />
      </div>
    );
  }

  // Read perfectly well and not text. NOT an error -- the GitHub code
  // view says exactly this about a binary, and detecting it by NUL byte
  // rather than by extension is why a `.txt` full of ELF and an
  // extensionless executable both land here.
  if (data.binary) {
    return (
      <div className="space-y-2">
        {back}
        <p className="rounded-md border border-[#30363d] px-4 py-8 text-center text-sm text-[#8b949e]">
          This is a binary file ({formatSize(data.size)}), so there is
          nothing to show.
        </p>
      </div>
    );
  }

  // Read perfectly well and genuinely empty. A third thing again, and
  // there are real ones: `.gitkeep` files appear across this corpus.
  if (data.content === "") {
    return (
      <div className="space-y-2">
        {back}
        <p className="rounded-md border border-[#30363d] px-4 py-8 text-center text-sm text-[#8b949e]">
          This file is empty.
        </p>
      </div>
    );
  }

  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center gap-2">
        {back}
        <span className="text-xs text-[#8b949e]">{formatSize(data.size)}</span>
      </div>
      {/* The truncation is STATED, above the text rather than below it: a
          window shown as if it were the whole file is worse than a
          refusal, and a reader who has scrolled to the bottom has already
          been misled. `role="status"` so it is announced rather than
          being a visual-only caveat.

          The figures are LIVE -- `data.size` and `data.content.length`
          from this response -- never a measured constant. A number
          measured once is correct on the day it is written and decays
          from then on, which `measuredFigures.test.ts` exists to enforce. */}
      {data.truncated ? (
        <p
          role="status"
          className="rounded border border-[#30363d] bg-[#161b22] px-3 py-2 text-xs text-[#8b949e]"
        >
          Showing the first {formatSize(data.content.length)} of{" "}
          {formatSize(data.size)}. Files are read up to a fixed limit so that
          a large one cannot be pulled over the connection whole.
        </p>
      ) : null}
      <pre className="overflow-x-auto rounded-md border border-[#30363d] bg-[#0d1117] p-3 text-xs leading-relaxed text-[#e6edf3]">
        <code>{data.content}</code>
      </pre>
    </div>
  );
}

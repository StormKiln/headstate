import { useState } from "react";
import { useIsMobile } from "@/lib/useIsMobile";
import { FileText } from "lucide-react";
import type { ClaudeFile, ImportNode } from "@/types/pr";
import { useClaudeMd, useClaudeMdText } from "@/api/hooks";
import { useActiveFilters } from "@/store/filters";
import { formatSize } from "@/lib/worktrees";
import { Markdown } from "./Markdown";
import { QueryError, errorMessage } from "./QueryError";
import { PartialScanNotice } from "./PartialScanNotice";
import { copyText } from "@/lib/clipboard";
import { toast } from "sonner";

/// Token counts are an ESTIMATE, and every label says so.
///
/// The backend divides characters by four. That is the standard rough
/// figure for English prose and it is wrong for code blocks and paths,
/// which these files are full of. A number labelled "tokens" that is
/// actually chars/4 is exactly the confidently-wrong figure this app
/// refuses to ship, so the word "est." travels with it everywhere.
function tokenLabel(n: number): string {
  return `~${n.toLocaleString()} est. tokens`;
}

/// A total that is a FLOOR, in the app's existing idiom (#972).
///
/// "at least" is what `ArtifactsPage` and `WorktreesPage` already put in
/// front of a size that is not fully measured, so this reuses the phrasing
/// rather than inventing a second one for the same fact. Reached when an
/// import's weight could not be counted: the real total is higher by an
/// unknown amount, and a user budgeting context otherwise reads a number
/// that is too small with nothing on screen to say so.
function totalLabel(n: number, partial: boolean): string {
  return partial ? `at least ${tokenLabel(n)}` : tokenLabel(n);
}

/// CLAUDE.md files for the selected repository.
///
/// Read-only: a wrong render costs a confused reader rather than a
/// corrupted config file.
export function ClaudeMdPage() {
  const filters = useActiveFilters();
  const repo = filters.repo;
  // `isError`, `error` and `refetch` as well as the data (#846). The
  // `= []` default made a REJECTED scan read as "No CLAUDE.md files in
  // this repository" -- a confident, wrong answer to a question the app
  // could not answer, on a page whose own doc comment above stakes its
  // design on "a wrong render costs a confused reader".
  const {
    data: scan,
    isLoading,
    isError,
    error,
    refetch,
  } = useClaudeMd(repo);
  // NOT `data.files = []`. The default lives here rather than on `data`
  // because the guards below need to see `undefined` -- a `= []` on the
  // query result is exactly what #846 was about, and #972 is the same
  // mistake one layer down: the scan now reports what it could not read,
  // and a defaulted-away `scan` would make that report unreachable.
  const files = scan?.files ?? [];
  // Everything the scan proved it could not read. A directory and a file
  // are ONE list to the reader -- both are "we could not look here" -- and
  // `PartialScanNotice` shows the message the Rust side attached to each,
  // which is what distinguishes a permission wall from a stray byte.
  const unreadablePaths = scan ? [...scan.unreadable_dirs, ...scan.unreadable_files] : [];
  const unreadable = unreadablePaths.length;
  const [selected, setSelected] = useState<string | undefined>(undefined);
  const isMobile = useIsMobile();

  // The selection falls back to the first file so the pane is never
  // empty when there is something to show.
  const active = files.find((f) => f.path === selected) ?? files[0];

  // On a phone the two panes are two screens, so which one is showing
  // has to key off whether the user has PICKED a file -- not off
  // `active`, which falls back to the first file and is therefore never
  // absent. The desktop keeps the fallback: its list is beside the
  // content, and an empty pane there would just look broken.
  const showingList = isMobile && selected === undefined;
  // The content read's own failure, separately from the scan's (#846).
  //
  // Two queries, two failures, and they are genuinely different
  // questions: the scan failing means the LIST is unknown, the read
  // failing means one file could not be opened while the list beside it
  // is fine. Collapsing them would either hide a readable list behind one
  // unreadable file or hide an unreadable file behind a list that worked.
  const {
    data: text,
    isLoading: textLoading,
    isError: textError,
    error: textErr,
    refetch: refetchText,
  } = useClaudeMdText(active?.path);

  if (!repo) {
    return (
      <p className="p-4 text-sm text-[#8b949e]">
        Choose a repository to see its CLAUDE.md files.
      </p>
    );
  }
  if (isLoading) {
    return <p className="p-4 text-sm text-[#8b949e]">Looking for CLAUDE.md files…</p>;
  }
  // BEFORE the empty state (#846). With `data = []` on a rejection the
  // empty branch was reached first and claimed the repository had none,
  // so an error arm placed after it would be unreachable in exactly the
  // case it exists for.
  if (isError) {
    return (
      <div className="p-4">
        <QueryError
          title="Could not look for CLAUDE.md files"
          message={errorMessage(error)}
          onRetry={() => void refetch()}
        />
      </div>
    );
  }
  // The FOURTH answer, and the one #846's fix could not reach (#972).
  //
  // That fix gave this page a correct `isError` arm ordered BEFORE the
  // empty arm. It is sound and untouched -- but the arm can only fire on
  // `isError`, and `scan_claude_md` could only ever return `Ok`. So a read
  // failure arrived as an empty list, the empty arm was reached first, and
  // the page said "No CLAUDE.md files in this repository" about a file the
  // user can see on disk.
  //
  // Ordered before the empty arm for the reason that fix states, and here
  // it is not merely a nicety: "this repository has none" and "we could
  // not read it" have opposite remedies.
  //
  // No retry, and NOT reported as a failed scan -- the decision #951 made
  // for `RepoPickerSidebar` and wrote into `PartialScanNotice`. `isError`
  // above is a rejection of the whole command; this is a walk that ran and
  // came back short, and a second identical walk will not read what the
  // first could not. The paths below are the action, not a button.
  if (files.length === 0 && unreadable > 0) {
    return (
      <div className="p-4">
        <p className="text-sm text-[#8b949e]">
          No CLAUDE.md file could be read. The paths below explain why — this repository may well
          have some.
        </p>
        <div className="mt-2">
          <PartialScanNotice
            unreadable={unreadablePaths}
            consequence="no CLAUDE.md file could be read from them."
          />
        </div>
      </div>
    );
  }
  if (files.length === 0) {
    // Only a scan that read EVERYTHING gets to say this. The arm above has
    // already taken the partial case, so this is now a true statement
    // rather than #846's confident wrong answer.
    return <p className="p-4 text-sm text-[#8b949e]">No CLAUDE.md files in this repository.</p>;
  }

  return (
    <div className={isMobile ? "flex h-full min-h-0 flex-col" : "flex h-full min-h-0"}>
      {/* The browser: every file, its own size, and what its whole tree
          costs.

          `w-96` is 384px and `shrink-0`, so at 390px it took the whole
          screen and the content pane -- `flex-1 min-w-0` -- collapsed to
          about six pixels: the CLAUDE.md text this page exists to show
          was invisible. On a phone the two become stacked screens, the
          pattern `PrDetailView` already uses: pick a file, read it, come
          back. */}
      <div
        className={
          isMobile
            ? showingList
              ? "min-h-0 flex-1 overflow-y-auto p-3"
              : "hidden"
            : "w-96 shrink-0 overflow-y-auto border-r border-[#30363d] p-3"
        }
      >
        {/* A PARTIAL scan, stated beside the files that DID read (#972).

            The SAME component #951 added for the worktree scan, not a
            second one: it is the same third answer with the same reasoning
            about why it is a notice rather than an error panel, and a
            parallel implementation of it is exactly the drift the app's
            shared constants exist to prevent.

            Above the list rather than instead of it. The files here are
            real, and blanking them to report one unreadable sibling would
            replace a silent loss with a louder one -- the trade
            `ArtifactsPage` states and the rule `transcript.rs`'s
            `is_partial()` gives. The all-unreadable case never reaches
            here; the arm above took it. */}
        <PartialScanNotice
          unreadable={unreadablePaths}
          consequence={`the ${files.length === 1 ? "file" : `${files.length} files`} below may not be all of them.`}
        />
        {files.map((f) => (
          <FileEntry
            key={f.path}
            file={f}
            repo={repo}
            active={f.path === active?.path}
            onSelect={() => setSelected(f.path)}
          />
        ))}
      </div>

      <div
        className={
          isMobile
            ? showingList
              ? "hidden"
              : "flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto p-4"
            : "min-w-0 flex-1 overflow-y-auto p-4"
        }
      >
        {isMobile && !showingList ? (
          <button
            type="button"
            onClick={() => setSelected(undefined)}
            className="tap-target -ml-1 mb-2 flex items-center self-start rounded px-2 text-sm text-[#58a6ff] hover:bg-[#161b22]"
          >
            ← All files
          </button>
        ) : null}
        {/* A chain that CANNOT end at nothing (#846).

            It used to be two arms and a `null`, and on a rejected read
            both arms failed: `textLoading` false, `text` undefined, and
            the pane rendered completely blank. The result was the file
            browser on the left, the correct file highlighted blue, and an
            entirely empty pane on the right -- no error, no retry. On a
            phone the user had navigated to a second SCREEN containing
            only an "← All files" button. A file renamed between the scan
            and the click (`staleTime: 30_000` makes that a real window)
            presented as an app that had failed to draw.

            The error arm is ordered BEFORE the data arm for the reason
            the page-level one is: `text` is undefined on a rejection, so
            a later arm could not distinguish the two. The final arm is a
            real state rather than a fallback -- there is genuinely
            nothing selected only when the repository has no files, which
            the guard above has already handled, so it says what it is
            instead of leaving the reader with a blank box. */}
        {textLoading ? (
          <p className="text-sm text-[#8b949e]">Reading…</p>
        ) : textError ? (
          <QueryError
            title="Could not read this file"
            message={errorMessage(textErr)}
            onRetry={() => void refetchText()}
          >
            {/* NAMES the file, because the list beside this pane is
                still correct and still highlighting a row: without the
                path, an error here reads as though the whole page
                failed rather than this one read. */}
            <p className="mx-auto mt-2 max-w-lg break-all font-mono text-xs text-[#8b949e]">
              {active?.path}
            </p>
          </QueryError>
        ) : text !== undefined ? (
          // Through the SAME sanitising renderer the rest of the app
          // uses. These files come off disk, and one containing raw HTML
          // must not be able to inject anything here.
          <Markdown>{text}</Markdown>
        ) : (
          <p className="text-sm text-[#8b949e]">Choose a file to read it.</p>
        )}
      </div>
    </div>
  );
}

function FileEntry({
  file,
  repo,
  active,
  onSelect,
}: {
  file: ClaudeFile;
  repo: string;
  active: boolean;
  onSelect: () => void;
}) {
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  // The path RELATIVE to the repository, split into its parts. A full
  // absolute path truncates in the middle -- which is exactly where the
  // distinguishing part of `<repo>/<dir>/CLAUDE.md` lives.
  const relative = file.path.startsWith(repo)
    ? file.path.slice(repo.length).replace(/^\//, "")
    : file.path;
  const parts = relative.split("/");
  const name = parts[parts.length - 1];
  const dir = parts.slice(0, -1).join("/");
  // Only worth stating separately when the tree adds something. On a
  // file with no imports the two numbers are equal and printing both
  // reads as a mistake.
  //
  // `total_partial` also qualifies: an import whose weight could not be
  // counted adds nothing to the number, so `total_tokens` equals `tokens`
  // and the old condition hid the row that carries the "at least" (#972)
  // -- the exact case in which the total most needs saying.
  const treeAdds = file.total_tokens > file.tokens || file.total_partial;

  return (
    <div className="mb-2">
      <button
        type="button"
        onClick={onSelect}
        // A path is what goes in a commit message or a terminal, and
        // there is nowhere else in this view to get one.
        onContextMenu={(e) => {
          e.preventDefault();
          setMenu({ x: e.clientX, y: e.clientY });
        }}
        aria-pressed={active}
        className={`flex w-full flex-col items-start gap-0.5 rounded px-2 py-1.5 text-left ${
          active ? "bg-[#1f6feb] text-white" : "text-[#e6edf3] hover:bg-[#161b22]"
        }`}
      >
        <span className="flex w-full items-center gap-1.5">
          <FileText className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          {/* The directory as an indent and a prefix, the FILE NAME in
              full. Position carries what a truncated absolute path
              destroys. */}
          {dir ? (
            <span className={`shrink-0 font-mono text-xs ${active ? "text-white" : "text-[#8b949e]"}`}>
              {dir}/
            </span>
          ) : null}
          <span className="min-w-0 flex-1 truncate font-mono text-xs">{name}</span>
        </span>
        {/* Full white, not `white/70` and `white/80`.

            These are 12px body text on the selected row's #1f6feb, where
            the dimmed variants measure 3.07:1 and 3.53:1 -- both under the
            4.5:1 normal-text threshold. No opacity clears it: white/90 is
            still only 4.07:1, so the de-emphasis and the requirement
            cannot both be had against this blue. The row is already
            distinguished from its neighbours by the blue itself, so the
            secondary text does not need to be dimmed on top of that. */}
        <span className={`text-xs ${active ? "text-white" : "text-[#8b949e]"}`}>
          {formatSize(file.bytes)} · {tokenLabel(file.tokens)}
          {treeAdds ? ` · ${totalLabel(file.total_tokens, file.total_partial)} with imports` : ""}
        </span>
      </button>
      {menu ? (
        <PathMenu
          x={menu.x}
          y={menu.y}
          relative={relative}
          absolute={file.path}
          onClose={() => setMenu(null)}
        />
      ) : null}
      {file.imports.length > 0 ? (
        <ul className="ml-4 mt-1 border-l border-[#30363d] pl-2">
          {file.imports.map((n, i) => (
            <ImportRow key={`${n.raw}-${i}`} node={n} />
          ))}
        </ul>
      ) : null}
    </div>
  );
}

function ImportRow({ node }: { node: ImportNode }) {
  return (
    <li className="text-xs">
      <span className="flex items-center gap-1.5">
        <span className="font-mono text-[#8b949e]">{node.raw}</span>
        {/* A broken or circular import is NAMED. Dropping it makes the
            tree look complete when it is not, and a cycle is a bug in
            the user's own config that nothing else will surface. */}
        {node.problem ? (
          <span className="rounded-full bg-[#f85149]/15 px-2 py-0.5 text-[#f85149]">
            {node.problem}
          </span>
        ) : (
          <span className="text-[#8b949e]">{tokenLabel(node.tokens)}</span>
        )}
      </span>
      {node.children.length > 0 ? (
        <ul className="ml-3 border-l border-[#30363d] pl-2">
          {node.children.map((c, i) => (
            <ImportRow key={`${c.raw}-${i}`} node={c} />
          ))}
        </ul>
      ) : null}
    </li>
  );
}

/// Copy this file's path, relative or absolute.
///
/// Relative to the REPOSITORY, because that is what goes in a commit
/// message or an issue; absolute for pasting into a terminal. Both go
/// through `copyText`, so a window without clipboard access reports a
/// failure rather than doing nothing -- the bug that made Claudify look
/// inert.
function PathMenu({
  x,
  y,
  relative,
  absolute,
  onClose,
}: {
  x: number;
  y: number;
  relative: string;
  absolute: string;
  onClose: () => void;
}) {
  const copy = (value: string, what: string) => {
    onClose();
    void copyText(value).then((failure) =>
      failure === null
        ? toast.success(`${what} copied to the clipboard`)
        : toast.error(`Could not copy the ${what.toLowerCase()}`, { description: failure }),
    );
  };

  return (
    <>
      {/* A full-screen backdrop, so clicking anywhere dismisses. A menu
          that can only be closed by choosing something is a trap. */}
      <button
        type="button"
        aria-label="Close menu"
        onClick={onClose}
        className="fixed inset-0 z-40 cursor-default"
      />
      <div
        role="menu"
        style={{ left: x, top: y }}
        className="fixed z-50 min-w-48 rounded-md border border-[#30363d] bg-[#161b22] p-1 shadow-lg"
      >
        <button
          type="button"
          role="menuitem"
          onClick={() => copy(relative, "Relative path")}
          className="block w-full rounded px-2 py-1.5 text-left text-xs text-[#e6edf3] hover:bg-[#21262d]"
        >
          Copy relative path
        </button>
        <button
          type="button"
          role="menuitem"
          onClick={() => copy(absolute, "Absolute path")}
          className="block w-full rounded px-2 py-1.5 text-left text-xs text-[#e6edf3] hover:bg-[#21262d]"
        >
          Copy absolute path
        </button>
      </div>
    </>
  );
}

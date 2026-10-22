import type { DiscoveryDiagnostic } from "../types/pr";
import { rootBoundaries } from "../lib/rootBoundaries";
import { useId, useState } from "react";

/// The banner for a worktree scan that could not read everything (#951).
///
/// # The third answer
///
/// A scan has three outcomes, not two. "We have not looked yet", "we
/// looked and there is nothing", and "we looked and could not tell" are
/// three different claims with three different remedies, and this
/// component is the third one. Before #951 the walk had nowhere to put it:
/// `scan_dirs_fast` returned a bare `Vec<Repo>`, so a repository whose
/// worktree listing failed was simply absent and read as "not a
/// repository".
///
/// `RepoPickerSidebar`'s empty copy is why that mattered most --
/// "No repositories found in the scanned folders" is a DIAGNOSIS pointing
/// at the user's settings, so a failed scan sent someone to fix a
/// configuration that was never wrong. `emptyStateGuard.test.ts` names it
/// the worst copy of the six it audits, and gave it an `isError` arm that
/// could never fire because the command could not reject.
///
/// # Why this is not an error panel
///
/// `QueryError` blanks the list and offers a retry, which is right when
/// the whole query failed. This is the OTHER case: some of the scan
/// succeeded. The repositories that did read are real and worth showing --
/// the trade `ArtifactsPage` states -- so this sits ABOVE the list rather
/// than instead of it, and offers no retry: an unreadable path is
/// unreadable for a reason that a second identical walk will not change,
/// and #846's `retry: false` reasoning is about exactly this.
///
/// # Why the reasons are shown
///
/// A count alone ("3 paths could not be read") is unactionable. The Rust
/// side sends `<path>: <why>` per entry for the reason
/// `claude/transcript.rs` gives for a message over a boolean: git refusing
/// a repository owned by another uid, a permission wall, and git missing
/// from a GUI-launched app's PATH send the user to three different places,
/// and only the message distinguishes them.
///
/// # A scan root that is itself a repository (#1948)
///
/// Discovery stops at a checkout (#1919), so a scan root that is a
/// repository lists one row and nothing nested inside it. The scan read
/// everything it looked at, so this is not a shortfall and not an alert:
/// it renders as a note BESIDE the banner, and only qualifies the numbers.
///
/// # Exclusions that could not be read (#1956)
///
/// The scan still ran, over the configured folders and without the
/// exclusions, so it covered MORE than asked rather than less: nothing is
/// missing and the counts are not qualified. It is a settings problem, so
/// it is a note pointing at Settings, never the shortfall banner.
export function PartialScanNotice(props: Parameters<typeof ShortfallNotice>[0]) {
  const roots = rootBoundaries(props.diagnostics);
  const settingsUnread = (props.diagnostics ?? []).some((d) => d.kind === "settingsError");
  return (
    <>
      <ShortfallNotice
        {...props}
        diagnostics={props.diagnostics?.filter((d) => d.kind !== "repositoryBoundary" && d.kind !== "settingsError")}
      />
      {roots.length > 0 && (
        <RootBoundaryNote roots={roots} consequence={props.consequence} compact={props.compact} />
      )}
      {settingsUnread && (
        <div role="note" className="border-b border-[#30363d] px-4 py-2 text-xs text-[#8b949e]">
          <p>Discovery exclusions could not be read, so excluded folders were scanned too.</p>
          <p className="mt-1">Review them in Settings &gt; Repositories.</p>
        </div>
      )}
    </>
  );
}

function RootBoundaryNote({
  roots,
  consequence,
  compact,
}: {
  roots: readonly string[];
  consequence: string;
  compact?: boolean;
}) {
  const one = roots.length === 1;
  const name = (root: string) => (compact ? root.split(/[\\/]/).filter(Boolean).pop() ?? root : root);
  const subject = one ? `The scan root ${name(roots[0])} is itself a repository` : `${roots.length} scan roots are themselves repositories`;
  const nested = `repositories nested inside ${one ? "it" : "them"} are not listed`;
  return (
    <div role="note" className="border-b border-[#30363d] px-4 py-2 text-xs text-[#8b949e]">
      <p>{compact ? `${subject}; ${nested}.` : `${subject}, and ${nested}, so ${consequence}`}</p>
      <p className="mt-1">To include one, add its folder as a scan root in Settings &gt; Repositories.</p>
    </div>
  );
}

function ShortfallNotice({
  unreadable,
  /// What the numbers on this surface are, given the shortfall. Each
  /// surface says its own, because "at least N repositories" and "at
  /// least N orphans" are different claims and a shared phrasing would
  /// be vaguer than either.
  consequence,
  /// Advice reports checks; scans can report both read errors and scope
  /// limits, so their summary must not call every notice a read failure.
  checks,
  collapsed = false,
  compact = false,
  diagnostics,
}: {
  unreadable: readonly string[];
  consequence: string;
  checks?: { total: number };
  collapsed?: boolean;
  compact?: boolean;
  diagnostics?: readonly DiscoveryDiagnostic[];
}) {
  const [expanded, setExpanded] = useState(false);
  const [page, setPage] = useState(0);
  const detailsId = useId();
  if (unreadable.length === 0) return null;
  const n = unreadable.length;
  const onlyLimits = diagnostics?.length === n && diagnostics.every((d) => d.kind === "depthLimit" || d.kind === "directoryBudget");
  const lead = checks
    ? `${n} of ${checks.total} checks could not run`
    : onlyLimits ? "Discovery limited" : `Scan incomplete (${n} notice${n === 1 ? "" : "s"})`;
  const large = compact || collapsed || n > 3;
  const pageSize = 50;
  const currentPage = Math.min(page, Math.ceil(n / pageSize) - 1);
  const start = currentPage * pageSize;
  const visible = unreadable.slice(start, start + pageSize);
  return (
    <div
      role="alert"
      className="border-b border-[#30363d] bg-[#161b22] px-4 py-2 text-xs text-[#8b949e]"
    >
      <p>
        {compact ? (onlyLimits ? "Discovery limited. Some repositories may be missing." : "Scan incomplete. Some repositories may be missing.") : <>{lead}, so {consequence}</>}
      </p>
      {large && (
        <button
          type="button"
          aria-expanded={expanded}
          aria-controls={detailsId}
          className="mt-1 underline"
          onClick={() => {
            setExpanded(!expanded);
            setPage(0);
          }}
        >
          {expanded ? "Hide details" : "Show details"}
        </button>
      )}
      {/* Do not mount thousands of hidden paths. Paging keeps every reason
          reachable without letting diagnostics displace repository navigation. */}
      {(!large || expanded) && (
        <div id={detailsId}>
          <ul
            tabIndex={0}
            aria-label="Scan diagnostics"
            className="mt-1 max-h-48 space-y-0.5 overflow-y-auto"
          >
            {visible.map((u, i) => (
              <li key={start + i} className="break-all font-mono text-[#8b949e]">
                {u}
              </li>
            ))}
          </ul>
          {n > pageSize && (
            <div className="mt-2 flex flex-wrap items-center gap-2">
              <span aria-live="polite">{start + 1}–{Math.min(start + pageSize, n)} of {n}</span>
              <button
                type="button"
                className="underline disabled:opacity-50"
                disabled={currentPage === 0}
                onClick={() => setPage(currentPage - 1)}
              >
                Previous
              </button>
              <button
                type="button"
                className="underline disabled:opacity-50"
                disabled={start + pageSize >= n}
                onClick={() => setPage(currentPage + 1)}
              >
                Next
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

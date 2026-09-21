import { useState } from "react";
import { useClaudeMdAdvice } from "@/api/hooks";
import type {
  ClaudeMdAdviceCheck,
  ClaudeMdAdviceCoverage,
  ClaudeMdAdviceFinding,
  ClaudeMdAdviceLocator,
  ClaudeMdAdviceReport,
  ClaudeMdAdviceSubject,
} from "@/types/pr";
import { current } from "@/lib/ariaCurrent";
import { copyText } from "@/lib/clipboard";
import { toast } from "sonner";
import { PartialScanNotice } from "./PartialScanNotice";
import { QueryError, errorMessage } from "./QueryError";

/// What each check is called. A `Record` so adding a variant to the
/// wire type without a label fails to compile.
const CHECK_LABEL: Record<ClaudeMdAdviceCheck, string> = {
  imports: "imports",
};

/// What each severity is called, and how it is coloured.
///
/// `unknown` gets its own word and its own colour, never a muted version
/// of anything: "could not decide" rendered quietly is how an unchecked
/// thing becomes a cleared one in a reader's head (#1042).
const SEVERITY: Record<ClaudeMdAdviceFinding["severity"], { label: string; className: string }> = {
  problem: { label: "problem", className: "text-[#f85149]" },
  advice: { label: "advice", className: "text-[#d29922]" },
  unknown: { label: "could not decide", className: "text-[#d29922]" },
};

/// A path as the row shows it: relative to the repository when it is
/// inside it, absolute otherwise. Display only; the wire keeps absolute
/// paths, and the brief prints them as the backend rendered them.
function shown(path: string, repo: string): string {
  return path.startsWith(repo) ? path.slice(repo.length).replace(/^\//, "") : path;
}

function locatorText(at: ClaudeMdAdviceLocator, repo: string): string {
  if (at.kind === "file") {
    // A line ONLY when the backend recorded one. `:0` or a guessed line
    // would send the reader to a confident wrong place.
    return at.line === null ? shown(at.path, repo) : `${shown(at.path, repo)}:${at.line}`;
  }
  return at.record === null ? `session ${at.sessionId}` : `session ${at.sessionId} record ${at.record}`;
}

/// The file a subject names, when it names one the page can open.
function subjectFile(s: ClaudeMdAdviceSubject): string | null {
  return s.kind === "directory" ? null : s.path;
}

/// Copy a brief and say whether it happened, the `PathMenu` shape in
/// `ClaudeMdPage`: `copyText` reports the no-clipboard case rather than
/// doing nothing, and the toast is what makes the click visible.
function copyBrief(text: string, what: string) {
  void copyText(text).then((failure) =>
    failure === null
      ? toast.success(`${what} copied to the clipboard`, {
          description: "Paste it into a Claude session to make the change.",
        })
      : toast.error(`Could not copy the ${what.toLowerCase()}`, { description: failure }),
  );
}

/// Advice about the CLAUDE.md files of one repository.
///
/// Collapsed by default and fetched only while open, like
/// `ConfigHealthPanel`, so the page's file list and content pane never
/// wait on it. Everything shown comes off the wire in the backend's
/// order: the panel maps over `report.findings` and `report.checks` and
/// never filters, sorts or concatenates briefs -- `combinedTokens` in
/// `ClaudeMdPage` is the hand-mirrored counter-example this avoids.
///
/// Nothing here has a fixed width. It inherits the rail's width, which at
/// 390px is the whole screen, and breaks words rather than overflowing.
export function ClaudeMdAdvicePanel({
  repo,
  activePath,
  onSelectFile,
}: {
  repo: string;
  /// The file the page is showing, so a finding about it reads as current.
  activePath: string | undefined;
  /// Show this file. On a phone this navigates to the file screen.
  onSelectFile: (path: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const { data, isError, error, isFetching, refetch } = useClaudeMdAdvice(repo, open);

  return (
    <div className="mt-3 border-t border-[#21262d] pt-2">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        className="tap-target text-xs text-[#58a6ff] hover:underline"
      >
        {open ? "Hide advice" : "Show advice about these files"}
      </button>
      {/* Three renders, never collapsed. A rejection is an error with a
          retry: the whole run failed. A missing report is a skeleton: not
          measured yet. A report is rendered as what it says, including
          the checks that could not run. */}
      {!open ? null : isError ? (
        <div className="mt-2">
          <QueryError
            title="Could not check these files"
            message={errorMessage(error)}
            onRetry={() => void refetch()}
          />
        </div>
      ) : !data ? (
        <Skeleton />
      ) : (
        <ReportView
          report={data}
          repo={repo}
          activePath={activePath}
          onSelectFile={onSelectFile}
          isFetching={isFetching}
          onRecheck={() => void refetch()}
        />
      )}
    </div>
  );
}

/// "Not measured yet", and nothing else. Never "no advice".
function Skeleton() {
  return (
    <div className="mt-2" aria-busy="true">
      <p className="text-xs text-[#8b949e]">Checking…</p>
      <ul className="mt-1 space-y-1">
        {[0, 1].map((i) => (
          <li key={i} className="h-3 w-full rounded bg-[#21262d] motion-safe:animate-pulse" />
        ))}
      </ul>
    </div>
  );
}

function ReportView({
  report,
  repo,
  activePath,
  onSelectFile,
  isFetching,
  onRecheck,
}: {
  report: ClaudeMdAdviceReport;
  repo: string;
  activePath: string | undefined;
  onSelectFile: (path: string) => void;
  isFetching: boolean;
  onRecheck: () => void;
}) {
  // Which checks could not run, from the wire's own coverage list. Read
  // here for the notice; the rows below map over the full list.
  const unknown = report.checks.filter((c) => c.run.state === "unknown");
  const everyRan = report.checks.every((c) => c.run.state === "ran");
  const n = report.findings.length;

  return (
    <div className="mt-2 space-y-2">
      {/* The shortfall FIRST, and stated as the producer wrote it. The
          findings below are real; what is missing is the checks that
          could not vouch for anything. */}
      <PartialScanNotice
        unreadable={unknown.map((c) => `${CHECK_LABEL[c.check]}: ${reason(c)}`)}
        consequence={`the ${n === 1 ? "finding" : `${n} findings`} below ${n === 1 ? "is" : "are"} at least the findings; ${unknown.length} of ${report.checks.length} checks could not run.`}
      />

      {unknown.length > 0 ? (
        <ul className="space-y-0.5">
          {unknown.map((c) => (
            <li key={c.check} className="break-words text-[11px] text-[#d29922]">
              <span className="font-semibold">{CHECK_LABEL[c.check]}</span> [could not check]{" "}
              {reason(c)}
            </li>
          ))}
        </ul>
      ) : null}

      {/* Wire order. The backend ranks worst first; re-sorting here would
          be a second ordering to keep in step with `Severity::rank`. */}
      {n > 0 ? (
        <ul className="space-y-2">
          {report.findings.map((f, i) => (
            <FindingRow
              key={`${f.check}:${f.subject.path}:${i}`}
              finding={f}
              repo={repo}
              activePath={activePath}
              onSelectFile={onSelectFile}
            />
          ))}
        </ul>
      ) : null}

      {/* Only a run in which EVERY check completed may say this. The
          partial arm above has already spoken for the other case. */}
      {n === 0 && everyRan ? (
        <p className="text-xs text-[#3fb950]">
          {report.checks.length} {report.checks.length === 1 ? "check" : "checks"} ran; nothing
          found.
        </p>
      ) : null}

      <div className="flex flex-wrap items-center gap-3">
        {n > 0 ? (
          <button
            type="button"
            onClick={() => copyBrief(report.brief, "All briefs")}
            className="tap-target rounded border border-[#30363d] px-2 py-1 text-xs text-[#e6edf3] hover:bg-[#161b22]"
          >
            Copy all briefs
          </button>
        ) : null}
        <button
          type="button"
          onClick={onRecheck}
          disabled={isFetching}
          className="tap-target text-xs text-[#58a6ff] hover:underline disabled:text-[#6e7681]"
        >
          {isFetching ? "Re-checking…" : "Re-check"}
        </button>
      </div>
    </div>
  );
}

function reason(c: ClaudeMdAdviceCoverage): string {
  return c.run.state === "unknown" ? c.run.reason : "";
}

function FindingRow({
  finding,
  repo,
  activePath,
  onSelectFile,
}: {
  finding: ClaudeMdAdviceFinding;
  repo: string;
  activePath: string | undefined;
  onSelectFile: (path: string) => void;
}) {
  const severity = SEVERITY[finding.severity];
  const file = subjectFile(finding.subject);
  return (
    <li className="break-words text-xs">
      <p>
        {/* Severity in TEXT as well as colour: colour alone is not an
            answer for a reader who cannot see it. */}
        <span className={`mr-1 ${severity.className}`}>[{severity.label}]</span>
        <span className="text-[#e6edf3]">{finding.finding}</span>
      </p>
      {/* The subject. A file is a button that shows it, carrying
          `aria-current` when it is the one on screen -- navigation, not
          a toggle. A directory has no file to show. */}
      {file !== null ? (
        <button
          type="button"
          onClick={() => onSelectFile(file)}
          aria-current={current(file === activePath)}
          className={`tap-target mt-0.5 rounded px-1 text-left font-mono text-[11px] ${
            file === activePath ? "bg-[#1f6feb] text-white" : "text-[#58a6ff] hover:bg-[#161b22]"
          }`}
        >
          {shown(file, repo)}
          {finding.subject.kind === "claudeMd" && finding.subject.section !== null
            ? ` ${finding.subject.section}`
            : ""}
        </button>
      ) : (
        <p className="mt-0.5 font-mono text-[11px] text-[#8b949e]">{shown(finding.subject.path, repo)}/</p>
      )}
      <ul className="mt-0.5 space-y-0.5">
        {finding.evidence.map((e, i) => (
          <li key={i} className="text-[11px] text-[#8b949e]">
            <span className="font-mono">{locatorText(e.at, repo)}</span> — {e.measured}
          </li>
        ))}
      </ul>
      {/* A plain button, not `aria-pressed`: copying is an action, not a
          state. The brief itself is not rendered; it is for an agent. */}
      <button
        type="button"
        onClick={() => copyBrief(finding.brief, "Brief")}
        className="tap-target mt-0.5 text-[11px] text-[#58a6ff] hover:underline"
      >
        Copy brief
      </button>
    </li>
  );
}

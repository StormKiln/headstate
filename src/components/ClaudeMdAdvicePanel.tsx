import { useState } from "react";
import { useClaudeMdAdvice } from "@/api/hooks";
import type {
  ClaudeMdAdviceCoverage,
  ClaudeMdAdviceFinding,
  ClaudeMdAdviceLocator,
  ClaudeMdAdviceReport,
  ClaudeMdAdviceSubject,
} from "@/types/pr";
import { current } from "@/lib/ariaCurrent";
import { copyText } from "@/lib/clipboard";
import {
  CHECK_LABEL,
  type AdviceGroup,
  type AdviceGrouping,
  groupFindings,
} from "@/lib/adviceGrouping";
import type { Filters } from "@/lib/derive";
import { useActiveFilters, useFilters } from "@/store/filters";
import { toast } from "sonner";
import { PartialScanNotice } from "./PartialScanNotice";
import { QueryError, errorMessage } from "./QueryError";

/// The three arrangements, and what the control calls them (#1291).
///
/// "Flat" is named rather than left as a bare "off", because it is a real
/// arrangement -- the backend's severity ranking, worst first -- and a
/// user who has grouped needs to be able to name the thing they are going
/// back to.
const GROUPING_OPTIONS: { value: AdviceGrouping; label: string }[] = [
  { value: "none", label: "Flat (worst first)" },
  { value: "check", label: "By check" },
  { value: "file", label: "By file" },
];

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
          report={data.report}
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

  // The grouping preference, from the per-view filter store where every
  // other view preference lives (#1291). Absent means the flat list.
  const { adviceGrouping } = useActiveFilters();
  const setFilter = useFilters((s) => s.setFilter);
  const grouping: AdviceGrouping = adviceGrouping ?? "none";

  // Partitioned, never re-sorted within a group. `groupFindings` states
  // the two orderings and why they differ.
  const groups = groupFindings(report, grouping);

  return (
    <div className="mt-2 space-y-2">
      {/* The shortfall FIRST, and stated as the producer wrote it. The
          findings below are real; what is missing is the checks that
          could not vouch for anything. */}
      <PartialScanNotice
        unreadable={unknown.map((c) => `${CHECK_LABEL[c.check]}: ${reason(c)}`)}
        consequence={`the ${n === 1 ? "finding" : `${n} findings`} below ${n === 1 ? "is" : "are"} at least the findings; ${unknown.length} of ${report.checks.length} checks could not run.`}
      />

      {/* The flat list keeps its own coverage list, because there is no
          by-check group to carry an Unknown. Under `"check"` the Unknowns
          move INTO their group, where the heading names the check and the
          reason sits under it -- printing them twice would read as two
          separate failures of the same producer. Under `"file"` they stay
          here: a check that could not run belongs to no file, and hanging
          it off one would invent a subject the producer never named. */}
      {unknown.length > 0 && grouping !== "check" ? (
        <ul className="space-y-0.5">
          {unknown.map((c) => (
            <li key={c.check} className="break-words text-[11px] text-[#d29922]">
              <span className="font-semibold">{CHECK_LABEL[c.check]}</span> [could not check]{" "}
              {reason(c)}
            </li>
          ))}
        </ul>
      ) : null}

      {/* The arrangement control. Offered whenever there is a report at
          all, including one whose only content is checks that could not
          run -- that is exactly the report the by-check view is most
          worth switching to. */}
      <div className="flex flex-wrap items-center gap-2">
        <label htmlFor="advice-grouping" className="text-[11px] text-[#8b949e]">
          Group:
        </label>
        <select
          id="advice-grouping"
          value={grouping}
          onChange={(e) =>
            setFilter("adviceGrouping", e.target.value as Filters["adviceGrouping"])
          }
          className="tap-target rounded border border-[#30363d] bg-[#0d1117] px-1 py-0.5 text-[11px] text-[#e6edf3]"
        >
          {GROUPING_OPTIONS.map((opt) => (
            <option key={opt.value} value={opt.value}>
              {opt.label}
            </option>
          ))}
        </select>
      </div>

      {/* One render path for all three arrangements: `"none"` is a single
          unlabelled group holding the wire list verbatim. Within a group
          the order is the backend's; between groups it is worst-first, so
          grouping can never bury a problem under a quiet file. */}
      {groups.map((g) => (
        <GroupSection
          key={g.key}
          group={g}
          labelled={grouping !== "none"}
          repo={repo}
          activePath={activePath}
          onSelectFile={onSelectFile}
        />
      ))}

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

/// One group's heading, its findings, and any check that could not run.
///
/// `labelled` is false for the flat arrangement, where the single group
/// is the whole list and a heading over it would name nothing.
///
/// A group whose check could not run renders its reason and NO findings,
/// and the two states read differently on purpose (#846, #1291): "could
/// not check" names the producer's own obstacle, while a check that ran
/// and found nothing produces no group at all -- it is accounted for by
/// the clean sentence below, which only a run in which every check
/// completed is allowed to print. The failure this avoids is a by-check
/// view where a producer that crashed and a producer that found nothing
/// both render as an absence.
function GroupSection({
  group,
  labelled,
  repo,
  activePath,
  onSelectFile,
}: {
  group: AdviceGroup;
  labelled: boolean;
  repo: string;
  activePath: string | undefined;
  onSelectFile: (path: string) => void;
}) {
  // Shortened against the repository root when the label leads with a
  // path, whatever the subject kind -- a directory has a path to shorten
  // and deliberately no file to open, so `file !== null` is the wrong
  // test. A by-check label has no path and is printed as written.
  const heading =
    group.pathLength === 0
      ? group.label
      : shown(group.label.slice(0, group.pathLength), repo) + group.label.slice(group.pathLength);
  return (
    <section className={labelled ? "border-l border-[#21262d] pl-2" : undefined}>
      {labelled ? (
        <h3 className="break-words text-[11px] font-semibold text-[#8b949e]">
          {heading}
          {/* The count is of findings only. An Unknown check is not a
              finding, and counting it as one would say the producer
              found something when it could not look. */}
          {group.findings.length > 0 ? ` (${group.findings.length})` : ""}
        </h3>
      ) : null}

      {group.unknownChecks.length > 0 ? (
        <ul className="mt-0.5 space-y-0.5">
          {group.unknownChecks.map((c) => (
            <li key={c.check} className="break-words text-[11px] text-[#d29922]">
              [could not check] {reason(c)}
            </li>
          ))}
        </ul>
      ) : null}

      {group.findings.length > 0 ? (
        <ul className="mt-1 space-y-2">
          {group.findings.map((f, i) => (
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
    </section>
  );
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

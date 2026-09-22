import { useState } from "react";
import { useClaudeMdAdvice, useUiPrefs } from "@/api/hooks";
import type {
  ClaudeMdAdviceCoverage,
  ClaudeMdAdviceFinding,
  ClaudeMdAdviceLocator,
  ClaudeMdAdviceReport,
  ClaudeMdAdviceResult,
  ClaudeMdAdviceSubject,
} from "@/types/pr";
import { current } from "@/lib/ariaCurrent";
import {
  CHECK_LABEL,
  type AdviceGroup,
  type AdviceGrouping,
  groupFindings,
} from "@/lib/adviceGrouping";
import type { Filters } from "@/lib/derive";
import { useActiveFilters, useFilters } from "@/store/filters";
import { adviceState, needsRefresh, type AdviceState } from "@/lib/adviceState";
import { freshnessLabel } from "@/lib/adviceFreshnessLabel";
import { ClaudifyAction } from "./ClaudifyAction";
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

/// Advice about the CLAUDE.md files of one repository, as the tab's body.
///
/// # What changed in #1290, and what deliberately did not
///
/// This was a collapsed panel in the file rail, fetched only while open.
/// It is now the body of a tab, and the fetch is started by SELECTING A
/// REPOSITORY rather than by pressing anything -- so the advice is ready,
/// or visibly building, by the time the user reaches the tab.
///
/// The reasoning the old `enabled` carried is preserved rather than
/// discarded. The file list and the content pane still never wait on the
/// producers: the advice queries are their own, the page renders its
/// panes without reading them, and nothing here can block a file from
/// appearing. What changed is the trigger, not the independence.
///
/// The distinction that `enabled` drew is preserved too, and it is the
/// one most easily lost in this move. "Never asked" and "asked and came
/// back with nothing" are different claims (#846), so a tab the user has
/// not visited while the fetch is in flight is BUILDING, not empty, and
/// `adviceState` keeps `"idle"` meaning only the never-asked case.
///
/// # Two queries, one surface
///
/// `mode: "cached"` always answers at cache speed. `mode: "fresh"` runs
/// every producer, including a whole-body read of every session under the
/// repository, and is therefore never what a repository click fires. The
/// cached call leads; the fresh one is fired behind it only when the
/// cached answer says `stale: true`, and `adviceState` composes the two
/// into the "from cache, refreshing" state the backend deliberately
/// cannot claim for itself (see `ClaudeMdAdviceFreshness`).
///
/// Everything shown comes off the wire in the backend's order: the panel
/// maps over `report.findings` and `report.checks` and never filters,
/// sorts or concatenates briefs.
///
/// Nothing here has a fixed width. At 390px it is the whole screen, and
/// it breaks words rather than overflowing.
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
  // The cached read, on as soon as there is a repository. This is the
  // auto-fetch #1290 asks for, and it is the CHEAP one: `Mode::Cached`
  // returns the stored report whenever one decodes and runs producers
  // only on a miss.
  const cached = useClaudeMdAdvice(repo, true, "cached");

  // A manual Refresh, remembered per repository.
  //
  // Keyed by repo PATH rather than held as a bare boolean, because a
  // boolean survives a repository switch: pressing Refresh on one
  // repository and clicking to another would fire a full producer run
  // against the new one that nobody asked for. Storing which repository
  // was asked makes the flag false for every other repository by
  // construction, with no effect to reset and no reset to forget.
  const [refreshAsked, setRefreshAsked] = useState<string | null>(null);

  // WHY the fresh call may run. Two reasons, and only two:
  //
  //  - the cached answer says a tracked input has changed (`stale`), so
  //    there is a better report to be had; or
  //  - the user pressed Refresh for THIS repository.
  //
  // `needsRefresh` is deliberately false for `"unverified"`: an input
  // that could not be READ will not read on a second run either, so
  // auto-firing there would spend the whole-body session read on every
  // visit to a repository with one unreadable file and learn nothing.
  // Refresh still works; what is refused is doing it unprompted.
  //
  // # Rapid repository switching
  //
  // This is why the trigger is derived from `cached.data` rather than
  // held in state. `cached.data` is the CURRENT repository's cached
  // result -- the query key carries the path, so switching repositories
  // makes it `undefined` until that repository's own cached call lands.
  // A fresh call can therefore only be enabled for a repository whose
  // cached report is already in hand and already says `stale`, which a
  // user clicking down a sidebar never reaches: each click invalidates
  // the previous repository's `cached.data` before any fresh call for it
  // is enabled. Clicking through ten repositories fires ten cached reads
  // and no producer runs.
  //
  // And a fresh call that DID start keeps its own query key, so it can
  // neither be mistaken for the new repository's answer nor race another
  // run of itself: TanStack dedupes by key, so one repository has at
  // most one fresh call in flight however many times the trigger
  // re-evaluates.
  const wantFresh = needsRefresh(cached.data) || refreshAsked === repo;
  const fresh = useClaudeMdAdvice(repo, wantFresh, "fresh");

  const state = adviceState(
    { ...cached, enabled: true },
    { ...fresh, enabled: wantFresh },
  );

  return (
    <div className="space-y-2">
      <AdviceBody
        state={state}
        repo={repo}
        activePath={activePath}
        onSelectFile={onSelectFile}
        onRefresh={() => {
          setRefreshAsked(repo);
          // A repository already asked for a refresh needs the query
          // re-run rather than re-enabled: the flag is already set, so
          // nothing would change and the button would look inert.
          if (refreshAsked === repo) void fresh.refetch();
        }}
        onRetry={() => {
          if (cached.isError) void cached.refetch();
          if (fresh.isError) void fresh.refetch();
        }}
      />
    </div>
  );
}

/// The five states, each rendered as itself.
///
/// Split out from the component above so the query wiring and the
/// rendering are separately readable, and so a test can drive every arm
/// from a plain `AdviceState` without standing up two queries.
function AdviceBody({
  state,
  repo,
  activePath,
  onSelectFile,
  onRefresh,
  onRetry,
}: {
  state: AdviceState;
  repo: string;
  activePath: string | undefined;
  onSelectFile: (path: string) => void;
  onRefresh: () => void;
  onRetry: () => void;
}) {
  switch (state.kind) {
    // Never asked. Reachable only with no repository selected, which the
    // page handles before it renders this -- but it is a real member of
    // the union rather than folded into "building", because the whole
    // point of keeping it is that it is not the same claim.
    case "idle":
      return <p className="text-xs text-[#8b949e]">Choose a repository to check its files.</p>;
    case "building":
      return <Skeleton />;
    case "failed":
      return (
        <>
          <QueryError
            title="Could not check these files"
            message={errorMessage(state.error)}
            onRetry={onRetry}
          />
          {/* A refresh that failed over a report already on screen keeps
              the report. The findings below were really computed, and
              withdrawing them because the attempt to better them failed
              would turn one failure into two. The error above says what
              happened; the label on the report says how old it is. */}
          {state.stale !== undefined ? (
            <div className="mt-2">
              <Freshness result={state.stale} refreshing={false} onRefresh={onRefresh} />
              <ReportView
                report={state.stale.report}
                repo={repo}
                activePath={activePath}
                onSelectFile={onSelectFile}
              />
            </div>
          ) : null}
        </>
      );
    case "report":
      return (
        <>
          <Freshness
            result={state.result}
            refreshing={state.refreshing}
            onRefresh={onRefresh}
          />
          <ReportView
            report={state.result.report}
            repo={repo}
            activePath={activePath}
            onSelectFile={onSelectFile}
          />
        </>
      );
  }
}

/// Where this report came from, above the findings it qualifies.
///
/// ABOVE rather than beside or below: a reader who scrolls into a
/// finding and acts on it has already passed this line, and a currency
/// caveat placed after the thing it qualifies is one most readers never
/// reach.
///
/// `aria-live="polite"` because the text CHANGES UNDER THE READER: the
/// composed "showing the last check while a new one runs" is replaced by
/// the fresh report's label when the fresh call lands, with no
/// interaction to prompt it. A silent swap is the one case where a
/// screen-reader user would be left acting on the older claim.
function Freshness({
  result,
  refreshing,
  onRefresh,
}: {
  result: ClaudeMdAdviceResult;
  refreshing: boolean;
  onRefresh: () => void;
}) {
  const label = freshnessLabel(result.freshness, result.computedAt, refreshing);
  const tone = {
    current: "text-[#3fb950]",
    stale: "text-[#d29922]",
    // The same amber as stale, not a muted grey. "Could not decide"
    // rendered quietly is how an unchecked thing becomes a cleared one
    // in a reader's head (#1042).
    unknown: "text-[#d29922]",
  }[label.tone];

  return (
    <div className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5" aria-live="polite">
      <span className={`break-words text-[11px] ${tone}`}>{label.text}</span>
      <span className="break-words text-[11px] text-[#8b949e]">{label.detail}</span>
      <button
        type="button"
        onClick={onRefresh}
        disabled={refreshing}
        className="tap-target text-[11px] text-[#58a6ff] hover:underline disabled:text-[#6e7681]"
      >
        {refreshing ? "Re-checking…" : "Re-check"}
      </button>
    </div>
  );
}

/// "Not measured yet", and nothing else. Never "no advice".
///
/// The state a tab the user has not visited is in while the first run is
/// going -- which is why it says "Checking" rather than showing an empty
/// box. `aria-busy` carries the same fact to a reader who cannot see the
/// pulse.
function Skeleton() {
  return (
    <div aria-busy="true">
      <p className="text-xs text-[#8b949e]">Checking…</p>
      <ul className="mt-1 space-y-1">
        {[0, 1].map((i) => (
          <li key={i} className="h-3 w-full rounded bg-[#21262d] motion-safe:animate-pulse" />
        ))}
      </ul>
    </div>
  );
}

/// The report itself: what could not be checked, then what was found.
///
/// Currency is NOT this component's business -- `Freshness` above owns
/// the "where did this come from" line and the Re-check button, so a
/// report rendered from cache and the same report rendered fresh are
/// byte-identical here. Two places rendering a currency claim is how the
/// two come to disagree.
function ReportView({
  report,
  repo,
  activePath,
  onSelectFile,
}: {
  report: ClaudeMdAdviceReport;
  repo: string;
  activePath: string | undefined;
  onSelectFile: (path: string) => void;
}) {
  // Whether Run can be offered at all (#1292). `terminal_command` is the
  // configured terminal and therefore the thing that answers "if one is
  // configured" -- the same setting `claude_launch_session` reads. There
  // is deliberately no second shell setting.
  const { prefs } = useUiPrefs();
  const terminalConfigured = (prefs?.terminal_command ?? "").trim() !== "";

  // Which position in `report.findings` a finding occupies.
  //
  // The INDEX is what a Claudify sends, and Rust resolves it against the
  // stored report -- so it has to be the wire position, not the position
  // within a group. `groupFindings` partitions the same objects rather
  // than copying them, so identity is exactly the right lookup: it is
  // true by construction for every arrangement, where a key built from
  // `check` and `path` would collide whenever one file has two findings
  // from the same check and silently Claudify the wrong one.
  const wireIndex = (f: ClaudeMdAdviceFinding) => report.findings.indexOf(f);

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
    <div className="space-y-2">
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
          terminalConfigured={terminalConfigured}
          wireIndex={wireIndex}
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

      {/* Re-check lives on the freshness line above, beside the claim it
          acts on, rather than here. */}
      {/* Claudify-all, on `Report.brief` -- "every finding's brief plus a
          `_Could not check: {reason}_` line per Unknown check". The
          whole-report equivalent of a finding's brief, and rendered by
          the same backend for the same reason, so this concatenates
          nothing. */}
      {n > 0 ? (
        <div className="flex flex-wrap items-center gap-3">
          <ClaudifyAction
            brief={report.brief}
            repo={repo}
            target={{ kind: "report" }}
            what="All briefs"
            terminalConfigured={terminalConfigured}
          />
        </div>
      ) : null}
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
  terminalConfigured,
  wireIndex,
}: {
  group: AdviceGroup;
  labelled: boolean;
  repo: string;
  activePath: string | undefined;
  onSelectFile: (path: string) => void;
  terminalConfigured: boolean;
  /// This finding's position in `report.findings`, which is what a
  /// Claudify sends. Passed down rather than recomputed, because the
  /// group does not hold the wire list.
  wireIndex: (f: ClaudeMdAdviceFinding) => number;
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
              index={wireIndex(f)}
              repo={repo}
              activePath={activePath}
              onSelectFile={onSelectFile}
              terminalConfigured={terminalConfigured}
            />
          ))}
        </ul>
      ) : null}
    </section>
  );
}

function FindingRow({
  finding,
  index,
  repo,
  activePath,
  onSelectFile,
  terminalConfigured,
}: {
  finding: ClaudeMdAdviceFinding;
  /// Position in `report.findings`. Claudify sends this, not the brief.
  index: number;
  repo: string;
  activePath: string | undefined;
  onSelectFile: (path: string) => void;
  terminalConfigured: boolean;
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
      {/* Claudify (#1292): the brief, copied or run. Plain buttons, not
          `aria-pressed` -- these are actions, not states. The brief
          itself is still not rendered inline; it is for an agent, and
          the Run panel shows it only as the argv that will carry it. */}
      <div className="mt-0.5">
        <ClaudifyAction
          brief={finding.brief}
          repo={repo}
          target={{ kind: "finding", index }}
          what="Brief"
          terminalConfigured={terminalConfigured}
        />
      </div>
    </li>
  );
}

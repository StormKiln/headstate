import type {
  ClaudeMdAdviceCheck,
  ClaudeMdAdviceCoverage,
  ClaudeMdAdviceFinding,
  ClaudeMdAdviceReport,
  ClaudeMdAdviceSubject,
} from "@/types/pr";

/// How the advice list is organised (#1291).
///
/// `"none"` is the flat list: the backend's own `Severity::rank` order end
/// to end, the one arrangement in which position means exactly one thing.
/// `"check"` is the DEFAULT since #1344, because for most repositories the
/// flat list is hundreds of rows and unusable. Every grouping reorders --
/// a critical finding becomes the first row of some group -- which is why
/// groups are ordered worst-first below, so a problem still leads.
export type AdviceGrouping = "none" | "check" | "file";

/// Severity rank, mirroring `Severity::rank` in
/// `src-tauri/src/claudemd/advice/mod.rs`.
///
/// Used ONLY to compare GROUPS against each other. Findings inside a
/// group keep the order the wire delivered them in -- the backend has
/// already ranked them and is stable within a rank, and a second sort
/// here would be a second ordering to keep in step with the first.
///
/// `unknown` ranks last of the three and still ABOVE nothing, which is
/// the point of the Rust comment this mirrors: a group holding only
/// "could not decide" is not a clean group, so it must not sort below
/// one that is.
const RANK: Record<ClaudeMdAdviceFinding["severity"], number> = {
  problem: 0,
  advice: 1,
  unknown: 2,
  note: 3,
};

/// The key of the Observations group: every `note` finding, in every
/// arrangement, and always last (#1339).
///
/// A Note is an observation with no recommendation -- a count of what a
/// check covered, a rule found already written. Left in its check's or
/// its file's group it would sit among the things to change and read as
/// one, which is the defect #1339 names. So it is partitioned out before
/// grouping, and an arrangement is only ever an arrangement of advice.
export const OBSERVATIONS_KEY = "observations";

/// Whether a finding is advice at all, as opposed to an observation.
/// What the panel counts, and what every advice group holds.
export function isAdvice(f: ClaudeMdAdviceFinding): boolean {
  return f.severity !== "note";
}

/// One group of findings, and the coverage rows that belong to it.
export interface AdviceGroup {
  /// Stable within a report: the React key and what tests assert on.
  key: string;
  /// What the heading reads. For a file grouping it LEADS with the
  /// subject's path, absolute as the wire carries it, so the panel can
  /// shorten it against the repository root; `pathLength` says how much
  /// of it is that path.
  label: string;
  /// How many leading characters of `label` are the absolute path, or 0
  /// when the label holds no path at all (every by-check group, and the
  /// flat arrangement's single group).
  ///
  /// Here rather than left to the panel because the panel would have to
  /// re-derive it from the subject -- a second copy of the rule that a
  /// skill's label is "path (skill: name)" and a directory's is "path/",
  /// living in a different file from the one that writes them. It cannot
  /// be replaced by `file !== null`: a directory group has a path to
  /// shorten and deliberately no file to open.
  pathLength: number;
  /// A file the group is about, when the group names one the page can
  /// open. `null` for a directory group (no file exists yet) and for
  /// every by-check group.
  file: string | null;
  /// In wire order. NEVER re-sorted.
  findings: ClaudeMdAdviceFinding[];
  /// Checks that could not run and belong under this group, with the
  /// producer's own reason. Non-empty only in the by-check grouping,
  /// where a check is the thing the group IS.
  ///
  /// A check that could not run has zero findings for a reason that is
  /// not "found nothing", and this is the view organised by check --
  /// letting it vanish for being empty is exactly the #846 failure
  /// (#1291).
  unknownChecks: ClaudeMdAdviceCoverage[];
}

/// What each check is called. Exported so the panel and the by-check
/// grouping name a check identically; a `Record` so a variant added to
/// the wire type without a label fails to compile.
export const CHECK_LABEL: Record<ClaudeMdAdviceCheck, string> = {
  imports: "imports",
  toolchain: "toolchain coverage",
  transcripts: "sessions",
  gaps: "missing subdirectory files",
  placement: "placement",
  rot: "rot",
  skills: "skills",
  shape: "content shape",
};

/// The group a subject belongs to under the by-file grouping.
///
/// Every `Subject` kind gets an arm and a label of its own. The switch is
/// exhaustive over the tagged union, so a kind added to
/// `ClaudeMdAdviceSubject` fails to compile here rather than falling into
/// a default arm and quietly becoming ungrouped -- "ungrouped because
/// unhandled" is the bug #1291 names, not a default.
///
/// The three kinds are genuinely different things and are labelled as
/// what they are:
///
/// - `claudeMd` -- a file that exists. The group is openable.
/// - `directory` -- "a finding about a file that does not exist yet", so
///   the label says the directory, with a trailing `/`, and carries NO
///   file: offering to open a file that is not there is a dead click.
/// - `skill` -- a `SKILL.md`, named by the name it is INVOKED with as
///   well as its path. Dropping these for not being CLAUDE.md would lose
///   the skills producer's whole output from the file view.
///
/// Keyed by kind AND path, so a directory `docs/` and a CLAUDE.md that
/// happens to live at `docs/` could never collide into one group.
function fileGroupOf(subject: ClaudeMdAdviceSubject): Omit<AdviceGroup, "findings" | "unknownChecks"> {
  const pathLength = subject.path.length;
  switch (subject.kind) {
    case "claudeMd":
      return {
        key: `claudeMd:${subject.path}`,
        label: subject.path,
        pathLength,
        file: subject.path,
      };
    case "directory":
      // The trailing slash is the whole signal that this is a place a
      // file is missing from rather than a file that was read. `file` is
      // null because there is nothing to open -- a button here would be
      // a dead click on a path that does not exist.
      return {
        key: `directory:${subject.path}`,
        label: `${subject.path}/`,
        pathLength,
        file: null,
      };
    case "skill":
      return {
        key: `skill:${subject.path}`,
        label: `${subject.path} (skill: ${subject.name})`,
        pathLength,
        file: subject.path,
      };
  }
}

/// Group a report's findings, preserving the backend's ranking.
///
/// Two orderings, and they are not the same rule:
///
/// - WITHIN a group, wire order, untouched. `Report.findings` arrives in
///   `Severity::rank` order and is stable within a rank; this function
///   only ever partitions that sequence, so each group is a subsequence
///   of it and the relative order of any two findings is the backend's.
///
/// - BETWEEN groups, by the most severe finding each group holds, then
///   by first appearance on the wire to break a tie. Alphabetical would
///   bury a `problem` in `zzz.md` under an `advice` in `aaa.md`, which
///   turns organising the list into re-ranking it -- the thing the epic
///   says grouping must not silently become. The tie-break is first
///   appearance rather than the label so that two equally severe groups
///   still appear in the order the backend thought about them, and so the
///   result is deterministic.
///
/// `"none"` returns a single unlabelled group, which is the flat list.
/// One code path renders every mode.
export function groupFindings(report: ClaudeMdAdviceReport, grouping: AdviceGrouping): AdviceGroup[] {
  // Partitioned, not sorted: each half is a subsequence of the wire. With
  // no Notes the advice half IS the wire array, untouched.
  const notes = report.findings.filter((f) => !isAdvice(f));
  const advice = notes.length === 0 ? report.findings : report.findings.filter(isAdvice);
  const observations: AdviceGroup[] =
    notes.length === 0
      ? []
      : [
          {
            key: OBSERVATIONS_KEY,
            label: "Observations",
            pathLength: 0,
            file: null,
            findings: notes,
            unknownChecks: [],
          },
        ];
  // A report of only Notes has no advice to list, and an empty flat
  // group above the observations would be read as the clean list.
  if (grouping === "none" && advice.length === 0 && notes.length > 0) return observations;
  return [...groupAdvice(report, advice, grouping), ...observations];
}

function groupAdvice(
  report: ClaudeMdAdviceReport,
  findings: ClaudeMdAdviceFinding[],
  grouping: AdviceGrouping,
): AdviceGroup[] {
  if (grouping === "none") {
    return [
      {
        key: "all",
        label: "",
        pathLength: 0,
        file: null,
        findings,
        unknownChecks: [],
      },
    ];
  }

  const groups = new Map<string, AdviceGroup>();
  // First appearance on the wire, which is the tie-break below.
  // Explicit rather than relying on the map's insertion order, because
  // the by-check pass below adds groups for Unknown checks AFTER the
  // findings loop and gives them positions past the end of the findings
  // list -- an ordering the map could not express on its own.
  const firstSeen = new Map<string, number>();

  for (const [i, f] of findings.entries()) {
    const shell =
      grouping === "check"
        ? { key: `check:${f.check}`, label: CHECK_LABEL[f.check], pathLength: 0, file: null }
        : fileGroupOf(f.subject);
    let group = groups.get(shell.key);
    if (group === undefined) {
      group = { ...shell, findings: [], unknownChecks: [] };
      groups.set(shell.key, group);
      firstSeen.set(shell.key, i);
    }
    // Push, never insert-by-rank: the wire order IS the rank order, and
    // appending preserves it exactly.
    group.findings.push(f);
  }

  if (grouping === "check") {
    // A check that could not run gets a group even with no findings, and
    // it is NOT an empty group meaning "clean" -- `unknownChecks` is what
    // the panel renders the reason from. Seeded after the findings loop
    // so a check with both findings and an Unknown coverage row (it can
    // only be one or the other today, but the wire does not forbid it)
    // keeps the position its findings earned.
    for (const c of report.checks) {
      if (c.run.state !== "unknown") continue;
      const key = `check:${c.check}`;
      let group = groups.get(key);
      if (group === undefined) {
        group = {
          key,
          label: CHECK_LABEL[c.check],
          pathLength: 0,
          file: null,
          findings: [],
          unknownChecks: [],
        };
        groups.set(key, group);
        // Ordered after every group that holds a finding, by giving it a
        // first-appearance past the end of the findings list. An Unknown
        // is a fact about coverage rather than a finding of any severity,
        // so it has no rank to compare -- but it must not be sorted away
        // either, and `worstRank` below treats an empty group as worse
        // than nothing so it stays visible among the results.
        firstSeen.set(key, findings.length + report.checks.indexOf(c));
      }
      group.unknownChecks.push(c);
    }
  }

  return [...groups.values()].sort((a, b) => {
    const rank = worstRank(a) - worstRank(b);
    if (rank !== 0) return rank;
    return (firstSeen.get(a.key) ?? 0) - (firstSeen.get(b.key) ?? 0);
  });
}

/// The rank of a group's most severe finding -- the smallest `RANK`, the
/// worst thing in it.
///
/// A group with no findings at all is a by-check group that exists only
/// because its check could not run. It ranks with `unknown`, NOT past the
/// end of the list: "could not check" and "could not decide" are the same
/// claim at two granularities, and sorting the coverage Unknowns below
/// every result would put the one thing the report cannot vouch for last,
/// which is precisely what `Severity::rank`'s comment refuses.
function worstRank(group: AdviceGroup): number {
  let worst = RANK.unknown;
  for (const f of group.findings) {
    const r = RANK[f.severity];
    if (r < worst) worst = r;
  }
  return worst;
}

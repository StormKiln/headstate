import type {
  ClaudeMdAdviceFinding,
  ClaudeMdAdviceLocator,
  ClaudeMdAdviceSubject,
} from "@/types/pr";
import { type AdviceGroup, REPOSITORY_ROOT, isRepositoryRoot } from "./adviceGrouping";

/// The words the advice panel shows, shared by the panel and the markdown
/// it copies (#1399).
///
/// One implementation of each, because the markdown is meant to read the
/// way the panel does: a path shortened one way on screen and another way
/// in the paste would send the reader to two different places.

/// What each severity is called. The panel colours it; this names it.
///
/// `unknown` gets its own words, never a muted version of anything:
/// "could not decide" rendered quietly is how an unchecked thing becomes
/// a cleared one in a reader's head (#1042).
export const SEVERITY_LABEL: Record<ClaudeMdAdviceFinding["severity"], string> = {
  problem: "problem",
  advice: "advice",
  unknown: "could not decide",
  note: "observation",
};

/// The order a group's heading counts severities in: worst first, the
/// same rank the backend sorts by.
const SEVERITY_ORDER: ClaudeMdAdviceFinding["severity"][] = ["problem", "advice", "unknown", "note"];

/// "1 problem", "2 advice", "3 could not decide", "4 observations".
export function severityCount(severity: ClaudeMdAdviceFinding["severity"], count: number): string {
  const label = SEVERITY_LABEL[severity];
  const plural = count !== 1 && (severity === "problem" || severity === "note");
  return `${count} ${label}${plural ? "s" : ""}`;
}

/// A group's findings counted by severity, worst first, zeroes omitted.
///
/// Findings only. An Unknown check is not a finding, and counting it as
/// one would say the producer found something when it could not look.
export function groupCounts(
  group: AdviceGroup,
): (readonly [ClaudeMdAdviceFinding["severity"], number])[] {
  return SEVERITY_ORDER.map(
    (s) => [s, group.findings.filter((f) => f.severity === s).length] as const,
  ).filter(([, c]) => c > 0);
}

/// A path as the row shows it: relative to the repository when it is
/// inside it, absolute otherwise. Display only; the wire keeps absolute
/// paths, and the brief prints them as the backend rendered them.
///
/// The repository itself is [`REPOSITORY_ROOT`], never the empty string
/// its shortening would leave (#1366).
function shown(path: string, repo: string): string {
  if (isRepositoryRoot(path, repo)) return REPOSITORY_ROOT;
  // Inside the repository only at a separator boundary (#1387). A bare
  // prefix test shortened `<repo>-other/CLAUDE.md`, a sibling that merely
  // shares the name, to the fragment `-other/CLAUDE.md`. Either separator,
  // because a Windows root arrives with `\`.
  const base = repo.replace(/[/\\]+$/, "");
  const rest = path.slice(base.length);
  return base !== "" && path.startsWith(base) && /^[/\\]/.test(rest)
    ? rest.replace(/^[/\\]+/, "")
    : path;
}

/// A directory subject as the row shows it: shortened, with the trailing
/// slash that says no file exists there yet -- except the repository
/// itself, which would otherwise read as a bare `/` (#1366).
function shownDir(path: string, repo: string): string {
  return isRepositoryRoot(path, repo) ? REPOSITORY_ROOT : `${shown(path, repo)}/`;
}

/// What a finding is about, as the Where column shows it.
export function subjectText(subject: ClaudeMdAdviceSubject, repo: string): string {
  switch (subject.kind) {
    case "claudeMd":
      return subject.section === null
        ? shown(subject.path, repo)
        : `${shown(subject.path, repo)} ${subject.section}`;
    case "skill":
      return shown(subject.path, repo);
    case "directory":
      return shownDir(subject.path, repo);
  }
}

/// Where a piece of evidence is.
export function locatorText(at: ClaudeMdAdviceLocator, repo: string): string {
  if (at.kind === "file") {
    // A line ONLY when the backend recorded one. `:0` or a guessed line
    // would send the reader to a confident wrong place.
    return at.line === null ? shown(at.path, repo) : `${shown(at.path, repo)}:${at.line}`;
  }
  return at.record === null ? `session ${at.sessionId}` : `session ${at.sessionId} record ${at.record}`;
}

/// A group's heading, shortened against the repository root when the
/// label leads with a path, whatever the subject kind -- a directory has
/// a path to shorten and deliberately no file to open, so `file !== null`
/// is the wrong test. A by-check label has no path and is printed as
/// written.
export function groupHeading(group: AdviceGroup, repo: string): string {
  return group.pathLength === 0
    ? group.label
    : shown(group.label.slice(0, group.pathLength), repo) + group.label.slice(group.pathLength);
}

/// What the findings on screen are worth when some checks could not run
/// (#1409) -- the clause after "N of M checks could not run, so". Shared
/// by the panel's notice and the copied markdown so the two can never
/// say different things about the same report.
///
/// Nothing found gets its own words: "the 0 findings below are at least
/// the findings" read as a clean pass, which is the one reading a
/// shortfall must not allow (absent is not zero, #846).
export function shortfallConsequence(n: number): string {
  if (n === 0) return "the empty list below is not a clean result.";
  return n === 1
    ? "the finding below is at least the findings."
    : `the ${n} findings below are at least the findings.`;
}

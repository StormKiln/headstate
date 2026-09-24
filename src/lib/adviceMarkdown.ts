import type { ClaudeMdAdviceFinding, ClaudeMdAdviceResult } from "@/types/pr";
import { freshnessLabel } from "./adviceFreshnessLabel";
import {
  type AdviceGroup,
  type AdviceGrouping,
  CHECK_LABEL,
  REPOSITORY_ROOT,
  groupFindings,
  isAdvice,
} from "./adviceGrouping";
import {
  SEVERITY_LABEL,
  groupCounts,
  groupHeading,
  locatorText,
  severityCount,
  shortfallConsequence,
  subjectText,
} from "./adviceText";

/// The advice panel as markdown, for pasting into a Claude session to
/// review (#1399).
///
/// This is the READER-FACING report -- what the panel shows, in the
/// arrangement it shows it -- not the briefs, which "Copy all briefs"
/// already carries and which are written for an agent to act on.
///
/// Every word comes from the same functions the panel renders with
/// (`adviceText`), so a path reads the same on screen and in the paste,
/// and the repository itself is "repository root" in both (#1366, #1387).

/// A table cell: one line, with `|` escaped so it cannot end the cell.
///
/// Escaped EVERYWHERE, code spans included. GitHub-flavoured markdown
/// splits a row on unescaped pipes before it parses inlines, so a bare
/// `|` inside backticks still breaks the row -- and `\|` inside a code
/// span renders as `|`. The backticks themselves are left alone, so a
/// code span stays a code span.
function cell(text: string): string {
  return oneLine(text).replace(/\|/g, "\\|");
}

/// Newlines folded to spaces, which is what markdown does to them inside
/// a paragraph anyway; a table row or a list item cannot hold one.
function oneLine(text: string): string {
  return text.replace(/\s*\n\s*/g, " ");
}

/// A path as a code span, with a fence longer than any run of backticks
/// inside it. "repository root" is a name, not a path, and stays prose.
function code(text: string): string {
  if (text === REPOSITORY_ROOT) return text;
  const longest = Math.max(0, ...(text.match(/`+/g) ?? []).map((run) => run.length));
  const fence = "`".repeat(longest + 1);
  const pad = text.startsWith("`") || text.endsWith("`") ? " " : "";
  return `${fence}${pad}${text}${pad}${fence}`;
}

/// What a flat arrangement's single group is called. The panel shows it
/// with no heading; the paste needs one to hang the counts off.
const FLAT_HEADING = "Findings, worst first";

/// One group: its heading with counts, the checks under it that could not
/// run, its findings as a table, and their evidence as nested lists.
export function groupMarkdown(group: AdviceGroup, repo: string): string {
  const title = groupHeading(group, repo) || FLAT_HEADING;
  const counts = groupCounts(group).map(([s, c]) => severityCount(s, c));
  // A group with no findings is a check that could not run. It says so
  // rather than showing no counts, which would read as a clean check.
  const suffix = counts.length > 0 ? counts.join(", ") : "could not check";
  const out = [`### ${title} (${suffix})`];

  for (const c of group.unknownChecks) {
    if (c.run.state === "unknown") out.push("", `_Could not check: ${oneLine(c.run.reason)}_`);
  }

  if (group.findings.length > 0) {
    out.push("", "| Severity | Finding | Where |", "| --- | --- | --- |");
    for (const f of group.findings) {
      out.push(
        `| ${SEVERITY_LABEL[f.severity]} | ${cell(f.finding)} | ${cell(code(subjectText(f.subject, repo)))} |`,
      );
    }
  }

  // After the table, because a cell cannot hold a list cleanly.
  const withEvidence = group.findings.filter((f) => f.evidence.length > 0);
  if (withEvidence.length > 0) {
    out.push("", "Evidence:", "");
    for (const f of withEvidence) out.push(...evidence(f, repo));
  }
  return out.join("\n");
}

function evidence(f: ClaudeMdAdviceFinding, repo: string): string[] {
  return [
    `- ${oneLine(f.finding)} (${code(subjectText(f.subject, repo))})`,
    ...f.evidence.map((e) => `  - ${code(locatorText(e.at, repo))} — ${oneLine(e.measured)}`),
  ];
}

/// The whole report, in the grouping the panel is showing.
///
/// `refreshing` and `now` are what the freshness line on screen was
/// rendered with, so the paste says the same thing about currency. The
/// time is also given absolutely: "3 hours ago" is wrong the day after
/// it is pasted.
export function reportMarkdown(
  result: ClaudeMdAdviceResult,
  repo: string,
  grouping: AdviceGrouping,
  refreshing: boolean,
  now: Date = new Date(),
): string {
  const { report } = result;
  const label = freshnessLabel(result.freshness, result.computedAt, refreshing, now);
  const out = [
    `## CLAUDE.md advice for ${code(repo)}`,
    "",
    `${label.text}. ${label.detail} (${result.computedAt}), by Headstate ${result.build}.`,
  ];

  // The same claims the panel makes above its groups, in the same terms.
  const unknown = report.checks.filter((c) => c.run.state === "unknown");
  const n = report.findings.filter(isAdvice).length;
  const notes = report.findings.length - n;
  if (unknown.length > 0) {
    out.push(
      "",
      `${unknown.length} of ${report.checks.length} checks could not run, so ${shortfallConsequence(n)}`,
    );
  } else if (n === 0) {
    const checks = `${report.checks.length} ${report.checks.length === 1 ? "check" : "checks"}`;
    out.push("", `${checks} ran; ${notes === 0 ? "nothing found." : "no advice."}`);
  }

  // Under by-check a check that could not run is its own group. Under
  // the other two it belongs to no group, so it is listed on its own --
  // never dropped (absent is not zero).
  if (unknown.length > 0 && grouping !== "check") {
    out.push("", "### Could not check", "");
    for (const c of unknown) {
      if (c.run.state === "unknown") out.push(`- ${CHECK_LABEL[c.check]}: ${oneLine(c.run.reason)}`);
    }
  }

  for (const g of groupFindings(report, grouping)) {
    // The flat arrangement's single group is empty when there is no
    // advice; the panel renders no rows for it, so neither does this.
    if (g.findings.length === 0 && g.unknownChecks.length === 0) continue;
    out.push("", groupMarkdown(g, repo));
  }
  return `${out.join("\n")}\n`;
}

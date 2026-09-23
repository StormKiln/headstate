import type { ClaudeMdAdviceReport } from "@/types/pr";
import { isAdvice } from "./adviceGrouping";

/// What a finished Re-check says (#1343).
///
/// A re-check that finds the same thing re-renders identical content, so
/// without this "nothing changed" and "nothing happened" look the same
/// (the #1050 class). The toast says the run finished, what it found, and
/// how that compares with the report it replaced.
///
/// Counts advice only: a Note is an observation, never counted as advice
/// (#1339). "No change" is claimed only when the findings are the same
/// findings, not merely the same number of them -- an equal count over
/// different findings is said as exactly that.
export function recheckSummary(
  replaced: ClaudeMdAdviceReport | undefined,
  next: ClaudeMdAdviceReport,
): { title: string; description: string | undefined } {
  const now = next.findings.filter(isAdvice);
  const title = `Re-checked: ${now.length} ${now.length === 1 ? "finding" : "findings"}`;
  if (replaced === undefined) return { title, description: undefined };

  const before = replaced.findings.filter(isAdvice);
  const delta = now.length - before.length;
  if (delta < 0) return { title, description: `${-delta} fewer than the report it replaced.` };
  if (delta > 0) return { title, description: `${delta} more than the report it replaced.` };

  const key = (f: ClaudeMdAdviceReport["findings"][number]) =>
    `${f.check}\u0000${f.severity}\u0000${f.subject.path}\u0000${f.finding}`;
  const a = before.map(key).sort();
  const b = now.map(key).sort();
  const same = a.every((k, i) => k === b[i]);
  return {
    title,
    description: same
      ? "No change from the report it replaced."
      : "The same number as the report it replaced, but not the same findings.",
  };
}

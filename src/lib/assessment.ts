import type { Assessment } from "@/types/pr";

/// One line summarising what a worktree is holding.
///
/// Reads left to right in the order a person decides: how much work,
/// how old, and whether it exists anywhere but this machine. That last
/// one is the fact that actually decides whether deleting is
/// recoverable, so it is never abbreviated away.
///
/// Every field is optional on the Rust side -- git can fail to answer
/// any of them -- and an absent number is SKIPPED rather than rendered
/// as zero. "0 commits ahead" and "we could not count" are opposite
/// answers, and printing the first for the second is exactly the
/// confident-wrong-answer failure this codebase keeps guarding against.
///
/// Skipping is not always the honest direction, though (#976). It is
/// right for a count, whose absence claims nothing. It is wrong where the
/// MISSING phrase is itself the reassurance: no "never pushed" reads as
/// pushed, and no word about uncommitted work reads as a clean tree. So
/// `has_upstream` and `uncommitted` each say "unknown" for `null` while a
/// measured zero stays silent as before.
export function assessmentSummary(a: Assessment): string {
  const parts: string[] = [];

  if (a.commits_ahead !== null) {
    parts.push(`${a.commits_ahead} commit${a.commits_ahead === 1 ? "" : "s"} ahead`);
  }
  if (a.files_changed !== null) {
    parts.push(`${a.files_changed} file${a.files_changed === 1 ? "" : "s"}`);
  }
  // Only when at least one side is known, and each side independently:
  // git reports insertions and deletions together, but a diff of pure
  // deletions genuinely has no insertions line.
  if (a.insertions !== null || a.deletions !== null) {
    parts.push(`+${a.insertions ?? 0}/-${a.deletions ?? 0}`);
  }
  if (a.last_activity !== null) parts.push(a.last_activity);
  // Uncommitted work is not in the diff against the base ref, so it is
  // the one number here that the commit counts cannot imply. A measured
  // zero stays out -- a clean tree is the common case and saying so on
  // every row is noise -- but `null` gets said, because the absence of
  // this phrase is what a reader takes for "clean" (#976).
  if (a.uncommitted === null) parts.push("uncommitted work unknown");
  else if (a.uncommitted > 0) {
    parts.push(`${a.uncommitted} uncommitted`);
  }
  // Three states, and only `false` is the warning. `null` means git could
  // not be asked, and asserting "never pushed" for it is a fabricated
  // claim about the fact that decides whether deleting is recoverable.
  if (a.has_upstream === null) parts.push("push state unknown");
  else if (!a.has_upstream) parts.push("never pushed");

  return parts.join(" · ");
}

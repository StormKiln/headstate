import type { PullRequest } from "@/types/pr";
import { readyAge } from "./readyAge";
import type { Pusher } from "./readyPusher";

/// The "Ready for review" strip as a markdown list (#1578), and the list
/// the strip's batch Claudify appends to its prompt (#1579).
///
/// ONE function for both, on purpose: the issues name this seam. A
/// second copy for the prompt would drift from the one people paste, and
/// then Claude would be told about a different set of facts than the
/// reader saw. `readyListMarkdown` is the only entry point; `forAgent`
/// ADDS a header line and lines under each item, and never changes the
/// item lines both share.
///
/// # What it lists
///
/// Exactly the rows it is given, in the order given. The caller passes
/// the rows the strip is SHOWING -- after its filters, in its sort --
/// so the copy is of what is on screen, not of what the query returned.
///
/// # Unknowns are left out or marked, never guessed
///
/// - A ready time that is absent or unparseable is "ready time unknown",
///   never an age (`readyAge` states why).
/// - On the shared line the last pusher appears only when KNOWN. On the
///   agent's lines it is always stated: a login, "not checked" (nobody
///   asked) or "unknown" (asked, could not tell). Neither of the last two
///   is ever written as "not you" -- the one reading that would let
///   Claude approve a pull request the viewer pushed last.
/// - An unresolved-conversation count that may be a floor is written as
///   one ("at least 3"). An absent count is "unresolved conversations
///   unknown", not zero.

/// Who last moved a pull request's head branch, as far as the app knows.
///
/// Three states because they are three different facts (#1576):
/// - `known`: read, and it names a login.
/// - `unknown`: asked, and the answer could not decide it.
/// - `not-checked`: never asked (not yet, or past the budget).
export type LastPusher =
  | { state: "known"; login: string }
  | { state: "unknown" }
  | { state: "not-checked" };

/// The strip's pusher (#1576) as the list states it.
///
/// `viewer` and `other` both name a login, and the list prints the login
/// either way: "you" would mean nothing once pasted elsewhere. `pending`
/// is nobody-asked, so it is "not checked", not "unknown".
export function lastPusherOf(p: Pusher): LastPusher {
  switch (p.state) {
    case "viewer":
    case "other":
      return { state: "known", login: p.login };
    case "unknown":
      return { state: "unknown" };
    case "pending":
      return { state: "not-checked" };
  }
}

/// One row of the list: the pull request, plus what the strip knows
/// about it beyond the list query.
export interface ReadyRow {
  pr: PullRequest;
  /// Absent means nobody asked.
  lastPusher?: LastPusher;
  /// True when `pr.unresolved_threads` counts only the threads that
  /// arrived, so the true number may be higher (#802).
  unresolvedIsFloor?: boolean;
}

export interface ReadyListOptions {
  /// When the list was built. Passed in, so the output is testable.
  now: Date;
  /// Add what an agent needs to re-check each pull request itself
  /// (#1579): the repository and number, the head commit, and the last
  /// pusher in every state, under a header that labels every value as a
  /// snapshot.
  forAgent?: boolean;
}

/// Escape a title so it cannot break the list or its link.
///
/// The characters that change the structure: `[` and `]` end the link
/// text, `|` starts a table cell where a paste lands in one, a backtick
/// opens a code span that can swallow the rest of the line, and `\`
/// would otherwise escape whatever we put after it. A line break would
/// end the list item, so it becomes a space.
export function escapeMarkdownText(text: string): string {
  return text.replace(/\r\n?|\n/g, " ").replace(/([\\[\]|`])/g, "\\$1");
}

/// A code span that holds any text, backticks included.
///
/// Git allows a backtick in a branch name. The CommonMark rule: fence
/// with one more backtick than the longest run inside, and pad with a
/// space when the text starts or ends with one.
function codeSpan(text: string): string {
  const flat = text.replace(/\r\n?|\n/g, " ");
  const longest = Math.max(0, ...(flat.match(/`+/g) ?? []).map((run) => run.length));
  const fence = "`".repeat(longest + 1);
  const pad = flat.startsWith("`") || flat.endsWith("`") ? " " : "";
  return `${fence}${pad}${flat}${pad}${fence}`;
}

/// A link target that cannot end the link early.
function linkTarget(url: string): string {
  return url.replace(/\s/g, "%20").replace(/\(/g, "%28").replace(/\)/g, "%29");
}

const CI_TEXT: Record<PullRequest["ci"], string> = {
  success: "CI green",
  failure: "CI failing",
  pending: "CI pending",
  none: "no CI checks",
};

const MERGE_TEXT: Record<PullRequest["merge"], string> = {
  mergeable: "no conflicts",
  conflicted: "has conflicts",
  // GitHub computes mergeability lazily; this is "not answered yet", not
  // "no conflicts".
  checking: "conflicts not yet computed",
};

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

function unresolvedText(row: ReadyRow): string {
  // Typed as a number, but a payload cached by an older build can lack
  // it, and absent is not zero.
  const n: unknown = row.pr.unresolved_threads;
  if (typeof n !== "number" || !Number.isFinite(n)) return "unresolved conversations unknown";
  const counted = plural(n, "unresolved conversation", "unresolved conversations");
  if (row.unresolvedIsFloor) return `at least ${counted}`;
  return n === 0 ? "no unresolved conversations" : counted;
}

function pusherText(p: LastPusher | undefined): string {
  if (p === undefined || p.state === "not-checked") return "not checked";
  if (p.state === "unknown") return "unknown (checked, could not be determined)";
  return `@${p.login}`;
}

/// The line for one pull request: every field the strip knows.
function entryLine(row: ReadyRow, now: Date): string {
  const { pr } = row;
  const parts: string[] = [`${pr.repo} #${pr.number}`];
  if (pr.author) parts.push(`by @${pr.author}`);
  if (pr.head_ref && pr.base_ref) {
    parts.push(`${codeSpan(pr.head_ref)} → ${codeSpan(pr.base_ref)}`);
  }
  const age = readyAge(pr.ready_at, now);
  parts.push(age.since === null ? "ready time unknown" : `ready ${age.text}`);
  parts.push(CI_TEXT[pr.ci] ?? "CI state unknown");
  parts.push(MERGE_TEXT[pr.merge] ?? "conflicts unknown");
  parts.push(pr.is_draft ? "draft" : "not a draft");
  parts.push(unresolvedText(row));
  if (row.lastPusher?.state === "known") parts.push(`last push by @${row.lastPusher.login}`);
  return `- [${escapeMarkdownText(pr.title)}](${linkTarget(pr.url)}) — ${parts.join(" · ")}`;
}

/// Minutes, in UTC, so the header reads the same wherever it is pasted.
function stamp(now: Date): string {
  return `${now.toISOString().slice(0, 16).replace("T", " ")} UTC`;
}

/// The strip's rows as a markdown list. See the module docs.
export function readyListMarkdown(rows: readonly ReadyRow[], opts: ReadyListOptions): string {
  const { now, forAgent = false } = opts;
  const count = plural(rows.length, "pull request", "pull requests");
  const lines: string[] = [`**Ready for review**: ${count}, as of ${stamp(now)}.`, ""];
  if (forAgent) {
    lines.push(
      "Everything after each link is Headstate's snapshot from that time, not a live value. " +
        "Re-check each one yourself before acting on it.",
      "",
    );
  }
  for (const row of rows) {
    lines.push(entryLine(row, now));
    if (forAgent) {
      const { pr } = row;
      lines.push(`  - repository: ${pr.repo}, number: ${pr.number}`);
      lines.push(`  - head commit: ${pr.head_oid ? codeSpan(pr.head_oid) : "unknown"}`);
      lines.push(`  - last pusher: ${pusherText(row.lastPusher)}`);
    }
  }
  return lines.join("\n") + "\n";
}

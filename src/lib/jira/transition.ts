/// Moving a Jira issue to another status (#2044): the pure half.
///
/// `acli` 1.3.39 moves an issue by status NAME and cannot list the
/// transitions its current status allows (`workitem view` returns
/// `transitions: null`; no command lists a workflow). So the choices shown
/// are statuses seen in the project's recent issues, said to be exactly
/// that, and a move the workflow forbids is found out by asking: Jira
/// refuses it, with its reason, and the reader is offered Jira itself.

import type { JiraStatus, TransitionResult } from "@/types/jira";
import { describeJiraError } from "@/api/jira";

export const TRANSITION_PROGRAM = "acli";

/// The arguments the desktop runs after `acli`, for the preview shown
/// before it does. `transition_args` in `src-tauri/src/jira/cli.rs` builds
/// the real ones; both sides assert the same literal in their tests, so
/// one cannot change without the other.
export function transitionArgv(key: string, status: string): string[] {
  return ["jira", "workitem", "transition", "--key", key, "--status", status, "--yes", "--json"];
}

/// Mirrors `is_status_name` in `cli.rs`: what the desktop will send.
export function isStatusName(status: string): boolean {
  return (
    status.trim() !== "" &&
    status === status.trim() &&
    [...status].length <= 255 &&
    !status.startsWith("-") &&
    // eslint-disable-next-line no-control-regex
    !/[\u0000-\u001f\u007f-\u009f]/.test(status)
  );
}

const same = (a: string, b: string) => a.localeCompare(b, undefined, { sensitivity: "accent" }) === 0;

/// The statuses worth offering: those seen, less the one it is in.
export function candidateStatuses(seen: readonly JiraStatus[], current: string | null): JiraStatus[] {
  return seen.filter((s) => s.name !== "" && (current === null || !same(s.name, current)));
}

/// `text` as a sentence: Jira's reasons sometimes end without a stop.
const sentence = (text: string) => (/[.!?]$/.test(text.trim()) ? text.trim() : `${text.trim()}.`);

export type Tone = "ok" | "warn" | "fail";

/// What one transition came to, in words. The status named as where the
/// issue IS comes only from the re-read; the one asked for is named only
/// as asked. `openInJira` says the reader may need Jira to finish it.
export function transitionOutcome(r: TransitionResult): { tone: Tone; text: string; openInJira: boolean } {
  const now = r.after?.status.name ?? null;
  const reread = r.reread_error ? describeJiraError(r.reread_error) : "it did not come back";
  switch (r.reply.state) {
    case "accepted":
      if (now === null) {
        return {
          tone: "warn",
          text: `acli reported the move to ${r.requested}, but ${r.key} could not be read again (${reread}), so where it is now is not confirmed.`,
          openInJira: true,
        };
      }
      return same(now, r.requested)
        ? { tone: "ok", text: `Jira now reports ${r.key} as ${now}.`, openInJira: false }
        : {
            tone: "warn",
            text: `Jira accepted the move to ${r.requested}, but now reports ${r.key} as ${now}.`,
            openInJira: true,
          };
    case "refused":
      return {
        tone: "fail",
        text: `Jira refused to move ${r.key} to ${r.requested}: ${sentence(r.reply.reason)}${now ? ` Jira reports it as ${now}.` : ""}`,
        openInJira: true,
      };
    case "unknown":
      return {
        tone: "warn",
        text: `Could not tell whether Jira moved ${r.key}: ${sentence(describeJiraError(r.reply.error))}${
          now ? ` Jira now reports it as ${now}.` : ` It could not be read again (${reread}).`
        }`,
        openInJira: true,
      };
  }
}

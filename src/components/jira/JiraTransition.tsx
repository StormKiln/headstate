import { useState } from "react";
import { describeJiraError, failureOf, useJiraProjectStatuses, useJiraTransition } from "@/api/jira";
import {
  TRANSITION_PROGRAM,
  candidateStatuses,
  isStatusName,
  transitionArgv,
  transitionOutcome,
  type Tone,
} from "@/lib/jira/transition";
import type { JiraIssue, TransitionResult } from "@/types/jira";
import { ActingOnDesktop } from "../ActingOnDesktop";
import { ArgvPreview } from "../ArgvPreview";
import { ExternalLink } from "../ExternalLink";

const TONE: Record<Tone, string> = {
  ok: "text-[#3fb950]",
  warn: "text-[#d29922]",
  fail: "text-[#f85149]",
};

const BUTTON = "rounded border border-[#30363d] px-2 py-0.5 text-xs text-[#e6edf3] hover:bg-[#21262d] disabled:opacity-50";

/// Move a Jira issue to another status, confirmed, then verified (#2044).
///
/// Reusable: the issue panel of My Jira issues mounts it with no `target`
/// and the reader picks a status; a caller that already knows where the
/// issue should go -- a drift notice's "Move ABC-1 to In Review" (#2042) --
/// passes `target` and the button goes straight to the confirmation.
///
/// Every move is confirmed with the exact argv on screen, and on a phone
/// with the machine it runs on. What is shown afterwards is the status
/// Jira reports when the issue is read again, never the one asked for.
/// Render only while Jira is on: the desktop refuses it otherwise.
export function JiraTransition({
  issue,
  site,
  target,
  onDone,
  onClose,
}: {
  issue: JiraIssue;
  /// The signed-in site, for "Open in Jira" when the issue carries no URL.
  site: string | null;
  /// The status to move to. Omitted: the reader chooses.
  target?: string;
  /// Called with what Jira reported once the move has been tried.
  onDone?: (r: TransitionResult) => void;
  /// Called when the reader dismisses it -- Cancel, or Done after a move.
  onClose?: () => void;
}) {
  const [step, setStep] = useState<"idle" | "choosing" | { confirm: string }>("idle");
  const move = useJiraTransition();
  const href = issue.url ?? (site ? `https://${site}/browse/${issue.key}` : null);
  const reset = () => {
    move.reset();
    setStep("idle");
    onClose?.();
  };

  if (move.isSuccess) {
    const o = transitionOutcome(move.data);
    return (
      <div role="status" className="flex flex-col gap-1 text-xs">
        <p className={TONE[o.tone]}>{o.text}</p>
        <div className="flex items-center gap-2">
          {o.openInJira && href ? (
            <ExternalLink href={href} className="text-[#58a6ff] hover:underline">
              Open in Jira
            </ExternalLink>
          ) : null}
          <button type="button" className={BUTTON} onClick={reset}>
            Done
          </button>
        </div>
      </div>
    );
  }
  if (move.isError) {
    return (
      <div role="alert" className="flex flex-col gap-1 text-xs">
        <p className="text-[#f85149]">
          Could not move {issue.key}: {describeJiraError(failureOf(move.error))}
        </p>
        <div className="flex items-center gap-2">
          {href ? (
            <ExternalLink href={href} className="text-[#58a6ff] hover:underline">
              Open in Jira
            </ExternalLink>
          ) : null}
          <button type="button" className={BUTTON} onClick={reset}>
            Done
          </button>
        </div>
      </div>
    );
  }

  if (typeof step === "object") {
    const status = step.confirm;
    return (
      <div aria-label={`Move ${issue.key}`} className="flex flex-col gap-1 rounded border border-[#30363d] p-2 text-xs">
        <p className="text-[#e6edf3]">
          Move {issue.key} from {issue.status.name || "its current status"} to {status}? This changes it in Jira for
          everyone who can see it.
        </p>
        <div aria-label="Command to run">
          <ArgvPreview program={TRANSITION_PROGRAM} args={transitionArgv(issue.key, status)} />
        </div>
        <ActingOnDesktop />
        <div className="mt-1 flex items-center gap-2">
          <button
            type="button"
            className={BUTTON}
            disabled={move.isPending}
            onClick={() => move.mutate({ key: issue.key, status }, { onSuccess: onDone })}
          >
            {move.isPending ? `Moving ${issue.key}…` : `Move to ${status}`}
          </button>
          <button type="button" className={BUTTON} disabled={move.isPending} onClick={reset}>
            Cancel
          </button>
        </div>
      </div>
    );
  }

  if (step === "choosing") {
    return <StatusChooser issue={issue} onPick={(s) => setStep({ confirm: s })} onCancel={reset} />;
  }

  return (
    <button
      type="button"
      className={`${BUTTON} self-start`}
      onClick={() => setStep(target ? { confirm: target } : "choosing")}
    >
      {target ? `Move ${issue.key} to ${target}` : "Move to…"}
    </button>
  );
}

/// The statuses to offer. Not "the transitions allowed": Jira does not
/// tell Headstate those, so these are the statuses seen in the project's
/// recent issues, said to be so, and any other name can be typed.
function StatusChooser({
  issue,
  onPick,
  onCancel,
}: {
  issue: JiraIssue;
  onPick: (status: string) => void;
  onCancel: () => void;
}) {
  const [typed, setTyped] = useState("");
  const seen = useJiraProjectStatuses(issue.project_key, true);
  const choices = candidateStatuses(seen.data?.statuses ?? [], issue.status.name || null);
  const failed = seen.isError
    ? describeJiraError(failureOf(seen.error))
    : seen.data?.error
      ? describeJiraError(seen.data.error)
      : null;
  const name = typed.trim();
  return (
    <div aria-label={`Move ${issue.key}`} className="flex flex-col gap-1.5 rounded border border-[#30363d] p-2 text-xs">
      <p className="text-[#8b949e]">
        Headstate cannot see which moves {issue.project_key}'s workflow allows from{" "}
        {issue.status.name || "this status"}. These statuses were seen in {issue.project_key}'s recent issues, and Jira
        refuses a move its workflow does not allow.
      </p>
      {seen.isPending ? (
        <p role="status" className="flex items-center gap-2 text-[#8b949e]">
          <span className="h-3 w-24 animate-pulse rounded bg-[#21262d]" aria-hidden="true" />
          Reading the statuses in {issue.project_key}…
        </p>
      ) : failed ? (
        <p className="text-[#f85149]">Could not read the statuses in {issue.project_key}: {failed}</p>
      ) : choices.length === 0 ? (
        <p className="text-[#8b949e]">No other status was seen in {issue.project_key}'s recent issues.</p>
      ) : (
        <ul className="flex flex-wrap gap-1.5">
          {choices.map((s) => (
            <li key={s.name}>
              <button type="button" className={BUTTON} onClick={() => onPick(s.name)}>
                {s.name}
              </button>
            </li>
          ))}
        </ul>
      )}
      <form
        className="flex flex-wrap items-center gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          if (isStatusName(name)) onPick(name);
        }}
      >
        <label className="flex items-center gap-1 text-[#8b949e]">
          Another status
          <input
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
            placeholder="as Jira names it"
            className="w-40 rounded border border-[#30363d] bg-[#0d1117] px-1.5 py-0.5 text-[#e6edf3]"
          />
        </label>
        <button type="submit" className={BUTTON} disabled={!isStatusName(name)}>
          Next
        </button>
        <button type="button" className={BUTTON} onClick={onCancel}>
          Cancel
        </button>
      </form>
    </div>
  );
}

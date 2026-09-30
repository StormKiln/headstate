import { useState } from "react";
import { toast } from "sonner";
import { useMergeStack } from "../api/hooks";
import { numberList } from "../lib/stack";
import type { PrDetail, StackMember } from "../types/pr";
import { Dialog, DialogContent, DialogTitle } from "./ui/dialog";

function memberBlocker(member: StackMember): string | null {
  if (member.is_draft) return "draft";
  if (member.review === "changes_requested") return "changes requested";
  if (member.review === "review_required") return "approval required";
  return null;
}

// The rollup includes optional checks. Without the base's required-check
// policy it is advisory, not evidence that GitHub must reject the operation.
function checkNote(member: StackMember): string | null {
  if (member.checks === "failure" || member.checks === "error") return "checks failing (required-check policy unknown)";
  if (member.checks === "pending" || member.checks === "expected") return "checks pending (required-check policy unknown)";
  return null;
}

/// Merge or queue a native GitHub stack (#1468).
///
/// GitHub merges a stacked pull request only as a stack, and that lands
/// every open pull request beneath it too, all or nothing. So this never
/// acts on the click: it opens a confirmation naming each pull request it
/// will land, bottom first, and acts only on the second click.
///
/// `lands` comes from `stackMergePlan`, which is null unless GitHub's
/// whole membership is known -- a confirmation must not understate what it
/// lands. `why` is the same availability reason the plain button would
/// carry; GitHub still evaluates every rule when the merge runs.
export function StackMerge({
  pr,
  lands,
  queue,
  why,
  compact = false,
}: {
  pr: PrDetail;
  lands: StackMember[];
  queue: boolean;
  why: string | null;
  compact?: boolean;
}) {
  const mergeStack = useMergeStack();
  const [confirming, setConfirming] = useState<{ head: string; scope: string } | null>(null);
  const [busy, setBusy] = useState(false);

  const count = `${lands.length} pull request${lands.length === 1 ? "" : "s"}`;
  const verb = queue ? "queues" : "merges";
  const individualLabel = queue ? "Add to merge queue" : "Merge";
  const label = lands.length === 1 ? individualLabel : `${queue ? "Queue" : "Merge"} ${count}…`;
  const predecessors = lands.filter((m) => m.number !== pr.number);
  const prerequisite = `GitHub cannot ${queue ? "queue" : "merge"} #${pr.number} alone: merge ${numberList(predecessors)} first.`;
  const blocker = lands.map((m) => {
    const reason = memberBlocker(m);
    return reason ? `#${m.number}: ${reason}` : null;
  }).find((reason) => reason !== null);
  const unavailable = why ?? blocker ?? null;
  const scope = lands.map((member) => member.number).join(",");
  const changed = confirming !== null && (confirming.head !== pr.head_oid || confirming.scope !== scope);

  const run = () => {
    if (!confirming || changed || unavailable !== null) return;
    const expectedHead = confirming.head;
    setConfirming(null);
    setBusy(true);
    mergeStack(pr.repo, pr.number, queue ? "merge_queue" : "direct_merge", expectedHead).then(
      (outcome) => {
        setBusy(false);
        switch (outcome.kind) {
          case "merged":
            toast.success(`${pr.repo} — merged ${numberList(lands)}`);
            break;
          case "enqueued":
            toast.success(`${pr.repo} — ${numberList(lands)} added to the merge queue`);
            break;
          case "failed":
            // Atomic: nothing landed. GitHub's reason is the useful part.
            toast.error(`Stack not ${queue ? "queued" : "merged"}: nothing landed`, {
              description: outcome.message,
            });
            break;
          case "in_progress":
            // Not a failure: GitHub accepted it and is still working.
            toast.info(`Stack ${queue ? "queueing" : "merge"} still in progress on GitHub`, {
              description: outcome.message,
            });
            break;
        }
      },
      (e: unknown) => {
        setBusy(false);
        toast.error(`Could not submit the stack for #${pr.number}`, {
          description: typeof e === "string" ? e : undefined,
        });
      },
    );
  };

  const individual = (
    <button type="button" disabled title={prerequisite}
      className="rounded border border-[#30363d] px-3 py-1.5 text-sm text-[#8b949e] opacity-50">
      {individualLabel}
    </button>
  );
  if (compact && predecessors.length > 0) return individual;

  return (
    <>
      {predecessors.length > 0 ? (
        <>
          {individual}
          <span className="text-xs text-[#8b949e]">{prerequisite}</span>
        </>
      ) : null}
      <button
        type="button"
        disabled={unavailable !== null || busy}
        onClick={() => setConfirming({ head: pr.head_oid, scope })}
        title={unavailable ?? `${label}: ${numberList(lands)}`}
        className={`rounded px-3 py-1.5 text-sm ${
          unavailable
            ? "border border-[#30363d] text-[#8b949e] opacity-50"
            : "bg-[#238636] font-medium text-white hover:bg-[#1a7f37]"
        }`}
      >
        {busy ? "Working…" : label}
      </button>
      {blocker ? <span className="text-xs text-[#8b949e]">{blocker}</span> : null}

      {confirming ? (
        <Dialog open onOpenChange={(open) => !open && setConfirming(null)}>
          <DialogContent className="max-w-lg">
            <DialogTitle>
              {queue ? `Add ${count} to the merge queue?` : `Merge ${count}?`}
            </DialogTitle>
            <p className="mt-3 text-sm text-[#8b949e]">
              This {verb} {numberList(lands)} together, bottom of the stack first. If one
              cannot be accepted, none are. {queue ? "Queued pull requests may land in separate merge groups." : ""}
            </p>
            <p className="mt-2 text-sm text-[#8b949e]">
              Full merge readiness is unknown. GitHub will check every included pull request’s rules before accepting this operation.
            </p>
            <ol className="mt-3 text-sm text-[#e6edf3]" aria-label="Pull requests this lands">
              {lands.map((m) => (
                <li key={m.number} className="py-0.5">
                  #{m.number} — {m.title}
                  {checkNote(m) ? <span className="text-[#d29922]"> — {checkNote(m)}</span> : null}
                </li>
              ))}
            </ol>
            {changed ? <p role="status" className="mt-2 text-sm text-[#d29922]">
              The pull request or its dependencies changed. Close this dialog and review the latest state before confirming again.
            </p> : unavailable ? <p role="status" className="mt-2 text-sm text-[#d29922]">{unavailable}</p> : null}
            <div className="mt-5 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setConfirming(null)}
                className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#21262d]"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={run}
                disabled={changed || unavailable !== null}
                className="rounded bg-[#238636] px-3 py-1.5 text-sm font-medium text-white hover:bg-[#1a7f37] disabled:opacity-50"
              >
                {queue ? `Queue ${count}` : `Merge ${count}`}
              </button>
            </div>
          </DialogContent>
        </Dialog>
      ) : null}
    </>
  );
}

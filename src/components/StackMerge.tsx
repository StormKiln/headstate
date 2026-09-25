import { useState } from "react";
import { toast } from "sonner";
import { useMergeStack } from "../api/hooks";
import { numberList } from "../lib/stack";
import type { PrDetail, StackMember } from "../types/pr";
import { Dialog, DialogContent, DialogTitle } from "./ui/dialog";

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
}: {
  pr: PrDetail;
  lands: StackMember[];
  queue: boolean;
  why: string | null;
}) {
  const mergeStack = useMergeStack();
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);

  const label = queue ? "Add stack to merge queue" : "Merge stack";
  const count = `${lands.length} pull request${lands.length === 1 ? "" : "s"}`;
  const verb = queue ? "queues" : "merges";

  const run = () => {
    setConfirming(false);
    setBusy(true);
    mergeStack(pr.repo, pr.number, queue ? "merge_queue" : "direct_merge", pr.head_oid).then(
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

  return (
    <>
      <button
        type="button"
        disabled={why !== null || busy}
        onClick={() => setConfirming(true)}
        title={why ?? `${label}: ${numberList(lands)}`}
        className={`rounded px-3 py-1.5 text-sm ${
          why
            ? "border border-[#30363d] text-[#8b949e] opacity-50"
            : "bg-[#238636] font-medium text-white hover:bg-[#1a7f37]"
        }`}
      >
        {busy ? "Working…" : label}
      </button>

      {confirming ? (
        <Dialog open onOpenChange={(open) => !open && setConfirming(false)}>
          <DialogContent className="max-w-lg">
            <DialogTitle>
              {queue ? `Add ${count} to the merge queue?` : `Merge ${count}?`}
            </DialogTitle>
            <p className="mt-3 text-sm text-[#8b949e]">
              This {verb} {numberList(lands)} together, bottom of the stack first. If one
              cannot land, none do.
            </p>
            <ol className="mt-3 text-sm text-[#e6edf3]" aria-label="Pull requests this lands">
              {lands.map((m) => (
                <li key={m.number} className="py-0.5">
                  #{m.number} — {m.title}
                </li>
              ))}
            </ol>
            <div className="mt-5 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setConfirming(false)}
                className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#21262d]"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={run}
                className="rounded bg-[#238636] px-3 py-1.5 text-sm font-medium text-white hover:bg-[#1a7f37]"
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

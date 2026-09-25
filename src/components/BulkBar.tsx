import { useState } from "react";
import { toast } from "sonner";
import { useActOnPrs } from "../api/hooks";
import type { PrActionName } from "../api/tauri";
import { deriveStacked } from "../lib/derive";
import { stackBlocksQueue, stackFactsFromList } from "../lib/stack";
import { useFilters } from "../store/filters";
import type { PullRequest } from "../types/pr";
import { Dialog, DialogContent, DialogTitle } from "./ui/dialog";

/// Actions offered in bulk.
///
/// Merge is deliberately absent. Merging many pull requests at once --
/// each changing the base the next merges onto -- is how you get a
/// cascade of conflicts and broken CI on main. The merge queue exists to
/// serialise exactly that, so bulk *enqueue* is the safe expression of
/// the same intent.
const BULK: { action: PrActionName; label: string }[] = [
  { action: "enqueue", label: "Add to merge queue" },
  { action: "ready", label: "Mark ready" },
  { action: "draft", label: "Convert to draft" },
  { action: "close", label: "Close PRs" },
];

export function prKey(pr: { repo: string; number: number }): string {
  return `${pr.repo}#${pr.number}`;
}

/// Why an action would do nothing to this pull request, or null.
///
/// A pull request already in the merge queue cannot be enqueued, and one
/// already ready cannot be marked ready. The single-PR paths have always
/// known this -- `PrActions` and `PrKebab` both switch on
/// `in_merge_queue` and offer `dequeue` instead -- and bulk did not, so
/// selecting eight rows with three already queued sent `enqueue` for all
/// eight and GitHub refused three of them (#752).
///
/// Phrased as the REASON rather than a boolean because the dialog shows
/// it: "3 already in the merge queue" is something a user can check
/// their selection against, where a count of skipped rows is not.
///
/// `close` and `draft` are deliberately absent. The list holds only open
/// pull requests, so `close` is never redundant; `draft` on a PR that is
/// already a draft would be, but the bar offers "Convert to draft"
/// alongside "Mark ready" and a selection mixing both is the ordinary
/// case -- which `noOp` already describes correctly for whichever one
/// the user picks.
function noOp(pr: PullRequest, action: PrActionName): string | null {
  switch (action) {
    case "enqueue":
      return pr.in_merge_queue ? "already in the merge queue" : null;
    case "ready":
      return pr.is_draft ? null : "already ready for review";
    case "draft":
      return pr.is_draft ? "already a draft" : null;
    default:
      return null;
  }
}

/// Why this action should not be SENT for this pull request, or null.
///
/// `noOp` (already in that state) plus the one refusal the list can
/// predict: queueing a pull request stacked on another open one, which
/// GitHub refuses (#1452). Stacking comes from `deriveStacked` over the
/// whole list passed in, so a parent outside that list is not seen -- the
/// detail view asks GitHub and gates on more; this gates on what it has.
function skipReason(
  pr: PullRequest,
  action: PrActionName,
  stacked: Map<string, number>,
): string | null {
  const same = noOp(pr, action);
  if (same !== null) return same;
  return action === "enqueue" ? stackBlocksQueue(stackFactsFromList(stacked.get(pr.id))) : null;
}

/// The bar shown while rows are selected.
///
/// Every action confirms here, not just Close. A single merge applies
/// immediately because it is one deliberate click on one pull request;
/// a batch is a different act -- the count is the whole risk, and the
/// dialog is where the user sees which rows they actually picked up.
export function BulkBar({ prs }: { prs: PullRequest[] }) {
  const { checked, clearChecked } = useFilters();
  const actOnPrs = useActOnPrs();
  const [pending, setPending] = useState<PrActionName | null>(null);
  const [busy, setBusy] = useState(false);

  // Resolve keys back to PRs against the *unfiltered* list, so a filter
  // narrowed after selecting cannot silently shrink the batch.
  const selected = prs.filter((pr) => checked.includes(prKey(pr)));
  if (selected.length === 0) return null;
  const stacked = deriveStacked(prs);
  const skip = (pr: PullRequest, action: PrActionName) => skipReason(pr, action, stacked);

  // Split the SELECTION for the pending action, without changing it.
  //
  // This is not the shrinking the unfiltered-list rule forbids. That
  // rule is about the batch changing behind the user's back -- a filter
  // narrowed after selecting, removing rows they never unticked. Here
  // nothing is removed: `checked` is untouched, every selected row is
  // still listed in the dialog, and the split is a property of the
  // ACTION rather than of the view. The same eight rows are still
  // selected the moment the dialog closes, and picking a different
  // action splits them differently.
  //
  // Deriving it per-action at confirmation time is what makes that
  // true. Filtering the selection when rows are ticked would bake one
  // action's notion of redundancy into the selection itself, which is
  // the silent shrink under another name (#752).
  const applicable = pending ? selected.filter((pr) => skip(pr, pending) === null) : selected;
  const skipped = pending ? selected.filter((pr) => skip(pr, pending) !== null) : [];

  const run = (action: PrActionName) => {
    // Send only the rows the action can change. The user has just been
    // shown exactly which ones those are and confirmed against that
    // count, so this acts on what was agreed rather than on a batch
    // three of whose members were always going to be refused.
    const targets = selected.filter((pr) => skip(pr, action) === null);
    if (targets.length === 0) {
      // Nothing to do, and saying so beats a "0 updated" toast that
      // reads like the batch silently failed.
      toast.info("Nothing to do — every selected pull request is already in that state");
      return;
    }
    setBusy(true);
    actOnPrs(
      targets.map((pr) => [pr.id, pr.repo, pr.number] as [string, string, number]),
      action,
    ).then(
      (outcomes) => {
        setBusy(false);
        const failed = outcomes.filter((o) => o.error !== null);
        const ok = outcomes.length - failed.length;
        // Never a bare "done": partial failure is the normal case, and a
        // single success message would hide the rejections.
        if (failed.length === 0) {
          // The skipped rows are named in the SAME sentence as the
          // updated ones. The toast used to say "8 updated" when five
          // changed, which is the dishonesty #752 is about -- and a
          // count alone would leave the user wondering which three.
          // "already in that state" only when it is true of every skipped
          // row; a stacked one was skipped because GitHub would refuse it.
          const allSame = selected.every((pr) => targets.includes(pr) || noOp(pr, action) !== null);
          const also =
            selected.length > targets.length
              ? ` — ${selected.length - targets.length} skipped${allSame ? ", already in that state" : ""}`
              : "";
          toast.success(`${ok} pull request${ok === 1 ? "" : "s"} updated${also}`);
          clearChecked();
        } else {
          toast.error(`${failed.length} of ${outcomes.length} failed`, {
            description: failed.map((f) => `${f.repo}#${f.number}: ${f.error}`).join("\n"),
          });
          // Keep the failures selected so they can be retried; drop the
          // ones that worked, or a retry would repeat them.
          const stillFailing = failed.map((f) => prKey(f));
          useFilters.getState().setChecked(stillFailing);
        }
      },
      (e: unknown) => {
        setBusy(false);
        toast.error("The batch could not run", {
          description: typeof e === "string" ? e : undefined,
        });
      },
    );
  };

  return (
    // Sticky because `<main>` is the scroll container and this bar
    // renders ABOVE the list: after scrolling down to check a row, the
    // bar you need to act on it had scrolled off the top. The worktrees
    // page already hit this and fixed it for its own confirm dialog.
    // `z-10` clears the rows; the opaque background stops text showing
    // through as they scroll under it.
    <div className="sticky top-0 z-10 mb-3 flex flex-wrap items-center gap-2 rounded-md border border-[#1f6feb] bg-[#0d1a2f] px-3 py-2 text-sm">
      <span className="font-medium">
        {selected.length} selected
      </span>
      <div className="ml-auto flex flex-wrap gap-2">
        {BULK.map(({ action, label }) => (
          <button
            key={action}
            type="button"
            disabled={busy}
            onClick={() => setPending(action)}
            className="rounded border border-[#30363d] px-2 py-1 hover:bg-[#161b22] disabled:opacity-50"
          >
            {label}
          </button>
        ))}
        <button
          type="button"
          onClick={clearChecked}
          className="rounded px-2 py-1 text-[#8b949e] hover:text-[#e6edf3]"
        >
          Clear
        </button>
      </div>

      {pending ? (
        <Dialog open onOpenChange={(o) => !o && setPending(null)}>
          <DialogContent className="max-w-2xl">
            {/* The title counts what will ACTUALLY change. Asking
                "Add 8 to the merge queue?" when three are already there
                is asking a question the app knows the answer to. */}
            <DialogTitle>
              {BULK.find((b) => b.action === pending)?.label} {applicable.length} pull request
              {applicable.length === 1 ? "" : "s"}?
            </DialogTitle>
            {skipped.length > 0 ? (
              // The count the user selected is still stated, so the
              // dialog reconciles with the "8 selected" in the bar
              // rather than appearing to have lost rows (#752).
              <p className="mt-2 text-sm text-[#d29922]">
                {selected.length} selected — {skipped.length}{" "}
                {new Set(skipped.map((pr) => skip(pr, pending))).size === 1
                  ? skip(skipped[0], pending)
                  : "cannot be changed (see below)"}
                , so {skipped.length === 1 ? "it" : "they"}{" "}
                will be skipped.
              </p>
            ) : null}
            {/* The full list, not a count: "Close 12 pull requests?" is
                not something anyone can act on safely.

                EVERY selected row is still listed, skipped ones included
                and marked. Dropping them would make the dialog disagree
                with the selection it is confirming, which is the silent
                shrink the unfiltered-list rule exists to prevent. */}
            <ul className="mt-3 max-h-64 overflow-y-auto text-sm text-[#8b949e]">
              {selected.map((pr) => {
                const why = pending ? skip(pr, pending) : null;
                return (
                  <li key={prKey(pr)} className={`py-0.5 ${why ? "opacity-60" : ""}`}>
                    {pr.repo}#{pr.number} — {pr.title}
                    {why ? <span className="ml-1 text-[#d29922]">({why})</span> : null}
                  </li>
                );
              })}
            </ul>
            <div className="mt-5 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setPending(null)}
                className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#21262d]"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => {
                  const a = pending;
                  setPending(null);
                  run(a);
                }}
                className={`rounded px-3 py-1.5 text-sm font-medium text-white ${
                  pending === "close"
                    ? "bg-[#da3633] hover:bg-[#c93c37]"
                    : "bg-[#1f6feb] hover:bg-[#316dca]"
                }`}
              >
                {BULK.find((b) => b.action === pending)?.label} {applicable.length} pull request
                {applicable.length === 1 ? "" : "s"}
              </button>
            </div>
          </DialogContent>
        </Dialog>
      ) : null}
    </div>
  );
}

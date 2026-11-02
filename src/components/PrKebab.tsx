import { openExternal } from "./ExternalLink";
import { useWritesPaused } from "@/lib/useWritesPaused";
import { copyText } from "../lib/clipboard";
import { Copy, ExternalLink as ExternalLinkIcon, MoreHorizontal } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { actionFailure } from "@/lib/notAskedRemedy";
import { IS_MOBILE_BUILD } from "@/lib/target";
import { useActOnPr, useSetAutoMerge, useUpdatePrBranch } from "../api/hooks";
import type { PrActionName } from "../api/tauri";
import { stackBlocksQueue, stackFactsFromList } from "../lib/stack";
import { inverseOf } from "../lib/undo";
import type { PullRequest } from "../types/pr";
import { PrClaudifyDialogs, PrClaudifyMenuItem } from "./PrClaudify";
import type { PrClaudifyDialog } from "./claudifyStart";
import { Dialog, DialogContent, DialogTitle } from "./ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "./ui/dropdown-menu";

/// A menu row. `data-highlighted` is the keyboard (and pointer)
/// highlight: focus moves item to item on the arrow keys, so hover alone
/// would leave a keyboard user with no visible position. Disabled items
/// keep pointer events so their `title` -- the reason -- still shows.
const ITEM =
  "flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-sm text-[#e6edf3] hover:bg-[#21262d] data-highlighted:bg-[#21262d] data-disabled:pointer-events-auto data-disabled:text-[#8b949e] data-disabled:opacity-50 data-disabled:hover:bg-transparent";

/// Why a row-level action is unavailable, or null when offered.
///
/// Greying out beats hiding. An absent "Merge" just looks broken, while
/// a disabled one carrying its reason teaches why -- and on a real
/// account this is the common case, not the edge: over half of a typical
/// open set is DIRTY or UNSTABLE at any moment.
///
/// The specific checks run before the general one so the tooltip names
/// the actual obstacle ("merge conflicts") rather than the catch-all.
/// Anything not listed here is left to GitHub, which refuses with a
/// toast -- better than guessing wrongly in the client.
function unavailable(pr: PullRequest, action: PrActionName, stackedOn?: number): string | null {
  switch (action) {
    case "merge":
      if (pr.is_draft) return "drafts cannot be merged";
      if (pr.merge === "conflicted") return "merge conflicts";
      if (pr.merge === "checking") return "GitHub is still checking mergeability";
      if (pr.ci === "failure") return "checks are failing";
      if (pr.merge_status !== "clean") return "GitHub has not confirmed this can merge";
      return null;
    case "enqueue":
      // Enqueueing a PR that cannot merge is rejected by GitHub anyway;
      // saying so up front beats a toast after the round trip.
      if (pr.is_draft) return "drafts cannot be queued";
      if (pr.merge === "conflicted") return "merge conflicts";
      // The same gate as the detail view's, on the evidence a row has
      // (#1452): `deriveStacked`'s parent, when the parent is in the list.
      return stackBlocksQueue(stackFactsFromList(stackedOn));
    default:
      return null;
  }
}

/// What the TOAST says, as a verb phrase.
///
/// Separate from `LABEL` because the button and the sentence want
/// different words: the button says "Close PR" so it cannot be mistaken
/// for closing the view (it sits beside "Back to list"), while the toast
/// says "closed" -- `LABEL[action].toLowerCase()` would render "close pr".
const VERB: Record<PrActionName, string> = {
  merge: "merged",
  close: "closed",
  reopen: "reopened",
  draft: "converted to draft",
  ready: "marked ready for review",
  enqueue: "added to merge queue",
  dequeue: "removed from merge queue",
};

/// What the BUTTON says.
const LABEL: Record<PrActionName, string> = {
  merge: "Merge",
  close: "Close PR",
  reopen: "Reopen",
  draft: "Convert to draft",
  ready: "Mark ready for review",
  enqueue: "Add to merge queue",
  dequeue: "Remove from merge queue",
};

/// Row actions.
///
/// `canWrite` is false on the review view: merging or closing someone
/// else's pull request is usually not yours to do, and offering an action
/// that fails with a permissions error is worse than not offering it.
export function PrKebab({
  pr,
  canWrite = true,
  stackedOn,
}: {
  pr: PullRequest;
  canWrite?: boolean;
  /// The open PR this one is stacked on, from `deriveStacked` (#1452).
  stackedOn?: number;
}) {
  // Null on the desktop and whenever the paired desktop is reachable,
  // so this changes nothing there.
  const paused = useWritesPaused();
  const act = useActOnPr();
  const updateBranch = useUpdatePrBranch();
  const setAuto = useSetAutoMerge();
  const [pending, setPending] = useState<PrActionName | null>(null);
  // Held here rather than in the menu item: the menu closes on click,
  // and the dialog must outlive it.
  const [claudify, setClaudify] = useState<PrClaudifyDialog | null>(null);
  const run = (action: PrActionName) => {
    act(pr.id, pr.repo, pr.number, action).then(
      () => {
        const back = inverseOf(action);
        toast.success(`${pr.repo}#${pr.number} — ${VERB[action]}`, {
          // Only for actions with a true inverse. Merge and close are
          // deliberately absent -- see `inverseOf`.
          action: back
            ? {
                label: "Undo",
                onClick: () => {
                  void act(pr.id, pr.repo, pr.number, back).then(
                    () =>
                      toast.success(
                        `${pr.repo}#${pr.number} — ${VERB[back]}`,
                      ),
                    (e: unknown) =>
                      toast.error(`Could not undo #${pr.number}`, {
                        description: actionFailure(typeof e === "string" ? e : undefined, IS_MOBILE_BUILD),
                      }),
                  );
                },
              }
            : undefined,
        });
      },
      (e: unknown) =>
        toast.error(`Could not ${VERB[action]} #${pr.number}`, {
          description: actionFailure(typeof e === "string" ? e : undefined, IS_MOBILE_BUILD),
        }),
    );
  };

  const writes: PrActionName[] = pr.is_draft
    ? ["ready", "close"]
    : pr.in_merge_queue
      ? ["dequeue", "close"]
      : ["merge", "enqueue", "draft", "close"];

  return (
    // The wrapper only stops the click reaching the row. React bubbles
    // synthetic events through a portal along the COMPONENT tree, so this
    // still catches clicks on the portalled items below.
    <div className="shrink-0" onClick={(e) => e.stopPropagation()}>
      <DropdownMenu>
        <DropdownMenuTrigger
          aria-label={`Actions for #${pr.number}`}
          className="rounded p-1 text-[#8b949e] hover:bg-[#21262d] hover:text-[#e6edf3]"
        >
          <MoreHorizontal className="h-4 w-4" aria-hidden="true" />
        </DropdownMenuTrigger>

        {/* Portalled and collision-aware. It used to be `absolute
            top-full` inside the row, so a row near the bottom of a
            clipping list opened its menu past the list's edge and the
            actions were cut off -- the same defect as the worktree row's
            menu (#2088). At the body nothing can clip it, and it flips
            above the button when there is no room below. */}
        <DropdownMenuContent
          align="end"
          className="w-56 rounded border border-[#30363d] bg-[#161b22] p-1 text-[#e6edf3] shadow-lg ring-0"
        >
          {canWrite
            ? writes.map((action) => {
                // Paused beats specific: naming "merge conflicts" for
                // a desktop the phone cannot reach describes the wrong
                // obstacle, and fixing it would not help.
                const why = paused ?? unavailable(pr, action, stackedOn);
                return (
                  <DropdownMenuItem
                    key={action}
                    disabled={why !== null}
                    title={why ?? LABEL[action]}
                    nativeButton
                    render={<button type="button" />}
                    onClick={() => {
                      // Close confirms; everything else applies now.
                      if (action === "close") setPending(action);
                      else run(action);
                    }}
                    className={ITEM}
                  >
                    {LABEL[action]}
                  </DropdownMenuItem>
                );
              })
            : null}

          {/* Only for BEHIND. On a DIRTY pull request this either fails or
              produces a conflicted merge commit, and an action that cannot
              work should not be offered at all -- unlike Merge, where the
              disabled-with-a-reason form teaches something. */}
          {/* The answer to the most common blocked state. On a real
              account 6 of 24 open PRs are UNSTABLE -- checks still
              running -- which is exactly "merge it the moment CI is
              green", and the only way to say so was to leave the app. */}
          {canWrite && pr.merge_status === "unstable" && !pr.is_draft ? (
            <DropdownMenuItem
              className={ITEM}
              onClick={() => {
                setAuto(pr.id, pr.repo, pr.number, pr.head_oid, true).then(
                  () => toast.success(`${pr.repo}#${pr.number} will merge when green`),
                  (e: unknown) =>
                    toast.error(`Could not enable auto-merge on #${pr.number}`, {
                      description: actionFailure(typeof e === "string" ? e : undefined, IS_MOBILE_BUILD),
                    }),
                );
              }}
            >
              Merge when green
            </DropdownMenuItem>
          ) : null}

          {canWrite && pr.merge_status === "behind" ? (
            <DropdownMenuItem
              className={ITEM}
              onClick={() => {
                updateBranch(pr.id, pr.repo, pr.number, pr.head_oid).then(
                  () => toast.success(`${pr.repo}#${pr.number} updated from base`),
                  (e: unknown) =>
                    toast.error(`Could not update #${pr.number}`, {
                      description: actionFailure(typeof e === "string" ? e : undefined, IS_MOBILE_BUILD),
                    }),
                );
              }}
            >
              Update branch
            </DropdownMenuItem>
          ) : null}

          {canWrite ? <DropdownMenuSeparator className="mx-0 my-1 bg-[#30363d]" /> : null}

          {/* Claudify (#1455). Mounted only while the menu is open, so
              rendering a list of rows does not start a worktree scan. */}
          <PrClaudifyMenuItem
            pr={pr}
            className={ITEM}
            onPick={(d) => {
              if (d !== null) setClaudify(d);
            }}
          />
          <DropdownMenuItem
            className={ITEM}
            onClick={() => {
              void copyText(pr.head_ref).then((failure) =>
                failure === null
                  ? toast.success("Branch name copied")
                  : toast.error("Could not copy the branch name", {
                      description: failure,
                    }),
              );
            }}
          >
            <Copy className="h-3.5 w-3.5" aria-hidden="true" />
            Copy branch name
          </DropdownMenuItem>
          <DropdownMenuItem className={ITEM} onClick={() => openExternal(pr.url)}>
            <ExternalLinkIcon className="h-3.5 w-3.5" aria-hidden="true" />
            View on GitHub
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      {pending ? (
        <Dialog open onOpenChange={(o) => !o && setPending(null)}>
          <DialogContent className="max-w-lg">
            <DialogTitle>Close this pull request?</DialogTitle>
            <p className="mt-3 text-sm text-[#8b949e]">
              {pr.repo}#{pr.number} — {pr.title}
            </p>
            <p className="mt-2 text-sm text-[#8b949e]">
              Closing loses the review context. The branch is untouched, and the pull
              request can be reopened.
            </p>
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
                  setPending(null);
                  run("close");
                }}
                className="rounded bg-[#da3633] px-3 py-1.5 text-sm font-medium text-white hover:bg-[#c93c37]"
              >
                Close pull request
              </button>
            </div>
          </DialogContent>
        </Dialog>
      ) : null}

      <PrClaudifyDialogs dialog={claudify} onClose={() => setClaudify(null)} />
    </div>
  );
}

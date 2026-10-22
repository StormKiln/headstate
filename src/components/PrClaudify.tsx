import { Bot, Sparkles } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import {
  claudeLaunchPr,
  claudeLaunchPrPreview,
  type LaunchTerms,
} from "@/api/tauri";
import { agentPrompt, toAgentContext } from "@/lib/agentPrompt";
import type { PrDetail, PullRequest } from "@/types/pr";
import { ActingOnDesktop } from "./ActingOnDesktop";
import {
  CLAUDIFY_CLASS,
  PLAIN_CLASS,
  compactClass,
  copyClaudify,
  shortReason,
  startClaudify,
  unavailableReason,
  useClaudifyCheckout,
  type PrClaudifyDialog,
} from "./claudifyStart";
import { LaunchTermsPicker } from "./LaunchTermsPicker";
import { Button } from "./ui/button";
import { Dialog, DialogContent, DialogTitle } from "./ui/dialog";

/// Claudify for a pull request (#1455).
///
/// Replaces "Copy for agent", which only put `agentPrompt` on the
/// clipboard. It follows the Worktrees Claudify exactly, because two
/// buttons with one name and two behaviours is a worse outcome than
/// either behaviour:
///
/// - Terminal configured (`!IS_MOBILE_BUILD` and a `terminal_command`):
///   open the terms dialog (#1214) with the exact argv, then
///   `claude_launch_pr`.
/// - Otherwise: copy the same line `claudify_pr_command` builds, or on
///   the phone -- no clipboard, no terminal -- show it to be typed at the
///   desktop.
///
/// Both need the repository's MAIN CHECKOUT on this machine, found in
/// the worktree scan by the `origin` remote (`mainCheckoutFor`). When
/// there is none the control says so and copies the prompt alone, rather
/// than starting `claude` somewhere that is not the repository.

/// Everything a Claudify control needs about one pull request.
function usePrClaudify(pr: PullRequest | PrDetail) {
  const { checkout, terminalConfigured } = useClaudifyCheckout([pr.repo]);
  const path = checkout.kind === "found" ? checkout.path : undefined;
  const prompt = agentPrompt(toAgentContext(pr, path));
  return { checkout, terminalConfigured, prompt };
}

function activate(
  pr: PullRequest | PrDetail,
  state: ReturnType<typeof usePrClaudify>,
  open: (d: PrClaudifyDialog) => void,
) {
  startClaudify(
    { ...state, repo: pr.repo, subject: `${pr.repo}#${pr.number}`, about: "this pull request" },
    open,
  );
}

/// The button, for the pull request detail view.
///
/// `compact` is the one-line desktop header (#1580): header-sized, with
/// the unavailable reason in its short form. The phone header wraps onto
/// its own line, so it keeps the full sentence.
export function PrClaudifyButton({
  pr,
  compact = false,
}: {
  pr: PullRequest | PrDetail;
  compact?: boolean;
}) {
  const state = usePrClaudify(pr);
  const [dialog, setDialog] = useState<PrClaudifyDialog | null>(null);
  const { checkout, terminalConfigured } = state;
  const reason = unavailableReason(pr.repo, checkout);

  return (
    <>
      {checkout.kind === "found" || checkout.kind === "pending" ? (
        <button
          type="button"
          // Disabled only while the scan is still answering, with the
          // reason beside it -- the "found" answer usually arrives from
          // the Worktrees cache before anyone reaches for it.
          disabled={checkout.kind === "pending"}
          aria-busy={checkout.kind === "pending" ? true : undefined}
          onClick={() => activate(pr, state, setDialog)}
          title={
            checkout.kind === "found"
              ? terminalConfigured
                ? `Start Claude Code on this pull request in ${checkout.path}`
                : `Copy a command that starts Claude Code on this pull request in ${checkout.path}`
              : undefined
          }
          className={compact ? compactClass(CLAUDIFY_CLASS) : CLAUDIFY_CLASS}
        >
          <Sparkles className="h-3.5 w-3.5" aria-hidden="true" />
          Claudify
        </button>
      ) : (
        <button
          type="button"
          onClick={() => activate(pr, state, setDialog)}
          className={compact ? compactClass(PLAIN_CLASS) : PLAIN_CLASS}
        >
          <Bot className="h-3.5 w-3.5" aria-hidden="true" />
          Copy prompt
        </button>
      )}
      {reason !== null ? (
        <span
          role={checkout.kind === "failed" ? "alert" : undefined}
          title={compact ? reason : undefined}
          className={`text-xs ${checkout.kind === "failed" ? "text-[#f85149]" : "text-[#8b949e]"}`}
        >
          {compact ? (
            <>
              <span aria-hidden="true">{shortReason(checkout)}</span>
              <span className="sr-only">{reason}</span>
            </>
          ) : (
            reason
          )}
        </span>
      ) : null}
      <PrClaudifyDialogs dialog={dialog} onClose={() => setDialog(null)} />
    </>
  );
}

/// The menu item, for the row kebab.
///
/// Mounted only while the menu is open, so a list of rows does not
/// start a worktree scan just by rendering. `onPick` closes the menu and
/// hands any dialog to the kebab, which outlives this item.
export function PrClaudifyMenuItem({
  pr,
  onPick,
}: {
  pr: PullRequest;
  onPick: (dialog: PrClaudifyDialog | null) => void;
}) {
  const state = usePrClaudify(pr);
  const { checkout } = state;
  const reason = unavailableReason(pr.repo, checkout);
  const claudify = checkout.kind === "found" || checkout.kind === "pending";

  return (
    <button
      type="button"
      role="menuitem"
      disabled={checkout.kind === "pending"}
      title={reason ?? undefined}
      onClick={() => {
        // Close the menu first; a dialog -- opened now, or after the
        // phone's round trip -- arrives through the same callback.
        onPick(null);
        activate(pr, state, onPick);
      }}
      className={`flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-sm hover:bg-[#21262d] disabled:opacity-50 ${
        claudify ? "text-[#a371f7]" : "text-[#e6edf3]"
      }`}
    >
      {claudify ? (
        <Sparkles className="h-3.5 w-3.5" aria-hidden="true" />
      ) : (
        <Bot className="h-3.5 w-3.5" aria-hidden="true" />
      )}
      <span className="flex flex-col">
        <span>{claudify ? "Claudify" : "Copy prompt"}</span>
        {checkout.kind === "none" || checkout.kind === "failed" ? (
          <span className="text-[11px] text-[#8b949e]">
            {checkout.kind === "none" ? "No local checkout" : "Checkout lookup failed"}
          </span>
        ) : null}
      </span>
    </button>
  );
}

/// The terms dialog and the phone's command dialog.
///
/// Hook-free on purpose (nothing from `@/api/hooks`), so the kebab can
/// render it on every row without every row subscribing to anything.
export function PrClaudifyDialogs({
  dialog,
  onClose,
}: {
  dialog: PrClaudifyDialog | null;
  onClose: () => void;
}) {
  // The terms live WITH the dialog, so closing it forgets them: a
  // remembered `bypassPermissions` must not apply to the next launch
  // the user did not look at (the Worktrees page's rule, #1214).
  const [terms, setTerms] = useState<LaunchTerms>({});
  const close = () => {
    setTerms({});
    onClose();
  };
  if (dialog === null) return null;

  if (dialog.kind === "phone") {
    return (
      <Dialog open onOpenChange={close}>
        <DialogContent className="max-w-lg">
          <DialogTitle>
            Claudify {dialog.subject}
          </DialogTitle>
          <ActingOnDesktop />
          <p className="text-sm text-[#8b949e]">
            {dialog.claudeInstalled
              ? `Run this on that desktop to start Claude Code on ${dialog.about}.`
              : "Run this on that desktop. Claude Code was not found there, so it may need installing first."}
          </p>
          {/* Selectable and wrapped, with no copy button: the phone's
              clipboard is what does not work here. */}
          <pre className="max-h-48 overflow-auto rounded border border-[#30363d] bg-[#0d1117] p-3 text-xs break-all whitespace-pre-wrap select-text">
            {dialog.command}
          </pre>
          <div className="flex justify-end gap-2">
            <Button variant="ghost" className="min-h-11" onClick={close}>
              Close
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    );
  }

  const { repo, subject, about, checkout, prompt } = dialog;
  return (
    <Dialog open onOpenChange={close}>
      <DialogContent className="max-w-2xl">
        <DialogTitle>
          Hand {subject} to Claude Code
        </DialogTitle>
        <p className="text-sm text-[#8b949e]">
          This opens the terminal you configured in Settings and starts Claude Code in {checkout} on
          the prompt for {about}.
        </p>
        <LaunchTermsPicker
          terms={terms}
          onChange={setTerms}
          preview={(t) => claudeLaunchPrPreview(checkout, repo, prompt, t)}
        />
        <div className="flex justify-end gap-2">
          <Button
            className="min-h-11"
            onClick={() => {
              const chosen = terms;
              close();
              claudeLaunchPr(checkout, repo, prompt, chosen).then(
                () =>
                  toast.success("Opening Claude Code in your terminal", {
                    description: `Starting in ${checkout}.`,
                  }),
                (e: unknown) =>
                  // No automatic fallback: a launch that silently copied
                  // would leave the user watching for a terminal that
                  // never opens. Copying is offered, as a choice.
                  toast.error("Could not open your terminal", {
                    description: typeof e === "string" ? e : undefined,
                    action: {
                      label: "Copy instead",
                      onClick: () => copyClaudify(repo, subject, about, checkout, prompt, () => {}),
                    },
                  }),
              );
            }}
          >
            Open in terminal
          </Button>
          <Button variant="ghost" className="min-h-11" onClick={close}>
            Cancel
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

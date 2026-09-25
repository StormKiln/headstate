import { Bot, Sparkles } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { useUiPrefs, useWorktrees } from "@/api/hooks";
import {
  claudeLaunchPr,
  claudeLaunchPrPreview,
  claudifyPrCommand,
  type LaunchTerms,
} from "@/api/tauri";
import { agentPrompt, toAgentContext } from "@/lib/agentPrompt";
import { copyText } from "@/lib/clipboard";
import { IS_MOBILE_BUILD } from "@/lib/target";
import { mainCheckoutFor } from "@/lib/worktrees";
import type { PrDetail, PullRequest } from "@/types/pr";
import { ActingOnDesktop } from "./ActingOnDesktop";
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

/// Where the main checkout lookup stands.
///
/// Four states, not three: "still scanning" and "scanned and not found"
/// are different facts (a skeleton and an answer), and a scan that FAILED
/// is neither -- it must not read as "you have no checkout".
export type CheckoutState =
  | { kind: "pending" }
  | { kind: "found"; path: string }
  /// `unreadable` is how many folders the scan could not read. Non-zero
  /// makes "not found" a floor, and the text qualifies it.
  | { kind: "none"; unreadable: number }
  | { kind: "failed"; error: string };

/// Everything a Claudify control needs about one pull request.
function usePrClaudify(pr: PullRequest | PrDetail) {
  const scan = useWorktrees();
  const { prefs } = useUiPrefs();
  // The Worktrees page's own test (#1126): never on the phone, where
  // the launch is `Class::Local` and would always be refused.
  const terminalConfigured = !IS_MOBILE_BUILD && (prefs?.terminal_command ?? "").trim() !== "";

  // Read before narrowing on `data`: the hook re-shapes the query
  // result, and TypeScript then narrows `error` away in that branch.
  const error: unknown = scan.error;
  let checkout: CheckoutState;
  if (scan.data === undefined) {
    checkout = scan.isError
      ? {
          kind: "failed",
          error: typeof error === "string" ? error : "the scan did not finish",
        }
      : { kind: "pending" };
  } else {
    const path = mainCheckoutFor(scan.data, pr.repo);
    checkout =
      path === null ? { kind: "none", unreadable: scan.unreadable.length } : { kind: "found", path };
  }

  const path = checkout.kind === "found" ? checkout.path : undefined;
  const prompt = agentPrompt(toAgentContext(pr, path));
  return { checkout, terminalConfigured, prompt };
}

/// A dialog a Claudify opened: the terms picker, or the phone's command.
///
/// State that OUTLIVES the control that opened it. The kebab closes its
/// menu on click, which unmounts the menu item, so the dialog belongs to
/// the kebab and this is what it holds.
export type PrClaudifyDialog =
  | { kind: "launch"; repo: string; number: number; checkout: string; prompt: string }
  | { kind: "phone"; repo: string; number: number; command: string; claudeInstalled: boolean };

/// Copy the prompt alone -- the route when there is no checkout to run in.
function copyPromptOnly(prompt: string) {
  void copyText(prompt).then((failure) =>
    failure === null
      ? toast.success("Prompt copied — paste it to an agent")
      : toast.error("Could not copy the prompt", { description: failure }),
  );
}

/// The copy half of Claudify: the Rust-built line, to the clipboard or,
/// on the phone, to a dialog.
function copyClaudify(
  repo: string,
  number: number,
  checkout: string,
  prompt: string,
  open: (d: PrClaudifyDialog) => void,
) {
  claudifyPrCommand(checkout, repo, prompt).then(
    async ({ command, claude_installed }) => {
      // The phone has no terminal and no usable clipboard; show the line
      // for the machine it runs on, as the Worktrees phone dialog does.
      if (IS_MOBILE_BUILD) {
        open({ kind: "phone", repo, number, command, claudeInstalled: claude_installed });
        return;
      }
      const failure = await copyText(command);
      if (failure !== null) {
        toast.error("Could not copy the command", { description: failure });
        return;
      }
      toast.success("Command copied to the clipboard", {
        description: claude_installed
          ? "Paste it in your terminal to start Claude Code on this pull request."
          : "Paste it in your terminal. Claude Code was not found on this machine.",
      });
    },
    (e: unknown) =>
      toast.error("Could not build the command", {
        description: typeof e === "string" ? e : undefined,
      }),
  );
}

/// What pressing Claudify does, decided in ONE place so the button and
/// the menu item cannot drift (the Worktrees page's `claudify` rule).
function activate(
  pr: PullRequest | PrDetail,
  state: ReturnType<typeof usePrClaudify>,
  open: (d: PrClaudifyDialog) => void,
) {
  const { checkout, terminalConfigured, prompt } = state;
  if (checkout.kind !== "found") {
    copyPromptOnly(prompt);
    return;
  }
  if (terminalConfigured) {
    open({ kind: "launch", repo: pr.repo, number: pr.number, checkout: checkout.path, prompt });
  } else {
    copyClaudify(pr.repo, pr.number, checkout.path, prompt, open);
  }
}

/// Why there is no Claudify, as a sentence the reader can act on.
function unavailableReason(repo: string, checkout: CheckoutState): string | null {
  switch (checkout.kind) {
    case "pending":
      return `Looking for a local checkout of ${repo}…`;
    case "failed":
      return `Could not look for a local checkout of ${repo}: ${checkout.error}`;
    case "none":
      // Qualified when the scan was partial: "not found" is then only
      // "not found in what could be read".
      return checkout.unreadable > 0
        ? `No local checkout of ${repo} was found in the folders that could be read (${checkout.unreadable} could not), so this copies the prompt only.`
        : `No local checkout of ${repo} was found in the scanned folders, so this copies the prompt only.`;
    case "found":
      return null;
  }
}

/// The Claudify styling from the Worktrees row -- purple, with the
/// sparkles -- at the size of the buttons beside it here.
const CLAUDIFY_CLASS =
  "flex w-fit items-center gap-1.5 rounded border border-[#8957e5]/40 px-3 py-1.5 text-sm text-[#a371f7] hover:bg-[#8957e5]/10 disabled:opacity-50";
const PLAIN_CLASS =
  "flex w-fit items-center gap-1.5 rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#161b22]";

/// The button, for the pull request detail view.
export function PrClaudifyButton({ pr }: { pr: PullRequest | PrDetail }) {
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
          // the Worktrees cache before anyone reaches this footer.
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
          className={CLAUDIFY_CLASS}
        >
          <Sparkles className="h-3.5 w-3.5" aria-hidden="true" />
          Claudify
        </button>
      ) : (
        <button type="button" onClick={() => activate(pr, state, setDialog)} className={PLAIN_CLASS}>
          <Bot className="h-3.5 w-3.5" aria-hidden="true" />
          Copy prompt
        </button>
      )}
      {reason !== null ? (
        <span
          role={checkout.kind === "failed" ? "alert" : undefined}
          className={`text-xs ${checkout.kind === "failed" ? "text-[#f85149]" : "text-[#8b949e]"}`}
        >
          {reason}
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
            Claudify {dialog.repo}#{dialog.number}
          </DialogTitle>
          <ActingOnDesktop />
          <p className="text-sm text-[#8b949e]">
            {dialog.claudeInstalled
              ? "Run this on that desktop to start Claude Code on this pull request."
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

  const { repo, number, checkout, prompt } = dialog;
  return (
    <Dialog open onOpenChange={close}>
      <DialogContent className="max-w-2xl">
        <DialogTitle>
          Hand {repo}#{number} to Claude Code
        </DialogTitle>
        <p className="text-sm text-[#8b949e]">
          This opens the terminal you configured in Settings and starts Claude Code in {checkout} on
          the prompt for this pull request.
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
                      onClick: () => copyClaudify(repo, number, checkout, prompt, () => {}),
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

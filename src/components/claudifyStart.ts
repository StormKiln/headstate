import { toast } from "sonner";
import { useUiPrefs, useWorktrees } from "@/api/hooks";
import { claudifyPrCommand } from "@/api/tauri";
import { copyText } from "@/lib/clipboard";
import { IS_MOBILE_BUILD } from "@/lib/target";
import { mainCheckoutFor } from "@/lib/worktrees";

/// What a pull request Claudify decides before anything renders: the
/// checkout lookup, what pressing it starts, and why it is unavailable.
/// `PrClaudify.tsx` holds the controls and dialogs that use these; they
/// live apart so that file exports only components (#1965), which is
/// what lets dev hot-reload swap it in place.

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

/// The first of `repos` with a main checkout on this machine, and whether
/// a terminal is configured -- what every pull request Claudify needs
/// before it can start (#1455, shared with the strip's batch in #1579).
///
/// `repo` is the repository whose checkout was found, or `null` when none
/// was. For one pull request `repos` is just its own.
export function useClaudifyCheckout(repos: readonly string[]): {
  checkout: CheckoutState;
  repo: string | null;
  terminalConfigured: boolean;
} {
  const scan = useWorktrees();
  const { prefs } = useUiPrefs();
  // The Worktrees page's own test (#1126): never on the phone, where
  // the launch is `Class::Local` and would always be refused.
  const terminalConfigured = !IS_MOBILE_BUILD && (prefs?.terminal_command ?? "").trim() !== "";

  // Read before narrowing on `data`: the hook re-shapes the query
  // result, and TypeScript then narrows `error` away in that branch.
  const error: unknown = scan.error;
  if (scan.data === undefined) {
    const checkout: CheckoutState = scan.isError
      ? {
          kind: "failed",
          error: typeof error === "string" ? error : "the scan did not finish",
        }
      : { kind: "pending" };
    return { checkout, repo: null, terminalConfigured };
  }
  for (const repo of repos) {
    const path = mainCheckoutFor(scan.data, repo);
    if (path !== null) return { checkout: { kind: "found", path }, repo, terminalConfigured };
  }
  return {
    checkout: { kind: "none", unreadable: scan.unreadable.length },
    repo: null,
    terminalConfigured,
  };
}

/// A dialog a Claudify opened: the terms picker, or the phone's command.
///
/// State that OUTLIVES the control that opened it. The kebab closes its
/// menu on click, which unmounts the menu item, so the dialog belongs to
/// the kebab and this is what it holds.
///
/// `subject` titles the dialog ("acme/widget#12"); `about` finishes its
/// sentences ("this pull request"). `repo` is the repository `checkout`
/// must be a checkout of -- Rust re-checks it.
export type PrClaudifyDialog =
  | {
      kind: "launch";
      repo: string;
      subject: string;
      about: string;
      checkout: string;
      prompt: string;
    }
  | { kind: "phone"; subject: string; about: string; command: string; claudeInstalled: boolean };

/// What a Claudify is about to start, however it was pressed.
export interface ClaudifyStart {
  /// The repository the checkout belongs to, or the pull request's own
  /// when there is no checkout.
  repo: string;
  subject: string;
  about: string;
  checkout: CheckoutState;
  terminalConfigured: boolean;
  prompt: string;
}

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
export function copyClaudify(
  repo: string,
  subject: string,
  about: string,
  checkout: string,
  prompt: string,
  open: (d: PrClaudifyDialog) => void,
) {
  claudifyPrCommand(checkout, repo, prompt).then(
    async ({ command, claude_installed }) => {
      // The phone has no terminal and no usable clipboard; show the line
      // for the machine it runs on, as the Worktrees phone dialog does.
      if (IS_MOBILE_BUILD) {
        open({ kind: "phone", subject, about, command, claudeInstalled: claude_installed });
        return;
      }
      const failure = await copyText(command);
      if (failure !== null) {
        toast.error("Could not copy the command", { description: failure });
        return;
      }
      toast.success("Command copied to the clipboard", {
        description: claude_installed
          ? `Paste it in your terminal to start Claude Code on ${about}.`
          : "Paste it in your terminal. Claude Code was not found on this machine.",
      });
    },
    (e: unknown) =>
      toast.error("Could not build the command", {
        description: typeof e === "string" ? e : undefined,
      }),
  );
}

/// What pressing Claudify does, decided in ONE place so the button, the
/// menu item and the strip's batch (#1579) cannot drift (the Worktrees
/// page's `claudify` rule).
export function startClaudify(s: ClaudifyStart, open: (d: PrClaudifyDialog) => void) {
  const { repo, subject, about, checkout, terminalConfigured, prompt } = s;
  if (checkout.kind !== "found") {
    copyPromptOnly(prompt);
    return;
  }
  if (terminalConfigured) {
    open({ kind: "launch", repo, subject, about, checkout: checkout.path, prompt });
  } else {
    copyClaudify(repo, subject, about, checkout.path, prompt, open);
  }
}

/// Why there is no Claudify, as a sentence the reader can act on.
/// `repo` names what was looked for: a repository, or a phrase for
/// several ("any of their repositories").
export function unavailableReason(repo: string, checkout: CheckoutState): string | null {
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
export const CLAUDIFY_CLASS =
  "flex w-fit items-center gap-1.5 rounded border border-[#8957e5]/40 px-3 py-1.5 text-sm text-[#bc8cff] hover:bg-[#8957e5]/10 disabled:opacity-50";
export const PLAIN_CLASS =
  "flex w-fit items-center gap-1.5 rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#161b22]";

/// The header's button size (#1580): the pinned Approve, Merge and GitHub
/// beside it are `px-2.5 py-1`, and one taller button in a one-line bar
/// reads as a different kind of control.
export function compactClass(cls: string): string {
  return cls.replace("px-3 py-1.5", "px-2.5 py-1");
}

/// The short form of `unavailableReason`, for the one-line desktop header
/// (#1580), which has no room for the sentence. The full sentence goes in
/// the title and to a screen reader, as the header's "Won't count toward
/// merging" does (#1451).
export function shortReason(checkout: CheckoutState): string | null {
  switch (checkout.kind) {
    case "pending":
      return "Looking for a checkout…";
    case "failed":
      return "Could not look for a checkout";
    case "none":
      // Qualified like the sentence: a partial scan found nothing only
      // in what it could read.
      return checkout.unreadable > 0 ? "No checkout in the readable folders" : "No local checkout";
    case "found":
      return null;
  }
}

import { Bot, Sparkles } from "lucide-react";
import { useState } from "react";
import { useViewer } from "@/api/hooks";
import { readyBatchPrompt } from "@/lib/readyClaudify";
import type { ReadyRow } from "@/lib/readyMarkdown";
import {
  CLAUDIFY_CLASS,
  PLAIN_CLASS,
  PrClaudifyDialogs,
  type PrClaudifyDialog,
  compactClass,
  shortReason,
  startClaudify,
  unavailableReason,
  useClaudifyCheckout,
} from "./PrClaudify";

/// Claudify for the whole Ready for review strip (#1579).
///
/// Starts Claude Code on one prompt: the owner's instruction to approve
/// and merge whichever of these pull requests are eligible, the viewer's
/// login, and the strip's rows as "Copy as markdown" lists them
/// (`readyBatchPrompt`, the same formatter as #1578).
///
/// # It starts Claude; it approves and merges nothing itself
///
/// Approving and merging are outward-facing. This button only opens the
/// same terms dialog every pull request Claudify opens (#1455, #1214):
/// the argv preview shows the whole prompt before anything runs, and the
/// permission mode is the user's choice there. With no terminal it copies
/// the command; on the phone it shows it for the desktop; with no local
/// checkout it copies the prompt alone -- all exactly as `PrClaudify`,
/// because it IS `PrClaudify`'s `startClaudify`.
///
/// # Where Claude starts
///
/// In the main checkout of the first shown row's repository that has one
/// on this machine. The pull requests may span repositories, and each is
/// named in the list by repository and number, so Claude addresses them
/// by name rather than by working directory.
///
/// # Disabled, with the reason, rather than hidden
///
/// - Nothing showing: there is nothing to hand over.
/// - The viewer's login not read (yet, or at all): the fifth condition,
///   "I did not push most recently", cannot be checked without it, and a
///   guessed login would be worse than none.
/// - The checkout lookup still answering, as `PrClaudify`.
export function ReadyClaudify({ rows }: { rows: readonly ReadyRow[] }) {
  const viewer = useViewer();
  // Every repository once, in the strip's order.
  const repos = [...new Set(rows.map((r) => r.pr.repo))];
  const { checkout, repo, terminalConfigured } = useClaudifyCheckout(repos);
  const [dialog, setDialog] = useState<PrClaudifyDialog | null>(null);

  const n = rows.length;
  const login = typeof viewer.data === "string" && viewer.data !== "" ? viewer.data : null;

  // Why the button cannot be pressed at all, most basic first.
  let blocked: string | null = null;
  if (n === 0) blocked = "No pull requests are showing, so there is nothing to hand to Claude.";
  else if (login === null) {
    blocked = viewer.isError
      ? "Your GitHub login could not be read, so the prompt cannot say which pushes are yours."
      : "Reading your GitHub login…";
  } else if (checkout.kind === "pending") {
    blocked = unavailableReason("these repositories", checkout);
  }

  // Past the block: a checkout, or the prompt-only route and why.
  const claudify = checkout.kind === "found" || checkout.kind === "pending";
  const reason = blocked ?? unavailableReason("any of these repositories", checkout);
  const short = blocked !== null ? null : shortReason(checkout);

  const press = () => {
    if (login === null || n === 0) return;
    startClaudify(
      {
        // The found checkout's repository; Rust re-checks the pair. With
        // no checkout the prompt is copied, and this is unused.
        repo: repo ?? rows[0].pr.repo,
        subject: `${n} ready ${n === 1 ? "pull request" : "pull requests"}`,
        about: n === 1 ? "this pull request" : `these ${n} pull requests`,
        checkout,
        terminalConfigured,
        prompt: readyBatchPrompt(rows, login, new Date()),
      },
      setDialog,
    );
  };

  return (
    <span className="inline-flex items-center gap-2 font-normal">
      <button
        type="button"
        disabled={blocked !== null}
        aria-busy={
          n > 0 && ((login === null && viewer.isPending) || checkout.kind === "pending")
            ? true
            : undefined
        }
        onClick={press}
        title={reason ?? undefined}
        className={compactClass(claudify ? CLAUDIFY_CLASS : PLAIN_CLASS)}
      >
        {claudify ? (
          <Sparkles className="h-3.5 w-3.5" aria-hidden="true" />
        ) : (
          <Bot className="h-3.5 w-3.5" aria-hidden="true" />
        )}
        {claudify ? "Claudify" : "Copy prompt"}
      </button>
      {reason !== null ? (
        <span
          data-ready-claudify-reason
          role={checkout.kind === "failed" && blocked === null ? "alert" : undefined}
          className={`text-xs ${checkout.kind === "failed" && blocked === null ? "text-[#f85149]" : "text-[#8b949e]"}`}
        >
          {/* The header has room for the short form; the whole sentence
              is in the title and read to a screen reader, as the PR
              header's compact Claudify does (#1580). */}
          {short !== null ? (
            <>
              <span aria-hidden="true">{short}</span>
              <span className="sr-only">{reason}</span>
            </>
          ) : (
            reason
          )}
        </span>
      ) : null}
      <PrClaudifyDialogs dialog={dialog} onClose={() => setDialog(null)} />
    </span>
  );
}

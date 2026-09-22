import { useEffect, useState } from "react";
import {
  claudeMdAdviceLaunch,
  claudeMdAdviceLaunchPreview,
  type ClaudifyTarget,
  type LaunchPreview,
} from "@/api/tauri";
import { copyText } from "@/lib/clipboard";
import { IS_MOBILE_BUILD } from "@/lib/target";
import { toast } from "sonner";
import { ArgvPreview } from "./ArgvPreview";

/// Turning one advice finding — or a whole report — into a prompt (#1292).
///
/// # The prompt is not authored here, and that is the point
///
/// `Finding.brief` is "Markdown for an agent, rendered by `brief::render`
/// at construction so it can never disagree with the fields above", and
/// `Finding::new` exists because "a `Finding` built by hand could carry a
/// `brief` that names a different subject than its `subject` field, and
/// the panel copies the brief without reading it".
///
/// So this component composes NO text. Copy sends `finding.brief`
/// verbatim, and Run passes an INDEX to Rust, which looks the same brief
/// up from the stored report and builds the command line itself. There
/// is no point at which TypeScript could produce a prompt that disagrees
/// with the finding it is attached to — which is precisely what
/// templating a second prompt here would reintroduce.
///
/// # Copy is always offered; Run is not
///
/// Run needs a configured terminal, and `read_ui_prefs().terminal_command`
/// is the one that answers that — the same setting `claude_launch_session`
/// uses. There is deliberately no second shell setting.
///
/// With no terminal configured this renders a SENTENCE saying so, not a
/// disabled button. #1292 names that specifically: "no terminal
/// configured must read as 'no terminal is configured', not as a disabled
/// button with no explanation and not as a silent no-op". A greyed
/// control with no text is the failure, because the remedy — set one in
/// Settings — is invisible from it.
///
/// # Run shows the line first
///
/// #1214 established it for the resume path and `LaunchTermsPicker` says
/// why: "the clipboard path let the user read the line before pasting it.
/// A spawn path takes that away." It matters MORE here. A resume command
/// is one short line; this one carries an entire Markdown brief, so
/// reading it is the only way to know what the session will be asked to
/// do. The preview is therefore a required step rather than a disclosure
/// the user can skip.
///
/// # Every failure is reported as a failure
///
/// Three of them, each with its own wording and none of them silent:
/// a clipboard that refused the write, a preview that could not be built
/// (which is where "no terminal is configured" surfaces as a refusal from
/// Rust), and a launch that did not start. The last is the one that must
/// never be mistaken for success — so there is no optimistic toast
/// anywhere: the success toast fires only after the promise resolves.
export function ClaudifyAction({
  /// The brief to copy, exactly as the backend rendered it.
  brief,
  /// The repository the report is about. The launch runs `claude` here.
  repo,
  /// Which brief Rust should look up for the run path.
  target,
  /// What to call this in a toast: "Brief" or "All briefs".
  what,
  /// Whether a terminal is configured, from `terminal_command`.
  terminalConfigured,
}: {
  brief: string;
  repo: string;
  target: ClaudifyTarget;
  what: string;
  terminalConfigured: boolean;
}) {
  const [showRun, setShowRun] = useState(false);

  return (
    <span className="inline-flex flex-wrap items-center gap-2">
      <button
        type="button"
        onClick={() => {
          // `copyText` reports the no-clipboard case rather than doing
          // nothing, and the toast is what makes the click visible. The
          // error arm is NOT a fallback that pretends it worked.
          void copyText(brief).then((failure) =>
            failure === null
              ? toast.success(`${what} copied to the clipboard`, {
                  description: "Paste it into a Claude session to make the change.",
                })
              : toast.error(`Could not copy the ${what.toLowerCase()}`, { description: failure }),
          );
        }}
        className="tap-target text-[11px] text-[#58a6ff] hover:underline"
      >
        Copy {what.toLowerCase()}
      </button>

      {/* Run is desktop-only: it opens a terminal WINDOW on this machine,
          which is `Class::Local`'s stated test. A phone is shown Copy
          alone rather than a control it could never complete. */}
      {IS_MOBILE_BUILD ? null : terminalConfigured ? (
        <button
          type="button"
          onClick={() => setShowRun((v) => !v)}
          aria-expanded={showRun}
          className="tap-target text-[11px] text-[#58a6ff] hover:underline"
        >
          {showRun ? "Cancel" : "Run in terminal…"}
        </button>
      ) : (
        // The sentence, not a disabled button. It names the remedy,
        // because "Run" greyed out with no text is a dead end.
        <span className="text-[11px] text-[#8b949e]">
          No terminal is configured, so this can only be copied. Set one in Settings › Claude
          Code to run it.
        </span>
      )}

      {showRun ? (
        <RunPanel
          repo={repo}
          target={target}
          what={what}
          onDone={() => setShowRun(false)}
        />
      ) : null}
    </span>
  );
}

/// The exact line, and the button that runs it.
function RunPanel({
  repo,
  target,
  what,
  onDone,
}: {
  repo: string;
  target: ClaudifyTarget;
  what: string;
  onDone: () => void;
}) {
  const [built, setBuilt] = useState<LaunchPreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [launching, setLaunching] = useState(false);

  // The built line. `live` guards an out-of-order response, the same
  // way `LaunchTermsPicker` does.
  useEffect(() => {
    let live = true;
    void claudeMdAdviceLaunchPreview(repo, target).then(
      (p) => {
        if (!live) return;
        setBuilt(p);
        setPreviewError(null);
      },
      (e: unknown) => {
        if (!live) return;
        // The preview reports the same refusals the launch would, so
        // this is where the user finds out — before pressing anything.
        setBuilt(null);
        setPreviewError(typeof e === "string" ? e : "the command line could not be built");
      },
    );
    return () => {
      live = false;
    };
  }, [repo, target]);

  return (
    <span className="mt-1 block w-full rounded-md border border-[#30363d] bg-[#161b22] p-2">
      <span className="block text-[11px] font-semibold text-[#e6edf3]">
        This is what will run:
      </span>
      {previewError !== null ? (
        <span className="mt-1 block text-[11px] text-[#f85149]">{previewError}</span>
      ) : built === null ? (
        <span className="mt-1 block text-[11px] text-[#8b949e]">Building the command line…</span>
      ) : (
        <>
          <ArgvPreview as="span" program={built.program} args={built.args} />
          <span className="mt-1 block text-[11px] text-[#8b949e]">
            Each box is one argument. The whole brief is a single argument, newlines included,
            so nothing in it is read as a command.
          </span>
        </>
      )}

      <span className="mt-2 flex flex-wrap items-center gap-2">
        <button
          type="button"
          // Unrunnable without a line to run. This is a disabled button
          // WITH an explanation above it, which is the case the
          // no-terminal sentence is deliberately not.
          disabled={built === null || launching}
          onClick={() => {
            setLaunching(true);
            void claudeMdAdviceLaunch(repo, target).then(
              () => {
                setLaunching(false);
                // Only after it resolved. An optimistic toast here is
                // exactly the "it must never look like the prompt ran"
                // failure.
                toast.success(`${what} opened in your terminal`, {
                  description: "Claude Code is starting on it there.",
                });
                onDone();
              },
              (e: unknown) => {
                setLaunching(false);
                // Says the LAUNCH failed, and keeps the panel open so
                // the line is still readable and Copy is still there.
                toast.error(`Could not run the ${what.toLowerCase()}`, {
                  description: typeof e === "string" ? e : "the terminal did not start.",
                });
              },
            );
          }}
          className="tap-target rounded border border-[#30363d] px-2 py-1 text-[11px] text-[#e6edf3] hover:bg-[#0d1117] disabled:text-[#6e7681]"
        >
          {launching ? "Starting…" : "Run it"}
        </button>
        <button
          type="button"
          onClick={onDone}
          className="tap-target text-[11px] text-[#8b949e] hover:underline"
        >
          Cancel
        </button>
      </span>
    </span>
  );
}

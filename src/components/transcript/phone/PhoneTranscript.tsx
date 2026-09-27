import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { claudeTranscriptBlockText } from "@/api/tauri";
import { useClaudeTranscriptMessages } from "@/api/hooks";
import { PullIndicator } from "@/components/PullIndicator";
import { Sheet, SheetContent, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { commandError } from "@/lib/errorKind";
import { maskingNote } from "@/lib/masked";
import { IS_MOBILE_BUILD } from "@/lib/target";
import { isRevealOff, isTranscriptsOff } from "@/lib/transcriptAccess";
import { usePullToRefresh } from "@/lib/usePullToRefresh";
import type { Liveness } from "../../../types/pr";
import type { TranscriptMessage, TranscriptSubagent } from "../../../types/transcript";
import { palette } from "../palette";
import { transcriptStreaming } from "../streaming";
import { deriveTaskChecklist, taskSummary } from "../tasks";
import { TaskChecklist } from "../TaskChecklist";
import { TranscriptViewer } from "../TranscriptViewer";
import type { LoadFullText } from "../types";
import { PhoneContext, type PhoneTranscriptContext } from "./context";
import { PhoneMessage } from "./PhoneMessage";
import { scaleStyle, useTextScale } from "./textScale";
import { thinkingStarts } from "./timing";

/// The shell's `renderMessage` for the phone. Module-level, so the shell
/// sees one function for the life of the app.
const renderPhoneMessage = (m: TranscriptMessage) => <PhoneMessage message={m} />;

/// One transcript on the phone (#1481): the viewer shell (#1479) with
/// the phone's renderer, and everything around it the phone needs.
///
/// # What is here, and what is not
///
/// | concern | where |
/// |---|---|
/// | scrolling, follow, "jump to latest", windowing | `TranscriptViewer` |
/// | what one message looks like | `renderPhoneMessage` |
/// | masked secrets, Reveal, transcripts turned off (#1488) | here |
/// | the task list, in a sheet (#1504) | here |
/// | pull to refresh | here |
/// | Dynamic Type | `textScale.ts`, applied per row |
///
/// # Reveal (#1488)
///
/// The desktop masks likely secrets before transcript text leaves it,
/// and says so in the answer's `masking`. When it also says
/// `reveal_allowed`, a Reveal button re-reads the page with
/// `reveal: true` -- a fresh read, from nothing, not an update of the
/// masked one. The masked read stays cached underneath, so a refused
/// reveal leaves the masked text on screen with the refusal beside it.
///
/// # Transcripts turned off (#1488)
///
/// The desktop's owner can switch a phone's transcript reading off; the
/// desktop then refuses the read. That is a setting, not a failure, and
/// is said as one, with where to change it.
///
/// # The touch shield
///
/// The app's own pull to refresh (`App.tsx`) listens on the main panel
/// and arms whenever THAT is at its top -- which, with the transcript
/// filling it and scrolling inside, is always. Without the shield every
/// downward drag anywhere in a transcript would re-poll GitHub. So a
/// touch that starts in the transcript stops here, after this view's
/// own gesture has seen it.
export function PhoneTranscript({
  path,
  liveness,
  label = "Transcript",
}: {
  path: string;
  liveness: Liveness;
  label?: string;
}) {
  const live = liveness.state === "running";
  const [reveal, setReveal] = useState(false);
  const revealed = useClaudeTranscriptMessages(path, reveal, live, true);
  // The masked read keeps running unless the revealed one is standing in
  // for it: a refused or failed reveal falls back to it at once.
  const masked = useClaudeTranscriptMessages(path, !reveal || revealed.isError, live);
  const showingRevealed = reveal && revealed.data !== undefined;
  const page = showingRevealed ? revealed.data : masked.data;
  const active = showingRevealed ? revealed : masked;

  const scale = useTextScale();
  const [tasksOpen, setTasksOpen] = useState(false);
  const [subagent, setSubagent] = useState<TranscriptSubagent | null>(null);

  const messages = useMemo(() => page?.messages ?? [], [page]);
  const truncated = page?.truncated ?? false;
  const checklist = useMemo(
    () => deriveTaskChecklist(messages, { truncated }),
    [messages, truncated],
  );
  const starts = useMemo(() => thinkingStarts(messages), [messages]);
  const onLoadFullText = useCallback<LoadFullText>(
    (a) => claudeTranscriptBlockText(path, a.messageId, a.index, showingRevealed),
    [path, showingRevealed],
  );
  const ctx = useMemo<PhoneTranscriptContext>(
    () => ({
      liveness,
      tasks: checklist,
      scale,
      thinkingStarts: starts,
      onLoadFullText,
      onOpenSubagent: setSubagent,
    }),
    [liveness, checklist, scale, starts, onLoadFullText],
  );

  // Pull to refresh on the viewer's own scroller, and the shield (see
  // the module docs). The viewport is the shell's; found once mounted.
  const wrapRef = useRef<HTMLDivElement>(null);
  const viewportRef = useRef<HTMLElement | null>(null);
  const hasMessages = messages.length > 0;
  useLayoutEffect(() => {
    viewportRef.current =
      wrapRef.current?.querySelector<HTMLElement>('[data-slot="message-scroller-viewport"]') ??
      null;
  }, [hasMessages]);
  // The query's own `refetch`, which is stable per query: a callback that
  // changed on every render would re-attach the gesture's listeners
  // mid-pull and lose the pull.
  const refetch = active.refetch;
  const refresh = useCallback(() => refetch(), [refetch]);
  const pull = usePullToRefresh(viewportRef, refresh, IS_MOBILE_BUILD && hasMessages);
  useEffect(() => {
    const el = wrapRef.current;
    if (!IS_MOBILE_BUILD || el === null) return;
    const stop = (e: TouchEvent) => e.stopPropagation();
    el.addEventListener("touchstart", stop, { passive: true });
    return () => el.removeEventListener("touchstart", stop);
    // Keyed like the viewport lookup: the wrapper mounts with the first
    // messages, not with this component.
  }, [hasMessages]);

  const summary = taskSummary(checklist);
  const note = maskingNote(page?.masking);
  const canReveal =
    page?.masking?.reveal_allowed === true && !page.masking.revealed && page.masking.hidden > 0;

  let body;
  if (page === undefined) {
    if (masked.isError) {
      body = isTranscriptsOff(masked.error) ? (
        <TranscriptsOff onCheck={() => void masked.refetch()} />
      ) : (
        // BEFORE the empty arm (#846): a rejection is not a transcript
        // with nothing in it.
        <p className="text-xs" style={{ color: palette.muted }}>
          Could not read its transcript
          {commandError(masked.error).message ? ` (${commandError(masked.error).message})` : ""}.
          This is not the same as the session having said nothing.{" "}
          <button
            type="button"
            className="underline"
            style={{ color: palette.link }}
            onClick={() => void masked.refetch()}
          >
            Try again
          </button>
        </p>
      );
    } else {
      body = (
        <p className="text-xs" style={{ color: palette.muted }}>
          Reading its transcript…
        </p>
      );
    }
  } else if (page.masking?.withheld) {
    body = <TranscriptsOff onCheck={() => void active.refetch()} />;
  } else if (page.messages.length === 0) {
    body = (
      <p className="text-xs" style={{ color: palette.muted }}>
        {page.truncated
          ? "No conversation in the part of this transcript that was read."
          : "Its transcript holds no conversation to show."}
      </p>
    );
  } else {
    body = (
      <div ref={wrapRef} className="relative flex min-h-0 flex-1 flex-col">
        <PullIndicator state={pull} />
        <PhoneContext.Provider value={ctx}>
          <TranscriptViewer
            messages={page.messages}
            renderMessage={renderPhoneMessage}
            streaming={transcriptStreaming(liveness)}
            label={label}
            // Within thumb reach: the jump button sits bottom-right, off
            // the centre line where the home indicator's swipe lives,
            // and a 44 pt target.
            className="[&_[data-slot=message-scroller-button]]:inset-s-auto [&_[data-slot=message-scroller-button]]:right-4 [&_[data-slot=message-scroller-button]]:min-h-11 [&_[data-slot=message-scroller-button]]:translate-x-0 [&_[data-slot=message-scroller-viewport]]:[-webkit-overflow-scrolling:touch]"
          />
        </PhoneContext.Provider>
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2" data-testid="phone-transcript">
      {page !== undefined ? (
        <div
          className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs"
          style={{ color: palette.muted }}
        >
          {page.truncated && page.messages.length > 0 ? (
            <span data-testid="transcript-truncated">
              The last {page.messages.length.toLocaleString()} message
              {page.messages.length === 1 ? "" : "s"}. Earlier ones are not shown.
            </span>
          ) : null}
          {note ? <span>{note}</span> : null}
          {canReveal ? (
            <button
              type="button"
              onClick={() => setReveal(true)}
              disabled={reveal && revealed.isFetching}
              className="tap-target rounded-md border px-2"
              style={{ borderColor: palette.border, color: palette.text }}
            >
              {reveal && revealed.isFetching ? "Revealing…" : "Reveal"}
            </button>
          ) : null}
          {showingRevealed && page.masking?.revealed ? (
            <>
              <span>Hidden text is shown.</span>
              <button
                type="button"
                onClick={() => setReveal(false)}
                className="tap-target rounded-md border px-2"
                style={{ borderColor: palette.border, color: palette.text }}
              >
                Hide it again
              </button>
            </>
          ) : null}
          {reveal && revealed.isError ? (
            <span role="alert" style={{ color: palette.warn }}>
              {isRevealOff(revealed.error)
                ? "This computer does not allow this phone to reveal hidden text."
                : `The hidden text could not be revealed: ${commandError(revealed.error).message}`}
            </span>
          ) : null}
          {checklist.tasks.length > 0 ? (
            <button
              type="button"
              aria-haspopup="dialog"
              aria-expanded={tasksOpen}
              onClick={() => setTasksOpen(true)}
              className="tap-target ml-auto rounded-md border px-2"
              style={{ borderColor: palette.border, color: palette.text }}
            >
              Tasks{summary ? ` · ${summary.text}` : ""}
            </button>
          ) : null}
        </div>
      ) : null}
      {body}
      <Sheet open={tasksOpen} onOpenChange={setTasksOpen}>
        <SheetContent side="bottom" className="max-h-[85dvh] overflow-y-auto">
          <SheetHeader>
            <SheetTitle>Tasks</SheetTitle>
          </SheetHeader>
          <div className="px-4 pb-4" style={scaleStyle(scale)}>
            <TaskChecklist checklist={checklist} variant="compact" />
          </div>
        </SheetContent>
      </Sheet>
      <Sheet open={subagent !== null} onOpenChange={(o) => (o ? null : setSubagent(null))}>
        <SheetContent side="bottom" className="flex h-[90dvh] flex-col">
          <SheetHeader>
            <SheetTitle className="pr-8 break-words">
              Subagent{subagent?.agent_type ? ` · ${subagent.agent_type}` : ""}
            </SheetTitle>
          </SheetHeader>
          <div className="flex min-h-0 flex-1 flex-col px-4 pb-4">
            {subagent?.transcript_path ? (
              <PhoneTranscript
                path={subagent.transcript_path}
                liveness={subagentLiveness(liveness)}
                label="Subagent transcript"
              />
            ) : null}
          </div>
        </SheetContent>
      </Sheet>
    </div>
  );
}

/// A subagent's own liveness is not tracked. Its session being over
/// settles it; otherwise whether it is still running is not known, and
/// its unanswered calls say that rather than "running".
function subagentLiveness(parent: Liveness): Liveness {
  return parent.state === "dead"
    ? parent
    : { state: "unknown", why: "a subagent's own process is not tracked" };
}

/// The desktop's owner turned this phone's transcripts off (#1488).
///
/// A setting, not a failure: said as one, with where it is changed.
/// "Check again" re-asks, because the switch can be turned back on while
/// this screen is open -- a retry that can succeed.
function TranscriptsOff({ onCheck }: { onCheck: () => void }) {
  return (
    <div className="text-sm" data-testid="transcripts-off">
      <p style={{ color: palette.text }}>
        Transcripts are turned off for this phone on the desktop.
      </p>
      <p className="mt-1 text-xs" style={{ color: palette.muted }}>
        They can be turned on under Settings &gt; Paired devices on that computer.{" "}
        <button type="button" className="underline" style={{ color: palette.link }} onClick={onCheck}>
          Check again
        </button>
      </p>
    </div>
  );
}

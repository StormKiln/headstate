import { type CSSProperties, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { claudeTranscriptBlockText } from "@/api/tauri";
import { useClaudeTranscriptLive } from "@/api/hooks";
import { PullIndicator } from "@/components/PullIndicator";
import { Sheet, SheetContent, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { commandError } from "@/lib/errorKind";
import { maskingNote } from "@/lib/masked";
import { IS_MOBILE_BUILD } from "@/lib/target";
import { isRevealOff, isTranscriptsOff } from "@/lib/transcriptAccess";
import { usePullToRefresh } from "@/lib/usePullToRefresh";
import type { ClaudeWaiting, Liveness } from "../../../types/pr";
import type { TranscriptMessage, TranscriptSubagent } from "../../../types/transcript";
import { Composer } from "../Composer";
import { subagentLiveness } from "../header";
import { palette } from "../palette";
import type { PendingMessage } from "../pending";
import { transcriptStreaming } from "../streaming";
import { FollowStatus } from "../FollowStatus";
import { deriveTaskChecklist, taskSummary } from "../tasks";
import { TaskChecklist } from "../TaskChecklist";
import {
  AwayCard,
  ExportControls,
  FindInSession,
  ShowControls,
  TurnOutline,
  UnreadDivider,
} from "../navigation";
import {
  useJumps,
  useOpenedMarker,
  useShown,
  useSinceYouLeft,
} from "../useNavigation";
import { TranscriptViewer, type TranscriptViewerHandle } from "../TranscriptViewer";
import type { LoadFullText } from "../types";
import { usePendingMessages } from "../usePendingMessages";
import { PhoneContext, type PhoneTranscriptContext } from "./context";
import { PhoneMessage, PhonePendingMessage } from "./PhoneMessage";
import { scaleStyle, useTextScale } from "./textScale";
import { thinkingStarts } from "./timing";

/// The shell's `renderMessage` for the phone. Module-level, so the shell
/// sees one function for the life of the app.
const renderPhoneMessage = (m: TranscriptMessage) => <PhoneMessage message={m} />;
/// Its `renderPending` (#1491), for the same reason.
const renderPhonePending = (p: PendingMessage) => <PhonePendingMessage pending={p} />;

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
/// | since you left, prompts, turns, find, show, export (#1484) | `../navigation.tsx`, in sheets |
///
/// # The data (#1476)
///
/// `useClaudeTranscriptLive`: the newest page, older pages as the reader
/// scrolls up, live growth while the session writes, at most a bounded
/// number of messages held -- and, backgrounded, only the reader's own
/// pages. `src/lib/transcriptFollow.ts` argues all of it.
///
/// # Reveal (#1488)
///
/// The desktop masks likely secrets before transcript text leaves it,
/// and says so in the answer's `masking`. When it also says
/// `reveal_allowed`, a Reveal button re-reads with `reveal: true` -- a
/// fresh follower, from nothing, not an update of the masked one. The
/// masked follower stays underneath, paused, so a refused reveal leaves
/// the masked text on screen with the refusal beside it.
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
  sessionId = null,
  waiting,
  openAt = "latest",
}: {
  path: string;
  liveness: Liveness;
  label?: string;
  /// The session `path` is the main transcript of, so the desktop's
  /// activity nudge for it reads at once (#1477). Omitted for a
  /// subagent's transcript, which is not nudged.
  sessionId?: string | null;
  /// For the "while you were away" card (#1484). Absent: not said.
  waiting?: ClaudeWaiting;
  /// Open at the newest turn, or at this device's "since you left"
  /// marker -- a notification's tap (#1486, #1484).
  openAt?: "latest" | "marker";
}) {
  const marker = useOpenedMarker(path);
  const openAtId = openAt === "marker" ? (marker?.id ?? null) : null;
  const [reveal, setReveal] = useState(false);
  const revealed = useClaudeTranscriptLive(path, {
    liveness,
    reveal: true,
    enabled: reveal,
    sessionId,
    openAt: openAtId,
  });
  // Refused or failed before it read anything: the masked text stays.
  const revealFailed =
    reveal && revealed.messages === undefined && revealed.status === "could-not-read";
  // The masked follower keeps running unless the revealed one is standing
  // in for it: a refused or failed reveal falls back to it at once.
  const masked = useClaudeTranscriptLive(path, {
    liveness,
    enabled: !reveal || revealFailed,
    sessionId,
    openAt: openAtId,
  });
  const showingRevealed = reveal && revealed.messages !== undefined;
  const active = showingRevealed ? revealed : masked;
  const held = active.messages;
  const masking = active.masking;

  const scale = useTextScale();
  const [tasksOpen, setTasksOpen] = useState(false);
  const [subagent, setSubagent] = useState<TranscriptSubagent | null>(null);

  const messages = useMemo(() => held ?? [], [held]);
  const truncated = active.hasOlder;
  const checklist = useMemo(
    () => deriveTaskChecklist(messages, { truncated }),
    [messages, truncated],
  );
  const starts = useMemo(() => thinkingStarts(messages), [messages]);
  // 7.10's sends (#1491). Nothing adds one in 7.9: the composer has no
  // `onSend` and is hidden behind `COMPOSER_ENABLED`.
  const pending = usePendingMessages(messages);
  const onLoadFullText = useCallback<LoadFullText>(
    (a) => claudeTranscriptBlockText(path, a.messageId, a.index, showingRevealed, a.offset),
    [path, showingRevealed],
  );
  const loadOlderUntil = active.loadOlderUntil;
  // Pages back until the call is held, not one page per tap (#1484).
  const onLoadEarlier = useMemo(
    () =>
      active.hasOlder
        ? (toolUseId: string | null) =>
            void loadOlderUntil((m) =>
              toolUseId === null
                ? true
                : m.blocks.some((b) => b.kind === "tool_call" && b.id === toolUseId),
            )
        : undefined,
    [active.hasOlder, loadOlderUntil],
  );
  const ctx = useMemo<PhoneTranscriptContext>(
    () => ({
      liveness,
      tasks: checklist,
      scale,
      thinkingStarts: starts,
      onLoadFullText,
      onOpenSubagent: setSubagent,
      onLoadEarlier,
    }),
    [liveness, checklist, scale, starts, onLoadFullText, onLoadEarlier],
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
  // The follower's own `refresh`, which is stable per follower: a
  // callback that changed on every render would re-attach the gesture's
  // listeners mid-pull and lose the pull.
  const refetch = active.refresh;
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

  const { show, messages: shown, hidden } = useShown(messages);
  const since = useSinceYouLeft({
    path,
    marker,
    messages,
    shown,
    hasOlder: active.hasOlder,
    tasks: checklist,
    waiting,
  });
  const [cardDismissed, setCardDismissed] = useState(false);
  const handle = useRef<TranscriptViewerHandle>(null);
  const jumps = useJumps({ live: active, messages, shown, handle });
  const [sheet, setSheet] = useState<"turns" | "find" | "options" | null>(null);
  const jumpTo = jumps.jumpTo;
  const onHit = useCallback(
    (h: { message_id: string; cursor: { offset: number; behind_digest: string } }) => {
      setSheet(null);
      jumpTo(h.message_id, h.cursor);
    },
    [jumpTo],
  );
  const dividerAt = since.dividerAt;
  const renderMessage = useCallback(
    (m: TranscriptMessage) =>
      m.id === dividerAt ? (
        <>
          <UnreadDivider />
          {renderPhoneMessage(m)}
        </>
      ) : (
        renderPhoneMessage(m)
      ),
    [dividerAt],
  );
  // Opened from a notification: land on the first unread message once.
  const landed = useRef(openAt !== "marker");
  useEffect(() => {
    if (landed.current || held === undefined) return;
    landed.current = true;
    if (dividerAt !== null) handle.current?.scrollTo(dividerAt);
  }, [held, dividerAt]);
  const goToMarker =
    marker === null
      ? undefined
      : since.beforeHeld
        ? () => jumps.jumpTo(marker.id, null)
        : dividerAt !== null
          ? () => void handle.current?.scrollTo(dividerAt)
          : undefined;

  const summary = taskSummary(checklist);
  const note = maskingNote(masking);
  const canReveal =
    masking?.reveal_allowed === true && !masking.revealed && masking.hidden > 0;
  const revealing = reveal && revealed.status === "loading";

  let body;
  if (held === undefined) {
    if (masked.status === "could-not-read") {
      body = isTranscriptsOff(masked.error) ? (
        <TranscriptsOff onCheck={() => void masked.refresh()} />
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
            onClick={() => void masked.refresh()}
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
  } else if (masking?.withheld) {
    body = <TranscriptsOff onCheck={() => void active.refresh()} />;
  } else if (held.length === 0) {
    body = (
      <p className="text-xs" style={{ color: palette.muted }}>
        {active.hasOlder
          ? "No conversation in the part of this transcript that was read."
          : "Its transcript holds no conversation to show."}
      </p>
    );
  } else {
    body = (
      // `--text-scale` reaches the jump button, which the viewer draws:
      // Dynamic Type sizes it like the rows (#1489).
      <div
        ref={wrapRef}
        className="relative flex min-h-0 flex-1 flex-col"
        style={{ "--text-scale": scale } as CSSProperties}
      >
        <PullIndicator state={pull} />
        {/* Beside the viewer, never instead of it: the viewer keeps the
            composer slot, pending rows and the live edge (#1490
            constraint 3), whatever the Show settings hide. */}
        {shown.length === 0 ? (
          <p className="text-xs" style={{ color: palette.muted }} data-testid="all-hidden">
            Everything loaded is hidden by the Show settings.
          </p>
        ) : null}
        <PhoneContext.Provider value={ctx}>
          <TranscriptViewer
            messages={shown}
            renderMessage={renderMessage}
            handle={handle}
            onRead={since.onRead}
            streaming={transcriptStreaming(liveness)}
            label={label}
            onReachStart={active.loadOlder}
            onReachEnd={active.loadNewer}
            onWindowChange={active.setViewport}
            atLiveEdge={active.atLiveEdge}
            onJumpToLatest={active.jumpToLatest}
            pending={pending.visible}
            renderPending={renderPhonePending}
            composer={<Composer variant="phone" />}
            // Within thumb reach: the jump button sits bottom-right, off
            // the centre line where the home indicator's swipe lives,
            // and a 44 pt target.
            className="[&_[data-slot=message-scroller-button]]:inset-s-auto [&_[data-slot=message-scroller-button]]:right-4 [&_[data-slot=message-scroller-button]]:min-h-11 [&_[data-slot=message-scroller-button]]:translate-x-0 [&_[data-slot=message-scroller-button]]:[zoom:var(--text-scale)] [&_[data-slot=message-scroller-viewport]]:[-webkit-overflow-scrolling:touch]"
          />
        </PhoneContext.Provider>
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2" data-testid="phone-transcript">
      {held !== undefined ? (
        <div
          className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs"
          style={{ color: palette.muted, ...scaleStyle(scale) }}
        >
          <FollowStatus live={active} />
          {note ? <span>{note}</span> : null}
          {canReveal ? (
            <button
              type="button"
              onClick={() => setReveal(true)}
              disabled={revealing}
              className="tap-target rounded-md border px-2"
              style={{ borderColor: palette.border, color: palette.text }}
            >
              {revealing ? "Revealing…" : "Reveal"}
            </button>
          ) : null}
          {showingRevealed && masking?.revealed ? (
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
          {revealFailed ? (
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
      {held !== undefined && held.length > 0 ? (
        <div
          role="group"
          aria-label="Navigate the transcript"
          className="flex flex-wrap items-center gap-2 text-xs"
          style={scaleStyle(scale)}
        >
          <button
            type="button"
            onClick={() => jumps.step(-1)}
            className="tap-target rounded-md border px-2"
            style={{ borderColor: palette.border, color: palette.text }}
          >
            <span aria-hidden>↑</span> Previous prompt
          </button>
          <button
            type="button"
            onClick={() => jumps.step(1)}
            className="tap-target rounded-md border px-2"
            style={{ borderColor: palette.border, color: palette.text }}
          >
            <span aria-hidden>↓</span> Next prompt
          </button>
          {(["turns", "find", "options"] as const).map((k) => (
            <button
              key={k}
              type="button"
              aria-haspopup="dialog"
              aria-expanded={sheet === k}
              onClick={() => setSheet(k)}
              className="tap-target rounded-md border px-2"
              style={{ borderColor: palette.border, color: palette.text }}
            >
              {k === "turns"
                ? "Turns"
                : k === "find"
                  ? "Find"
                  : `Options${hidden > 0 ? ` · ${hidden.toLocaleString()} hidden` : ""}`}
            </button>
          ))}
        </div>
      ) : null}
      {held !== undefined && !cardDismissed ? (
        <div style={scaleStyle(scale)}>
          <AwayCard since={since} onGo={goToMarker} onDismiss={() => setCardDismissed(true)} />
        </div>
      ) : null}
      {/* Said from a region mounted with the host (#1489), as the
          desktop does. */}
      <span role="status" className="sr-only" data-testid="jump-note-announce">
        {jumps.note}
      </span>
      {jumps.note !== null ? (
        <p className="text-xs" style={{ color: palette.warn, ...scaleStyle(scale) }}>
          {jumps.note}
        </p>
      ) : null}
      {body}
      <Sheet open={sheet !== null} onOpenChange={(o) => (o ? null : setSheet(null))}>
        <SheetContent side="bottom" className="max-h-[85dvh] overflow-y-auto">
          <SheetHeader>
            <SheetTitle>
              {sheet === "turns" ? "Turns" : sheet === "find" ? "Find in this session" : "Options"}
            </SheetTitle>
          </SheetHeader>
          <div className="px-4 pb-4" style={scaleStyle(scale)}>
            {sheet === "turns" ? (
              <TurnOutline path={path} reveal={showingRevealed} onJump={onHit} />
            ) : sheet === "find" ? (
              <FindInSession path={path} reveal={showingRevealed} onJump={onHit} />
            ) : sheet === "options" ? (
              <div className="flex flex-col gap-4">
                <ShowControls show={show} hidden={hidden} />
                <ExportControls
                  messages={messages}
                  hasOlder={active.hasOlder}
                  atLiveEdge={active.atLiveEdge}
                />
              </div>
            ) : null}
          </div>
        </SheetContent>
      </Sheet>
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
                liveness={subagentLiveness(liveness, subagent)}
                label="Subagent transcript"
              />
            ) : null}
          </div>
        </SheetContent>
      </Sheet>
    </div>
  );
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

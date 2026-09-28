/// One session's transcript on the desktop (#1480): the viewer shell
/// (#1479) with the terminal renderer, the task checklist beside it
/// (#1504), a density toggle, and subagent transcripts opened in place --
/// and navigation (#1484): "since you left", `j`/`k` between prompts,
/// the turn outline and find in a side panel, Show and Export
/// (`navigation.tsx`, `useNavigation.ts`).
///
/// Its one desktop host is a session's Transcript tab (#1546); the phone
/// layout renders `PhoneTranscript` there instead (#1481).
///
/// The data is `useClaudeTranscriptLive` (#1476): the newest page first,
/// older pages as the reader scrolls up, live growth on an adaptive
/// cadence, and a bounded number of messages held
/// (`src/lib/transcriptFollow.ts`). Everything below the read takes a
/// message list and does not care where it came from.

import {
  type KeyboardEvent,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useClaudeTranscriptLive } from "../../api/hooks";
import { claudeTranscriptBlockText } from "../../api/tauri";
import { useFilters } from "../../store/filters";
import type { ClaudeWaiting, Liveness } from "../../types/pr";
import type { TranscriptMessage, TranscriptSubagent } from "../../types/transcript";
import { errorMessage } from "../QueryError";
import { Composer } from "./Composer";
import { FollowStatus } from "./FollowStatus";
import { subagentLiveness } from "./header";
import { palette } from "./palette";
import type { PendingMessage } from "./pending";
import { transcriptStreaming } from "./streaming";
import { TaskChecklist } from "./TaskChecklist";
import { deriveTaskChecklist } from "./tasks";
import {
  TerminalMessage,
  TerminalPendingMessage,
  type TerminalEnv,
  type TranscriptDensity,
} from "./TerminalMessage";
import {
  AwayCard,
  ExportControls,
  FindInSession,
  ShowControls,
  TurnOutline,
  UnreadDivider,
} from "./navigation";
import {
  turnKeys,
  useJumps,
  useOpenedMarker,
  useShown,
  useSinceYouLeft,
} from "./useNavigation";
import { TranscriptViewer, type TranscriptViewerHandle } from "./TranscriptViewer";
import { type TurnFooter, turnFooters } from "./turnFooter";
import type { LoadFullText, OpenSubagent } from "./types";
import { usePendingMessages } from "./usePendingMessages";

const NO_MESSAGES: readonly TranscriptMessage[] = [];

export function DesktopTranscript({
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
  /// activity nudge for it reads at once (#1477). A subagent's
  /// transcript below is not nudged and follows on its cadence.
  sessionId?: string | null;
  /// Whether the session is waiting on the reader, for the "while you
  /// were away" card (#1484). Absent: not said.
  waiting?: ClaudeWaiting;
  /// Open at the newest turn, or at this device's "since you left"
  /// marker (a notification's tap, #1484).
  openAt?: "latest" | "marker";
}) {
  // A subagent opened from a call replaces the main transcript here, with
  // a way back. A stack, because a subagent can open its own.
  const [stack, setStack] = useState<TranscriptSubagent[]>([]);
  const open = stack[stack.length - 1] ?? null;
  const onOpenSubagent = useCallback<OpenSubagent>((sub) => setStack((s) => [...s, sub]), []);

  if (open && open.transcript_path) {
    return (
      <div className="flex min-h-0 flex-1 flex-col gap-2" data-testid="subagent-transcript">
        <div className="flex flex-wrap items-center gap-2 text-xs">
          <button
            type="button"
            onClick={() => setStack((s) => s.slice(0, -1))}
            className="tap-target -ml-1 rounded px-2 hover:bg-[#161b22] focus-visible:outline focus-visible:outline-2"
            style={{ color: palette.link }}
          >
            ← {stack.length > 1 ? "Back to the previous subagent" : "Back to the main transcript"}
          </button>
          <span style={{ color: palette.text }}>
            Subagent{open.agent_type ? `: ${open.agent_type}` : ""}
          </span>
        </div>
        <Loaded
          // Keyed so a different subagent is a fresh read, not the last
          // one's messages under a new name.
          key={open.transcript_path}
          path={open.transcript_path}
          // From its session's: see `subagentLiveness`.
          liveness={subagentLiveness(liveness, open)}
          label={`Subagent transcript${open.agent_type ? `: ${open.agent_type}` : ""}`}
          onOpenSubagent={onOpenSubagent}
        />
      </div>
    );
  }
  return (
    <Loaded
      path={path}
      liveness={liveness}
      label={label}
      onOpenSubagent={onOpenSubagent}
      sessionId={sessionId}
      waiting={waiting}
      openAt={openAt}
    />
  );
}

/// The side panel beside the transcript: the turn outline or find.
type Panel = "turns" | "find" | null;

function Loaded({
  path,
  liveness,
  label,
  onOpenSubagent,
  sessionId = null,
  waiting,
  openAt = "latest",
}: {
  path: string;
  liveness: Liveness;
  label: string;
  onOpenSubagent: OpenSubagent;
  sessionId?: string | null;
  waiting?: ClaudeWaiting;
  openAt?: "latest" | "marker";
}) {
  const marker = useOpenedMarker(path);
  const live = useClaudeTranscriptLive(path, {
    liveness,
    sessionId,
    openAt: openAt === "marker" ? (marker?.id ?? null) : null,
  });
  const density = useFilters((f) => f.transcriptDensity);
  const setDensity = useFilters((f) => f.setTranscriptDensity);

  const messages = live.messages;
  const truncated = live.hasOlder;
  // Read by "Copy turn" on click. Updated after commit, never in render.
  const holder = useRef<readonly TranscriptMessage[]>([]);
  useLayoutEffect(() => {
    holder.current = messages ?? [];
  }, [messages]);

  const onLoadFullText = useCallback<LoadFullText>(
    (a) => claudeTranscriptBlockText(path, a.messageId, a.index, false, a.offset),
    [path],
  );
  const loadOlderUntil = live.loadOlderUntil;
  // "Load earlier" beside a result pages back until its call is held,
  // not one page per click (#1476's follow-up, #1484).
  const onLoadEarlier = useMemo(
    () =>
      live.hasOlder
        ? (toolUseId: string | null) =>
            void loadOlderUntil((m) =>
              toolUseId === null
                ? true
                : m.blocks.some((b) => b.kind === "tool_call" && b.id === toolUseId),
            )
        : undefined,
    [live.hasOlder, loadOlderUntil],
  );
  const streaming = transcriptStreaming(liveness);
  const env = useMemo<TerminalEnv>(
    () => ({
      liveness,
      density: density === "compact" ? "compact" : "comfortable",
      onLoadFullText,
      onOpenSubagent,
      onLoadEarlier,
      messages: () => holder.current,
    }),
    [liveness, density, onLoadFullText, onOpenSubagent, onLoadEarlier],
  );
  const footers = useMemo(
    () => turnFooters(messages ?? [], streaming === true),
    [messages, streaming],
  );
  const tasks = useMemo(
    () => deriveTaskChecklist(messages ?? [], { truncated }),
    [messages, truncated],
  );
  const all = messages ?? NONE;
  const { show, messages: shown, hidden } = useShown(all);
  const placedFooters = useMemo(() => footersOnShown(footers, all, shown), [footers, all, shown]);
  const since = useSinceYouLeft({
    path,
    marker,
    messages: all,
    shown,
    hasOlder: live.hasOlder,
    tasks,
    waiting,
  });
  const [cardDismissed, setCardDismissed] = useState(false);
  const handle = useRef<TranscriptViewerHandle>(null);
  const jumps = useJumps({ live, messages: all, shown, handle });
  const [panel, setPanel] = useState<Panel>(null);
  // Where `Escape` hands the focus back when it closes a panel.
  const panelButtons = useRef<Record<"turns" | "find", HTMLButtonElement | null>>({
    turns: null,
    find: null,
  });
  const onPanelButton = useCallback((k: "turns" | "find", el: HTMLButtonElement | null) => {
    panelButtons.current[k] = el;
  }, []);
  const toLatest = useCallback(() => handle.current?.scrollToLatest(), []);

  // Opened from a notification: land on the first unread message once.
  const landed = useRef(openAt !== "marker");
  const dividerAt = since.dividerAt;
  useEffect(() => {
    if (landed.current || messages === undefined) return;
    landed.current = true;
    if (dividerAt !== null) handle.current?.scrollTo(dividerAt);
  }, [messages, dividerAt]);

  const renderMessage = useCallback(
    (m: TranscriptMessage) => (
      <>
        {m.id === dividerAt ? <UnreadDivider /> : null}
        <TerminalMessage
          message={m}
          footer={placedFooters.get(m.id)}
          env={env}
          tasks={holdsTaskCall(m) ? tasks : undefined}
        />
      </>
    ),
    [placedFooters, env, tasks, dividerAt],
  );
  // 7.10's sends (#1491). Nothing adds one in 7.9: the composer below
  // has no `onSend` and is hidden behind `COMPOSER_ENABLED`.
  const pending = usePendingMessages(messages ?? NO_MESSAGES);
  const renderPending = useCallback(
    (p: PendingMessage) => <TerminalPendingMessage pending={p} density={env.density} />,
    [env.density],
  );

  if (messages === undefined) {
    // BEFORE any empty arm (#846): no messages on a rejection is not a
    // transcript with nothing in it -- and a read not answered yet is
    // not one either.
    return live.status === "could-not-read" ? (
      <p className="text-xs" style={{ color: palette.muted }}>
        Could not read its transcript
        {errorMessage(live.error) ? ` (${errorMessage(live.error)})` : ""}. This is not the same
        as the session having said nothing.{" "}
        <button
          type="button"
          className="underline"
          style={{ color: palette.link }}
          onClick={() => void live.refresh()}
        >
          Try again
        </button>
      </p>
    ) : (
      <p className="text-xs" style={{ color: palette.muted }}>
        Reading its transcript…
      </p>
    );
  }
  if (messages.length === 0) {
    return (
      <p className="text-xs" style={{ color: palette.muted }}>
        {live.hasOlder
          ? "No conversation in the part of this transcript that was read."
          : "Its transcript holds no conversation to show."}
      </p>
    );
  }
  const goToMarker =
    marker === null
      ? undefined
      : since.beforeHeld
        ? () => jumps.jumpTo(marker.id, null)
        : dividerAt !== null
          ? () => void handle.current?.scrollTo(dividerAt)
          : undefined;
  // `Escape` closes what it is inside (#1489): an open Show or Export
  // menu, else the side panel -- and hands the focus back to what
  // opened it, so the keyboard is never left on a control that went away.
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      const menu = (e.target as HTMLElement).closest<HTMLDetailsElement>("details[open]");
      if (menu) {
        e.preventDefault();
        menu.open = false;
        menu.querySelector("summary")?.focus();
        return;
      }
      if (panel !== null) {
        e.preventDefault();
        const opener = panel;
        setPanel(null);
        panelButtons.current[opener]?.focus();
        return;
      }
    }
    turnKeys(jumps.step, toLatest)(e);
  };
  return (
    <div
      className="@container flex min-h-0 flex-1 flex-col gap-2"
      onKeyDown={onKeyDown}
      data-testid="desktop-transcript"
    >
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs" style={{ color: palette.muted }}>
        <FollowStatus live={live} />
        {/* A jump's note, said from a region mounted with the host: one
            that arrives with its text already in it -- or is hidden until
            it has some -- is often not read (#1489). */}
        <span role="status" className="sr-only" data-testid="jump-note-announce">
          {jumps.note}
        </span>
        <NavButtons panel={panel} setPanel={setPanel} step={jumps.step} onButton={onPanelButton} />
        <details className="relative">
          <summary className="cursor-pointer" style={{ color: palette.link }}>
            Show{hidden > 0 ? ` (${hidden.toLocaleString()} hidden)` : ""}
          </summary>
          <div
            className="absolute z-10 mt-1 w-64 rounded border p-2"
            style={{ background: palette.surface, borderColor: palette.border }}
          >
            <ShowControls show={show} hidden={hidden} />
          </div>
        </details>
        <details className="relative">
          <summary className="cursor-pointer" style={{ color: palette.link }}>
            Export
          </summary>
          <div
            className="absolute z-10 mt-1 w-80 rounded border p-2"
            style={{ background: palette.surface, borderColor: palette.border }}
          >
            <ExportControls messages={all} hasOlder={live.hasOlder} atLiveEdge={live.atLiveEdge} />
          </div>
        </details>
        <DensityToggle value={env.density} onChange={setDensity} />
      </div>
      {!cardDismissed ? (
        <AwayCard since={since} onGo={goToMarker} onDismiss={() => setCardDismissed(true)} />
      ) : null}
      {jumps.note !== null ? (
        <p className="text-xs" style={{ color: palette.warn }}>
          {jumps.note}
        </p>
      ) : null}
      {tasks.tasks.length > 0 ? (
        // Narrow panes (the session detail) fold the checklist above the
        // transcript; wide ones pin it beside it. A container query, not
        // the viewport: the same window holds both hosts.
        <details
          className="rounded border p-2 @2xl:hidden"
          style={{ background: palette.surface, borderColor: palette.border }}
          data-testid="task-checklist-folded"
        >
          <summary className="cursor-pointer text-xs" style={{ color: palette.text }}>
            Tasks
          </summary>
          <TaskChecklist checklist={tasks} variant="terminal" />
        </details>
      ) : null}
      <div className="flex min-h-0 flex-1 gap-2">
        <div
          className="flex min-h-0 min-w-0 flex-1 flex-col rounded border"
          style={{ background: palette.ground, borderColor: palette.border }}
        >
          {/* Beside the viewer, never instead of it: the viewer keeps the
              composer slot, pending rows and the live edge (#1490
              constraint 3), whatever the Show settings hide. */}
          {shown.length === 0 ? (
            <p className="p-3 text-xs" style={{ color: palette.muted }} data-testid="all-hidden">
              Everything loaded is hidden by the Show settings.
            </p>
          ) : null}
          <TranscriptViewer
            messages={shown}
            renderMessage={renderMessage}
            streaming={streaming}
            label={label}
            onReachStart={live.loadOlder}
            onReachEnd={live.loadNewer}
            onWindowChange={live.setViewport}
            atLiveEdge={live.atLiveEdge}
            onJumpToLatest={live.jumpToLatest}
            pending={pending.visible}
            renderPending={renderPending}
            composer={<Composer variant="desktop" />}
            handle={handle}
            onRead={since.onRead}
            className="min-h-0 flex-1"
          />
        </div>
        {panel !== null ? (
          <aside
            aria-label={panel === "turns" ? "Turns" : "Find in this session"}
            className="w-72 shrink-0 overflow-y-auto rounded border p-2"
            style={{ background: palette.surface, borderColor: palette.border }}
          >
            {panel === "turns" ? (
              <TurnOutline path={path} onJump={(h) => jumps.jumpTo(h.message_id, h.cursor)} />
            ) : (
              <FindInSession
                path={path}
                autoFocus
                onJump={(h) => jumps.jumpTo(h.message_id, h.cursor)}
              />
            )}
          </aside>
        ) : null}
        {tasks.tasks.length > 0 ? (
          <aside
            className="hidden w-64 shrink-0 overflow-y-auto rounded border p-2 @2xl:block"
            style={{ background: palette.surface, borderColor: palette.border }}
          >
            <TaskChecklist checklist={tasks} variant="terminal" />
          </aside>
        ) : null}
      </div>
    </div>
  );
}

const NONE: readonly TranscriptMessage[] = [];

/// Footers placed on shown rows: a turn's footer whose row the Show
/// settings hid moves to the last shown row of the same turn. Its
/// figures are the whole turn's, computed before filtering.
function footersOnShown(
  footers: Map<string, TurnFooter>,
  all: readonly TranscriptMessage[],
  shown: readonly TranscriptMessage[],
): Map<string, TurnFooter> {
  if (shown === all) return footers;
  const visible = new Set(shown.map((m) => m.id));
  const turnOf = new Map(all.map((m) => [m.id, m.turn_id]));
  const lastShown = new Map<string | null, string>();
  for (const m of shown) lastShown.set(m.turn_id, m.id);
  const out = new Map<string, TurnFooter>();
  for (const [id, f] of footers) {
    const host = visible.has(id) ? id : lastShown.get(turnOf.get(id) ?? null);
    if (host !== undefined && !out.has(host)) out.set(host, f);
  }
  return out;
}

/// Previous and next prompt, and the side panel's two modes.
function NavButtons({
  panel,
  setPanel,
  step,
  onButton,
}: {
  panel: Panel;
  setPanel: (p: Panel) => void;
  step: (dir: -1 | 1) => void;
  /// Each panel's button, for `Escape` to return the focus to.
  onButton: (k: "turns" | "find", el: HTMLButtonElement | null) => void;
}) {
  const btn = "rounded border px-2 py-0.5 focus-visible:outline focus-visible:outline-2";
  const style = (on: boolean) => ({
    borderColor: palette.border,
    background: on ? palette.userBand : "transparent",
    color: on ? palette.text : palette.muted,
  });
  return (
    <div role="group" aria-label="Navigate the transcript" className="flex gap-1">
      <button
        type="button"
        className={btn}
        style={style(false)}
        onClick={() => step(-1)}
        title="Previous prompt (k)"
        aria-label="Previous prompt"
        aria-keyshortcuts="k"
      >
        <span aria-hidden>↑</span> Prompt
      </button>
      <button
        type="button"
        className={btn}
        style={style(false)}
        onClick={() => step(1)}
        title="Next prompt (j)"
        aria-label="Next prompt"
        aria-keyshortcuts="j"
      >
        <span aria-hidden>↓</span> Prompt
      </button>
      <button
        ref={(el) => onButton("turns", el)}
        type="button"
        className={btn}
        style={style(panel === "turns")}
        aria-pressed={panel === "turns"}
        onClick={() => setPanel(panel === "turns" ? null : "turns")}
      >
        Turns
      </button>
      <button
        ref={(el) => onButton("find", el)}
        type="button"
        className={btn}
        style={style(panel === "find")}
        aria-pressed={panel === "find"}
        onClick={() => setPanel(panel === "find" ? null : "find")}
      >
        Find
      </button>
    </div>
  );
}

function holdsTaskCall(m: TranscriptMessage): boolean {
  return m.blocks.some(
    (b) =>
      b.kind === "tool_call" &&
      (b.args.tool === "task_create" ||
        b.args.tool === "task_update" ||
        b.args.tool === "task_get" ||
        b.args.tool === "task_list"),
  );
}

function DensityToggle({
  value,
  onChange,
}: {
  value: TranscriptDensity;
  onChange: (d: TranscriptDensity) => void;
}) {
  return (
    <div role="group" aria-label="Transcript density" className="ml-auto flex gap-1">
      {(["comfortable", "compact"] as const).map((d) => (
        <button
          key={d}
          type="button"
          aria-pressed={value === d}
          onClick={() => onChange(d)}
          className="rounded border px-2 py-0.5 capitalize focus-visible:outline focus-visible:outline-2"
          style={{
            borderColor: palette.border,
            background: value === d ? palette.userBand : "transparent",
            color: value === d ? palette.text : palette.muted,
          }}
        >
          {d}
        </button>
      ))}
    </div>
  );
}

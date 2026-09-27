/// The desktop transcript renderer (#1480, epic #1473): a transcript read
/// the way Claude Code prints it in a terminal.
///
/// One left-aligned column. `Message` / `MessageHeader` / `MessageFooter`
/// give the structure and there is no bubble -- bubbles are the phone's
/// (#1481). What each record kind looks like:
///
/// | kind | rendering |
/// |---|---|
/// | user prompt, queued prompt, shell input | a full-width band, left accent bar, `>` (or `!`) glyph, time right-aligned: the strongest separator |
/// | assistant text | `⏺` and the text through `TranscriptMarkdown` (#1482) |
/// | tool call | `ToolCall`, variant `terminal` (#1483): `⏺ Bash(…)` then `⎿` and a one-line result, folded |
/// | thinking | `ThinkingBlock`, shown dimmed, its label a toggle |
/// | everything else | a thin labelled divider, its text under it (folded when long) |
/// | a message being sent, not in the transcript yet (#1491) | the user band dashed and dimmed, its delivery state beneath (`TerminalPendingMessage`) |
///
/// Under each assistant turn a muted footer gives its tokens, duration
/// and model (`turnFooter.ts`). Messages and whole turns copy as markdown
/// (`transcriptCopy.ts`); code blocks copy from their own button in
/// `TranscriptMarkdown`.
///
/// Text the desktop masked for a phone renders through `MaskedText`
/// (#1488). The desktop's own reads carry no markers, so there this is
/// the markdown path; a marker, where one arrives, is drawn as a pill
/// rather than parsed as markdown.
///
/// # Why the environment is a prop, not a context
///
/// The viewer re-renders every mounted row when its data changes. Each
/// row is memoised on its message, its footer's figures and one stable
/// `env` object, so a poll that brings one new message re-renders one
/// row, not four hundred.

import { memo, type ReactNode } from "react";
import { toast } from "sonner";
import { Message, MessageContent, MessageFooter, MessageHeader } from "@/components/ui/message";
import { copyText } from "@/lib/clipboard";
import { splitMasked } from "@/lib/masked";
import { cn } from "@/lib/utils";
import type { Liveness } from "../../types/pr";
import type { TranscriptBlock, TranscriptMessage } from "../../types/transcript";
import { MaskedText } from "../MaskedText";
import { TranscriptMarkdown } from "../TranscriptMarkdown";
import { ClippedText } from "./ClippedText";
import { linesOf } from "./diff";
import { Fold } from "./Fold";
import { messageName, PENDING_NAME } from "./messageName";
import { MonoOutput } from "./output";
import { palette } from "./palette";
import { type PendingMessage, pendingStatus } from "./pending";
import { countLabel, durationBetween, formatDuration } from "./summary";
import { TaskStatusRow } from "./TaskStatusRow";
import { ThinkingBlock } from "./ThinkingBlock";
import { ToolCall } from "./ToolCall";
import { ToolResultOrphan } from "./ToolResultOrphan";
import { dividerLabel, messageMarkdown, turnMarkdown } from "./transcriptCopy";
import type { TaskListState } from "./tasks";
import type { TurnFooter } from "./turnFooter";
import type { LoadFullText, OpenSubagent } from "./types";

export type TranscriptDensity = "comfortable" | "compact";

/// What every row needs from outside. Keep it referentially stable: it
/// is part of each row's memo key.
export interface TerminalEnv {
  /// The session's liveness, passed down (#1209): only a live session's
  /// unanswered call is "running".
  liveness: Liveness;
  density: TranscriptDensity;
  onLoadFullText?: LoadFullText;
  onOpenSubagent?: OpenSubagent;
  /// Page back until the call a result answers is held (#1476, #1484):
  /// `toolUseId` names it, `null` when the result did not record one.
  /// Absent when nothing earlier exists.
  onLoadEarlier?: (toolUseId: string | null) => void;
  /// Every message the viewer holds, read when a turn is copied -- on
  /// click, never on render.
  messages: () => readonly TranscriptMessage[];
}

export function TerminalMessage(props: {
  message: TranscriptMessage;
  footer: TurnFooter | undefined;
  env: TerminalEnv;
  /// The session's task list (#1504), passed ONLY to a message holding a
  /// task call: it changes with every new message, and a row that does
  /// not draw it should not re-render for it.
  tasks?: TaskListState;
}) {
  return <MemoTerminalMessage {...props} />;
}

const MemoTerminalMessage = memo(
  function TerminalRow({
    message: m,
    footer,
    env,
    tasks,
  }: {
    message: TranscriptMessage;
    footer: TurnFooter | undefined;
    env: TerminalEnv;
    tasks?: TaskListState;
  }) {
    const compact = env.density === "compact";
    return (
      <Message
        role="article"
        aria-label={messageName(m)}
        data-kind={m.kind.kind}
        data-density={env.density}
        className={cn("flex-col", compact ? "gap-0.5 text-[13px]" : "gap-1.5 text-sm")}
      >
        {/* A `turn_duration` record hosting its turn's footer IS the
            footer; one that does not is still said, as a divider. */}
        {m.kind.kind === "turn_duration" && footer ? null : <Body m={m} env={env} tasks={tasks} />}
        {footer ? <Footer footer={footer} /> : null}
      </Message>
    );
  },
  (a, b) =>
    a.message === b.message &&
    a.env === b.env &&
    a.tasks === b.tasks &&
    footerEqual(a.footer, b.footer),
);

/// A message being sent that the transcript does not hold yet (#1491):
/// the user band, drawn provisionally -- a dashed bar, dimmed text, no
/// time (it has no record, so there is no recorded time to show) and
/// no copy buttons -- with what is known about its delivery beneath.
/// `pending.ts` has the states.
export function TerminalPendingMessage({
  pending: p,
  density,
}: {
  pending: PendingMessage;
  density: TranscriptDensity;
}) {
  const compact = density === "compact";
  const status = pendingStatus(p);
  return (
    <Message
      role="article"
      aria-label={PENDING_NAME}
      data-kind="pending"
      data-pending-state={p.state}
      data-density={density}
      className={cn("flex-col", compact ? "gap-0.5 text-[13px]" : "gap-1.5 text-sm")}
    >
      <MessageContent
        className={cn("gap-1 rounded-sm border-l-2 border-dashed", compact ? "px-2 py-1" : "px-3 py-2")}
        style={{ background: palette.userBand, borderColor: palette.accent }}
      >
        <MessageHeader className="gap-2 px-0 text-[11px]" style={{ color: palette.muted }}>
          <span>You</span>
        </MessageHeader>
        <div className="flex gap-2">
          <span aria-hidden className="shrink-0 font-mono font-bold" style={{ color: palette.accent }}>
            &gt;
          </span>
          <p className={cn("min-w-0 flex-1", WRAP)} style={{ color: palette.muted }}>
            {p.text}
          </p>
        </div>
        <p
          role="status"
          className="text-[11px]"
          style={{ color: status.tone === "muted" ? palette.muted : palette[status.tone] }}
        >
          {status.text}
        </p>
      </MessageContent>
    </Message>
  );
}

function footerEqual(a: TurnFooter | undefined, b: TurnFooter | undefined): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  return (
    a.outputTokens === b.outputTokens &&
    a.tokensPartial === b.tokensPartial &&
    a.durationMs === b.durationMs &&
    a.durationSource === b.durationSource &&
    a.qualifier === b.qualifier &&
    a.models.join("\n") === b.models.join("\n")
  );
}

/// Per-row props the blocks need beside the shared `env`.
interface RowProps {
  m: TranscriptMessage;
  env: TerminalEnv;
  tasks?: TaskListState;
}

function Body({ m, env, tasks }: RowProps) {
  const k = m.kind;
  switch (k.kind) {
    case "user_prompt":
      return m.is_meta ? <Divider m={m} env={env} /> : <UserBand m={m} env={env} glyph=">" />;
    case "queued_prompt":
      return <UserBand m={m} env={env} glyph=">" note="queued" />;
    case "shell_input":
      return <UserBand m={m} env={env} glyph="!" command={k.command} />;
    case "assistant":
    case "tool_results":
      return <Assistant m={m} env={env} tasks={tasks} />;
    case "task_status":
      return (
        <TaskStatusRow
          kind={k}
          description={firstText(m)}
          variant="terminal"
        />
      );
    default:
      return <Divider m={m} env={env} />;
  }
}

function firstText(m: TranscriptMessage): string | null {
  for (const b of m.blocks) if (b.kind === "text") return b.text;
  return null;
}

/// Wall-clock time of a recorded timestamp, or `null` when there is none
/// or it does not parse -- then no time is shown, never a made-up one.
function clock(ts: string | null): { short: string; full: string } | null {
  if (!ts) return null;
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return null;
  return {
    short: d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }),
    full: d.toLocaleString(),
  };
}

/// Long unbroken text wraps rather than widening the column: a 2,000
/// character token must not scroll the page sideways.
const WRAP = "whitespace-pre-wrap [overflow-wrap:anywhere]";

function UserBand({
  m,
  env,
  glyph,
  note,
  command,
}: {
  m: TranscriptMessage;
  env: TerminalEnv;
  glyph: string;
  note?: string;
  command?: string;
}) {
  const time = clock(m.timestamp);
  const compact = env.density === "compact";
  const opener = m.turn_id === m.id;
  const texts = m.blocks.filter((b) => b.kind === "text");
  const rest = m.blocks.filter((b) => b.kind !== "text");
  return (
    // Space above, as well as the band, bar and glyph: a new turn reads
    // as one in greyscale too (#1489).
    <MessageContent
      className={cn(
        "gap-1 rounded-sm border-l-2",
        compact ? "mt-1 px-2 py-1" : "mt-3 px-3 py-2",
      )}
      style={{ background: palette.userBand, borderColor: palette.accent }}
    >
      <MessageHeader className="gap-2 px-0 text-[11px]" style={{ color: palette.muted }}>
        <span>You{note ? ` (${note})` : ""}</span>
        {m.kind.kind === "user_prompt" && m.kind.origin && m.kind.origin !== "human" ? (
          <span>from {m.kind.origin.replace(/_/g, " ")}</span>
        ) : null}
        <span className="ml-auto flex items-center gap-2">
          <CopyMarkdown what="message" markdown={() => messageMarkdown(m)} />
          {opener ? (
            <CopyMarkdown what="turn" markdown={() => turnMarkdown(env.messages(), m.id)} />
          ) : null}
          {time ? (
            <time dateTime={m.timestamp ?? undefined} title={time.full}>
              {time.short}
            </time>
          ) : null}
        </span>
      </MessageHeader>
      <div className="flex gap-2" style={{ color: palette.text }}>
        <span aria-hidden className="shrink-0 font-mono font-bold" style={{ color: palette.accent }}>
          {glyph}
        </span>
        <div className="min-w-0 flex-1">
          {command !== undefined ? (
            <p className={cn("font-mono text-[12px]", WRAP)}>
              <MaskedText text={command} />
            </p>
          ) : null}
          {texts.map((b) =>
            b.kind === "text" ? (
              <ClippedText
                key={b.index}
                text={b.text}
                clip={b.clip}
                address={{ messageId: m.id, index: b.index, offset: m.offset }}
                onLoadFullText={env.onLoadFullText}
              >
                {(t) => (
                  <p className={WRAP}>
                    <MaskedText text={t} />
                  </p>
                )}
              </ClippedText>
            ) : null,
          )}
          {rest.map((b) => (
            <BlockView key={b.index} m={m} b={b} env={env} />
          ))}
          {m.blocks.length === 0 && command === undefined ? (
            <p style={{ color: palette.muted }}>(nothing in this message)</p>
          ) : null}
        </div>
      </div>
    </MessageContent>
  );
}

function Assistant({ m, env, tasks }: RowProps) {
  const compact = env.density === "compact";
  return (
    <MessageContent className={cn("relative pr-8", compact ? "gap-1" : "gap-2")}>
      <span className="absolute right-0 top-0">
        <CopyMarkdown what="message" markdown={() => messageMarkdown(m)} />
      </span>
      {m.is_sidechain ? (
        <MessageHeader className="px-0 text-[11px]" style={{ color: palette.muted }}>
          In a subagent
        </MessageHeader>
      ) : null}
      {m.blocks.length === 0 ? (
        <p className="text-[12px]" style={{ color: palette.muted }}>
          (nothing in this message)
        </p>
      ) : (
        m.blocks.map((b) => <BlockView key={b.index} m={m} b={b} env={env} tasks={tasks} />)
      )}
    </MessageContent>
  );
}

function BlockView({ m, b, env, tasks }: RowProps & { b: TranscriptBlock }) {
  switch (b.kind) {
    case "text":
      return (
        <div className="flex gap-2">
          <span
            aria-hidden
            className="shrink-0 select-none text-sm leading-relaxed"
            style={{ color: palette.text }}
          >
            ⏺
          </span>
          <div className="min-w-0 flex-1">
            <ClippedText
              text={b.text}
              clip={b.clip}
              address={{ messageId: m.id, index: b.index, offset: m.offset }}
              onLoadFullText={env.onLoadFullText}
            >
              {(t) => <Prose text={t} />}
            </ClippedText>
          </div>
        </div>
      );
    case "thinking":
      return (
        <ThinkingBlock
          block={b}
          messageId={m.id}
          offset={m.offset}
          variant="terminal"
          onLoadFullText={env.onLoadFullText}
          collapsible
        />
      );
    case "tool_call":
      return (
        <ToolCall
          call={b}
          variant="terminal"
          liveness={env.liveness}
          // From the call's record to its result's (#1483's wiring).
          durationMs={durationBetween(m.timestamp, b.result?.timestamp ?? null)}
          onLoadFullText={env.onLoadFullText}
          onOpenSubagent={env.onOpenSubagent}
          tasks={tasks}
        />
      );
    case "tool_result":
      return (
        <ToolResultOrphan
          block={b}
          variant="terminal"
          onLoadFullText={env.onLoadFullText}
          onOpenSubagent={env.onOpenSubagent}
          onLoadEarlier={env.onLoadEarlier}
        />
      );
    case "image":
      return (
        <p className="font-mono text-[12px]" style={{ color: palette.muted }}>
          [image
          {b.image.media_type ? `, ${b.image.media_type}` : ""}
          {b.image.width !== null && b.image.height !== null
            ? `, ${b.image.width}×${b.image.height}`
            : ""}
          {b.image.approx_bytes !== null
            ? `, about ${Math.max(1, Math.round(b.image.approx_bytes / 1024)).toLocaleString()} KB`
            : ""}
          : not shown]
        </p>
      );
    case "other":
      return (
        <p className="font-mono text-[12px]" style={{ color: palette.muted }}>
          [a {b.block_type} block, not shown]
        </p>
      );
  }
}

/// Markdown, unless the text carries a masking marker: a marker parsed
/// as markdown would print its brackets, so masked text is drawn plain
/// with its pills.
function Prose({ text }: { text: string }) {
  const parts = splitMasked(text);
  const masked = parts.length > 1 || !("text" in parts[0]);
  return masked ? (
    <p className={WRAP} style={{ color: palette.text }}>
      <MaskedText text={text} />
    </p>
  ) : (
    // The first block's top margin would drop the text below its bullet.
    <div className="min-w-0 [overflow-wrap:anywhere] [&>div>:first-child]:mt-0">
      <TranscriptMarkdown>{text}</TranscriptMarkdown>
    </div>
  );
}

/// A record that is not the conversation: a thin rule with its label, and
/// its text beneath. Short text sits on the rule; longer text is folded,
/// with its line count, so a compaction summary does not bury the turn.
function Divider({ m, env }: { m: TranscriptMessage; env: TerminalEnv }) {
  const label = dividerLabel(m);
  const error = m.kind.kind === "api_error" || (m.kind.kind === "hook_output" && m.kind.outcome === "error");
  const texts = m.blocks.filter((b) => b.kind === "text");
  const others = m.blocks.filter((b) => b.kind !== "text");
  const joined = texts.map((b) => (b.kind === "text" ? b.text : "")).join("\n");
  const short = texts.length === 1 && !joined.includes("\n") && joined.length <= 160 && texts[0].kind === "text" && texts[0].clip === null;
  const mono = m.kind.kind === "command_output" || m.kind.kind === "hook_output";
  return (
    <div
      className={cn("w-full min-w-0 text-[11px]", env.density === "compact" ? "py-0" : "py-0.5")}
      style={{ color: palette.muted }}
    >
      <div className="flex items-center gap-2" role="note" aria-label={label}>
        <span aria-hidden className="h-px min-w-4 flex-1" style={{ background: palette.border }} />
        <span className="max-w-[80%] text-center [overflow-wrap:anywhere]" style={error ? { color: palette.error } : undefined}>
          {label}
          {short && joined !== "" ? (
            <span style={{ color: palette.text }}>
              {": "}
              <MaskedText text={joined} />
            </span>
          ) : null}
        </span>
        <span aria-hidden className="h-px min-w-4 flex-1" style={{ background: palette.border }} />
      </div>
      {!short && texts.length > 0 ? (
        <div className="mx-auto max-w-full pl-4">
          {texts.map((b) =>
            b.kind === "text" ? (
              <Fold
                key={b.index}
                label={label}
                count={countLabel(linesOf(b.text).length, "line", b.clip !== null)}
                variant="terminal"
                title={label}
              >
                <ClippedText
                  text={b.text}
                  clip={b.clip}
                  address={{ messageId: m.id, index: b.index, offset: m.offset }}
                  onLoadFullText={env.onLoadFullText}
                >
                  {(t) => (mono ? <MonoOutput text={t} error={error} /> : <Prose text={t} />)}
                </ClippedText>
              </Fold>
            ) : null,
          )}
        </div>
      ) : null}
      {others.length > 0 ? (
        <div className="pl-4">
          {others.map((b) => (
            <BlockView key={b.index} m={m} b={b} env={env} />
          ))}
        </div>
      ) : null}
    </div>
  );
}

function Footer({ footer: f }: { footer: TurnFooter }) {
  const parts: ReactNode[] = [];
  if (f.durationMs !== null) {
    parts.push(
      <span
        key="d"
        title={
          f.durationSource === "recorded"
            ? "As Claude Code recorded the turn's duration"
            : "From the turn's first and last recorded times"
        }
      >
        {formatDuration(f.durationMs)}
      </span>,
    );
  }
  if (f.outputTokens !== null) {
    const n = f.outputTokens.toLocaleString();
    parts.push(
      <span key="t">
        {f.tokensPartial ? `at least ${n}` : n} output token{f.outputTokens === 1 ? "" : "s"}
      </span>,
    );
  }
  if (f.models.length > 0) parts.push(<span key="m">{f.models.join(", ")}</span>);
  if (f.qualifier === "in_progress") parts.push(<span key="q">so far</span>);
  if (f.qualifier === "began_above") {
    parts.push(<span key="q">this turn began before what was read</span>);
  }
  if (parts.length === 0) return null;
  return (
    <MessageFooter
      className="flex-wrap gap-x-2 px-0 text-[11px] font-normal"
      style={{ color: palette.muted }}
      data-testid="turn-footer"
    >
      {parts.flatMap((p, i) => (i === 0 ? [p] : [<span key={`s${i}`} aria-hidden>·</span>, p]))}
    </MessageFooter>
  );
}

/// Copy as markdown, the 7.6 way (#1399): built on click, `copyText`
/// names a failure, and a toast makes the click visible either way.
/// Quiet until hovered or focused, and always in the tab order.
function CopyMarkdown({ what, markdown }: { what: "message" | "turn"; markdown: () => string }) {
  return (
    <button
      type="button"
      aria-label={`Copy ${what} as markdown`}
      title={`Copy ${what} as markdown`}
      onClick={() => {
        void copyText(markdown()).then((failure) => {
          if (failure !== null) {
            toast.error(`Could not copy the ${what}`, { description: failure });
            return;
          }
          toast.success(`Copied the ${what} as markdown`);
        });
      }}
      // Muted until pointed at or focused, never faded: link blue at 60%
      // opacity measured 3.2:1 on the user band (#1489).
      className="text-[11px] text-[#8b949e] hover:text-[#58a6ff] hover:underline focus-visible:text-[#58a6ff] focus-visible:outline focus-visible:outline-2"
    >
      {what === "turn" ? "Copy turn" : "Copy"}
    </button>
  );
}

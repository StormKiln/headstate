import { useId, useState, type ReactNode } from "react";
import { Bubble, BubbleContent } from "@/components/ui/bubble";
import { Message, MessageContent } from "@/components/ui/message";
import type { TranscriptImage, TranscriptMessage } from "../../../types/transcript";
import { MaskedText } from "../../MaskedText";
import { TranscriptMarkdown } from "../../TranscriptMarkdown";
import { ClippedText } from "../ClippedText";
import { messageName, PENDING_NAME } from "../messageName";
import { MonoOutput } from "../output";
import { palette } from "../palette";
import { type PendingMessage, pendingStatus } from "../pending";
import { linesOf } from "../diff";
import { countLabel, durationBetween, formatDuration } from "../summary";
import { TaskStatusRow } from "../TaskStatusRow";
import { ToolResultOrphan } from "../ToolResultOrphan";
import type { ThinkingBlock } from "../types";
import { usePhone } from "./context";
import { scaleStyle } from "./textScale";
import { thoughtLabel } from "./timing";
import { ToolChip } from "./ToolChip";

/// One message as the phone draws it (#1481): a transcript as the Claude iOS
/// app draws a conversation.
///
/// | record | drawn as |
/// |---|---|
/// | your prompt, slash command, shell input, queued prompt | a right-aligned bubble (`Message align="end"` + `Bubble`) |
/// | Claude's reply | full-width prose, no bubble (`TranscriptMarkdown`) |
/// | a tool call | a chip; tapping it opens the whole call in a sheet (`ToolChip`) |
/// | thinking | "Thought for about 14 s", collapsible, open by default |
/// | system, hook and meta records | a centred small-caps divider, its detail beneath |
/// | a message being sent, not in the transcript yet (#1491) | the right-aligned bubble dimmed and dashed, its delivery state beneath (`PhonePendingMessage`) |
///
/// Every text is drawn through `MaskedText` or `TranscriptMarkdown`,
/// both of which turn the desktop's masking markers into "hidden" pills.
/// Every kind the read model has renders as SOMETHING -- an unrecognised
/// record names its type -- never as nothing.
export function PhoneMessage({ message: m }: { message: TranscriptMessage }) {
  const phone = usePhone();
  return (
    // Dynamic Type, applied to this row's content (see `textScale.ts`).
    <div
      role="article"
      aria-label={messageName(m)}
      data-slot="phone-message"
      data-kind={m.kind.kind}
      style={scaleStyle(phone.scale)}
    >
      <Body m={m} />
    </div>
  );
}

/// The message's text blocks, in order.
function textsOf(m: TranscriptMessage) {
  return m.blocks.flatMap((b) => (b.kind === "text" ? [b] : []));
}

/// The message's text as a divider's detail, or nothing when it has
/// none -- so no "Show" is offered over an empty region.
function plainBody(m: TranscriptMessage): ReactNode {
  return textsOf(m).length > 0 ? <PlainTexts m={m} /> : undefined;
}

function firstText(m: TranscriptMessage): string | null {
  return textsOf(m)[0]?.text ?? null;
}

function Body({ m }: { m: TranscriptMessage }) {
  const k = m.kind;
  if (m.is_meta && (k.kind === "user_prompt" || k.kind === "assistant")) {
    // Harness-written context in a user or assistant record: not the
    // user speaking, and not Claude's reply.
    return <SystemRow label="Added context" body={plainBody(m)} collapsed={linesIn(m)} />;
  }
  switch (k.kind) {
    case "user_prompt":
      return (
        <UserBubble>
          <PlainTexts m={m} />
          <OtherBlocks m={m} />
        </UserBubble>
      );
    case "slash_command":
      return (
        <UserBubble>
          <p className="font-mono">
            <MaskedText text={k.name.startsWith("/") ? k.name : `/${k.name}`} />
          </p>
          <PlainTexts m={m} />
        </UserBubble>
      );
    case "shell_input":
      return (
        <UserBubble>
          <p className="font-mono whitespace-pre-wrap break-words">
            <span aria-hidden style={{ color: palette.muted }}>
              !{" "}
            </span>
            <MaskedText text={k.command} />
          </p>
        </UserBubble>
      );
    case "queued_prompt":
      return (
        <UserBubble label="Queued">
          <PlainTexts m={m} />
        </UserBubble>
      );
    case "command_output":
      return (
        <div>
          <Label>{k.command ? `Output of ${k.command}` : "Command output"}</Label>
          {textsOf(m).map((t) => (
            <ClippedTextFor key={t.index} m={m} index={t.index} text={t.text} clip={t.clip}>
              {(s) => <MonoOutput text={s} />}
            </ClippedTextFor>
          ))}
        </div>
      );
    case "assistant":
      return <AssistantBody m={m} />;
    case "tool_results":
      return <ToolResults m={m} />;
    case "agent_notification":
      return (
        <SystemRow
          label={`Background agent${k.status ? ` ${k.status.replace(/_/g, " ")}` : ""}`}
          body={plainBody(m)}
          collapsed={linesIn(m)}
        />
      );
    case "task_status":
      return (
        <div className="text-center">
          <TaskStatusRow kind={k} description={firstText(m)} variant="compact" />
        </div>
      );
    case "injected":
      return <SystemRow label="Added context" body={plainBody(m)} collapsed={linesIn(m)} />;
    case "interruption":
      return (
        <SystemRow
          label={k.during_tool_use ? "Interrupted during a tool call" : "Interrupted"}
          tone="warn"
        />
      );
    case "compaction_boundary":
      return <SystemRow label="Conversation compacted" detail={compactionDetail(k)} />;
    case "compaction_summary":
      return (
        <SystemRow
          label="Summary of the earlier conversation"
          body={textsOf(m).length > 0 ? <MarkdownTexts m={m} /> : undefined}
          collapsed={linesIn(m)}
        />
      );
    case "summary":
      return <SystemRow label="Summary" body={plainBody(m)} />;
    case "api_error":
      return (
        <SystemRow
          label={apiErrorLabel(k)}
          detail={apiErrorDetail(k)}
          tone="error"
          body={plainBody(m)}
          collapsed={linesIn(m)}
        />
      );
    case "hook_output":
      return (
        <SystemRow
          label={`Hook${k.event ? ` ${k.event}` : ""}${k.name ? ` · ${k.name}` : ""}`}
          detail={hookDetail(k)}
          tone={k.outcome === "success" ? "muted" : "warn"}
          body={plainBody(m)}
          collapsed={linesIn(m)}
        />
      );
    case "turn_duration":
      return (
        <SystemRow
          label={
            m.duration_ms !== null ? `Turn took ${formatDuration(m.duration_ms)}` : "Turn ended"
          }
        />
      );
    case "notice":
      return (
        <SystemRow
          label={k.subtype.replace(/_/g, " ")}
          detail={k.level}
          tone={k.level === "error" ? "error" : k.level === "warning" ? "warn" : "muted"}
          body={plainBody(m)}
        />
      );
    case "model_change":
      return <SystemRow label="Model changed" detail={`${k.from} → ${k.to}`} />;
    case "permission_mode_change":
      return <SystemRow label="Permission mode" detail={k.mode} />;
    case "unrecognised":
      return <SystemRow label={k.record_type} detail="not a record this version shows" />;
  }
}

type Kind<K extends TranscriptMessage["kind"]["kind"]> = Extract<
  TranscriptMessage["kind"],
  { kind: K }
>;

function compactionDetail(k: Kind<"compaction_boundary">): string | null {
  const parts: string[] = [];
  if (k.trigger) parts.push(k.trigger);
  if (k.pre_tokens !== null && k.post_tokens !== null) {
    parts.push(`${k.pre_tokens.toLocaleString()} → ${k.post_tokens.toLocaleString()} tokens`);
  } else if (k.pre_tokens !== null) {
    parts.push(`from ${k.pre_tokens.toLocaleString()} tokens`);
  }
  return parts.length > 0 ? parts.join(" · ") : null;
}

function apiErrorLabel(k: Kind<"api_error">): string {
  return `API error${k.status !== null ? ` ${k.status}` : ""}`;
}

function apiErrorDetail(k: Kind<"api_error">): string | null {
  const parts: string[] = [];
  if (k.error_type) parts.push(k.error_type.replace(/_/g, " "));
  if (k.retry_attempt !== null) {
    parts.push(
      `retry ${k.retry_attempt}${k.max_retries !== null ? ` of ${k.max_retries}` : ""}${
        k.retry_in_ms !== null ? ` in ${formatDuration(k.retry_in_ms)}` : ""
      }`,
    );
  }
  return parts.length > 0 ? parts.join(" · ") : null;
}

function hookDetail(k: Kind<"hook_output">): string {
  const parts = [k.outcome.replace(/_/g, " ")];
  if (k.exit_code !== null) parts.push(`exit ${k.exit_code}`);
  if (k.prevented_continuation === true) parts.push("stopped the turn");
  return parts.join(" · ");
}

/// A right-aligned bubble: something the user said or typed.
function UserBubble({ label, children }: { label?: string; children: ReactNode }) {
  return (
    <Message align="end">
      <MessageContent>
        <Bubble variant="muted" align="end">
          {label ? (
            <span className="px-1 text-[0.75em]" style={{ color: palette.muted }}>
              {label}
            </span>
          ) : null}
          <BubbleContent className="rounded-2xl text-[1em] leading-relaxed">
            {children}
          </BubbleContent>
        </Bubble>
      </MessageContent>
    </Message>
  );
}

/// A message being sent that the transcript does not hold yet (#1491):
/// the user's bubble, drawn provisionally -- dimmed -- with what is
/// known about its delivery beneath it, right-aligned under the bubble
/// as the iOS apps do. `pending.ts` has the states.
export function PhonePendingMessage({ pending: p }: { pending: PendingMessage }) {
  const phone = usePhone();
  const status = pendingStatus(p);
  return (
    <div
      role="article"
      aria-label={PENDING_NAME}
      data-slot="phone-message"
      data-kind="pending"
      data-pending-state={p.state}
      style={scaleStyle(phone.scale)}
    >
      <Message align="end">
        <MessageContent className="gap-1">
          {/* Dimmed with a muted colour, not opacity: a faded row's
              contrast is whatever the blend happens to give (#1489). */}
          <Bubble variant="muted" align="end">
            <BubbleContent
              className="rounded-2xl border border-dashed text-[1em] leading-relaxed"
              style={{ color: palette.muted, borderColor: palette.muted }}
            >
              <p className="whitespace-pre-wrap break-words">{p.text}</p>
            </BubbleContent>
          </Bubble>
          <p
            role="status"
            className="max-w-[85%] self-end px-1 text-right text-[0.75em]"
            style={{ color: TONE_COLOUR[status.tone] }}
          >
            {status.text}
          </p>
        </MessageContent>
      </Message>
    </div>
  );
}

/// The full-text fetch, addressed for this message.
function ClippedTextFor({
  m,
  index,
  text,
  clip,
  children,
}: {
  m: TranscriptMessage;
  index: number;
  text: string;
  clip: Parameters<typeof ClippedText>[0]["clip"];
  children: (text: string) => ReactNode;
}) {
  const phone = usePhone();
  return (
    <ClippedText
      text={text}
      clip={clip}
      address={{ messageId: m.id, index, offset: m.offset }}
      onLoadFullText={phone.onLoadFullText}
    >
      {children}
    </ClippedText>
  );
}

/// Text blocks as plain, wrapped text: what the user typed is shown as
/// typed, not reinterpreted as markdown.
function PlainTexts({ m }: { m: TranscriptMessage }) {
  const texts = textsOf(m);
  if (texts.length === 0) return null;
  return (
    <>
      {texts.map((t) => (
        <ClippedTextFor key={t.index} m={m} index={t.index} text={t.text} clip={t.clip}>
          {(s) => (
            <p className="whitespace-pre-wrap break-words">
              <MaskedText text={s} />
            </p>
          )}
        </ClippedTextFor>
      ))}
    </>
  );
}

function MarkdownTexts({ m }: { m: TranscriptMessage }) {
  return (
    <>
      {textsOf(m).map((t) => (
        <ClippedTextFor key={t.index} m={m} index={t.index} text={t.text} clip={t.clip}>
          {(s) => <TranscriptMarkdown>{s}</TranscriptMarkdown>}
        </ClippedTextFor>
      ))}
    </>
  );
}

/// Images and blocks this build does not draw, said rather than dropped.
function OtherBlocks({ m }: { m: TranscriptMessage }) {
  return (
    <>
      {m.blocks.map((b) =>
        b.kind === "image" ? (
          <ImageNote key={b.index} image={b.image} />
        ) : b.kind === "other" ? (
          <p key={b.index} className="text-[0.8125em]" style={{ color: palette.muted }}>
            A {b.block_type.replace(/_/g, " ")} block is not shown here.
          </p>
        ) : null,
      )}
    </>
  );
}

function ImageNote({ image }: { image: TranscriptImage }) {
  return (
    <p className="text-[0.8125em]" style={{ color: palette.muted }}>
      [image{image.media_type ? `, ${image.media_type}` : ""}
      {image.width !== null && image.height !== null ? `, ${image.width}×${image.height}` : ""}]
    </p>
  );
}

/// Claude's reply: prose full width, thinking and tool calls in the
/// order they were written.
function AssistantBody({ m }: { m: TranscriptMessage }) {
  if (m.blocks.length === 0) {
    // An empty reply is a fact about the record (the read model keeps
    // it), so it is said rather than drawn as a gap.
    return (
      <p className="text-[0.8125em] italic" style={{ color: palette.muted }}>
        (nothing in this reply)
      </p>
    );
  }
  return (
    <div className="flex min-w-0 flex-col gap-2 text-[0.9375em]" style={{ color: palette.text }}>
      {m.blocks.map((b) => {
        switch (b.kind) {
          case "text":
            return (
              <ClippedTextFor key={b.index} m={m} index={b.index} text={b.text} clip={b.clip}>
                {(s) => <TranscriptMarkdown>{s}</TranscriptMarkdown>}
              </ClippedTextFor>
            );
          case "thinking":
            return <Thought key={b.index} m={m} block={b} />;
          case "tool_call":
            return <ToolChip key={b.index} call={b} callAt={m.timestamp} />;
          case "tool_result":
            return <ToolResultOrphanFor key={`r${b.index}`} block={b} />;
          case "image":
            return <ImageNote key={b.index} image={b.image} />;
          case "other":
            return (
              <p key={b.index} className="text-[0.8125em]" style={{ color: palette.muted }}>
                A {b.block_type.replace(/_/g, " ")} block is not shown here.
              </p>
            );
        }
      })}
    </div>
  );
}

function ToolResultOrphanFor({
  block,
}: {
  block: Extract<TranscriptMessage["blocks"][number], { kind: "tool_result" }>;
}) {
  const phone = usePhone();
  return (
    <ToolResultOrphan
      block={block}
      variant="compact"
      onLoadFullText={phone.onLoadFullText}
      onOpenSubagent={phone.onOpenSubagent}
      onLoadEarlier={phone.onLoadEarlier}
    />
  );
}

/// Results whose calls are in an earlier part of the transcript.
function ToolResults({ m }: { m: TranscriptMessage }) {
  return (
    <div className="flex min-w-0 flex-col gap-2">
      {m.blocks.map((b) =>
        b.kind === "tool_result" ? (
          <ToolResultOrphanFor key={b.index} block={b} />
        ) : b.kind === "text" ? (
          <ClippedTextFor key={b.index} m={m} index={b.index} text={b.text} clip={b.clip}>
            {(s) => <MonoOutput text={s} />}
          </ClippedTextFor>
        ) : null,
      )}
    </div>
  );
}

/// "Thought for about 14 s", open by default (the epic's decided
/// default), collapsible to one line.
function Thought({ m, block }: { m: TranscriptMessage; block: ThinkingBlock }) {
  const phone = usePhone();
  const [open, setOpen] = useState(true);
  const regionId = useId();
  if (!block.recorded) {
    // The model thought; the transcript kept only the signature.
    return (
      <p className="text-[0.8125em] italic" style={{ color: palette.muted }}>
        <span aria-hidden>✻ </span>Thinking not recorded
      </p>
    );
  }
  const label = thoughtLabel(durationBetween(phone.thinkingStarts.get(m.id) ?? null, m.timestamp));
  return (
    <div className="min-w-0">
      <button
        type="button"
        aria-expanded={open}
        aria-controls={regionId}
        // How much it hides, as the desktop's fold says it (#1489).
        aria-label={`${label}, ${countLabel(linesOf(block.text).length, "line", block.clip !== null)}`}
        onClick={() => setOpen((o) => !o)}
        className="flex min-h-8 items-center gap-1.5 text-left text-[0.8125em] focus-visible:outline focus-visible:outline-2"
        style={{ color: palette.muted }}
      >
        <span aria-hidden>✻</span>
        <span>{label}</span>
        <span aria-hidden>{open ? "▾" : "▸"}</span>
      </button>
      <div
        id={regionId}
        hidden={!open}
        className="border-l-2 pl-3 text-[0.8125em]"
        style={{ borderColor: palette.border }}
      >
        {open ? (
          <ClippedTextFor m={m} index={block.index} text={block.text} clip={block.clip}>
            {(s) => (
              <p
                className="whitespace-pre-wrap break-words italic"
                style={{ color: palette.muted }}
              >
                <MaskedText text={s} />
              </p>
            )}
          </ClippedTextFor>
        ) : null}
      </div>
    </div>
  );
}

function Label({ children }: { children: ReactNode }) {
  return (
    <p className="mb-0.5 text-[0.75em]" style={{ color: palette.muted }}>
      {children}
    </p>
  );
}

const TONE_COLOUR = {
  muted: palette.muted,
  warn: palette.warn,
  error: palette.error,
} as const;

/// How much a collapsed row hides, for its toggle's name: "3 lines",
/// "at least 40 lines" when a block was clipped.
function linesIn(m: TranscriptMessage): string {
  const texts = textsOf(m);
  const n = texts.reduce((sum, t) => sum + linesOf(t.text).length, 0);
  return countLabel(n, "line", texts.some((t) => t.clip !== null));
}

/// A centred small-caps divider for a system, hook or meta record, with
/// its detail beneath -- behind "Show" when `collapsed`, because hook
/// output and injected context can run to pages. `collapsed` is how much
/// is behind it ("12 lines"), which the toggle's name says (#1489).
function SystemRow({
  label,
  detail = null,
  body,
  tone = "muted",
  collapsed,
}: {
  label: string;
  detail?: string | null;
  body?: ReactNode;
  tone?: keyof typeof TONE_COLOUR;
  collapsed?: string;
}) {
  const [open, setOpen] = useState(collapsed === undefined);
  const regionId = useId();
  return (
    <div className="flex min-w-0 flex-col items-center gap-0.5 text-center">
      <p
        role="note"
        className="flex w-full items-center gap-2 text-[0.8125em] font-medium tracking-wide [font-variant-caps:all-small-caps] before:h-px before:flex-1 before:bg-[#30363d] before:content-[''] after:h-px after:flex-1 after:bg-[#30363d] after:content-['']"
        style={{ color: TONE_COLOUR[tone] }}
      >
        <span className="max-w-[80%] break-words">{label}</span>
      </p>
      {detail ? (
        <p className="max-w-full text-[0.75em] break-words" style={{ color: palette.muted }}>
          {detail}
        </p>
      ) : null}
      {body ? (
        collapsed !== undefined ? (
          <>
            <button
              type="button"
              aria-expanded={open}
              aria-controls={regionId}
              aria-label={`${open ? "Hide" : "Show"} ${label}, ${collapsed}`}
              onClick={() => setOpen((o) => !o)}
              className="min-h-8 text-[0.75em] underline-offset-2 hover:underline focus-visible:outline focus-visible:outline-2"
              style={{ color: palette.link }}
            >
              {open ? "Hide" : "Show"}
            </button>
            <div
              id={regionId}
              hidden={!open}
              className="w-full text-left text-[0.8125em]"
              style={{ color: palette.muted }}
            >
              {open ? body : null}
            </div>
          </>
        ) : (
          <div className="w-full text-[0.8125em]" style={{ color: palette.muted }}>
            {body}
          </div>
        )
      ) : null}
    </div>
  );
}

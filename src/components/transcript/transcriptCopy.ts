/// A transcript message, or a whole turn, as markdown for the clipboard
/// (#1480). The copy path is 7.6's (#1399): built on click, handed to
/// `copyText`, and a toast either way -- see `CopyMarkdown` in
/// `TerminalMessage.tsx`.
///
/// The paste is for reading elsewhere -- an issue, a chat, another
/// session -- so it says what the screen says, in the same words where
/// the screen has words: a clipped block says it was clipped and by how
/// much, a call with no result says it has none, and a record the model
/// does not recognise is named rather than dropped.

import type { ClaudeToolArgs } from "../../types/pr";
import type { TranscriptBlock, TranscriptMessage } from "../../types/transcript";

/// A fenced block whose fence is longer than any backtick run inside
/// it, the rule `adviceMarkdown.ts`'s `code` applies to spans.
export function fenced(text: string, lang = ""): string {
  const longest = Math.max(0, ...(text.match(/`+/g) ?? []).map((r) => r.length));
  const fence = "`".repeat(Math.max(3, longest + 1));
  return `${fence}${lang}\n${text}\n${fence}`;
}

/// A code span, with the same rule.
function span(text: string): string {
  const one = text.replace(/\s*\n\s*/g, " ");
  const longest = Math.max(0, ...(one.match(/`+/g) ?? []).map((r) => r.length));
  const fence = "`".repeat(longest + 1);
  const pad = one.startsWith("`") || one.endsWith("`") ? " " : "";
  return `${fence}${pad}${one}${pad}${fence}`;
}

function quoted(text: string): string {
  return text
    .split("\n")
    .map((l) => (l === "" ? ">" : `> ${l}`))
    .join("\n");
}

function clipNote(clip: { shown_chars: number; total_chars: number } | null): string | null {
  return clip
    ? `_(showing the first ${clip.shown_chars.toLocaleString()} of ${clip.total_chars.toLocaleString()} characters)_`
    : null;
}

/// What a call was asked to do, in one line.
function callLine(name: string, args: ClaudeToolArgs): string {
  switch (args.tool) {
    case "bash":
      return `${name}(${span(args.command)})`;
    case "read":
    case "write":
    case "edit":
    case "multi_edit":
      return `${name}(${span(args.file_path)})`;
    case "grep":
    case "glob":
      return `${name}(${span(args.pattern)}${args.path ? ` in ${span(args.path)}` : ""})`;
    case "task":
      return `${name}(${args.description ?? "subagent"}${args.subagent_type ? `, ${args.subagent_type}` : ""})`;
    case "todo_write":
      return `${name}(${args.todos.length + args.todos_omitted} items)`;
    case "web_fetch":
      return `${name}(${args.url})`;
    case "web_search":
      return `${name}(${span(args.query)})`;
    case "task_create":
      return `${name}(${args.subject})`;
    case "task_update":
      return `${name}(${[
        args.task_id !== null ? `#${args.task_id}` : null,
        args.status !== null ? `→ ${args.status.replace(/_/g, " ")}` : null,
        args.subject,
      ]
        .filter(Boolean)
        .join(" ")})`;
    case "task_get":
      return args.task_id !== null ? `${name}(#${args.task_id})` : name;
    case "task_list":
      return name;
    case "other":
      return args.keys.length > 0 ? `${name}(${args.keys.join(", ")})` : name;
    case "none":
      return name;
  }
}

function blockMarkdown(b: TranscriptBlock): string[] {
  switch (b.kind) {
    case "text":
      return [b.text, ...(clipNote(b.clip) ? [clipNote(b.clip)!] : [])];
    case "thinking":
      return b.recorded
        ? [quoted(`_Thinking:_ ${b.text}`), ...(clipNote(b.clip) ? [clipNote(b.clip)!] : [])]
        : ["> _Thinking (not recorded)_"];
    case "tool_call": {
      const out = [`**⏺ ${callLine(b.name, b.args)}**`];
      if (b.result === null) {
        out.push("_No result in what was read._");
      } else {
        if (b.result.is_error === true) out.push("_The tool reported an error._");
        if (b.result.text !== "") out.push(fenced(b.result.text));
        else out.push("_(no output)_");
        const note = clipNote(b.result.clip);
        if (note) out.push(note);
      }
      return out;
    }
    case "tool_result": {
      const out = ["_Result of a call in an earlier part of the transcript:_"];
      if (b.is_error === true) out.push("_The tool reported an error._");
      out.push(b.text !== "" ? fenced(b.text) : "_(no output)_");
      const note = clipNote(b.clip);
      if (note) out.push(note);
      return out;
    }
    case "image":
      return [`_[image${b.image.media_type ? `, ${b.image.media_type}` : ""}, not copied]_`];
    case "other":
      return [`_[${b.block_type} block, not shown]_`];
  }
}

/// The label a record that is not the conversation gets, shared with the
/// renderer's dividers so the paste and the screen say the same thing.
export function dividerLabel(m: TranscriptMessage): string {
  const k = m.kind;
  switch (k.kind) {
    case "slash_command":
      return k.name.startsWith("/") ? k.name : `/${k.name}`;
    case "interruption":
      return k.during_tool_use ? "Interrupted by user during a tool call" : "Interrupted by user";
    case "compaction_boundary": {
      const how = k.trigger ? ` (${k.trigger})` : "";
      const sizes =
        k.pre_tokens !== null && k.post_tokens !== null
          ? `: ${k.pre_tokens.toLocaleString()} → ${k.post_tokens.toLocaleString()} tokens`
          : k.pre_tokens !== null
            ? `: from ${k.pre_tokens.toLocaleString()} tokens`
            : "";
      return `Conversation compacted${how}${sizes}`;
    }
    case "compaction_summary":
      return "Summary of the compacted conversation";
    case "summary":
      return "Session summary";
    case "model_change":
      return `Model changed to ${k.to} (from ${k.from})`;
    case "permission_mode_change":
      return `Permission mode: ${k.mode}`;
    case "hook_output": {
      const parts = [
        k.event ? `${k.event} hook` : "Hook",
        k.name,
        k.outcome.replace(/_/g, " "),
        k.exit_code !== null ? `exit ${k.exit_code}` : null,
        k.prevented_continuation === true ? "stopped the turn" : null,
      ].filter(Boolean);
      return parts.join(" · ");
    }
    case "api_error": {
      const parts = [
        `API error${k.status !== null ? ` ${k.status}` : ""}${k.error_type ? ` (${k.error_type})` : ""}`,
        k.retry_attempt !== null
          ? `retry ${k.retry_attempt}${k.max_retries !== null ? ` of ${k.max_retries}` : ""}`
          : null,
        k.retry_in_ms !== null ? `in ${(k.retry_in_ms / 1000).toFixed(1)} s` : null,
      ].filter(Boolean);
      return parts.join(" · ");
    }
    case "notice":
      return `${k.subtype.replace(/_/g, " ")}${k.level ? ` (${k.level})` : ""}`;
    case "agent_notification":
      return `Background task notification${k.status ? `: ${k.status.replace(/_/g, " ")}` : ""}`;
    case "task_status":
      return `Background task${k.status ? `: ${k.status.replace(/_/g, " ")}` : ": status not recorded"}`;
    case "injected":
      return `Added to the conversation${k.origin ? ` by ${k.origin.replace(/_/g, " ")}` : ""}`;
    case "command_output":
      return k.command ? `Output of ${k.command}` : "Command output";
    case "turn_duration":
      return "Turn complete";
    case "unrecognised":
      return `Unrecognised record: ${k.record_type}`;
    case "user_prompt":
      return m.is_meta ? "Added by Claude Code" : "You";
    case "queued_prompt":
      return "You (queued)";
    case "shell_input":
      return "Shell command";
    case "assistant":
      return "Claude";
    case "tool_results":
      return "Tool results";
  }
}

/// One message as markdown.
export function messageMarkdown(m: TranscriptMessage): string {
  const k = m.kind;
  const body = m.blocks.flatMap(blockMarkdown);
  if ((k.kind === "user_prompt" && !m.is_meta) || k.kind === "queued_prompt") {
    const head = k.kind === "queued_prompt" ? "**You** _(queued)_" : "**You**";
    const text = m.blocks.flatMap((b) => (b.kind === "text" ? [b.text] : []));
    const rest = m.blocks.filter((b) => b.kind !== "text").flatMap(blockMarkdown);
    return [head, quoted(text.join("\n\n")), ...rest].join("\n\n");
  }
  if (k.kind === "shell_input") return ["**You**", fenced(`! ${k.command}`, "sh")].join("\n\n");
  if (k.kind === "assistant" || k.kind === "tool_results") {
    return body.length > 0 ? body.join("\n\n") : "_(nothing in this message)_";
  }
  return [`_— ${dividerLabel(m)} —_`, ...body].join("\n\n");
}

/// A whole turn: every message holding `turnId`, in order, from the
/// messages given. Says so when the turn began above what was read.
export function turnMarkdown(messages: readonly TranscriptMessage[], turnId: string | null): string {
  const inTurn = messages.filter((m) => m.turn_id === turnId);
  const out = inTurn.map(messageMarkdown);
  if (turnId === null) out.unshift("_This turn began before the part of the transcript that was read._");
  return `${out.join("\n\n")}\n`;
}

/// Several messages as one markdown document (#1484): the session as
/// loaded, or a range of turns. Says so when it is not the whole
/// session, at the end where the gap is.
export function messagesMarkdown(
  messages: readonly TranscriptMessage[],
  { earlierUnloaded, laterUnloaded }: { earlierUnloaded: boolean; laterUnloaded: boolean },
): string {
  const out = messages.map(messageMarkdown);
  if (earlierUnloaded) {
    out.unshift("_Earlier messages in this session were not loaded, so this starts partway through._");
  }
  if (laterUnloaded) out.push("_Later messages in this session were not loaded._");
  return `${out.join("\n\n")}\n`;
}

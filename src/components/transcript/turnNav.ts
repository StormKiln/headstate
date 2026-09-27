/// Moving between turns, and finding the row a message id lands on
/// (#1484). Pure.

import type { TranscriptMessage } from "../../types/transcript";

/// A turn the reader can jump to: something they typed. The read model
/// marks an opener by giving it its own id as `turn_id`; meta prompts
/// and sidechains are not the reader's.
export function isOpener(m: TranscriptMessage): boolean {
  return (
    m.turn_id === m.id &&
    !m.is_meta &&
    !m.is_sidechain &&
    (m.kind.kind === "user_prompt" || m.kind.kind === "slash_command" || m.kind.kind === "shell_input")
  );
}

/// The row that shows message `id`: the message itself, or -- for a tool
/// result the merge absorbed into its call -- the message holding that
/// call. `null` when neither is held.
export function rowFor(messages: readonly TranscriptMessage[], id: string): string | null {
  for (const m of messages) {
    if (m.id === id) return m.id;
    for (const b of m.blocks) {
      if (b.kind === "tool_call" && b.result !== null && b.result.message_id === id) return m.id;
    }
  }
  return null;
}

/// The opener before (`-1`) or after (`1`) the message `from`, among
/// `messages`. `from === null` (nothing laid out) starts from the end.
/// `null` when there is none in what is held.
export function adjacentOpener(
  messages: readonly TranscriptMessage[],
  from: string | null,
  dir: -1 | 1,
): TranscriptMessage | null {
  let i = from === null ? messages.length : messages.findIndex((m) => m.id === from);
  if (i < 0) i = messages.length;
  for (let j = i + dir; j >= 0 && j < messages.length; j += dir) {
    if (isOpener(messages[j])) return messages[j];
  }
  return null;
}

/// The loaded turns, oldest first: each opener and the messages of its
/// turn up to the next. Messages before the first opener (a turn that
/// began above what was read) are not a turn here.
export function loadedTurns(
  messages: readonly TranscriptMessage[],
): { opener: TranscriptMessage; messages: TranscriptMessage[] }[] {
  const out: { opener: TranscriptMessage; messages: TranscriptMessage[] }[] = [];
  for (const m of messages) {
    if (isOpener(m)) out.push({ opener: m, messages: [m] });
    else out[out.length - 1]?.messages.push(m);
  }
  return out;
}

/// An opener's one-line text: its first text block, or its command.
export function openerText(m: TranscriptMessage): string {
  const text = m.blocks.find((b) => b.kind === "text");
  const t = text && text.kind === "text" ? text.text : "";
  const s =
    m.kind.kind === "slash_command"
      ? `${m.kind.name} ${t}`
      : m.kind.kind === "shell_input"
        ? `! ${m.kind.command}`
        : t;
  return s.replace(/\s+/g, " ").trim();
}

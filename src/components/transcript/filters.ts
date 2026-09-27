/// What the transcript shows (#1484): thinking, tool calls and their
/// output, system and meta records, and subagent sidechains, each shown
/// or hidden, remembered per device (`transcriptShow` in the store).
/// Everything is shown by default -- the decided default.
///
/// Filtering is a view over the messages the follow holds, never a
/// change to them: the task checklist, the turn footers and the "since
/// you left" summary are computed over everything, so hiding a row
/// never changes a figure. A filter that hides anything says how much.

import type { TranscriptMessage } from "../../types/transcript";

export interface TranscriptShow {
  thinking: boolean;
  tools: boolean;
  system: boolean;
  sidechains: boolean;
}

export const SHOW_ALL: TranscriptShow = {
  thinking: true,
  tools: true,
  system: true,
  sidechains: true,
};

/// A stored value as a whole setting: a key it lacks (written by an
/// older build, or not written at all) is shown.
export function showFrom(stored: Partial<TranscriptShow> | undefined | null): TranscriptShow {
  const s = stored ?? {};
  return {
    thinking: s.thinking !== false,
    tools: s.tools !== false,
    system: s.system !== false,
    sidechains: s.sidechains !== false,
  };
}

export const SHOW_LABELS: Record<keyof TranscriptShow, string> = {
  thinking: "Thinking",
  tools: "Tool calls and output",
  system: "System and meta records",
  sidechains: "Subagent sidechains",
};

/// Records that are not the conversation: what the harness, hooks and
/// the API wrote around it. `turn_duration` is not here -- it carries a
/// turn's footer -- and neither are interruptions, compactions or slash
/// command output, which change what the conversation means.
const SYSTEM_KINDS = new Set<TranscriptMessage["kind"]["kind"]>([
  "hook_output",
  "notice",
  "api_error",
  "permission_mode_change",
  "model_change",
  "injected",
  "agent_notification",
  "task_status",
  "unrecognised",
]);

function isSystem(m: TranscriptMessage): boolean {
  return m.is_meta || SYSTEM_KINDS.has(m.kind.kind);
}

export interface Filtered {
  messages: readonly TranscriptMessage[];
  /// Messages and blocks the filters hid. 0 when nothing was.
  hidden: number;
}

/// `messages` as the filters show them. The same array when every
/// filter is off, so nothing downstream re-renders for a no-op.
export function applyShow(messages: readonly TranscriptMessage[], show: TranscriptShow): Filtered {
  if (show.thinking && show.tools && show.system && show.sidechains) {
    return { messages, hidden: 0 };
  }
  let hidden = 0;
  const out: TranscriptMessage[] = [];
  for (const m of messages) {
    if ((!show.sidechains && m.is_sidechain) || (!show.system && isSystem(m))) {
      hidden++;
      continue;
    }
    const blocks = m.blocks.filter(
      (b) =>
        !(
          (!show.thinking && b.kind === "thinking") ||
          (!show.tools && (b.kind === "tool_call" || b.kind === "tool_result"))
        ),
    );
    if (blocks.length === m.blocks.length) {
      out.push(m);
      continue;
    }
    // A message that held only hidden blocks goes; one that keeps some
    // stays, with what it keeps.
    if (blocks.length === 0) {
      hidden++;
      continue;
    }
    hidden += m.blocks.length - blocks.length;
    out.push({ ...m, blocks });
  }
  return { messages: out, hidden };
}

/// The shown message to land on for `id`: itself when it is shown, else
/// the nearest shown message before it (the row the reader would scroll
/// past it at), else the nearest after. `null` when nothing is shown.
export function shownAnchor(
  all: readonly TranscriptMessage[],
  shown: readonly TranscriptMessage[],
  id: string,
): string | null {
  const visible = new Set(shown.map((m) => m.id));
  if (visible.has(id)) return id;
  const i = all.findIndex((m) => m.id === id);
  if (i < 0) return null;
  for (let j = i - 1; j >= 0; j--) if (visible.has(all[j].id)) return all[j].id;
  for (let j = i + 1; j < all.length; j++) if (visible.has(all[j].id)) return all[j].id;
  return null;
}

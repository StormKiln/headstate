/// Joining transcript pages (#1220). Rust side:
/// `src-tauri/src/claude/transcript_page.rs`, whose module docs argue
/// where the merge happens and why.
///
/// # What this does, and what it deliberately does not
///
/// Pages arrive settled WITHIN themselves. A single read of the same
/// records would also have settled ACROSS them: a call at the end of one
/// page answered by a result at the start of the next, a turn that opened
/// on an earlier page, and a model change between the last assistant
/// message of one page and the first of the next. This joins pages so
/// that all three come out exactly as the single read gives them --
/// `transcriptPages.test.ts` replays pages the Rust side produced against
/// its single read of the same file, message for message.
///
/// It does NOT re-implement the Rust `settle`. Every decision that needs
/// the record format -- which assistant messages count for a model
/// change, what opens a turn, which block is a call -- was taken by the
/// server, and the page carries the answer: `seam`, `turn_id: null`, a
/// `tool_call` with a null `result`, a standing `tool_result`. What is
/// left is matching by id.

import type {
  TranscriptMessage,
  TranscriptPosition,
  TranscriptToolOutput,
  TranscriptWindow,
} from "../types/transcript";

type Block = TranscriptMessage["blocks"][number];

/// Join contiguous pages, oldest first, into the messages one read of
/// the same records would give.
///
/// Pages must TILE: each one's `end` is the next one's `start`. A gap
/// would pair a result with a call across records nobody read, so it is
/// refused rather than merged.
export function mergeWindows(windows: readonly TranscriptWindow[]): TranscriptMessage[] {
  for (let i = 1; i < windows.length; i++) {
    if (windows[i - 1].end.offset !== windows[i].start.offset) {
      throw new Error(
        `transcript pages do not tile: one ends at byte ${windows[i - 1].end.offset} ` +
          `and the next starts at ${windows[i].start.offset}`,
      );
    }
  }
  const out: TranscriptMessage[] = [];
  const seen = new Set<string>();
  // The turn in effect at the end of what is merged so far, and the
  // model of its last real assistant message.
  let turn: string | null = null;
  let model: string | null = null;
  for (const w of windows) {
    const first = w.seam.first_model;
    for (const m of w.page.messages) {
      if (seen.has(m.id)) {
        // A record written twice, landing in two pages. A single read
        // keeps the first and never parses the second; but the second's
        // page may have absorbed results into it, and those results are
        // their own records. Stand them back up rather than lose them.
        for (const o of absorbedResults(m)) {
          if (seen.has(o.message_id)) continue;
          out.push(standing(o, turn));
          seen.add(o.message_id);
        }
        continue;
      }
      const msg: TranscriptMessage =
        m.turn_id === null && turn !== null ? { ...m, turn_id: turn } : m;
      if (first !== null && m.id === first.message_id && model !== null && model !== first.model) {
        const id = `${m.id}/model`;
        if (!seen.has(id)) {
          out.push(modelChange(id, model, first.model, first.timestamp, msg.turn_id));
          seen.add(id);
        }
      }
      out.push(msg);
      seen.add(msg.id);
      turn = msg.turn_id;
    }
    if (w.seam.last_model !== null) model = w.seam.last_model;
  }
  return pairByToolUseId(out);
}

/// Move each standing result into the unanswered call it names, by
/// `tool_use_id`: the first call with that id, first result to claim it,
/// never replacing a result already there. Then drop tool-result
/// messages left with nothing to show.
function pairByToolUseId(messages: TranscriptMessage[]): TranscriptMessage[] {
  const calls = new Map<string, [number, number]>();
  messages.forEach((m, mi) =>
    m.blocks.forEach((b, bi) => {
      if (b.kind === "tool_call" && b.id !== null && !calls.has(b.id)) calls.set(b.id, [mi, bi]);
    }),
  );
  const claimed = new Set<string>();
  const moves: { from: [number, number]; to: [number, number] }[] = [];
  messages.forEach((m, mi) =>
    m.blocks.forEach((b, bi) => {
      if (b.kind !== "tool_result" || b.tool_use_id === null) return;
      const at = calls.get(b.tool_use_id);
      if (at === undefined) return;
      const call = messages[at[0]].blocks[at[1]];
      const key = `${at[0]}:${at[1]}`;
      if (call.kind !== "tool_call" || call.result !== null || claimed.has(key)) return;
      claimed.add(key);
      moves.push({ from: [mi, bi], to: at });
    }),
  );
  if (moves.length === 0) return messages;

  // Copy only what changes: pages the caller holds are not mutated.
  const next = messages.map((m) => ({ ...m, blocks: [...m.blocks] }));
  const removed = new Set(moves.map((mv) => `${mv.from[0]}:${mv.from[1]}`));
  for (const { from, to } of moves) {
    const block = messages[from[0]].blocks[from[1]];
    const call = next[to[0]].blocks[to[1]];
    if (block.kind !== "tool_result" || call.kind !== "tool_call") continue;
    next[to[0]].blocks[to[1]] = { ...call, result: output(block) };
  }
  return next
    .map((m, mi) => ({ ...m, blocks: m.blocks.filter((_, bi) => !removed.has(`${mi}:${bi}`)) }))
    .filter((m) => !(m.kind.kind === "tool_results" && m.blocks.length === 0));
}

/// A standing result block as the output it becomes inside its call:
/// every field but the block's `kind` tag, so a field the Rust output
/// gains later is carried without this changing.
function output(block: Extract<Block, { kind: "tool_result" }>): TranscriptToolOutput {
  const out: TranscriptToolOutput & { kind?: string } = { ...block };
  delete out.kind;
  return out;
}

function absorbedResults(m: TranscriptMessage): TranscriptToolOutput[] {
  return m.blocks.flatMap((b) => (b.kind === "tool_call" && b.result !== null ? [b.result] : []));
}

/// A result whose record's own message was folded away, standing again.
/// Only the fields the output carries are known; the rest are absent,
/// not zero.
function standing(o: TranscriptToolOutput, turn: string | null): TranscriptMessage {
  return {
    id: o.message_id,
    id_source: "uuid",
    turn_id: turn,
    kind: { kind: "tool_results" },
    timestamp: null,
    model: null,
    api_message_id: null,
    usage: null,
    duration_ms: null,
    is_meta: false,
    is_sidechain: false,
    blocks: [{ kind: "tool_result", ...o }],
    offset: o.offset,
    // The record's own, which the output carries (#1476).
    oversized_bytes: o.oversized_bytes,
  };
}

/// The derived marker the Rust `model_changes` writes, field for field.
function modelChange(
  id: string,
  from: string,
  to: string,
  timestamp: string | null,
  turn: string | null,
): TranscriptMessage {
  return {
    id,
    id_source: "derived",
    turn_id: turn,
    kind: { kind: "model_change", from, to },
    timestamp,
    model: null,
    api_message_id: null,
    usage: null,
    duration_ms: null,
    is_meta: false,
    is_sidechain: false,
    blocks: [],
    offset: null,
    oversized_bytes: null,
  };
}

/// "messages ~4,200–4,400 of ~16,700 (estimate)".
///
/// A count only when the position is one (`exact`); otherwise every
/// figure carries "~" and the label says "estimate", so a scrubber never
/// implies a precision a byte offset does not have (#1220). A figure the
/// server could not give is left out, never shown as 0.
export function positionLabel(p: TranscriptPosition): string {
  const approx = p.exact ? "" : "~";
  // One "~" on the front of a range: it is approximate as a whole.
  const n = (v: number) => `${approx}${v.toLocaleString()}`;
  const of = p.total === null ? "" : ` of ${n(p.total)}`;
  const tail = p.exact ? "" : " (estimate)";
  if (p.first === null || p.last === null) {
    return p.total === null ? "no messages here" : `no messages here,${of}${tail}`;
  }
  const span =
    p.first === p.last
      ? `message ${n(p.first)}`
      : `messages ${n(p.first)}–${p.last.toLocaleString()}`;
  return `${span}${of}${tail}`;
}

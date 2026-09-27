/// "Since you left" (#1484): where this device last read a transcript,
/// and what happened after it.
///
/// # The marker
///
/// Per transcript, per device: the newest message the reader has had on
/// screen, kept in `localStorage`. A convenience of this device, not a
/// record anyone else reads -- a phone and the desktop keep their own --
/// so storage that throws (a private window, a quota, a webview that
/// refuses it) costs the marker and nothing else. Every access is in a
/// `try`.
///
/// The marker only moves forward: scrolling back through history does not
/// make what came after it unread again. "Forward" is by file offset,
/// which orders records; a derived message has none and never moves it.
///
/// # The summary
///
/// Computed here, on the client, from the messages held. When the
/// marker's own message is not held and older pages exist, some of what
/// happened since is in pages that are not loaded: every count is then a
/// floor and says "at least" (the root rule: qualify, or suppress).

import type { ClaudeWaiting } from "../../types/pr";
import type { TranscriptMessage } from "../../types/transcript";
import { changeFromArgs, creationFromWrite, diffStat } from "./diff";
import { type TaskListState, taskSummary } from "./tasks";
import { isOpener } from "./turnNav";
import type { ToolCallBlock } from "./types";

export interface ReadMarker {
  /// The message's id.
  id: string;
  /// Where its record starts: what orders two markers.
  offset: number;
}

const PREFIX = "headstate.transcript.read:";

function key(path: string): string {
  return `${PREFIX}${path}`;
}

/// The stored marker for `path`, or `null` when none was stored, the
/// stored value is not one, or storage cannot be read.
export function readMarker(path: string): ReadMarker | null {
  try {
    const raw = globalThis.localStorage?.getItem(key(path));
    if (!raw) return null;
    const v: unknown = JSON.parse(raw);
    if (
      typeof v === "object" &&
      v !== null &&
      typeof (v as ReadMarker).id === "string" &&
      typeof (v as ReadMarker).offset === "number"
    ) {
      return { id: (v as ReadMarker).id, offset: (v as ReadMarker).offset };
    }
    return null;
  } catch {
    return null;
  }
}

/// Move the marker to `m` if that is forward of where it is. Returns
/// whether it moved. A message without an offset never moves it.
export function advanceMarker(path: string, m: TranscriptMessage): boolean {
  if (m.offset === null) return false;
  const held = readMarker(path);
  if (held !== null && held.offset >= m.offset) return false;
  try {
    globalThis.localStorage?.setItem(key(path), JSON.stringify({ id: m.id, offset: m.offset }));
    return true;
  } catch {
    return false;
  }
}

/// Where the unread messages start in `messages`, given the marker.
///
/// - `index`: the first unread message's index, or `messages.length`
///   when nothing held is unread.
/// - `placed`: the marker's position is known -- its message is held, or
///   nothing older exists. When it is not, everything held after it may
///   be unread and more may be unloaded: no divider can be drawn at it.
export function unreadFrom(
  messages: readonly TranscriptMessage[],
  marker: ReadMarker,
  hasOlder: boolean,
): { index: number; placed: boolean } {
  const at = messages.findIndex((m) => m.id === marker.id);
  if (at >= 0) return { index: at + 1, placed: true };
  // Not held by id: by offset, which orders records in the file.
  const index = messages.findIndex((m) => m.offset !== null && m.offset > marker.offset);
  const first = messages.find((m) => m.offset !== null);
  const beforeHeld = first !== undefined && first.offset! > marker.offset;
  return {
    index: index < 0 ? messages.length : index,
    placed: !(beforeHeld && hasOlder),
  };
}

export interface AwaySummary {
  turns: number;
  toolCalls: number;
  /// `null` when no file was edited.
  files: { count: number; added: number; removed: number; partial: boolean } | null;
  /// `null` when no test run was seen.
  tests: { runs: number; last: string } | null;
  /// Every count is a floor: part of what happened is not loaded.
  atLeast: boolean;
  /// "3 turns · 41 tool calls · …", the card's line.
  text: string;
}

/// A command that runs a test suite. Matched on the command's words,
/// which is a guess and is only ever used to count, never to judge.
const TEST_COMMAND =
  /(^|[\s;&|(])((npx|pnpm|yarn|bunx?|npm)\s+(run\s+)?)?(vitest|jest|mocha|pytest|rspec|phpunit|tox)\b|\bcargo\s+(test|nextest)\b|\bgo\s+test\b|\b(npm|yarn|pnpm|bun)\s+(run\s+)?test\b|\bmake\s+test\S*|\b(dotnet|mvn|gradle|gradlew)\s+test\b|\bpython3?\s+-m\s+(pytest|unittest)\b/;

export function isTestCommand(command: string): boolean {
  return TEST_COMMAND.test(command);
}

function plural(n: number, word: string): string {
  return `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;
}

function edited(call: ToolCallBlock): { path: string; added: number; removed: number; partial: boolean } | null {
  const a = call.args;
  if (a.tool !== "edit" && a.tool !== "multi_edit" && a.tool !== "write") return null;
  // An edit that has not come back, or came back refused, changed nothing
  // this summary can vouch for.
  if (call.result === null || call.result.is_error === true) return null;
  let change = call.result.change ?? changeFromArgs(a);
  // A write with no recorded change: every line is new, and what it
  // replaced is not known -- the removed count is a floor.
  let unknownRemoved = false;
  if (change === null && a.tool === "write") {
    change = creationFromWrite(a.file_path, a.content);
    unknownRemoved = true;
  }
  if (change === null) return null;
  const { added, removed } = diffStat(change);
  const partial =
    unknownRemoved ||
    change.hunks_omitted > 0 ||
    change.hunks.some((h) => h.lines_omitted > 0) ||
    (a.tool === "write" && a.truncated) ||
    (a.tool === "multi_edit" && a.edits_omitted > 0);
  return { path: change.file_path ?? a.file_path, added, removed, partial };
}

function testOutcome(call: ToolCallBlock): string {
  const r = call.result;
  if (r === null) return "last has no result yet";
  if (r.is_error === true) return "last failed";
  if (r.is_error === false) return "last passed";
  return "last result not recorded";
}

function waitingText(w: ClaudeWaiting | undefined): string | null {
  if (w === undefined || w.state !== "now") return null;
  if (w.kind === "idle_prompt") return "now waiting for your input";
  if (w.kind === "permission_prompt") return "now waiting for your permission";
  return `now waiting (${w.kind.replace(/_/g, " ")})`;
}

/// What happened after the marker, or `null` when nothing held is
/// unread (and nothing unloaded could be).
export function awaySummary(
  messages: readonly TranscriptMessage[],
  marker: ReadMarker,
  opts: { hasOlder: boolean; tasks?: TaskListState; waiting?: ClaudeWaiting },
): AwaySummary | null {
  const { index, placed } = unreadFrom(messages, marker, opts.hasOlder);
  const unread = messages.slice(index);
  if (unread.length === 0 && placed) return null;
  const atLeast = !placed;

  let turns = 0;
  let toolCalls = 0;
  const files = new Map<string, { added: number; removed: number; partial: boolean }>();
  let testRuns = 0;
  let lastTest: ToolCallBlock | null = null;
  for (const m of unread) {
    if (isOpener(m)) turns++;
    for (const b of m.blocks) {
      if (b.kind !== "tool_call") continue;
      toolCalls++;
      const e = edited(b);
      if (e !== null) {
        const f = files.get(e.path) ?? { added: 0, removed: 0, partial: false };
        files.set(e.path, {
          added: f.added + e.added,
          removed: f.removed + e.removed,
          partial: f.partial || e.partial,
        });
      }
      if (b.args.tool === "bash" && isTestCommand(b.args.command)) {
        testRuns++;
        lastTest = b;
      }
    }
  }

  const q = atLeast ? "at least " : "";
  const parts = [`${q}${plural(turns, "turn")}`, `${q}${plural(toolCalls, "tool call")}`];
  let fileSummary: AwaySummary["files"] = null;
  if (files.size > 0) {
    let added = 0;
    let removed = 0;
    let partial = false;
    for (const f of files.values()) {
      added += f.added;
      removed += f.removed;
      partial ||= f.partial;
    }
    fileSummary = { count: files.size, added, removed, partial };
    const lines = `${partial ? "at least " : ""}+${added.toLocaleString()} −${removed.toLocaleString()}`;
    parts.push(`${q}${plural(files.size, "file")} edited (${lines})`);
  }
  let tests: AwaySummary["tests"] = null;
  if (lastTest !== null) {
    tests = { runs: testRuns, last: testOutcome(lastTest) };
    parts.push(`tests: ${q}${plural(testRuns, "run")}, ${tests.last}`);
  }
  const t = opts.tasks ? taskSummary(opts.tasks) : null;
  if (t !== null) parts.push(t.text);
  const w = waitingText(opts.waiting);
  if (w !== null) parts.push(w);
  return { turns, toolCalls, files: fileSummary, tests, atLeast, text: parts.join(" · ") };
}

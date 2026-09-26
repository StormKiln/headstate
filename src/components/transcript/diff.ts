/// Unified-diff arithmetic for the Edit / MultiEdit / Write renderers
/// (#1483). Pure, so the numbering is tested without rendering.
///
/// A RECORDED hunk carries `old_start`/`new_start`, and every line after
/// it is numbered by walking the lines: context advances both sides,
/// a removal the old side, an addition the new side. A RECONSTRUCTED
/// hunk carries neither, and gets no numbers -- nothing recorded any,
/// and numbering from 1 would invent them.

import type { ClaudeFileChange, ClaudeToolArgs } from "../../types/pr";

type Hunk = ClaudeFileChange["hunks"][number];
type Op = Hunk["lines"][number]["op"];

/// A run of a line's text, and whether it differs from the line it was
/// paired with.
export interface Segment {
  text: string;
  changed: boolean;
}

export interface NumberedLine {
  op: Op;
  text: string;
  /// `null` on the side a line is not on, and on every line of a
  /// reconstructed hunk.
  oldNo: number | null;
  newNo: number | null;
  /// Word-level runs, when this line was paired with its counterpart
  /// and the pair was small enough to compare. `null` renders the line
  /// whole.
  segments: Segment[] | null;
}

export interface NumberedHunk {
  /// `@@ -12,4 +12,5 @@`, or `null` for a reconstructed hunk. The
  /// counts are dropped (`@@ -12 +12 @@`) when lines were omitted from
  /// the hunk, because counts taken from the shown lines would be wrong.
  header: string | null;
  lines: NumberedLine[];
  linesOmitted: number;
}

/// `countsKnown: false` drops the header's counts for a hunk whose shown
/// lines are known to be partial in a way `lines_omitted` does not say
/// (a clipped Write).
export function numberHunk(h: Hunk, { countsKnown = true } = {}): NumberedHunk {
  const recorded = h.old_start !== null && h.new_start !== null;
  let oldNo = h.old_start ?? 0;
  let newNo = h.new_start ?? 0;
  const lines: NumberedLine[] = h.lines.map((l) => {
    let o: number | null = null;
    let n: number | null = null;
    if (recorded) {
      if (l.op !== "added") o = oldNo++;
      if (l.op !== "removed") n = newNo++;
    }
    return { op: l.op, text: l.text, oldNo: o, newNo: n, segments: null };
  });
  pairWords(lines);
  let header: string | null = null;
  if (recorded) {
    if (h.lines_omitted > 0 || !countsKnown) {
      header = `@@ -${h.old_start} +${h.new_start} @@`;
    } else {
      const oldCount = h.lines.filter((l) => l.op !== "added").length;
      const newCount = h.lines.filter((l) => l.op !== "removed").length;
      header = `@@ -${h.old_start},${oldCount} +${h.new_start},${newCount} @@`;
    }
  }
  return { header, lines, linesOmitted: h.lines_omitted };
}

/// Pair each run of removals with the run of additions right after it,
/// line for line, and mark the words that differ.
function pairWords(lines: NumberedLine[]) {
  let i = 0;
  while (i < lines.length) {
    if (lines[i].op !== "removed") {
      i++;
      continue;
    }
    const rStart = i;
    while (i < lines.length && lines[i].op === "removed") i++;
    const aStart = i;
    while (i < lines.length && lines[i].op === "added") i++;
    const pairs = Math.min(aStart - rStart, i - aStart);
    for (let k = 0; k < pairs; k++) {
      const d = wordDiff(lines[rStart + k].text, lines[aStart + k].text);
      if (d) {
        lines[rStart + k].segments = d.old;
        lines[aStart + k].segments = d.new;
      }
    }
  }
}

/// The largest token product compared. An LCS is quadratic, and a
/// minified line would otherwise stall the render it decorates.
const MAX_WORD_CELLS = 40_000;

function tokens(s: string): string[] {
  return s.match(/\w+|\s+|[^\w\s]/g) ?? [];
}

/// Word-level difference between two lines, as runs. `null` when the
/// lines are too long to compare or share nothing -- a line with every
/// word marked says less than the whole-line colour already does.
export function wordDiff(a: string, b: string): { old: Segment[]; new: Segment[] } | null {
  const x = tokens(a);
  const y = tokens(b);
  if (x.length * y.length > MAX_WORD_CELLS || x.length === 0 || y.length === 0) return null;
  const w = y.length + 1;
  const t = new Uint32Array((x.length + 1) * w);
  for (let i = x.length - 1; i >= 0; i--) {
    for (let j = y.length - 1; j >= 0; j--) {
      t[i * w + j] =
        x[i] === y[j] ? t[(i + 1) * w + j + 1] + 1 : Math.max(t[(i + 1) * w + j], t[i * w + j + 1]);
    }
  }
  if (t[0] === 0) return null;
  const keepX = new Array<boolean>(x.length).fill(false);
  const keepY = new Array<boolean>(y.length).fill(false);
  let i = 0;
  let j = 0;
  while (i < x.length && j < y.length) {
    if (x[i] === y[j]) {
      keepX[i++] = true;
      keepY[j++] = true;
    } else if (t[(i + 1) * w + j] >= t[i * w + j + 1]) {
      i++;
    } else {
      j++;
    }
  }
  return { old: runs(x, keepX), new: runs(y, keepY) };
}

function runs(toks: string[], keep: boolean[]): Segment[] {
  const out: Segment[] = [];
  toks.forEach((tok, i) => {
    const changed = !keep[i];
    const last = out[out.length - 1];
    if (last && last.changed === changed) last.text += tok;
    else out.push({ text: tok, changed });
  });
  return out;
}

/// The lines of a text, `\r\n` normalised, a trailing newline not
/// counted as a line.
export function linesOf(s: string): string[] {
  if (s === "") return [];
  const lines = s.replace(/\r\n/g, "\n").split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
}

/// A Write that created its file, as a diff: every line added,
/// numbered from 1 -- a new file's numbers are its own, not invented.
/// The content is the call's argument, which Claude Code wrote down.
export function creationFromWrite(filePath: string, content: string): ClaudeFileChange {
  return {
    file_path: filePath,
    source: "recorded",
    hunks: [
      {
        old_start: 0,
        new_start: 1,
        lines: linesOf(content).map((text) => ({ op: "added" as const, text })),
        lines_omitted: 0,
      },
    ],
    hunks_omitted: 0,
    created: true,
  };
}

/// A diff built from an Edit or MultiEdit call's own arguments, for a
/// call whose result carried no recorded change (still running, never
/// answered, or refused). RECONSTRUCTED, and so labelled: the replaced
/// text and its replacement, with no context and no line numbers.
export function changeFromArgs(args: ClaudeToolArgs): ClaudeFileChange | null {
  const hunk = (old: string, next: string): Hunk => ({
    old_start: null,
    new_start: null,
    lines: [
      ...linesOf(old).map((text) => ({ op: "removed" as const, text })),
      ...linesOf(next).map((text) => ({ op: "added" as const, text })),
    ],
    lines_omitted: 0,
  });
  switch (args.tool) {
    case "edit":
      return {
        file_path: args.file_path,
        source: "reconstructed",
        hunks: [hunk(args.old_string, args.new_string)],
        hunks_omitted: 0,
        created: null,
      };
    case "multi_edit":
      return {
        file_path: args.file_path,
        source: "reconstructed",
        hunks: args.edits.map((e) => hunk(e.old_string, e.new_string)),
        hunks_omitted: args.edits_omitted,
        created: null,
      };
    default:
      return null;
  }
}

/// Lines added and removed across the shown hunks.
export function diffStat(change: ClaudeFileChange): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const h of change.hunks) {
    for (const l of h.lines) {
      if (l.op === "added") added++;
      else if (l.op === "removed") removed++;
    }
  }
  return { added, removed };
}

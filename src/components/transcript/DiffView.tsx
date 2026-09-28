import { useId, useState } from "react";
import type { ClaudeFileChange } from "../../types/pr";
import { MaskedText } from "../MaskedText";
import { diffStat, numberHunk, type NumberedLine } from "./diff";
import { palette } from "./palette";
import type { ToolVariant } from "./types";

/// A file change as a real unified diff (#1483): hunk headers, line
/// numbers, word-level highlights, EXPANDED by default.
///
/// A `reconstructed` change is labelled as one and has no numbers and
/// no header -- see `diff.ts`. The terminal variant has two gutters
/// (old, new); the compact one keeps one, the number on the side the
/// line is on, so a phone line keeps its width for code.
///
/// Separation does not rest on colour (#1489): every line carries its
/// `+`/`-`/space, and changed words are underlined as well as tinted.
export function DiffView({
  change,
  variant,
  note,
  defaultOpen = true,
  countsKnown = true,
}: {
  change: ClaudeFileChange;
  variant: ToolVariant;
  /// A sentence above the diff, e.g. that the edit was not applied.
  note?: string;
  defaultOpen?: boolean;
  /// `false` when the shown lines are partial in a way the change does
  /// not itself record (a clipped Write): counts become floors.
  countsKnown?: boolean;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const bodyId = useId();
  const { added, removed } = diffStat(change);
  const omitted =
    !countsKnown || change.hunks_omitted > 0 || change.hunks.some((h) => h.lines_omitted > 0);
  const stat = `${omitted ? "at least " : ""}+${added} −${removed}`;
  const path = change.file_path ?? "a file";

  return (
    <div
      className="mt-1 rounded border"
      style={{ borderColor: palette.border, background: palette.ground }}
    >
      <button
        type="button"
        aria-expanded={open}
        aria-controls={bodyId}
        aria-label={`Diff of ${path}, ${added} added, ${removed} removed${omitted ? ", partly shown" : ""}`}
        onClick={() => setOpen((o) => !o)}
        className="flex w-full flex-wrap items-baseline gap-x-2 px-1.5 py-0.5 text-left text-[11px] focus-visible:outline focus-visible:outline-2"
        style={{ color: palette.muted }}
      >
        <span aria-hidden>{open ? "▾" : "▸"}</span>
        <span className="font-mono break-all" style={{ color: palette.text }}>
          {path}
        </span>
        {change.created === true ? <span>new file</span> : null}
        <span>{stat}</span>
        {change.source === "reconstructed" ? <span>reconstructed</span> : null}
      </button>
      <div id={bodyId} hidden={!open}>
        {open ? (
          <>
            {note ? (
              <p className="px-1.5 text-[11px]" style={{ color: palette.warn }}>
                {note}
              </p>
            ) : null}
            {change.source === "reconstructed" ? (
              <p className="px-1.5 text-[11px]" style={{ color: palette.muted }}>
                Reconstructed from the replaced text: no surrounding lines or line numbers were
                recorded.
              </p>
            ) : null}
            {change.hunks.map((h, i) => {
              const nh = numberHunk(h, { countsKnown });
              return (
                <div key={i} className="mt-0.5">
                  {nh.header ? (
                    <div
                      className="px-1.5 font-mono text-[11px]"
                      style={{ color: palette.muted, background: palette.surface }}
                    >
                      {nh.header}
                    </div>
                  ) : null}
                  <div role="table" aria-label={`Hunk ${i + 1}`} className="font-mono text-[11px] leading-snug">
                    {nh.lines.map((l, j) => (
                      <DiffRow key={j} line={l} variant={variant} />
                    ))}
                  </div>
                  {nh.linesOmitted > 0 ? (
                    <p className="px-1.5 text-[11px]" style={{ color: palette.warn }}>
                      {nh.linesOmitted.toLocaleString()} more line
                      {nh.linesOmitted === 1 ? "" : "s"} in this hunk are not shown.
                    </p>
                  ) : null}
                </div>
              );
            })}
            {change.hunks_omitted > 0 ? (
              <p className="px-1.5 text-[11px]" style={{ color: palette.warn }}>
                {change.hunks_omitted.toLocaleString()} more changed region
                {change.hunks_omitted === 1 ? "" : "s"} in this file are not shown.
              </p>
            ) : null}
          </>
        ) : null}
      </div>
    </div>
  );
}

const SIGN = { added: "+", removed: "-", context: " " } as const;
const OP_NAME = { added: "added", removed: "removed", context: "unchanged" } as const;

function DiffRow({ line, variant }: { line: NumberedLine; variant: ToolVariant }) {
  const bg =
    line.op === "added" ? palette.addedBg : line.op === "removed" ? palette.removedBg : undefined;
  const fg =
    line.op === "added"
      ? palette.addedText
      : line.op === "removed"
        ? palette.removedText
        : palette.muted;
  const gutter = "w-10 shrink-0 select-none pr-1 text-right tabular-nums";
  const num = (n: number | null) => (n === null ? "" : String(n));
  return (
    <div role="row" className="flex" style={{ background: bg }}>
      {variant === "terminal" ? (
        <>
          <span role="cell" className={gutter} style={{ color: palette.muted }} data-testid="old-no">
            {num(line.oldNo)}
          </span>
          <span role="cell" className={gutter} style={{ color: palette.muted }} data-testid="new-no">
            {num(line.newNo)}
          </span>
        </>
      ) : (
        <span role="cell" className={gutter} style={{ color: palette.muted }} data-testid="line-no">
          {num(line.op === "removed" ? line.oldNo : line.newNo)}
        </span>
      )}
      <span role="cell" className="min-w-0 flex-1 whitespace-pre-wrap break-all" style={{ color: fg }}>
        <span className="sr-only">{OP_NAME[line.op]}: </span>
        <span aria-hidden>{SIGN[line.op]}</span>
        {line.segments
          ? line.segments.map((s, k) =>
              s.changed ? (
                <mark
                  key={k}
                  className="underline decoration-1 underline-offset-2"
                  style={{
                    background:
                      line.op === "added" ? palette.addedWordBg : palette.removedWordBg,
                    color: line.op === "added" ? palette.addedWordText : palette.removedWordText,
                  }}
                >
                  <MaskedText text={s.text} />
                </mark>
              ) : (
                <span key={k}>
                  <MaskedText text={s.text} />
                </span>
              ),
            )
          : // A phone's copy may carry masking markers (#1488).
            <MaskedText text={line.text} />}
      </span>
    </div>
  );
}

import { useId, useState } from "react";
import { ClippedText } from "./ClippedText";
import { linesOf } from "./diff";
import { palette } from "./palette";
import { countLabel } from "./summary";
import type { LoadFullText, ThinkingBlock as Block, ToolVariant } from "./types";

/// A thinking block (#1483). Shown, dimmed, per the epic's default.
///
/// `recorded: false` is a block whose text Claude Code did not keep --
/// only its signature. It renders as "not recorded", never as an empty
/// thought: the model did think; the transcript did not write it down.
///
/// `collapsible` (the desktop, #1480) makes the "Thinking" label a
/// toggle that hides the text. Open by default either way: the default
/// is to show thinking.
export function ThinkingBlock({
  block,
  messageId,
  offset,
  variant,
  onLoadFullText,
  collapsible = false,
}: {
  block: Block;
  /// The message the block is in: with `block.index`, the full-text
  /// address.
  messageId: string;
  /// That message's `offset`, the fetch's hint (#1476).
  offset: number | null;
  variant: ToolVariant;
  onLoadFullText?: LoadFullText;
  collapsible?: boolean;
}) {
  const [open, setOpen] = useState(true);
  const regionId = useId();
  const size = variant === "terminal" ? "font-mono text-[12px]" : "text-[13px]";
  if (!block.recorded) {
    return (
      <p className={`${size} italic`} style={{ color: palette.muted }}>
        ✻ Thinking (not recorded)
      </p>
    );
  }
  return (
    <div className={size} role="group" aria-label="Thinking">
      {collapsible ? (
        <button
          type="button"
          aria-expanded={open}
          aria-controls={regionId}
          // How much it hides, as `Fold` says it (#1489).
          aria-label={`Thinking, ${countLabel(linesOf(block.text).length, "line", block.clip !== null)}`}
          onClick={() => setOpen((o) => !o)}
          className="italic hover:underline focus-visible:outline focus-visible:outline-2"
          style={{ color: palette.muted }}
        >
          ✻ Thinking {open ? "▾" : "▸"}
        </button>
      ) : (
        <p className="italic" style={{ color: palette.muted }}>
          ✻ Thinking
        </p>
      )}
      {/* A rule down its side, as the phone draws it: thinking reads as
          apart from the reply in greyscale, not only by its dimmer colour
          (#1489). */}
      <div
        id={regionId}
        hidden={!open}
        className="border-l-2 border-dotted pl-2"
        style={{ borderColor: palette.border }}
      >
        {open ? (
          <ClippedText
            text={block.text}
            clip={block.clip}
            address={{ messageId, index: block.index, offset }}
            onLoadFullText={onLoadFullText}
          >
            {(t) => (
              <p className="whitespace-pre-wrap break-words italic" style={{ color: palette.muted }}>
                {t}
              </p>
            )}
          </ClippedText>
        ) : null}
      </div>
    </div>
  );
}

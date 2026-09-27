import { ClippedText } from "./ClippedText";
import { palette } from "./palette";
import type { LoadFullText, ThinkingBlock as Block, ToolVariant } from "./types";

/// A thinking block (#1483). Shown, dimmed, per the epic's default.
///
/// `recorded: false` is a block whose text Claude Code did not keep --
/// only its signature. It renders as "not recorded", never as an empty
/// thought: the model did think; the transcript did not write it down.
export function ThinkingBlock({
  block,
  messageId,
  variant,
  onLoadFullText,
}: {
  block: Block;
  /// The message the block is in: with `block.index`, the full-text
  /// address.
  messageId: string;
  variant: ToolVariant;
  onLoadFullText?: LoadFullText;
}) {
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
      <p className="italic" style={{ color: palette.muted }}>
        ✻ Thinking
      </p>
      <ClippedText
        text={block.text}
        clip={block.clip}
        address={{ messageId, index: block.index }}
        onLoadFullText={onLoadFullText}
      >
        {(t) => (
          <p className="whitespace-pre-wrap break-words italic" style={{ color: palette.muted }}>
            {t}
          </p>
        )}
      </ClippedText>
    </div>
  );
}

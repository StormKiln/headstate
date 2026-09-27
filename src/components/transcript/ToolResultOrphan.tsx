import { ClippedText } from "./ClippedText";
import { DiffView } from "./DiffView";
import { linesOf } from "./diff";
import { Fold } from "./Fold";
import { MonoOutput } from "./output";
import { palette } from "./palette";
import { countLabel, isClipped } from "./summary";
import { SubagentLink } from "./ToolCall";
import type { LoadFullText, OpenSubagent, ToolResultBlock, ToolVariant } from "./types";

/// A result whose call is not in the loaded messages (#1483): the call
/// is in an earlier page.
///
/// NOT a missing call and not a fault -- a call that was not read yet.
/// Worded that way, with the button that reads it when the renderer
/// can page back. Without the call there is no tool name, so the output
/// is shown as output.
export function ToolResultOrphan({
  block,
  variant,
  onLoadFullText,
  onOpenSubagent,
  onLoadEarlier,
}: {
  block: ToolResultBlock;
  variant: ToolVariant;
  onLoadFullText?: LoadFullText;
  onOpenSubagent?: OpenSubagent;
  /// Page back until the call is held, not one page per click (#1484).
  /// Omitted when there is nothing earlier to load, so no button is
  /// offered that cannot work.
  onLoadEarlier?: (toolUseId: string | null) => void;
}) {
  const error = block.is_error === true;
  const clipped = isClipped(block.clip);
  return (
    <div
      role="group"
      aria-label="Tool result from an earlier call"
      className={variant === "terminal" ? "font-mono text-[12px]" : "text-[13px]"}
      style={{ color: palette.text }}
      data-state="call_in_earlier_page"
    >
      <p className="text-[11px]" style={{ color: palette.muted }}>
        Result of a call in an earlier part of the transcript.
        {onLoadEarlier ? (
          <>
            {" "}
            <button
              type="button"
              className="underline focus-visible:outline focus-visible:outline-2"
              style={{ color: palette.link }}
              onClick={() => onLoadEarlier(block.tool_use_id)}
            >
              Load earlier messages
            </button>
          </>
        ) : null}
      </p>
      {error ? (
        <p className="text-[11px]" style={{ color: palette.error }}>
          The tool reported an error.
        </p>
      ) : null}
      {block.change ? <DiffView change={block.change} variant={variant} /> : null}
      {block.subagent ? <SubagentLink sub={block.subagent} onOpen={onOpenSubagent} /> : null}
      {block.text === "" && !clipped ? null : (
        <Fold
          label="Output"
          count={countLabel(linesOf(block.text).length, "line", clipped)}
          variant={variant}
          title="Tool result"
        >
          <ClippedText
            text={block.text}
            clip={block.clip}
            address={{ messageId: block.message_id, index: block.index, offset: block.offset }}
            onLoadFullText={onLoadFullText}
          >
            {(t) => <MonoOutput text={t} error={error} />}
          </ClippedText>
        </Fold>
      )}
    </div>
  );
}

/// A TEXT-ONLY stand-in renderer for the transcript viewer shell (#1479).
///
/// TEMPORARY. #1480 (the desktop terminal renderer) and #1481 (the
/// phone's bubbles) replace this; it exists so the shell has a host and
/// can be seen and tested before they land. It renders a message's text
/// blocks and says, rather than hides, that anything else is not shown.
/// `TranscriptPreview` in `ClaudeCodePage.tsx` stays available beside it
/// until then.

import { Message, MessageContent, MessageHeader } from "@/components/ui/message";
import { MaskedText } from "../MaskedText";
import type { TranscriptMessage } from "../../types/transcript";

/// Who a message is from, in the words the old preview uses.
function speaker(m: TranscriptMessage): string {
  switch (m.kind.kind) {
    case "user_prompt":
    case "slash_command":
    case "shell_input":
    case "queued_prompt":
      return "You";
    case "assistant":
      return "Claude";
    default:
      // The read model's own name for the kind, readable: an
      // `unrecognised` record says what it was rather than vanishing.
      return m.kind.kind === "unrecognised"
        ? m.kind.record_type
        : m.kind.kind.replace(/_/g, " ");
  }
}

export function renderPlaceholderMessage(m: TranscriptMessage) {
  const texts = m.blocks.flatMap((b) =>
    b.kind === "text" ? [{ index: b.index, text: b.text, clip: b.clip }] : [],
  );
  const other = m.blocks.length - texts.length;
  const mine = speaker(m) === "You";
  return (
    <Message align={mine ? "end" : "start"}>
      <MessageContent>
        <MessageHeader className="px-0">{speaker(m)}</MessageHeader>
        {texts.map((t) => (
          <div key={t.index} className="text-xs">
            {/* The desktop's masking markers render as pills on the phone (#1488). */}
            <p className="whitespace-pre-wrap break-words text-[#e6edf3]">
              <MaskedText text={t.text} />
            </p>
            {t.clip ? (
              <p className="mt-1 text-[11px] text-[#8b949e]">
                Showing the first {t.clip.shown_chars.toLocaleString()} of{" "}
                {t.clip.total_chars.toLocaleString()} characters.
              </p>
            ) : null}
          </div>
        ))}
        {other > 0 ? (
          <p className="text-[11px] text-[#8b949e]">
            {other.toLocaleString()} {other === 1 ? "block" : "blocks"} other than text not shown
            here.
          </p>
        ) : texts.length === 0 ? (
          <p className="text-[11px] text-[#6e7681]">(nothing in this message)</p>
        ) : null}
      </MessageContent>
    </Message>
  );
}

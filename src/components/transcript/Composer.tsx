/// The message composer (#1491): what 7.10 turns on to send a message to
/// a running session. In 7.9 it is built, laid out in the viewer's
/// composer slot, and hidden behind `COMPOSER_ENABLED` -- and nothing
/// passes it `onSend`, so it could not send even if shown.
///
/// | variant | shape |
/// |---|---|
/// | `desktop` | a textarea docked under the transcript; Enter sends, Shift+Enter is a new line |
/// | `phone` | a bar that clears the home indicator (safe area) and rises with the keyboard (`keyboardInset.ts`); the button sends |
///
/// Its height does not depend on what is typed: the textarea scrolls
/// rather than grows, so typing never moves the transcript above it.
///
/// # A send
///
/// `onSend(text)` is the host's: it creates the pending message
/// (`usePendingMessages`), starts the command and returns the pending
/// message's `clientId` -- or `null` when it did not start one. Only
/// then does the composer clear itself and ask the viewer to scroll
/// (`sendScroll.ts`), so a send that did not start keeps its text.

import { type CSSProperties, type FormEvent, type KeyboardEvent, useId, useState } from "react";
import { cn } from "@/lib/utils";
import { useKeyboardInset } from "./keyboardInset";
import { palette } from "./palette";
import { useSendScroll } from "./sendScroll";

export function Composer({
  variant,
  onSend,
}: {
  variant: "desktop" | "phone";
  /// Absent: there is no send path, and the control says so.
  onSend?: (text: string) => string | null;
}) {
  const [text, setText] = useState("");
  const scroll = useSendScroll();
  const phone = variant === "phone";
  const inset = useKeyboardInset(phone);
  const noteId = useId();
  const canSend = onSend !== undefined;

  const submit = () => {
    if (!onSend || text.trim() === "") return;
    const clientId = onSend(text);
    if (clientId === null) return;
    setText("");
    scroll?.(clientId);
  };
  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    submit();
  };
  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    // The phone's return key is a new line; its button sends.
    if (phone || e.key !== "Enter" || e.shiftKey || e.nativeEvent.isComposing) return;
    e.preventDefault();
    submit();
  };

  return (
    <form
      data-slot="composer"
      data-variant={variant}
      onSubmit={onSubmit}
      className={cn(
        "flex items-end gap-2 border-t",
        phone
          ? "px-3 pt-2 pb-[max(0.5rem,env(safe-area-inset-bottom),var(--keyboard-inset))]"
          : "p-2",
      )}
      style={{
        borderColor: palette.border,
        background: palette.surface,
        ...(phone ? { "--keyboard-inset": `${inset}px` } : {}),
      } as CSSProperties}
    >
      <textarea
        aria-label="Message"
        aria-describedby={canSend ? undefined : noteId}
        rows={phone ? 1 : 2}
        value={text}
        disabled={!canSend}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={onKeyDown}
        className={cn(
          "min-w-0 flex-1 resize-none overflow-y-auto rounded-md border px-2 py-1.5 focus-visible:outline focus-visible:outline-2",
          phone ? "h-11 text-base" : "h-14 font-mono text-sm",
        )}
        style={{ borderColor: palette.border, background: palette.ground, color: palette.text }}
      />
      {canSend ? null : (
        <p id={noteId} className="sr-only">
          Sending messages from here is not available.
        </p>
      )}
      <button
        type="submit"
        disabled={!canSend || text.trim() === ""}
        className={cn(
          "shrink-0 rounded-md border px-3 focus-visible:outline focus-visible:outline-2 disabled:opacity-50",
          phone ? "h-11" : "h-8 text-sm",
        )}
        style={{ borderColor: palette.border, color: palette.text }}
      >
        Send
      </button>
    </form>
  );
}

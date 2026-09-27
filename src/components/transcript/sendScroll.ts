/// What a send does to the scroll position (#1491, #1490 constraint 4).
///
/// The message scroller's own guidance for a chat: sending re-engages
/// follow-output (`scrollToEnd`) and anchors the new turn (`scrollAnchor`
/// on the user's message), so the sent message is placed at the top of
/// the view and the reply streams in beneath it -- even if the reader
/// had scrolled up to read something older when they sent.
///
/// The viewer provides this through `TranscriptSendContext`; the
/// composer calls it once the host has created the pending message.

import { createContext, useContext } from "react";

export interface SendScrollDeps {
  /// The viewer's window reaches the newest message. When it does not,
  /// the rows the new message goes after are not mounted, and the
  /// viewer must re-window to the tail before it can scroll there.
  atTail: boolean;
  reducedMotion: boolean;
  scrollToEnd: (options: { behavior: ScrollBehavior }) => void;
  /// Re-window to the tail, then scroll to the end once it is mounted:
  /// what the viewer's "jump to latest" already does.
  rewindToTail: () => void;
  /// Mark this pending message's row as the scroll anchor.
  anchor: (clientId: string) => void;
}

export function sendScroll(clientId: string, deps: SendScrollDeps): void {
  deps.anchor(clientId);
  if (deps.atTail) {
    deps.scrollToEnd({ behavior: deps.reducedMotion ? "auto" : "smooth" });
  } else {
    deps.rewindToTail();
  }
}

/// `null` outside a `TranscriptViewer`: a composer rendered anywhere else
/// sends without moving any scroller.
export const TranscriptSendContext = createContext<((clientId: string) => void) | null>(null);

export function useSendScroll(): ((clientId: string) => void) | null {
  return useContext(TranscriptSendContext);
}

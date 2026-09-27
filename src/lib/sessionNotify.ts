import { useFilters } from "@/store/filters";

/// How often the open phone app asks for session transitions (#1486).
///
/// The companion's Rust side does the asking and the comparing, against
/// the same stored marks the background window uses, so a transition is
/// either toasted in the app or notified from the background -- never
/// both.
export const TOAST_POLL_MS = 20_000;

/// How long a session toast stays before it dismisses itself.
export const TOAST_MS = 8_000;

/// Open the transcript of the session a notification was about (#1486).
///
/// It opens AT this device's "since you left" marker (#1484): the
/// notification is about what happened while the reader was away, so
/// the screen starts where they left off. With no marker stored -- never
/// read here -- the transcript opens at the newest message as any other.
export function openFromNotification(sessionId: string): void {
  useFilters.getState().openClaudeTranscript(sessionId, "marker");
}

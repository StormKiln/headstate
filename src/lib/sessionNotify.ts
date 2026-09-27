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
/// The phone's transcript screen (#1481) opens at the NEWEST message.
/// Opening at the "since you left" marker needs the follow model's
/// message-id anchor (#1476), which has not landed; when it does, this
/// is the one place that should pass it.
export function openFromNotification(sessionId: string): void {
  useFilters.getState().openClaudeTranscript(sessionId);
}

import { useIsMobile } from "@/lib/useIsMobile";

/// Which transcript renderer a host draws (#1481, epic #1473): the
/// phone's bubbles or the desktop's terminal.
///
/// A LAYOUT question, so `useIsMobile()` -- true on the mobile build
/// unconditionally and on any viewport under the phone breakpoint -- and
/// not `IS_MOBILE_BUILD` alone (see `src/lib/target.ts`). A desktop
/// window dragged phone-narrow in a browser gets the phone renderer,
/// because it is laid out like a phone; what the renderer may DO is a
/// separate, capability question each piece answers with
/// `IS_MOBILE_BUILD` (the pull gesture, for one).
export type TranscriptRenderer = "phone" | "desktop";

export function useTranscriptRenderer(): TranscriptRenderer {
  return useIsMobile() ? "phone" : "desktop";
}

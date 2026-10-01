import { useEffect, useRef } from "react";
import type { TranscriptFollower } from "@/lib/transcriptFollow";
import type { TranscriptActivity } from "@/types/transcript";
import { claudeTranscriptWatch } from "./tauri";
import { listen, type UnlistenFn } from "./transport";
import { safeUnlisten } from "./unlisten";

export const TRANSCRIPT_ACTIVITY_EVENT = "claude-transcript-activity";
const RENEW_MS = 10_000;

// Older peers and explicit privacy/path refusals cannot improve on a timer.
// Capacity/connection failures can: retry at the lease cadence, never in a loop.
function permanent(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  return /not a Headstate command|unknown command|command.*not found|transcript watch refused|does not allow this phone to read session transcripts/i.test(message);
}

/// Optional acceleration only. Ordinary page polling always remains the owner
/// of correctness. No unregister: another window may share this opaque lease.
export function useTranscriptWatch(follower: TranscriptFollower, path: string | null, enabled: boolean) {
  const refused = useRef<TranscriptFollower | null>(null);
  useEffect(() => {
    if (!enabled || !path || refused.current === follower) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let unlisten: UnlistenFn | undefined;
    let watchId: string | undefined;
    let expires = 0;
    async function renew() {
      try {
        if (!unlisten) {
          const stop = await listen<TranscriptActivity>(TRANSCRIPT_ACTIVITY_EVENT, ({ payload }) => {
            if (!cancelled && Date.now() < expires && payload.watch_id === watchId) follower.nudge(payload.size);
          });
          if (cancelled) { safeUnlisten(stop); return; }
          unlisten = stop;
        }
        const lease = await claudeTranscriptWatch(path as string);
        if (cancelled) return;
        watchId = lease.watch_id;
        expires = Date.now() + lease.expires_in_ms;
      } catch (error) {
        if (cancelled) return;
        if (permanent(error)) {
          refused.current = follower;
          safeUnlisten(unlisten);
          unlisten = undefined;
          return;
        }
      }
      if (!cancelled) timer = setTimeout(() => void renew(), RENEW_MS);
    }
    void renew();
    return () => {
      cancelled = true;
      clearTimeout(timer);
      safeUnlisten(unlisten);
    };
  }, [enabled, follower, path]);
}

import { useEffect } from "react";
import { toast } from "sonner";
import {
  pollSessionToasts,
  takeNotificationSession,
  type SessionToast,
} from "@/api/phoneNotify";
import { openFromNotification, TOAST_MS, TOAST_POLL_MS } from "@/lib/sessionNotify";
import { useFilters } from "@/store/filters";

/// The session on screen, which gets no toast: the owner is looking at
/// it.
function viewedSession(): string | undefined {
  const s = useFilters.getState();
  if (s.view !== "claude-code") return undefined;
  return s.claudeSelected;
}

/// One in-app toast, through the app's own toaster. Keyed by session, so
/// a newer state for the same session replaces the older toast rather
/// than stacking under it.
function show(t: SessionToast): void {
  toast(t.title, {
    id: `session-${t.session_id}`,
    description: t.body,
    duration: TOAST_MS,
    action: { label: "Open", onClick: () => openFromNotification(t.session_id) },
  });
}

/// The phone's session notifications while the app is OPEN (#1486): a
/// tapped notification opens its session, and transitions in OTHER
/// sessions appear as in-app toasts. Mounted by `App` on the mobile build
/// only -- every command here is the companion's own. Renders nothing.
///
/// Both run on becoming visible, which is how a tap from the lock screen
/// arrives: iOS opens the app, the page becomes visible, and the tapped
/// session is taken from where the notification plugin parked it.
export function SessionNotifications() {
  useEffect(() => {
    let alive = true;
    const takeTapped = () => {
      takeNotificationSession()
        .then((id) => {
          if (alive && id) openFromNotification(id);
        })
        .catch((e: unknown) => {
          console.error(`could not read the tapped notification: ${String(e)}`);
        });
    };
    const poll = () => {
      if (document.visibilityState !== "visible") return;
      pollSessionToasts(viewedSession())
        .then((toasts) => {
          if (alive) toasts.forEach(show);
        })
        // A desktop that is away, or too old to have the command, costs
        // the toasts and nothing else; the next poll tries again.
        .catch(() => {});
    };
    const onVisible = () => {
      if (document.visibilityState === "visible") {
        takeTapped();
        poll();
      }
    };
    takeTapped();
    poll();
    const every = setInterval(poll, TOAST_POLL_MS);
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      alive = false;
      clearInterval(every);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, []);
  return null;
}

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { call } from "./transport";

/// The phone's own notification preferences (#789).
///
/// # Why these are not the desktop's `NotifyPrefs`
///
/// The desktop's `get_notify_prefs` is `Class::Local` on the remote
/// surface, so the phone cannot read or write it -- and that
/// classification is correct: which notifications a desktop shows at
/// that desktop is a decision made there.
///
/// It is also the right answer on the merits. The two devices are in
/// different places, and a person may well want CI failures on the
/// laptop they are working at and only new pull requests on the phone in
/// their pocket. One shared setting could not express that, so the phone
/// keeps its own in the companion's store and this module is how the
/// Settings UI reaches them.
///
/// Everything here is mobile-only by construction, the same way
/// `pairing.ts` is: `get_phone_notify_prefs` and `set_phone_notify_prefs`
/// are the companion's OWN commands, absent from the desktop build, and
/// `CLIENT_COMMANDS` in `remote.ts` is what routes them to the companion
/// rather than onto the wire. The desktop build must not import this
/// module; the Settings dialog guards that behind `IS_MOBILE_BUILD`.
///
/// # The delivery tradeoff, which the UI must state
///
/// Notifications are delivered from inside the background refresh window
/// iOS grants, so delivery is BEST-EFFORT: a new pull request surfaces
/// within the hour rather than instantly. That is why the panel carries
/// a sentence saying so -- a user who expects instant and gets hourly
/// concludes the feature is broken rather than that it is working as
/// designed.

/// Mirrors the Rust `notify::PhoneNotifyPrefs`.
///
/// Field names match the desktop's `NotifyPrefs` where the categories
/// match, so the two read as one vocabulary. There is deliberately no
/// `ci_failed` or `conflicted`: those come from the desktop's poll loop
/// and the phone is not the thing polling GitHub.
export interface PhoneNotifyPrefs {
  /// The master switch. Separate from the categories so turning
  /// notifications off does not lose what was picked underneath.
  enabled: boolean;
  /// A pull request appearing that was not there before.
  new_pr: boolean;
  /// The DESKTOP's battery: low charge, fast discharge, or discharging
  /// on AC. All three in one category, because they are one subject.
  health_battery: boolean;
  /// The DESKTOP's CPU being busy with nothing in particular.
  health_cpu: boolean;
  /// Claude Code sessions on the desktop: a turn finished, a session is
  /// waiting for input or permission, or a turn errored (#1486). The
  /// global switch; each session can be muted on its own too.
  sessions: boolean;
  /// Whether a session notification shows the session's opening prompt
  /// under its state. Off by default: the lock screen names the project
  /// and the state only.
  session_snippet: boolean;
}

/// One in-app toast for a session transition (#1486). Mirrors the Rust
/// `notify::SessionToast`, which the companion's `poll_session_toasts`
/// returns: the same transitions a background window would notify about,
/// computed in Rust against the same stored marks so one transition is
/// never both toasted and notified.
export interface SessionToast {
  session_id: string;
  title: string;
  body: string;
}

/// Module-private rather than exported alongside the hook, unlike
/// `tauri.ts`'s wrappers. Those are exported because `surfaceGuard.test`
/// reads them to prove no component can reach a `Class::Local` command.
/// These two are not on the remote surface at all -- they are the
/// companion's own -- so there is nothing for that test to check, and an
/// exported wrapper with one caller is one more way to call a phone-only
/// command from a desktop component.
const getPhoneNotifyPrefs = () => call<PhoneNotifyPrefs>("get_phone_notify_prefs");

const setPhoneNotifyPrefs = (prefs: PhoneNotifyPrefs) =>
  call<void>("set_phone_notify_prefs", { prefs });

/// The phone's notification preferences, with an optimistic local write.
///
/// `staleTime: Infinity` and a `setQueryData` after the write, matching
/// `useNotifyPrefs`: nothing but this UI changes the value, so refetching
/// it would only ever return what was just written -- and a checkbox that
/// visibly waits for a round trip to the Rust side reads as a stuck
/// control.
///
/// `enabled` is the caller's to pass: on a desktop build these commands
/// do not exist, so the query must not run at all rather than fail and
/// render an error.
export function usePhoneNotifyPrefs(enabled = true) {
  const qc = useQueryClient();
  const query = useQuery({
    queryKey: ["phone-notify-prefs"],
    queryFn: getPhoneNotifyPrefs,
    staleTime: Infinity,
    enabled,
  });
  const set = (prefs: PhoneNotifyPrefs) =>
    setPhoneNotifyPrefs(prefs).then(() => {
      qc.setQueryData(["phone-notify-prefs"], prefs);
    });
  return { prefs: query.data, set };
}

const getSessionMutes = () => call<string[]>("get_session_mutes");

const setSessionMuted = (sessionId: string, muted: boolean) =>
  call<void>("set_session_muted", { sessionId, muted });

/// Whether one session's notifications are muted on this phone (#1486),
/// with an optimistic local write for `usePhoneNotifyPrefs`'s reason.
///
/// `enabled` as there: these are the companion's own commands and do not
/// exist on the desktop build.
export function useSessionMute(sessionId: string, enabled = true) {
  const qc = useQueryClient();
  const query = useQuery({
    queryKey: ["session-mutes"],
    queryFn: getSessionMutes,
    staleTime: Infinity,
    enabled,
  });
  const muted = query.data?.includes(sessionId);
  const set = (next: boolean) =>
    setSessionMuted(sessionId, next).then(() => {
      qc.setQueryData<string[]>(["session-mutes"], (prev) => {
        const rest = (prev ?? []).filter((id) => id !== sessionId);
        return next ? [...rest, sessionId] : rest;
      });
    });
  return { muted, set, loaded: query.data !== undefined };
}

/// The in-app toasts since the last poll. `viewing` is the session on
/// screen, which gets none.
export const pollSessionToasts = (viewing: string | undefined) =>
  call<SessionToast[]>("poll_session_toasts", { viewing: viewing ?? null });

/// The session of the notification the owner last tapped, or null.
/// Cleared as it is read, so one tap opens its session once.
export const takeNotificationSession = () => call<string | null>("take_notification_session");

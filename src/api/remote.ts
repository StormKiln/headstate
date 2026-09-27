import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Transport } from "./transport";

/// The mobile transport: the companion app's webview talking to its own
/// Rust process (`src-mobile`), which forwards every command to the
/// paired desktop over pinned mTLS and replays the desktop's events.
///
/// Two kinds of command reach `call`:
///
/// - The desktop's commands, exactly as `tauri.ts` names them. They go
///   to the companion's `remote_call`, which refuses anything outside
///   the desktop's allowlist before it is sent, signs destructive
///   commands, serves `get_cached` from the cached snapshot while the
///   desktop is unreachable, and refuses write and destructive commands
///   while the desktop is unreachable, has revoked this phone, or is too
///   old to drive -- each as a rejection whose message names the
///   desktop and the reason. The class table lives in Rust alone; this
///   file does not keep a copy.
/// - The companion's own commands (`CLIENT_COMMANDS`), invoked directly:
///   `pair_from_qr({payload, deviceName?})`, `unpair()`,
///   `connection_state()`, `subscribe_events()`, and the phone's own
///   notification preferences (#789). `connection.ts` polls
///   `connection_state` through here; its `stale` field is the marker
///   for a list that may be out of date.
///
///   `get_phone_notify_prefs` / `set_phone_notify_prefs` are the phone's
///   OWN settings and deliberately not the desktop's: the desktop's
///   `get_notify_prefs` is `Class::Local` on the remote surface, and
///   correctly so -- which notifications a desktop shows at that desktop
///   is a decision made there. It is also the right answer on the
///   merits: the two devices are in different places, and someone may
///   well want CI failures on the laptop they are working at and only
///   new pull requests on the phone in their pocket. One shared setting
///   could not express that.
///
/// Events need no forwarding: the companion re-emits each desktop event
/// under its own name (`prs-updated`, `poll-state`, ...), plus
/// `connection-state` on every change, so `listen` is Tauri's. The
/// first `listen` asks the companion to open the event stream, and the
/// page's return to the foreground asks again: iOS ends the stream when
/// the app suspends, and re-subscribing is how the phone catches up.
const CLIENT_COMMANDS = new Set([
  "pair_from_qr",
  "unpair",
  "connection_state",
  "remote_call",
  "subscribe_events",
  "get_phone_notify_prefs",
  "set_phone_notify_prefs",
  // Session notifications (#1486): per-session mutes, the in-app toast
  // poll, and the session of the notification last tapped. All the
  // phone's own; `poll_session_toasts` reaches the desktop itself, from
  // Rust, so its transitions share the background window's state.
  "get_session_mutes",
  "set_session_muted",
  "poll_session_toasts",
  "take_notification_session",
]);

let installed = false;

/// Open (or wake) the event stream. Rejected while unpaired, which is
/// not an error here: pairing starts the stream itself, and the next
/// foreground resume asks again.
function subscribeEvents(): void {
  invoke("subscribe_events").catch((e: unknown) => {
    // Rejected while unpaired is normal and documented -- pairing
    // starts the stream itself. Anything else is a resume that failed
    // to reopen the stream, which used to be indistinguishable from a
    // normal one because every rejection was swallowed. There is no
    // console on a phone, so a log line is the only way this is ever
    // diagnosable.
    const message = e instanceof Error ? e.message : String(e);
    if (!/not paired/i.test(message)) {
      console.error(`could not open the event stream: ${message}`);
    }
  });
}

/// Once: the first subscription, and the foreground hook for every
/// later one. Installed lazily rather than at import so that importing
/// this module on the desktop build has no side effect.
function ensureSubscribed(): void {
  if (installed) return;
  installed = true;
  subscribeEvents();
  if (typeof document !== "undefined") {
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "visible") subscribeEvents();
    });
  }
}

export const remote: Transport = {
  call: <T>(name: string, args?: Record<string, unknown>) =>
    CLIENT_COMMANDS.has(name)
      ? args === undefined
        ? invoke<T>(name)
        : invoke<T>(name, args)
      : invoke<T>("remote_call", { command: name, args: args ?? {} }),
  listen: (event, cb) => {
    ensureSubscribed();
    return listen(event, cb);
  },
};

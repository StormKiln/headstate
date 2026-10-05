import { retireSourceOwnership } from "./sourceRefreshHooks";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { call, listen, type UnlistenFn } from "./transport";

/// What the mobile crate's `connection_state` command answers with.
///
/// The command belongs to issue #514 (the mobile client commands) and
/// does not exist yet; this file is the frontend's side of that
/// contract, written first so the connection banner has something to
/// render against. `state` is the five-way enum from the design spec;
/// `desktop` is the paired desktop's name from the pairing QR, null
/// while unpaired; `last_poll` is when the desktop last reported a
/// successful GitHub poll over `/v1/events`, ISO 8601, null before the
/// first one; `protocol_version` is what the desktop's `/v1/hello`
/// answered, absent or null until the phone has heard one. A report
/// from before the field existed reads the same as "not known".
interface ConnectionReport {
  state: "unpaired" | "connecting" | "connected" | "unreachable" | "revoked";
  desktop: string | null;
  last_poll: string | null;
  protocol_version?: number | null;
  /// True unless connected to a desktop the companion can drive: what
  /// the list's stale marker reads, and when the companion refuses
  /// write and destructive commands (`remote_call` rejects with a
  /// message naming the desktop and the reason).
  stale?: boolean;
  /// Whether THIS phone holds a post-quantum step-up key. Absent or
  /// null when it could not be determined -- a locked keychain is not
  /// evidence of a classical-only device. Desktop builds never set it.
  has_mldsa?: boolean | null;
}

/// The connection as the UI sees it.
///
/// `local` is the desktop app talking to its own Rust process: there is
/// no connection to report, and the banner renders nothing. `unknown` is
/// the mobile build before `connection_state` has answered -- including
/// when the command is not there, which is the case until #514 lands.
/// The rest are the command's own states, with the desktop's name
/// attached because every banner line needs it. `connected` also
/// carries the desktop's protocol version, because that is the one
/// state in which the phone would otherwise go on to issue commands: a
/// desktop older than `REQUIRED_PROTOCOL_VERSION` (`src/lib/protocol.ts`)
/// is reachable and yet must not be driven, and the banner says so.
export type ConnectionState =
  | { kind: "local" }
  | { kind: "unknown" }
  | { kind: "unpaired" }
  | {
      kind: "connected";
      desktop: string;
      lastPoll: string | null;
      /// Null while unknown; never null once the desktop has answered.
      protocolVersion: number | null;
      /// See `stale` on `ConnectionReport`. False in the ordinary
      /// connected case; true for a desktop that answers but must not
      /// be driven, such as one below the required protocol.
      stale: boolean;
    }
  | {
      kind: "connecting" | "unreachable" | "revoked";
      desktop: string;
      lastPoll: string | null;
      stale: boolean;
    };

/// Whether what the app is showing may be out of date.
///
/// The companion serves `get_cached` from its stored snapshot while the
/// desktop is away, so the list renders normally with no hint that its
/// rows are hours old -- which is exactly what this answers. `local` is
/// never stale: the desktop holds the data itself.
export function isStale(state: ConnectionState): boolean {
  switch (state.kind) {
    // The desktop holds the data itself, and an unpaired phone is
    // showing nothing to be stale about.
    case "local":
    case "unknown":
    case "unpaired":
      return false;
    default:
      return state.stale;
  }
}

/// How often the banner re-asks WITHOUT an event to go on.
///
/// The companion pushes `connection-state` on every change, so this is a
/// backstop rather than the mechanism: it catches a change whose event
/// was missed, which is most plausible around a suspension. Thirty
/// seconds costs the phone two webview wake-ups a minute instead of
/// twelve, and no state transition waits on it in practice.
const CONNECTION_POLL_MS = 30_000;

/// The event the companion emits on every connection change. Must match
/// `STATE_EVENT` in `src-mobile/src/connection.rs`.
const STATE_EVENT = "connection-state";

/// The mobile crate's answer. Through the transport, like every other
/// command: on the mobile build `remote.ts` invokes the companion's own
/// `connection_state` directly.
function connectionState(): Promise<ConnectionReport> {
  return call<ConnectionReport>("connection_state");
}

function fromReport(report: ConnectionReport): ConnectionState {
  if (report.state === "unpaired") return { kind: "unpaired" };
  // Absent means stale for every state but `connected`: a report from
  // before the field existed still describes a desktop the phone cannot
  // reach, and defaulting THAT to fresh would mark hours-old rows live.
  const stale = report.stale ?? report.state !== "connected";
  // A paired desktop always has a name -- it came from the QR -- but
  // the wire type allows null, and a banner reading "null is
  // unreachable" is worse than a generic noun.
  const desktop = report.desktop ?? "Desktop";
  if (report.state === "connected") {
    return {
      kind: "connected",
      desktop,
      lastPoll: report.last_poll,
      // Missing and null both mean "not known": the banner treats an
      // unknown version as fine, so a report that predates the field
      // does not turn into an update demand.
      protocolVersion: report.protocol_version ?? null,
      stale,
    };
  }
  return { kind: report.state, desktop, lastPoll: report.last_poll, stale };
}

const LOCAL: ConnectionState = { kind: "local" };

/// The desktop's answer: a constant. No query, and therefore no
/// QueryClient needed -- the banner mounts in every window and must
/// not make the desktop pay for a question only the phone asks.
function useLocalConnectionState(): ConnectionState {
  return LOCAL;
}

/// The phone's answer: the companion pushes every change, and a slow
/// poll underneath catches anything a missed event would have stranded.
///
/// It used to poll alone, every five seconds. `connection.rs` emits
/// `connection-state` on EVERY change (`STATE_EVENT`) and `remote.ts`
/// documents it -- "plus `connection-state` on every change, so `listen`
/// is Tauri's" -- and nothing listened. So the phone woke its webview
/// twelve times a minute for a value it was being handed for free, and
/// still showed a state up to five seconds stale.
///
/// The poll is kept, at a much longer interval, as a safety net: an
/// event dropped while the app was suspended would otherwise leave the
/// banner wrong until the next change, and a wrong connection banner is
/// what the stale marker depends on.
function useRemoteConnectionState(): ConnectionState {
  const client = useQueryClient();
  const { data } = useQuery({
    queryKey: ["connection-state"],
    queryFn: connectionState,
    refetchInterval: CONNECTION_POLL_MS,
    // A missing command rejects every time; retrying three times with
    // backoff before the next interval only delays the same answer.
    retry: false,
  });

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let live = true;
    try {
      // `listen` throws SYNCHRONOUSLY outside a Tauri runtime -- see the
      // note on the pass-throughs in `transport.ts`, which is why this
      // is a try/catch and not only a `.catch`. Without a runtime there
      // are no events to receive, and the poll below is the whole
      // mechanism; the banner is exactly as correct as it was before.
      void listen<ConnectionReport>(STATE_EVENT, (e) => {
        // Written straight into the cache rather than held in component
        // state: the query is the single source for this value, and two
        // copies would disagree the moment a poll landed between events.
        if (["unpaired", "revoked"].includes(e.payload.state)) {
          retireSourceOwnership(client);
          void client.cancelQueries();
          client.removeQueries({ predicate: query => query.queryKey[0] !== "connection-state" });
        }
        client.setQueryData(["connection-state"], e.payload);
      }).then(
        (off) => {
          // Unmounted before the subscription resolved: stop it now
          // rather than leaking a listener outliving the component.
          if (live) unlisten = off;
          else off();
        },
        () => {},
      );
    } catch {
      // As above: fall back to polling alone.
    }
    return () => {
      live = false;
      try {
        unlisten?.();
      } catch {
        // Tearing down a listener the runtime no longer knows about is
        // not worth failing an unmount over.
      }
    };
  }, [client]);

  return data === undefined ? { kind: "unknown" } : fromReport(data);
}

/// Whether this phone holds a post-quantum step-up key.
///
/// `null` means unanswered -- no keys yet, a keychain that would not
/// open, or a desktop build. Deliberately NOT folded into
/// `ConnectionState`: that union describes the LINK to a desktop and
/// each variant is shaped by what is known in that state, while this is
/// a fact about the phone's own hardware that does not vary with
/// connectivity. Adding it to five variants would imply it does.
///
/// Reads the cache `useRemoteConnectionState` already fills, so this
/// costs no extra call -- `connection_state` is one command answering
/// both questions.
export function usePhoneHasMldsa(): boolean | null {
  const { data } = useQuery({
    queryKey: ["connection-state"],
    queryFn: connectionState,
    refetchInterval: CONNECTION_POLL_MS,
    retry: false,
    enabled: import.meta.env.VITE_TARGET === "mobile",
  });
  return data?.has_mldsa ?? null;
}

/// The current connection to the paired desktop.
///
/// Chosen once, at module load, from the build target: the target is a
/// compile-time constant, so this is one hook or the other for the life
/// of the process rather than a hook called conditionally.
export const useConnectionState: () => ConnectionState =
  import.meta.env.VITE_TARGET === "mobile" ? useRemoteConnectionState : useLocalConnectionState;

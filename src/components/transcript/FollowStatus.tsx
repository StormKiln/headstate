/// What the transcript follow is doing, in words (#1476): the status line
/// both hosts -- the desktop's and the phone's -- show above the viewer.
///
/// Each of the follow's states says something different, and none may
/// read as another (`src/lib/transcriptFollow.ts` lists them):
///
/// - a read that failed says so, with the time of the last one that did
///   not, and a retry -- a failed read of a live file can succeed next;
/// - "not following" says WHY: the reader is far back (they can jump to
///   the latest), or the session is not running -- never as quiet output;
/// - a time is a fixed clock time, never "3 s ago": a relative phrase
///   re-rendered from the clock would keep counting after the follow
///   stopped.

import type { TranscriptLive } from "@/api/hooks";
import { commandError } from "@/lib/errorKind";
import { positionLabel } from "@/lib/transcriptPages";
import { palette } from "./palette";

/// "14:03:07" from epoch ms; the instant comes from the follow.
function clock(ms: number): string {
  const at = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(at.getHours())}:${p(at.getMinutes())}:${p(at.getSeconds())}`;
}

function reason(e: unknown): string {
  return commandError(e).message;
}

/// What a screen reader is told when the follow's state CHANGES
/// (#1489): the state sentence without its clock time. The visible line
/// carries the time and is not a live region -- it changes on every
/// read, and announcing each one would talk over the transcript.
function followAnnouncement(
  live: Pick<TranscriptLive, "status" | "atLiveEdge" | "lastReadAt" | "error">,
): string {
  switch (live.status) {
    case "loading":
      return "";
    case "following":
      return "Following.";
    case "idle":
      return "Following; nothing new for a minute.";
    case "paused":
      return live.atLiveEdge
        ? "Paused while this is not on screen."
        : "Not following while earlier messages are shown.";
    case "stopped":
      return "Not following: the session is not running.";
    case "could-not-read":
      return live.lastReadAt === null
        ? `Could not read this transcript: ${reason(live.error)}.`
        : `Could not read new output: ${reason(live.error)}.`;
  }
}

export function FollowStatus({ live }: { live: TranscriptLive }) {
  const at = live.lastReadAt === null ? null : clock(live.lastReadAt);
  let line: string | null;
  switch (live.status) {
    case "loading":
      line = null;
      break;
    case "following":
      line = `Following. Last read at ${at}.`;
      break;
    case "idle":
      line = `Following; nothing new for a minute. Last read at ${at}.`;
      break;
    case "paused":
      line = live.atLiveEdge
        ? `Paused while this is not on screen. Last read at ${at}.`
        : "Not following while earlier messages are shown.";
      break;
    case "stopped":
      line = `Read at ${at}. Not following: the session is not running.`;
      break;
    case "could-not-read":
      line =
        at === null
          ? `Could not read this transcript: ${reason(live.error)}.`
          : `Could not read new output: ${reason(live.error)}. Showing what was read at ${at}.`;
      break;
  }
  const older = live.older;
  return (
    <>
      <span role="status" className="sr-only" data-testid="transcript-follow-announce">
        {followAnnouncement(live)}
      </span>
      {line !== null ? (
        <p data-testid="transcript-read-status" data-state={live.status}>
          {line}
          {live.status === "could-not-read" ? (
            <>
              {" "}
              <button
                type="button"
                className="underline"
                style={{ color: palette.link }}
                onClick={() => void live.refresh()}
              >
                Try again
              </button>
            </>
          ) : null}
          {!live.atLiveEdge ? (
            <>
              {" "}
              <button
                type="button"
                className="underline"
                style={{ color: palette.link }}
                onClick={live.jumpToLatest}
              >
                Jump to the latest
              </button>
            </>
          ) : null}
        </p>
      ) : null}
      {live.hasOlder && live.position !== null ? (
        <p data-testid="transcript-truncated">
          Showing {positionLabel(live.position)}. Earlier messages load as you scroll up.
        </p>
      ) : null}
      {older.state === "loading" ? <p>Loading earlier messages…</p> : null}
      {older.state === "failed" ? (
        <p role="alert" style={{ color: palette.warn }}>
          Earlier messages could not be loaded: {reason(older.error)}.{" "}
          <button
            type="button"
            className="underline"
            style={{ color: palette.link }}
            onClick={live.loadOlder}
          >
            Try again
          </button>
        </p>
      ) : null}
    </>
  );
}

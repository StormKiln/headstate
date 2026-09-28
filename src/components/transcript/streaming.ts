import type { Liveness } from "../../types/pr";

/// Whether a session's last turn is still being written, for the
/// transcript viewer's `aria-busy` (#1479).
///
/// From the session's own published status, which `Liveness` carries
/// only on `running`. `undefined` where it was not established -- a
/// running session with no status, or liveness that could not be
/// checked -- because `aria-busy` should assert only what we know.
/// #1476's follow may refine this from the transcript itself.
export function transcriptStreaming(liveness: Liveness): boolean | undefined {
  if (liveness.state === "dead") return false;
  if (liveness.state !== "running") return undefined;
  if (liveness.status === "busy") return true;
  if (liveness.status === "idle") return false;
  return undefined;
}

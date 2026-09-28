/// What the transcript's session header says, as data (#1485).
///
/// Pure, so each state can be tested apart from the layout, and so the
/// desktop and the phone headers cannot word the same state two ways.
///
/// # Liveness: three answers, plus one the transcript adds
///
/// `Liveness` comes from `claude/liveness.rs` (via the session detail),
/// the same derivation the overview counts from since #1534. Since #1315 a session
/// that published only a `.key` makes the rows in its folder read
/// `unknown`, naming the pid and the folder in `why`. The header takes
/// that verdict as it is: `unknown` renders as "could not tell", with
/// the reason shown, and never as stopped. A stopped reading would offer
/// a resume that starts a second copy of a live session.
///
/// A running session with no transcript is the fourth wording. A bare
/// `claude` writes no transcript until its first prompt, so "no
/// transcript" there is "not yet", not "not found".
///
/// # Waiting: "not recorded" is not "not waiting"
///
/// `ClaudeWaiting`'s `no` arm has three reasons. `never-observed` means
/// no notification was ever recorded for this session: we were not
/// watching, which is a different fact from having watched and seen it
/// move on. The header words the two apart.

import type { ClaudeWaiting, Liveness } from "../../types/pr";
import type { TranscriptSubagent } from "../../types/transcript";

export type LivenessTone = "running" | "stopped" | "unknown";

export interface LivenessView {
  tone: LivenessTone;
  label: string;
  /// The grounds, shown beside the label. `null` on `running`, whose
  /// grounds are the pid.
  detail: string | null;
}

/// The liveness line. `noTranscript` is whether the session has no
/// transcript to read yet (no path recorded, or not on disk).
export function livenessView(liveness: Liveness, noTranscript: boolean): LivenessView {
  switch (liveness.state) {
    case "running": {
      if (noTranscript) {
        return { tone: "running", label: "Running, no transcript yet", detail: null };
      }
      // `status` is a published refinement of a derived answer, carried
      // only on `running`; see `Liveness`.
      const label = liveness.status ? `Running · ${liveness.status}` : "Running";
      return { tone: "running", label, detail: null };
    }
    case "dead":
      return { tone: "stopped", label: "Stopped", detail: liveness.why };
    case "unknown":
      return {
        tone: "unknown",
        label: "Could not tell whether it is running",
        detail: liveness.why,
      };
  }
}

export type WaitingTone = "now" | "past" | "not-waiting" | "not-recorded";

export interface WaitingView {
  tone: WaitingTone;
  label: string;
  /// For the `title`: the kind as sent and, for `past`, why the present
  /// tense could not be claimed.
  title: string | null;
}

/// What the session is waiting for, in the present tense only when a
/// live process backs it (`ClaudeWaiting.now` is constructible only
/// against `Liveness::Running`).
export function waitingView(waiting: ClaudeWaiting, clock: (iso: string) => string): WaitingView {
  switch (waiting.state) {
    case "now":
      return {
        tone: "now",
        label: waitingFor(waiting.kind),
        title: `${waiting.kind} at ${clock(waiting.at)}`,
      };
    case "last-seen":
      return {
        tone: "past",
        label: `Last seen ${lowerFirst(waitingFor(waiting.kind))} at ${clock(waiting.at)}`,
        title: `${waiting.kind}: ${waiting.why}`,
      };
    case "no":
      return waiting.reason === "never-observed"
        ? { tone: "not-recorded", label: "Whether it is waiting is not recorded", title: null }
        : { tone: "not-waiting", label: "Not waiting for you", title: null };
  }
}

/// The two #1067 kinds get their own sentence; anything else is shown as
/// sent, never relabelled.
function waitingFor(kind: string): string {
  if (kind === "idle_prompt") return "Waiting for your input";
  if (kind === "permission_prompt") return "Waiting for your permission";
  return `Waiting: ${kind}`;
}

function lowerFirst(s: string): string {
  return s.charAt(0).toLowerCase() + s.slice(1);
}

/// A subagent's liveness, from its parent session's (#1481, #1485).
///
/// A subagent's own process is not tracked; it runs inside its parent's.
/// So:
///
/// | subagent | parent | result |
/// |---|---|---|
/// | `completed` | any | stopped: it finished |
/// | not finished | running | running, under the parent's pid |
/// | not finished | stopped | stopped, with the parent's reason |
/// | not finished | unknown | unknown, with the parent's reason |
///
/// The published busy/idle is the parent's, not the subagent's, so it is
/// not carried over.
export function subagentLiveness(
  parent: Liveness,
  sub: Pick<TranscriptSubagent, "status">,
): Liveness {
  if (sub.status === "completed") return { state: "dead", why: "the subagent completed" };
  switch (parent.state) {
    case "running":
      return { state: "running", pid: parent.pid, status: null };
    case "dead":
      return { state: "dead", why: `its session is not running: ${parent.why}` };
    case "unknown":
      return {
        state: "unknown",
        why: `whether its session is running could not be told: ${parent.why}`,
      };
  }
}

/// A folder, shortened for a phone: the home directory as `~`, and only
/// the last two parts when there are more.
///
/// The frontend does not know the home directory, so it matches the
/// shapes the three platforms give it. A path matching none of them is
/// shortened by its tail alone.
export function abbreviatePath(path: string): string {
  const sep = path.includes("\\") && !path.includes("/") ? "\\" : "/";
  const home = path.match(/^(\/Users\/[^/]+|\/home\/[^/]+|[A-Za-z]:\\Users\\[^\\]+)(?=$|[/\\])/);
  const rest = home ? path.slice(home[0].length) : path;
  const parts = rest.split(sep).filter((p) => p !== "");
  if (parts.length <= 2) return home ? `~${rest}` : path;
  return `${home ? `~${sep}` : ""}…${sep}${parts.slice(-2).join(sep)}`;
}

/// A duration, in the two largest units that apply.
function formatElapsed(ms: number): string {
  const min = Math.floor(ms / 60_000);
  if (min < 1) return "under a minute";
  if (min < 60) return `${min} min`;
  const h = Math.floor(min / 60);
  if (h < 24) return `${h} h ${min % 60} min`;
  const d = Math.floor(h / 24);
  return `${d} d ${h % 24} h`;
}

/// How long the session spans, and the words for it, or `null` when
/// there is no honest figure.
///
/// Measured from `first_seen_at`, which is when the session was first
/// seen, so a resumed session's span includes the time it was stopped.
/// The words say "since first seen" for that reason.
export function elapsedView(
  firstSeen: string,
  lastActivity: string | null,
  running: boolean,
  now: number,
): string | null {
  const start = Date.parse(firstSeen);
  if (Number.isNaN(start)) return null;
  if (running) {
    if (now === 0 || now < start) return null;
    return `${formatElapsed(now - start)} since first seen`;
  }
  if (lastActivity === null) return null;
  const end = Date.parse(lastActivity);
  if (Number.isNaN(end) || end < start) return null;
  return `${formatElapsed(end - start)} from first seen to last activity`;
}

/// A token count, short: `1,234`, `45.3k`, `2.1M`.
export function compactCount(n: number): string {
  if (n < 10_000) return n.toLocaleString();
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}

import { useEffect, useState } from "react";

const MINUTE = 60_000;
const HOUR = 3_600_000;
const DAY = 86_400_000;

/// How far ahead of this machine's clock a ready time may be and still be
/// read as clock skew. Further than that, the time is wrong, and a wrong
/// time must not print as a confident "just now".
const SKEW = 5 * MINUTE;

/// `fresh` ≤ 24h, `aging` ≤ 48h, `stale` after; `unknown` when there is
/// no time to measure from (#1407).
export type ReadyTone = "fresh" | "aging" | "stale" | "unknown";

/// The palette the app already uses for success, warning and failure,
/// plus its muted grey for "we do not know". Literal class strings so
/// Tailwind's scanner sees every one.
///
/// Shared by the review queue's chip and the detail header (#1457), so a
/// pull request cannot read green in one and amber in the other.
export const READY_TONE_CLASS: Record<ReadyTone, string> = {
  fresh: "border-[#3fb950]/40 text-[#3fb950]",
  aging: "border-[#d29922]/40 text-[#d29922]",
  stale: "border-[#f85149]/40 text-[#f85149]",
  unknown: "border-[#8b949e]/40 text-[#8b949e]",
};

export interface ReadyAge {
  /// Compact age for the row: "45m", "5h", "1d 4h", "3d", or "age unknown".
  text: string;
  tone: ReadyTone;
  /// The exact ready-for-review instant, for the accessible label. Null
  /// exactly when `tone` is `unknown`.
  since: Date | null;
}

/// How long a pull request has been ready for review (#1407).
///
/// ELAPSED time, not business days: 24 and 48 hours mean the same in
/// every time zone, which is what the requester chose.
///
/// Measured from `ready_at`, not `created_at`: a pull request drafted for
/// a week and marked ready an hour ago has waited an hour.
///
/// Absent or unparseable is UNKNOWN, never zero. `ready_at` is optional on
/// the wire because a snapshot cached by an older build, or sent by an
/// older desktop to the companion, does not carry it -- and rendering
/// that as "<1m" in green would be the most legible possible lie.
///
/// Pure: `now` is passed in, so the thresholds are tested without a clock.
export function readyAge(readyAt: string | null | undefined, now: Date): ReadyAge {
  const unknown: ReadyAge = { text: "age unknown", tone: "unknown", since: null };
  if (!readyAt) return unknown;
  const since = new Date(readyAt);
  const t = since.getTime();
  if (Number.isNaN(t)) return unknown;
  const raw = now.getTime() - t;
  if (raw < -SKEW) return unknown;
  const elapsed = Math.max(0, raw);

  const tone: ReadyTone = elapsed <= DAY ? "fresh" : elapsed <= 2 * DAY ? "aging" : "stale";
  return { text: compact(elapsed), tone, since };
}

/// Minutes under an hour, hours under a day, days-and-hours under three
/// days -- where the hours still decide the colour -- and whole days after.
function compact(ms: number): string {
  if (ms < MINUTE) return "<1m";
  if (ms < HOUR) return `${Math.floor(ms / MINUTE)}m`;
  if (ms < DAY) return `${Math.floor(ms / HOUR)}h`;
  const d = Math.floor(ms / DAY);
  if (d >= 3) return `${d}d`;
  const h = Math.floor((ms % DAY) / HOUR);
  return h === 0 ? `${d}d` : `${d}d ${h}h`;
}

/// The current time, re-sampled every `intervalMs` while mounted, so an
/// age keeps counting while the view stays open without a re-fetch.
///
/// `useCountdown`'s pattern: the clock is read at mount and on each tick,
/// never during render, and each tick reads `Date.now()` rather than
/// counting ticks, so a webview throttled in the background shows the
/// true age when it comes back.
export function useNow(intervalMs: number): Date {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), intervalMs);
    return () => clearInterval(id);
  }, [intervalMs]);
  return now;
}

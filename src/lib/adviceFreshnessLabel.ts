import type { ClaudeMdAdviceFreshness } from "@/types/pr";
import { relativeTime } from "./time";

/// How a report's currency reads, and how loudly.
///
/// `tone` is a category, not a colour: the component picks the class. A
/// label that carried its own hex would put the app's palette in a file
/// about wording.
export interface FreshnessLabel {
  /// The claim itself. A sentence fragment, not a badge word: "current"
  /// alone has been the source of every currency bug this codebase has
  /// fixed, because a reader supplies their own definition for it.
  text: string;
  /// What the claim rests on -- when the producers ran, or why currency
  /// could not be established. Never folded into `text`: the two answer
  /// different questions and a user chasing a wrong finding needs the
  /// second one.
  detail: string;
  tone: "current" | "stale" | "unknown";
}

/// Say where a report came from, in words that do not overclaim.
///
/// The three freshness states get three DIFFERENT sentences, and the
/// differences are the point (#1042, #846):
///
/// - `"fresh"` is the only one allowed to say the report is current, and
///   it distinguishes a run that just happened from a stored report
///   whose fingerprint was recomputed in full and matched. Both read as
///   current because both mean every tracked input was read.
/// - `"cached"` says it came from the store. `stale` splits it: an
///   unchanged cached report is current in substance, a stale one is
///   explicitly not, and `refreshing` turns the stale phrasing into the
///   composed "and a new one is running" -- the state the backend
///   deliberately cannot claim for itself.
/// - `"unverified"` says currency is UNKNOWN and gives the producer's
///   own reason. It is never phrased as fresh-with-a-caveat: a matching
///   fingerprint here proves nothing because it omitted something both
///   times, so the tone is `"unknown"` and the word "current" does not
///   appear in it at all.
///
/// `refreshing` is only ever true for a stale cached report, because
/// that is the only state this app fires an automatic refresh for -- but
/// it is honoured wherever it is passed, so a future manual Refresh over
/// any state reads correctly rather than silently dropping the fact.
export function freshnessLabel(
  freshness: ClaudeMdAdviceFreshness,
  computedAt: string,
  refreshing: boolean,
  now: Date = new Date(),
): FreshnessLabel {
  const ran = `Checked ${relativeTime(computedAt, now)}`;
  switch (freshness.state) {
    case "fresh":
      return {
        text: refreshing ? "Up to date, re-checking now" : "Up to date",
        detail: freshness.recomputed ? `${ran}, just now` : `${ran}; nothing has changed since`,
        tone: refreshing ? "stale" : "current",
      };
    case "cached":
      if (!freshness.stale) {
        return {
          text: refreshing ? "From the last check, re-checking now" : "From the last check",
          detail: `${ran}; nothing tracked has changed since`,
          tone: refreshing ? "stale" : "current",
        };
      }
      return {
        // THE composed state. "Refreshing" is a claim about this
        // client's own in-flight request, not about anything the
        // backend told us -- which is exactly why it can be said here
        // and not there.
        text: refreshing
          ? "Out of date — showing the last check while a new one runs"
          : "Out of date — something has changed since this was checked",
        detail: ran,
        tone: "stale",
      };
    case "unverified":
      return {
        // NOT "current". The word is absent on purpose: an input could
        // not be read, so whether this report matches the files on disk
        // is a question nobody has an answer to.
        text: refreshing
          ? "Currency unknown — re-checking now"
          : "Currency unknown — an input could not be read",
        // The producer's own reason, verbatim. A count or a category
        // would send the reader to the wrong place; a permission wall
        // and a deleted file are different problems.
        detail: `${ran}; ${freshness.reason}`,
        tone: "unknown",
      };
  }
}

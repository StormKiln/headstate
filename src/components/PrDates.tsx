import { Fragment } from "react";
import type { PrDetail } from "@/types/pr";
import { READY_TONE_CLASS, readyAge, useNow } from "@/lib/readyAge";
import { relativeTime } from "@/lib/time";

/// Re-read the clock once a minute, as the review queue's chip does: the
/// coarsest unit printed is minutes, and a colour threshold crossing is
/// at most this late.
const TICK_MS = 60_000;

/// The detail header's dates (#1457): opened, ready for review, last commit.
///
/// Each is OMITTED when it cannot be read -- absent, unparseable, or further
/// in the future than clock skew explains -- never printed as "just now".
/// `readyAge` is the one parser for all three, so they share its skew rule.
///
/// Ready for review takes the review queue's colours, from the same
/// thresholds, so the header agrees with the chip on the row it came from.
/// A draft has no ready time and shows none; the header already says draft.
///
/// "Last commit", never "last push": `committedDate` is the committer's
/// clock, and a rebase or late push leaves it earlier than the push.
export function PrDates({ pr }: { pr: Pick<PrDetail, "created_at" | "ready_at" | "last_commit_at"> }) {
  const now = useNow(TICK_MS);
  const items: { key: string; label: string; iso: string; className?: string }[] = [];

  const opened = readyAge(pr.created_at, now);
  if (opened.since && pr.created_at) {
    items.push({ key: "opened", label: "opened", iso: pr.created_at });
  }
  const ready = readyAge(pr.ready_at, now);
  if (ready.since && pr.ready_at) {
    items.push({
      key: "ready",
      label: "ready for review",
      iso: pr.ready_at,
      className: READY_TONE_CLASS[ready.tone],
    });
  }
  const commit = readyAge(pr.last_commit_at, now);
  if (commit.since && pr.last_commit_at) {
    items.push({ key: "commit", label: "last commit", iso: pr.last_commit_at });
  }

  return (
    <>
      {items.map((it) => (
        <Fragment key={it.key}>
          <span aria-hidden="true">·</span>
          <span data-pr-date={it.key} className={it.className}>
            {it.label}{" "}
            <time dateTime={it.iso} title={new Date(it.iso).toLocaleString()}>
              {relativeTime(it.iso, now)}
            </time>
          </span>
        </Fragment>
      ))}
    </>
  );
}

import type { TaskHealth } from "@/api/tauri";

/// A background loop's own name, as a reader would say it.
///
/// The backend identifiers are stable keys, not prose: "health-sampler"
/// in a sentence reads like a log line, and a user has no reason to know
/// that string.
function label(task: string): string {
  switch (task) {
    case "health-sampler":
      return "The health sampler";
    case "claude-live":
      return "The Claude Code session reader";
    default:
      // Not "Unknown task": a loop added later with no case here should
      // still be reportable, and its key is more useful than a shrug.
      return task;
  }
}

/// How long ago, roughly.
function ago(ms: number, now: number): string {
  const mins = Math.floor((now - ms) / 60_000);
  if (mins < 1) return "less than a minute ago";
  if (mins < 60) return `${mins} minute${mins === 1 ? "" : "s"} ago`;
  const hours = Math.floor(mins / 60);
  return `${hours} hour${hours === 1 ? "" : "s"} ago`;
}

/// What a degraded background loop means for what is on screen (#1145).
///
/// # The gap that reads two ways
///
/// The health sampler writes a row every 60 seconds and, on a failure,
/// logs a warning and carries on -- deliberately, so one broken loop
/// cannot stop the other. Nothing reached the screen, so a gap in the
/// chart could mean the app was closed OR that the sampler ran every
/// minute for an hour and failed to write every time.
///
/// The page said the first. That is #1042's shape exactly: the honest
/// reading and the alarming one look identical, so the user is told the
/// reassuring one.
///
/// This says which. It renders NOTHING while the loops are working,
/// because a permanent "the sampler is fine" line is the kind of caveat
/// nobody reads.
export function BackgroundHealthNotice({
  tasks,
  now,
}: {
  tasks: TaskHealth[] | undefined;
  /// Passed in rather than read from the clock during render, the rule
  /// this page already follows for `scannedAt`.
  now: number;
}) {
  // `degraded` and not `consecutive_failures > 0`: one blip is weather,
  // and the threshold lives in Rust so this and any future notifier
  // cannot disagree about what counts.
  const bad = (tasks ?? []).filter((t) => t.degraded);
  if (bad.length === 0) return null;

  return (
    <div
      className="rounded-md border border-[#f85149]/40 bg-[#f85149]/5 px-3 py-2"
      role="alert"
    >
      {bad.map((t) => (
        <div key={t.task} className="text-xs">
          <p className="font-semibold text-[#f85149]">
            {label(t.task)} has failed {t.consecutive_failures} time
            {t.consecutive_failures === 1 ? "" : "s"} in a row.
          </p>
          {/* The REASON, verbatim. Without it the user knows only that
              something is wrong, which is where they were before. */}
          {t.last_error ? (
            <p className="mt-0.5 break-words text-[#8b949e]">{t.last_error}</p>
          ) : null}
          {/* THE sentence this component exists for. The chart's gap is
              the app's fault, not the user's, and the page otherwise
              tells them the opposite. */}
          <p className="mt-0.5 text-[#8b949e]">
            {t.task === "health-sampler"
              ? "The app has been running, so any gap in the chart below is missing data rather than time the app was closed."
              : "Session history and crash detection have not been updating."}{" "}
            {t.last_success_ms === null
              ? "It has not succeeded once since the app started."
              : `Last succeeded ${ago(t.last_success_ms, now)}.`}
          </p>
        </div>
      ))}
    </div>
  );
}

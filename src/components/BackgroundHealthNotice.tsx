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
    case "claude-index":
      return "The transcript search index";
    case "claude-live":
      return "The Claude Code session reader";
    case "github-poll":
      return "GitHub pull request polling";
    case "github-continuation":
      return "Loading the rest of a long GitHub queue";
    case "gitlab-poll":
      return "GitLab merge request polling";
    case "claude-activity":
      return "Live Claude Code session activity";
    case "jira-refresh":
      return "Jira status refresh";
    default:
      // Not "Unknown task": a loop added later with no case here should
      // still be reportable, and its key is more useful than a shrug.
      return task;
  }
}

/// What a broken loop means for what is on screen, in the reader's terms.
function consequence(task: string): string {
  switch (task) {
    case "health-sampler":
      return "The app has been running, so any gap in the chart below is missing data rather than time the app was closed.";
    case "claude-index":
      return "Transcript search results may be incomplete.";
    case "claude-live":
      return "Session history and crash detection have not been updating.";
    case "github-poll":
    case "gitlab-poll":
      return "Pull request lists and the tray badge may be out of date.";
    case "github-continuation":
      return "A long pull request list may stay partly loaded.";
    case "claude-activity":
      return "Running sessions may be slow to show new activity.";
    case "jira-refresh":
      return "Jira statuses may be out of date.";
    default:
      return "Data it maintains may be out of date.";
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

/// Loops whose parking means a switch the user turned off, so "off" is
/// worth saying. Others park while their source is not set up at all,
/// which a line on this page for every user would only repeat.
const SWITCHED = new Set(["jira-refresh"]);

/// Background loops that are deliberately not running right now (#2036):
/// waiting out a rate limit, or switched off. Neither is a failure, and
/// neither is a stall; without this line both read as "nothing is
/// happening and nobody said why".
export function BackgroundTaskStates({ tasks }: { tasks: TaskHealth[] | undefined }) {
  const lines = (tasks ?? []).flatMap((t) => {
    if (t.retry_at_ms != null) {
      const at = new Date(t.retry_at_ms).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
      return [{ task: t.task, text: `${label(t.task)} is retrying: it was asked to wait, and will try again at ${at}.` }];
    }
    if (t.parked === true && SWITCHED.has(t.task)) {
      return [{ task: t.task, text: `${label(t.task)} is off: it is switched off in Settings.` }];
    }
    return [];
  });
  if (lines.length === 0) return null;
  return (
    <div className="flex flex-col gap-0.5 text-xs text-[#8b949e]">
      {lines.map((l) => (
        <p key={l.task}>{l.text}</p>
      ))}
    </div>
  );
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
          <p className="font-semibold text-[#ff7b72]">
            {/* Stalled and failing are different facts (#1937): a stuck
                loop has failed zero times, and "failed 0 times in a row"
                would read as fine. A stall wins when both hold -- it is
                not running at all now -- and the last error still shows. */}
            {t.stalled
              ? `${label(t.task)} has stopped: it has not finished a cycle ${t.last_tick_ms == null ? "since the app started" : `since ${ago(t.last_tick_ms, now)}`}.`
              : `${label(t.task)} has failed ${t.consecutive_failures} time${t.consecutive_failures === 1 ? "" : "s"} in a row.`}
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
            {consequence(t.task)}{" "}
            {t.last_success_ms === null
              ? "It has not succeeded once since the app started."
              : `Last succeeded ${ago(t.last_success_ms, now)}.`}
          </p>
        </div>
      ))}
    </div>
  );
}

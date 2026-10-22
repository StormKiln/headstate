import { relaunch } from "@tauri-apps/plugin-process";
import { IS_MOBILE_BUILD } from "@/lib/target";
import { notAskedRemedy, signInRemedy } from "@/lib/notAskedRemedy";

/// The panel shown when Headstate DECLINED to issue a request, as
/// distinct from one that was issued and failed.
///
/// # Why this is not `QueryError`
///
/// `QueryError` says something went wrong. Nothing did -- no request was
/// ever constructed -- so this is a status, not an alert. #1050 is the
/// defect it exists for: "a stats load the process declined to issue
/// reported that GitHub had not answered. Both halves were wrong, and the
/// retry offered could not work."
///
/// # Which action, per reason (#2004)
///
/// Whether a retry can work depends on WHY the request was declined, so
/// the action is chosen by `notAskedRemedy`, never fixed:
///
/// - a budget or admission refusal can clear, so "Try again" is offered
///   when the caller has a retry to give;
/// - no credential cannot clear on retry, so the sign-in remedy is
///   offered instead -- `gh auth login`, then a relaunch, which is the
///   only way the token is read again. The desktop gets a button for the
///   relaunch; the phone cannot relaunch its desktop, so it gets the words;
/// - a source that is not set up to be fetched gets no action at all.
///
/// No reset time is offered: none of the declined reasons carries one
/// across the wire today, and guessing one would be a confident number
/// that might be wrong.
export function NotAskedNotice({
  message,
  title,
  onRetry,
}: {
  message: string;
  title?: string;
  onRetry?: () => void;
}) {
  const remedy = notAskedRemedy(message);
  return (
    <div
      role="status"
      className="rounded-md border border-[#30363d] bg-[#161b22] px-4 py-8 text-center"
    >
      <p className="text-sm font-semibold text-[#e6edf3]">
        {title ?? "Headstate has not asked GitHub"}
      </p>
      {/* The reason, then the remedy. The message is the Rust side's own
          prose with the wire marker stripped. */}
      {message ? (
        <p className="mx-auto mt-2 max-w-lg break-words text-sm text-[#8b949e]">{message}</p>
      ) : null}
      <p className="mx-auto mt-2 max-w-lg text-xs text-[#8b949e]">
        {remedy === "sign-in"
          ? `Nothing was sent to GitHub. ${signInRemedy(IS_MOBILE_BUILD)}`
          : "Nothing was sent to GitHub."}
      </p>
      {remedy === "sign-in" && !IS_MOBILE_BUILD ? (
        <div className="mt-4 flex items-center justify-center">
          <button
            type="button"
            onClick={() => void relaunch()}
            className="tap-target rounded border border-[#30363d] px-3 py-1.5 text-sm text-[#e6edf3] hover:bg-[#0d1117]"
          >
            Relaunch Headstate
          </button>
        </div>
      ) : remedy === "retry" && onRetry ? (
        <div className="mt-4 flex items-center justify-center">
          <button
            type="button"
            onClick={onRetry}
            className="tap-target rounded border border-[#30363d] px-3 py-1.5 text-sm text-[#e6edf3] hover:bg-[#0d1117]"
          >
            Try again
          </button>
        </div>
      ) : null}
    </div>
  );
}

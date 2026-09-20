import type { ReactNode } from "react";
import { ReportLink } from "./ReportLink";
import { commandError } from "@/lib/errorKind";
import { NotAskedNotice } from "./NotAskedNotice";

/// The panel shown when a query FAILED, as distinct from returning nothing.
///
/// The distinction is the whole point. A rejected query left `data` at its
/// `[]` default, and the empty-list copy then told the user "no pull
/// requests match these filters" -- a confident, wrong answer to a question
/// the app could not actually answer. An error has to look like an error.
///
/// `message` is the rejection text from the Tauri command, which is already
/// display-ready prose on the Rust side (see `AuthError`'s Display impls);
/// it is rendered verbatim rather than re-wrapped, and never contains the
/// token.
export function QueryError({
  title,
  message,
  onRetry,
  report = false,
  reportView,
  reportDiagnostics,
  children,
}: {
  title: string;
  message?: string;
  onRetry?: () => void;
  /// Offer "Report this" beside the retry (#1148).
  ///
  /// OPT-IN rather than automatic, because not every failure is worth a
  /// bug report: "no network" and "your token expired" are the user's
  /// to fix, and a Report link on those invites issues that can only be
  /// closed with "this is working as intended". The caller knows which
  /// of its failures are surprising; this component does not.
  report?: boolean;
  /// The view to name in the report, when the caller knows it.
  reportView?: string;
  /// Whether diagnostic logging was on, when the caller knows it.
  ///
  /// `undefined` is "not known", which the report omits rather than
  /// printing as "off" (#1042).
  reportDiagnostics?: boolean;
  children?: ReactNode;
}) {
  // DELEGATED here rather than at each of the dozen call sites (#1124).
  // Every page that can show a query failure can also be handed a
  // rejection the app never issued, and teaching each one separately is
  // how half of them would be missed. `QueryError` already receives the
  // only thing the decision needs.
  //
  // Branches on the KIND rather than on the prose (#1230). The marker
  // is stripped by `commandError`, so what reaches the screen is
  // display-ready in both arms -- a wire detail on screen is the failure
  // `cancelled.ts` exists because of.
  const err = message === undefined ? undefined : commandError(message);
  if (err?.kind === "not-asked") {
    return <NotAskedNotice message={err.message} />;
  }
  return (
    <div
      role="alert"
      className="rounded-md border border-[#f85149]/40 bg-[#f85149]/5 px-4 py-8 text-center"
    >
      <p className="text-sm font-semibold text-[#f85149]">{title}</p>
      {/* `err.message`, not `message`: the marker is stripped on BOTH
          arms, so no path through this component can put a wire detail
          on screen. Identical prose for every rejection that carries no
          marker, which is nearly all of them. */}
      {message && err ? (
        <p className="mx-auto mt-2 max-w-lg break-words text-sm text-[#8b949e]">{err.message}</p>
      ) : null}
      {children}
      {onRetry || report ? (
        <div className="mt-4 flex items-center justify-center gap-3">
          {onRetry ? (
            <button
              type="button"
              onClick={onRetry}
              className="tap-target rounded border border-[#30363d] px-3 py-1.5 text-sm text-[#e6edf3] hover:bg-[#161b22]"
            >
              Try again
            </button>
          ) : null}
          {/* Second, always: retrying is the remedy and reporting is
              what you do when it does not work. The order is the order
              to try them in. */}
          {report ? (
            <ReportLink
              error={message ?? title}
              view={reportView}
              diagnostics={reportDiagnostics}
              className="text-sm underline hover:no-underline"
            />
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

/// The same panel, sized for a 256px column.
///
/// `QueryError` is a full-width `px-4 py-8 text-center` block that does not
/// fit a sidebar, which is why four sidebars hand-rolled their own failure
/// arm instead -- and hand-rolled them differently: six "Try again"
/// affordances across the app in three shapes, four bordered buttons at
/// three sizes and two that were bare links with no padding at all (#974).
/// That is a reason for a narrow variant, not for four spellings.
///
/// What it deliberately does NOT do is unify the copy or the handler. The
/// per-surface wording is different CLAIMS about different scans -- "Could
/// not scan for repositories." and "Could not scan for worktrees." and
/// "build output or virtualenvs" are not interchangeable, and a user told
/// only that a scan failed cannot tell whether a group is absent because it
/// failed or because there are none. Likewise `onRetry` is a handler rather
/// than one `refetch`, because `ArtifactSidebar` retries only what actually
/// failed: re-running a 26-second virtualenv scan that succeeded would throw
/// away a result the page is still rendering.
///
/// `tap-target` because a failed scan's retry is the only route out of a
/// failed scan, and at `text-xs` with no padding the link form was roughly a
/// 15px hit area against the 44px minimum `index.css` establishes.
export function NarrowQueryError({
  message,
  onRetry,
}: {
  message: string;
  onRetry: () => void;
}) {
  return (
    <div role="alert" className="px-3 py-2">
      <p className="text-xs text-[#f85149]">{message}</p>
      <button
        type="button"
        onClick={onRetry}
        className="tap-target mt-1 rounded border border-[#30363d] px-2 py-0.5 text-xs text-[#e6edf3] hover:bg-[#161b22]"
      >
        Try again
      </button>
    </div>
  );
}

/// Normalises whatever a rejected Tauri command threw into display prose.
///
/// Tauri surfaces a Rust `Err(String)` as a rejected promise carrying the
/// bare string, but a transport-level failure rejects with an `Error`. Both
/// reach this function, and neither should render as "[object Object]".
export function errorMessage(err: unknown): string | undefined {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  return undefined;
}

import { useMemo, useState } from "react";
import type { ReportContext } from "../lib/report";
import { ReportDialog } from "./ReportDialog";

/// What the CALLER knows that the environment lookups cannot find.
///
/// Passed in rather than read here: the view lives in a store and the
/// diagnostics flag in a query, and the crash panel has neither.
export type ReportExtra = Omit<ReportContext, "error">;

/// "Report this" on an error banner.
///
/// # It must not use a query hook, and that is structural
///
/// `ErrorBoundary` wraps `QueryClientProvider` in `main.tsx` -- on
/// purpose, so a throw inside the provider still lands on a readable
/// screen. This component renders inside that boundary's panel, so a
/// `useQuery` here (or in `ReportDialog`) would throw "No QueryClient
/// set" WHILE RENDERING THE CRASH SCREEN, replacing a readable error
/// with a blank window.
///
/// That is why `diagnostics` is a prop rather than read here: callers
/// that live under the provider pass it, and the crash panel omits it.
/// An omitted value is "unknown" (or the desktop's own answer, once the
/// bundle arrives); a default of `false` would be a claim that logging
/// was off (#1042).
///
/// # A button that opens a preview, not a link that opens a form
///
/// It used to be an anchor straight to a prefilled issue. The user saw
/// the report for the first time on GitHub, and a form that opened with
/// every field empty (#1575) was the whole of what they got. It now
/// opens `ReportDialog`, which shows the exact report, lets the user edit
/// or cut any section, and only then offers "Open on GitHub". The app
/// still never submits.
///
/// The button renders IMMEDIATELY and does not wait on any lookup: an
/// element that appeared only after an IPC call answered was once
/// permanently absent when the call never did, which is what "Report
/// this does nothing" actually was.
export function ReportLink({
  error,
  view,
  diagnostics,
  componentStack,
  className,
}: {
  error: string;
} & ReportExtra & {
    /// Overrides the banner's inline spacing.
    ///
    /// The default `ml-2` is right beside banner text and wrong in a
    /// crash panel, where this is a block-level action rather than a
    /// trailing word (#1148).
    className?: string;
  }) {
  const [open, setOpen] = useState(false);
  // Stable per error, so the dialog's memoised report is not rebuilt on
  // every parent render.
  const context = useMemo(
    () => ({ error, view, diagnostics, componentStack }),
    [error, view, diagnostics, componentStack],
  );

  return (
    <>
      <button
        type="button"
        onClick={() => setOpen(true)}
        aria-haspopup="dialog"
        className={className ?? "ml-2 underline hover:no-underline"}
      >
        Report this
      </button>
      {open ? <ReportDialog context={context} onClose={() => setOpen(false)} /> : null}
    </>
  );
}

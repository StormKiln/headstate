import { ExternalLink } from "./ExternalLink";
import { useEffect, useMemo, useState } from "react";
import { errorOnlyReport, reportUrl, type ReportExtra } from "../lib/reportError";
import { issueUrl } from "../lib/report";

/// "Report this" on an error banner.
///
/// # It must not use a query hook, and that is structural
///
/// `ErrorBoundary` wraps `QueryClientProvider` in `main.tsx` -- on
/// purpose, so a throw inside the provider still lands on a readable
/// screen. This component renders inside that boundary's panel, so a
/// `useQuery` here would throw "No QueryClient set" WHILE RENDERING THE
/// CRASH SCREEN, replacing a readable error with a blank window.
///
/// That is why `diagnostics` is a prop rather than read here: callers
/// that live under the provider pass it, and the crash panel omits it.
/// An omitted line is honest about not knowing; a default of `false`
/// would be a claim that logging was off (#1042).
///
/// An anchor, not a button that opens something: the app reaches the
/// browser through `ExternalLink` everywhere else, and the URL is a
/// prefilled FORM rather than a submission -- the user is the only one
/// who can confirm nothing sensitive survived scrubbing, and filing
/// publicly on someone's behalf without showing them is not something an
/// app should do.
///
/// The URL is built asynchronously (version and platform come from the
/// backend), but the link renders IMMEDIATELY with a URL that carries
/// only the error. It used to render nothing until the lookups
/// resolved, so an IPC call that never answered left the link
/// permanently absent -- which is what "Report this does nothing"
/// actually was: not a dead click, a missing element.
///
/// The richer URL replaces it once the environment is known. Worst case
/// the user files a report without the platform line, which is far
/// better than not being able to file one.
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
  // The context is stable per error, and `useEffect` below depends on
  // it. Built as one object so a caller passing an inline literal does
  // not re-run the lookups on every render.
  const extra = useMemo(
    () => ({ view, diagnostics, componentStack }),
    [view, diagnostics, componentStack],
  );
  // The immediate URL, recomputed during RENDER rather than seeded once
  // in `useState`. The initializer runs on the first render only, and
  // `ErrorBoundary` learns the component stack in `componentDidCatch`,
  // which is after that -- so a seeded-once link would carry the stack
  // only if the environment lookups happened to resolve, and would
  // silently drop the most valuable part of a crash report when they
  // timed out. (Re-seeding from the effect would work too, and is a
  // synchronous `setState` in an effect: a cascading render, which
  // `react-hooks` rejects.)
  //
  // The caller's context is already in hand, unlike the environment, so
  // it goes in here and not only in the resolved URL.
  const immediate = useMemo(() => issueUrl(errorOnlyReport(error, extra)), [error, extra]);
  /// The richer URL, once the environment answers. `null` until then.
  ///
  /// Not cleared when `error` changes: the effect's cleanup sets
  /// `live = false` before the next one runs, so an in-flight lookup
  /// for a PREVIOUS error cannot write here. That guard is load-bearing
  /// and tested -- without it a slow lookup files a report about the
  /// wrong failure.
  ///
  /// Clearing it in the effect instead would be a synchronous
  /// `setState` in an effect, which is a cascading render and which
  /// `react-hooks` rejects.
  const [resolved, setResolved] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    reportUrl(error, extra).then(
      (url) => live && setResolved(url),
      // Non-fatal: the banner still says what went wrong.
      () => {},
    );
    return () => {
      live = false;
    };
  }, [error, extra]);

  const url = resolved ?? immediate;

  return (
    <ExternalLink href={url} className={className ?? "ml-2 underline hover:no-underline"}>
      Report this
    </ExternalLink>
  );
}

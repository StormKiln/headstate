import { getVersion } from "@tauri-apps/api/app";
import { diagnosticBundle } from "../api/tauri";
import type { DiagnosticBundle } from "../types/report";

/// The environment lookups behind "Report this".
///
/// Plain promises, not query hooks: `ReportLink` renders inside
/// `ErrorBoundary`'s crash panel, which sits ABOVE `QueryClientProvider`
/// -- a `useQuery` there would throw while rendering the crash screen.
///
/// Every lookup is best-effort. A missing version is worth far less than
/// a report that never opens, so each resolves to `null` rather than
/// rejecting, and the report says "unknown" for it.
export async function lookupVersion(): Promise<string | null> {
  return settled(getVersion(), null, 2000);
}

/// The desktop's diagnostic bundle, or `null` if it could not be read.
///
/// Five seconds, longer than the version: the bundle runs `gh --version`
/// (bounded at 3s on the Rust side) and, on the phone, crosses the
/// network to the desktop.
export async function lookupBundle(): Promise<DiagnosticBundle | null> {
  return settled(diagnosticBundle(), null, 5000);
}

/// A promise's value, a fallback if it rejects, and a fallback if it
/// never settles at all.
///
/// `.catch` covers rejection but NOT a hang, and an IPC call that never
/// answers left the old report pending forever -- which is what "Report
/// this does nothing" looked like.
function settled<T, F>(p: Promise<T>, fallback: F, ms: number): Promise<T | F> {
  return Promise.race([
    p.catch(() => fallback),
    new Promise<F>((resolve) => setTimeout(() => resolve(fallback), ms)),
  ]);
}

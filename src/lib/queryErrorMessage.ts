// Moved from `QueryError.tsx` so that file exports only components (#1965),
// which is what lets dev hot-reload swap it in place. Not `cancelled.ts`'s
// `errorMessage`, which also drops a dismissed prompt.

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

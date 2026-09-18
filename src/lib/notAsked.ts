/// The marker a command rejects with when Headstate declined to issue a
/// request, rather than issuing one that failed. Must match `NOT_ASKED`
/// in `src-tauri/src/commands.rs`.
///
/// EXPORTED so the agreement can be asserted against that Rust source
/// rather than restated -- `cancelled.ts` records what happens
/// otherwise: three hand-written copies of one marker, a TS test
/// comparing its own copy to the TS one, and a changed Rust constant
/// that left every test on both sides passing while the user saw
/// `headstate:cancelled` in a toast.
export const NOT_ASKED = "headstate:not-asked";

/// Whether this rejection is a question Headstate never asked.
///
/// The distinction is #1050's, and it is the difference between two
/// sentences that are not interchangeable: "GitHub did not answer" and
/// "we did not ask". When `GhClient` holds no client, no request is
/// constructed -- so reporting a failed request is wrong in both halves,
/// and the retry it offers cannot work. Nothing about pressing "Try
/// again" makes a token appear.
export function isNotAsked(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  return message.startsWith(NOT_ASKED);
}

/// The rejection's prose, with the marker stripped.
///
/// The marker is a wire detail and must never reach the screen --
/// `cancelled.ts` exists because a marker did exactly that.
export function notAskedMessage(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  return message.startsWith(NOT_ASKED) ? message.slice(NOT_ASKED.length).trim() : message;
}

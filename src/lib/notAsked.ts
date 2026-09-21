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
///
/// # Why the readers are gone (#1230)
///
/// `isNotAsked` and `notAskedMessage` used to live here, and every
/// consumer asked them whether a rejection's PROSE began with this
/// marker. That is the defect #1202 named: a distinction the Rust side
/// knows, flattened at the boundary, and guessed back from the sentence
/// at each site that needs it.
///
/// The marker itself is NOT the defect and does not move. It is how the
/// classification survives an IPC boundary that carries a command's
/// `Err(String)` as a string, and `commands.rs` is explicit that #1202
/// did not supersede it. What changed is that it is now read in exactly
/// ONE place -- `commandError` in `errorKind.ts`, the client-side twin
/// of Rust's `CommandError::classify` -- which hands every consumer a
/// typed `kind` and a message with the marker already stripped.
///
/// So this file is down to the constant and its cross-language
/// assertion, which is all it was ever uniquely for.
export const NOT_ASKED = "headstate:not-asked";

/// The marker a failure carries when GitHub REJECTED the token we sent,
/// rather than one never being asked for (#1230). Must match
/// `AUTH_EXPIRED` in `src-tauri/src/commands.rs`.
///
/// EXPORTED for the same reason `NOT_ASKED` is: so the agreement is
/// asserted against that Rust source by `mirroredConstants.test.ts`
/// rather than restated here. A hand-written second copy is exactly the
/// drift `cancelled.ts` records, and a marker that has drifted does not
/// fail -- it silently stops classifying, and the remedy quietly
/// disappears from the banner with every test on both sides passing.
///
/// Read in one place only, `commandError` in `errorKind.ts`, which
/// strips it. It must never reach the screen.
export const AUTH_EXPIRED = "headstate:expired-token";

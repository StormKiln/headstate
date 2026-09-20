import { NOT_ASKED } from "./notAsked";

/// The kind of a command rejection, as the remote wire now carries it
/// (#1202).
///
/// EXPORTED as a value, not just a type, so the agreement with Rust can
/// be asserted against that source rather than restated. `cancelled.ts`
/// records what happens otherwise: three hand-written copies of one
/// marker, a TS test comparing its own copy to the TS one, and a changed
/// Rust constant that left every test on both sides passing while the
/// user saw `headstate:cancelled` in a toast.
///
/// `mirroredConstants.test.ts` asserts this list equals the `ErrorKind`
/// variants in `src-tauri/src/remote/error_kind.rs`, in BOTH directions,
/// so adding a kind on either side alone fails the suite.
export const ERROR_KINDS = ["not-asked", "other"] as const;

export type ErrorKind = (typeof ERROR_KINDS)[number];

/// A rejection as the remote wire carries it.
///
/// `message` is the command's own prose, verbatim. A reader that ignores
/// `kind` sees exactly what it saw before this type existed -- which is
/// why the typed channel could be added without changing a single one of
/// the 138 command signatures.
export interface CommandError {
  kind: ErrorKind;
  message: string;
}

/// Whether a value is a wire rejection carrying a kind.
///
/// Deliberately narrow: an unknown `kind` string is NOT accepted. The
/// vocabulary is asserted against Rust, so a value outside it means the
/// two sides have drifted, and guessing at that point is how a false
/// classification reaches the screen. A caller that gets `false` falls
/// back to reading the message, which is today's behaviour.
export function isCommandError(value: unknown): value is CommandError {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  return (
    typeof v.message === "string" &&
    typeof v.kind === "string" &&
    (ERROR_KINDS as readonly string[]).includes(v.kind)
  );
}

/// Whatever a rejected command threw, as a `CommandError` (#1230).
///
/// # Why this classifies here rather than in Rust
///
/// #1202 classified at `surface::res` and put `{kind, message}` on the
/// HTTP wire. That object does NOT reach this code on either transport:
///
/// - On the desktop, `local.ts` calls Tauri's own IPC, which carries a
///   command's `Err(String)` as a bare string. `surface::res` is the
///   REMOTE surface and is never on this path.
/// - On the phone, the companion's Rust proxy parses the desktop's
///   `{kind, message}` body in `client.rs`' `status_message`, returns
///   the `message` alone, and `remote_call` hands the webview a
///   `Result<Value, String>`. The kind is read and dropped one process
///   before the webview.
///
/// So the marker embedded in the prose is still the only classification
/// that crosses to this side, on BOTH transports -- which is exactly why
/// `notAsked.ts` could keep working untouched through #1202. Rejecting
/// a second Rust-side scheme in favour of reading the marker once, here,
/// is the same trade #1202 made: classify at the single point everything
/// converges on, rather than at every consuming site.
///
/// `transport.ts` is that point on this side. Every command on either
/// transport goes through it.
///
/// # The marker is stripped
///
/// `message` is display-ready prose. The marker is a wire detail and
/// must never reach the screen -- `cancelled.ts` exists because one did,
/// and a user saw `headstate:cancelled` in a toast. A caller rendering
/// `err.message` verbatim is therefore correct by construction, which is
/// the property that lets `notAskedMessage` retire.
export function commandError(error: unknown): CommandError {
  // An object that already carries a kind is taken at its word: it came
  // from a producer that classified it, and re-reading its prose could
  // only ever disagree with it. Nothing on today's transports produces
  // one -- see above -- but `isCommandError` is the shape #1202 defined
  // for the wire, and honouring it here is what lets a future producer
  // deliver a kind this side cannot derive from prose at all.
  if (isCommandError(error)) return error;

  const message = error instanceof Error ? error.message : String(error);
  if (message.startsWith(NOT_ASKED)) {
    return { kind: "not-asked", message: message.slice(NOT_ASKED.length).trim() };
  }
  return { kind: "other", message };
}

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

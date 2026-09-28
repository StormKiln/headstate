import { commandError } from "./errorKind";

/// The desktop's refusals of a phone's transcript reads (#1488), as this
/// side recognises them (#1481).
///
/// The listener refuses before dispatch, with `Refusal`'s own sentence
/// (`src-tauri/src/remote/privacy.rs`) as a 403's body; the companion
/// hands the webview that sentence and nothing else. So the sentence's
/// opening is the only thing that crosses -- the same trade `notAsked.ts`
/// makes with its marker -- and `transcriptAccess.test.ts` reads
/// `privacy.rs` and fails if the Rust wording moves away from these.

/// The start of `Refusal::TranscriptsOff`'s message.
export const TRANSCRIPTS_OFF = "This computer does not allow this phone to read session transcripts.";

/// The start of `Refusal::RevealOff`'s message.
export const REVEAL_OFF = "This computer does not allow this phone to reveal hidden text.";

/// Whether a rejection is the desktop saying this phone may not read
/// transcripts at all -- a switch the desktop's owner can turn on, not
/// a failure to read.
export function isTranscriptsOff(error: unknown): boolean {
  return commandError(error).message.startsWith(TRANSCRIPTS_OFF);
}

/// Whether a rejection is the desktop refusing a reveal it does not
/// allow this phone.
export function isRevealOff(error: unknown): boolean {
  return commandError(error).message.startsWith(REVEAL_OFF);
}

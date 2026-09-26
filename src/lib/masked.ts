/// Transcript text that crossed to a phone with its likely secrets
/// masked (#1488).
///
/// The desktop replaces each masked span with a marker before the text
/// leaves it -- `remote/privacy.rs` owns the rule and documents why the
/// marker is inline rather than a span table:
///
/// ```text
/// ⟦hidden:<kind>⟧
/// ```
///
/// This splits a string on those markers so a renderer can draw a
/// "hidden" pill where each one was. Text from the desktop's own window
/// never carries a marker, so on the desktop this is the identity.

import type { TranscriptMasking } from "@/types/pr";

/// The kinds a marker can name. Mirrors `KINDS` in
/// `src-tauri/src/remote/privacy.rs`; `masked.test.ts` reads that file
/// and fails if the two drift.
export const MASK_KINDS = [
  "github-token",
  "private-key",
  "api-key",
  "token",
  "bearer",
  "password",
  "secret",
] as const;

export type MaskKind = (typeof MASK_KINDS)[number];

/// One piece of a masked string: text to print, or a span the desktop
/// hid.
export type MaskedPart = { text: string } | { hidden: MaskKind };

/// Mirrors `MARKER_OPEN` / `MARKER_CLOSE` in `remote/privacy.rs`.
export const MARKER_OPEN = "⟦hidden:";
export const MARKER_CLOSE = "⟧";

const MARKER = new RegExp(`${MARKER_OPEN}(${MASK_KINDS.join("|")})${MARKER_CLOSE}`, "g");

/// What a person reads for each kind, in the pill's title.
export const MASK_LABELS: Record<MaskKind, string> = {
  "github-token": "a GitHub token",
  "private-key": "a private key",
  "api-key": "an API key",
  token: "an access token",
  bearer: "an authorization credential",
  password: "a password",
  secret: "a secret value",
};

/// The sentence a pane shows under masked text, or null when there is
/// nothing to say: the desktop's own answers carry no `masking` at all,
/// and a revealed or clean answer hid nothing. The count comes from the
/// response, never from counting pills, so it covers text the pane did
/// not draw.
export function maskingNote(masking: TranscriptMasking | undefined): string | null {
  if (!masking || masking.revealed || masking.hidden === 0) return null;
  const n = masking.hidden;
  return `${n} likely secret${n === 1 ? " was" : "s were"} hidden before this text left the computer.`;
}

/// Split `text` into printable runs and hidden spans, in order.
///
/// A marker naming a kind this build does not know is left as text: a
/// newer desktop adding a kind must not make an older phone drop part of
/// the transcript.
export function splitMasked(text: string): MaskedPart[] {
  if (!text.includes(MARKER_OPEN)) return [{ text }];
  const parts: MaskedPart[] = [];
  let at = 0;
  for (const m of text.matchAll(MARKER)) {
    const start = m.index;
    if (start > at) parts.push({ text: text.slice(at, start) });
    parts.push({ hidden: m[1] as MaskKind });
    at = start + m[0].length;
  }
  if (at < text.length) parts.push({ text: text.slice(at) });
  return parts;
}

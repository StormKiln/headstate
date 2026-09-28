import { describe, expect, it } from "vitest";
import privacyRs from "../../src-tauri/src/remote/privacy.rs?raw";
import { isRevealOff, isTranscriptsOff, REVEAL_OFF, TRANSCRIPTS_OFF } from "./transcriptAccess";

/// The sentence each `Refusal` variant carries, read out of the Rust
/// source: the webview recognises the refusal by it, so the two must not
/// drift apart (#1481).
function refusalText(variant: string): string {
  const re = new RegExp(`#\\[error\\("([^"]+)"\\)\\]\\s*${variant},`);
  const m = re.exec(privacyRs);
  if (!m) throw new Error(`privacy.rs has no #[error] for Refusal::${variant}`);
  return m[1];
}

describe("the desktop's transcript refusals", () => {
  it("match privacy.rs's Refusal sentences", () => {
    expect(refusalText("TranscriptsOff").startsWith(TRANSCRIPTS_OFF)).toBe(true);
    expect(refusalText("RevealOff").startsWith(REVEAL_OFF)).toBe(true);
  });

  it("recognises each refusal as it arrives, and nothing else as one", () => {
    expect(isTranscriptsOff(refusalText("TranscriptsOff"))).toBe(true);
    expect(isTranscriptsOff(new Error(refusalText("TranscriptsOff")))).toBe(true);
    expect(isTranscriptsOff(refusalText("RevealOff"))).toBe(false);
    expect(isRevealOff(refusalText("RevealOff"))).toBe(true);
    expect(isTranscriptsOff("the desktop did not answer")).toBe(false);
    // Quoting the sentence later on is prose about it, not the refusal.
    expect(isTranscriptsOff(`it said: ${TRANSCRIPTS_OFF}`)).toBe(false);
  });
});

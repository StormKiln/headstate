import { describe, expect, it } from "vitest";

/// #1514: the old transcript preview pane is retired from every host.
///
/// The live paged viewer (#1476, `useClaudeTranscriptLive`) and its two
/// renderers (#1480, #1481) replaced it, and the phone build kept showing
/// the old pane beside them -- two transcript views of one session that
/// behaved differently. This scans every source file under `src/` for the
/// retired names, so a host that brings the old pane (or its data hook,
/// its interim cap, or the three commands only it called) back fails here
/// rather than shipping a second transcript view.
///
/// A glob rather than a list of imports, for `emptyStateGuard.test.ts`'s
/// reason: the file that reintroduces one is the file nobody listed.
///
/// What it cannot see: a reintroduction under a new name. The render
/// tests in `ClaudeCodePage.test.tsx` and `ClaudeCodePage.mobile.test.tsx`
/// assert the pane itself is absent on both builds.
const RETIRED = [
  "TranscriptPreview",
  "PreviewBlock",
  "useClaudeTranscriptFollow",
  "FOLLOW_MAX_MESSAGES",
  "capFollow",
  "call_above_window",
  "claudeTranscriptMessages",
  "claudeTranscriptTail",
  "claudeTranscriptFollow",
  "claude_transcript_messages",
  "claude_transcript_tail",
  "claude_transcript_follow",
];

const SOURCES = import.meta.glob<string>("/src/**/*.{ts,tsx}", {
  query: "?raw",
  import: "default",
  eager: true,
});

describe("the retired transcript preview", () => {
  it("scans a real tree", () => {
    expect(Object.keys(SOURCES).length).toBeGreaterThan(100);
  });

  it("is referenced by no source file", () => {
    const found: string[] = [];
    for (const [file, text] of Object.entries(SOURCES)) {
      if (file.endsWith("/retiredTranscriptPreview.test.ts")) continue;
      for (const name of RETIRED) {
        if (new RegExp(`\\b${name}\\b`).test(text)) found.push(`${file}: ${name}`);
      }
    }
    expect(found).toEqual([]);
  });
});

import { describe, expect, it } from "vitest";
// `?raw`, not `node:fs`: the project carries no `@types/node`
// (`src/raw.d.ts`, `surfaceGuard.test.ts`).
import desktopTsx from "./DesktopTranscript.tsx?raw";
import phoneTsx from "./phone/PhoneTranscript.tsx?raw";

/// Both hosts keep the 7.10 send groundwork wired (#1490, #1491).
///
/// `sendGroundwork.test.tsx` proves the viewer, the composer and the
/// pending model work, but it drives them through its own harness. It
/// does not render either host. While `COMPOSER_ENABLED` is off and
/// nothing creates a pending message, a host that stopped passing
/// `pending`, `renderPending` or `composer` would still render the same
/// thing, so no test that renders a host would notice. Resolving a merge
/// conflict in a host's `<TranscriptViewer>` element is the likely way
/// to lose them.
///
/// This reads each host's source and checks the element. It cannot see
/// whether the element stays mounted in every state. A host must not
/// swap the viewer out for a message (for example, when filters hide
/// every row), because the composer slot lives inside it.

/// The text of the host's `<TranscriptViewer ... />` element: from its
/// opening tag to the line that closes it. An inline child such as
/// `<Composer ... />` closes on its own line, so it does not end the match.
function viewerElement(source: string): string {
  const src = source.replace(/\r\n/g, "\n");
  const start = src.indexOf("<TranscriptViewer\n");
  expect(start, "the host renders <TranscriptViewer>").toBeGreaterThanOrEqual(0);
  const close = /^\s*\/>\s*$/m.exec(src.slice(start));
  expect(close, "the element closes on a line of its own").not.toBeNull();
  return src.slice(start, start + close!.index);
}

describe.each([
  { host: "DesktopTranscript", source: desktopTsx, variant: "desktop" },
  { host: "PhoneTranscript", source: phoneTsx, variant: "phone" },
])("$host keeps the send groundwork wired", ({ source, variant }) => {
  it("holds pending messages, reconciled against the transcript", () => {
    expect(source).toMatch(/usePendingMessages\(/);
  });

  it("passes the pending messages, their renderer and the composer to the viewer", () => {
    const el = viewerElement(source);
    expect(el).toMatch(/\bpending=\{pending\.visible\}/);
    expect(el).toMatch(/\brenderPending=\{/);
    expect(el).toContain(`composer={<Composer variant="${variant}" />}`);
  });
});

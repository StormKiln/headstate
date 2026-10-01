import type { TranscriptMessage } from "../../types/transcript";

type LoadOlderUntil = (wanted: (message: TranscriptMessage) => boolean) => Promise<boolean>;

/// Keep this callback's scope separate from the host render. A callback
/// memoized when hasOlder changes must not retain that render's message
/// list through V8's shared closure context after those pages are evicted.
export function earlierCallLoader(hasOlder: boolean, loadOlderUntil: LoadOlderUntil) {
  if (!hasOlder) return undefined;
  return (toolUseId: string | null): void => {
    void loadOlderUntil((m) =>
      toolUseId === null || m.blocks.some((b) => b.kind === "tool_call" && b.id === toolUseId),
    );
  };
}

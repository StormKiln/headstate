import { MaskedText } from "../MaskedText";
import { TranscriptMarkdown } from "../TranscriptMarkdown";
import { palette } from "./palette";

/// Tool output as the terminal printed it: monospace, wrapped, never
/// reflowed. Errors are red (#1483), and say so in words beside them.
export function MonoOutput({ text, error = false }: { text: string; error?: boolean }) {
  return (
    <pre
      className="mt-0.5 max-h-[60vh] overflow-auto whitespace-pre-wrap break-words rounded p-1.5 font-mono text-[11px]"
      style={{ background: palette.surface, color: error ? palette.error : palette.text }}
    >
      {text === "" ? (
        <span style={{ color: palette.muted }}>(no output)</span>
      ) : (
        // A phone's copy may carry the desktop's masking markers (#1488).
        <MaskedText text={text} />
      )}
    </pre>
  );
}

/// Prose a tool returned -- a subagent's report, a fetched page, search
/// results -- through `TranscriptMarkdown` (#1482): raw HTML as text, no
/// image loaded, links that show where they go. Tool output is exactly
/// the untrusted text that renderer exists for.
export function ProseOutput({ text }: { text: string }) {
  return (
    <div className="mt-0.5 break-words font-sans">
      <TranscriptMarkdown>{text}</TranscriptMarkdown>
    </div>
  );
}

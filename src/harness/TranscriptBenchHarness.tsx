import { useState } from "react";
import { PhoneTranscript } from "../components/transcript/phone/PhoneTranscript";
import { DesktopTranscript } from "../components/transcript/DesktopTranscript";
import type { Liveness } from "../types/pr";

/// The page `transcriptBench.tsx` mounts: an "Open" button, then the
/// transcript host. Its own module because an entry module that declares
/// a component and exports nothing cannot be hot-reloaded (#1965).
export function TranscriptBenchHarness({ path, liveness, sessionId, phone }: { path: string; liveness: Liveness; sessionId: string | null; phone: boolean }) {
  const Host = phone ? PhoneTranscript : DesktopTranscript;
  const [open, setOpen] = useState(false);
  return (
    <div className="flex h-screen flex-col bg-[#0d1117] p-3">
      {!open ? (
        <button type="button" onClick={() => setOpen(true)} className="text-[#e6edf3]">
          Open
        </button>
      ) : (
        <Host path={path} liveness={liveness} sessionId={sessionId} />
      )}
    </div>
  );
}

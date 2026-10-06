import type { Liveness } from "@/types/pr";
import { possibleProcesses } from "@/lib/possibleProcesses";

/** Always visible on touch, display only; never a process-control affordance. */
export function PossibleProcessEvidence({ liveness }: { liveness: Liveness }) {
  if (liveness.state !== "unknown") return null;
  const evidence = possibleProcesses(liveness.possible_processes);
  if (!evidence) return null;
  return <span className="block break-all text-[11px] text-[#8b949e]">
    {evidence.candidates.map((candidate, index) => <span className="block" key={`${candidate.pid}-${index}`}>
      pid {candidate.pid} · {candidate.cwd === null ? "folder unknown" : candidate.cwd}{candidate.cwd_truncated ? "… (folder truncated)" : ""}
    </span>)}
    {evidence.candidates.length < evidence.total ? <span className="block">
      {evidence.candidates.length ? `Showing ${evidence.candidates.length} of ${evidence.total} possible processes` : "Possible process details unavailable"}
    </span> : null}
  </span>;
}

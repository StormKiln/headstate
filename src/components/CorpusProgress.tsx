import type { PopulationProgress } from "../types/pr";

const labels: Record<PopulationProgress["status"], string> = {
  running: "Scan running",
  complete: "Scan complete",
  deferred: "Scan has work remaining",
  cancelled: "Scan cancelled",
  timed_out: "Scan reached its time limit",
  failed: "Scan encountered failures",
  invalidated: "Files changed during the scan",
};

export function CorpusProgress({ progress }: { progress?: PopulationProgress | null }) {
  if (!progress) return null;
  const phase = progress.phase;
  const activity = phase === "discovery" ? "Discovering sessions" : phase === "metadata" ? "Reading session metadata" : phase === "usage" ? "Measuring session usage" : phase === "index" ? "Indexing session content" : "Scan running";
  const unit = phase === "discovery" ? "inspected" : phase === "metadata" ? "stored" : phase === "usage" ? "measured" : phase === "index" ? "indexed" : "current";
  return (
    <span role="status">
      {progress.status === "running" ? activity : labels[progress.status]}. {progress.completed.toLocaleString()} sessions {unit}
      {progress.total_known ? ` of ${progress.total.toLocaleString()}` : "; total not yet known"}.
      {progress.remaining > 0 ? ` ${progress.remaining.toLocaleString()} remaining.` : ""}
      {progress.failed > 0 ? ` ${progress.failed.toLocaleString()} failed.` : ""}
    </span>
  );
}

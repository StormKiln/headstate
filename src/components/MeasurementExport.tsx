import { useEffect, useState } from "react";
import { exportMeasurements, measurementStatus } from "../api/tauri";
import type { MeasurementStatus } from "../types/measurement";

/** Desktop-local measurements; actual producers and phone capture are separate. */
export function MeasurementExport({ diagnosticLogging }: { diagnosticLogging: boolean }) {
  const [status, setStatus] = useState<MeasurementStatus>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<string>();
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let current = true;
    void measurementStatus().then(value => { if (current) { setStatus(value); setError(undefined); } }, () => {
      if (current) setError("Measurement status unavailable. This desktop may need an update.");
    });
    return () => { current = false; };
  }, [diagnosticLogging, attempt]);
  const save = async () => {
    setBusy(true); setResult(undefined); setError(undefined);
    try {
      const receipt = await exportMeasurements();
      setResult(receipt.canceled ? "Save canceled. No report was saved." : `Saved ${receipt.records} measurement records.${receipt.incomplete ? " Coverage is incomplete; review the gaps before drawing conclusions." : ""}`);
      setAttempt(value => value + 1);
    } catch {
      setError("Could not save the measurement report. Finish any open save dialog, then try a writable destination.");
    } finally { setBusy(false); }
  };
  return <section aria-label="Measurement report" className="mt-2 space-y-2 text-xs">
    <p>Measurement reports contain allowlisted counts and timings, with anonymous references. They do not include the detailed log or repository, pull request, or transcript text. Saving does not send the report anywhere.</p>
    {status ? <>
      <p>Measurement capture {status.enabled ? "on" : "off"}. {status.durableRecords} retained records.</p>
      <p>{status.oldestWallMs !== null && status.newestWallMs !== null ? `Retained interval: ${new Date(status.oldestWallMs).toLocaleString()} – ${new Date(status.newestWallMs).toLocaleString()}` : "No durable measurement interval yet."}</p>
      {status.incomplete ? <p role="status">Coverage is incomplete. {status.dropped} dropped, {status.invalid} invalid, {status.rotatedOut} rotated out. Other refusals and gaps are included in the report.</p> : null}
      {status.writerState === "unavailable" ? <p role="alert">Measurement storage is unavailable. Restart Headstate and try again.</p> : null}
    </> : !error ? <p role="status">Reading measurement status…</p> : null}
    <button type="button" disabled={busy || !status || status.writerState === "unavailable"} onClick={() => void save()} className="rounded border border-[#30363d] px-2 py-1 disabled:opacity-50">{busy ? "Saving measurement report…" : "Save measurement report…"}</button>
    {error ? <><p role="alert">{error}</p><button type="button" onClick={() => setAttempt(value => value + 1)}>Retry measurement status</button></> : null}
    {result ? <p role="status">{result}</p> : null}
  </section>;
}

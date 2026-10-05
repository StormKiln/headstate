import { useState } from "react";
import { useDiskInventory } from "../api/useDiskInventory";
import { claudeRevealPath } from "../api/tauri";
import { IS_MOBILE_BUILD } from "../lib/target";
import type { DiskObservation } from "../types/disk";
function size(value: string | null, unit: "GiB" | "GB"): string {
    if (value === null)
        return "Unknown";
    const bytes = BigInt(value);
    const base = unit === "GiB" ? 1073741824n : 1000000000n;
    const tenth = (bytes < 0n ? -bytes : bytes) * 10n / base;
    return `${bytes < 0n ? "-" : ""}${tenth / 10n}.${tenth % 10n} ${unit}`;
}
function percent(part: string | null, total: string | null) { if (part === null || total === null || BigInt(total) <= 0n)
    return null; const tenth = BigInt(part) * 1000n / BigInt(total); return `${tenth / 10n}.${tenth % 10n}%`; }
function time(stamp: number) { return new Date(stamp * 1000).toLocaleString(); }
function Measurement({ observation: o, unit, onReveal }: {
    observation: DiskObservation;
    unit: "GiB" | "GB";
    onReveal: (path: string) => void;
}) {
    return <div className="space-y-3 text-sm">
  <p>{o.volume.label} {o.volume.mount} · {o.volume.scope}</p>
  <p>{o.volume.used === null ? "Used space unavailable" : `${size(o.volume.used, unit)} used`} · {size(o.volume.capacity, unit)} capacity · {size(o.volume.available, unit)} available</p>
  <p>Observed {time(o.started_at)} to {time(o.finished_at)}. {o.approximate ? "Approximate: files may change during measurement." : ""}</p>
  {o.volume.limitation && <p>{o.volume.limitation}</p>}
  <p>{o.measured === null ? "Measured allocation unavailable" : `${size(o.measured, unit)} measured allocation`} · {o.status !== "complete" ? "Partial coverage; measured amounts are lower bounds." : ""}</p>
  <p>{o.remainder === null ? "Unexplained amount unavailable" : BigInt(o.remainder) < 0n ? `${size((-BigInt(o.remainder)).toString(), unit)} over reported usage` : `${size(o.remainder, unit)} unexplained`}</p>
  <p>{o.accounting_note}</p>
  <p>Folder amounts are exclusive contributions to this scan. Overlapping paths and hard links are counted once; filesystem metadata, snapshots and shared storage may remain unexplained.</p>
  {o.categories.map(category => <details key={category.name}><summary>{category.name}</summary><p>{size(category.allocated, unit)} allocated; {size(category.logical, unit)} logical. {percent(category.allocated, o.volume.used) !== null ? `${percent(category.allocated, o.volume.used)} of reported used space` : "Percentage unavailable"}</p>
   {o.locations.filter(l => category.locations.includes(l.id)).map(location => <div key={location.id} className="my-2 rounded border p-2"><p>{location.path}</p>{location.aliases.length > 0 && <p>Also known as: {location.aliases.join(", ")}</p>}<p>{size(location.allocated, unit)} allocated · {size(location.logical, unit)} logical · Reclaimable: {size(location.reclaimable, unit)}</p><p>{location.complete ? "Measured" : "Partially measured"}{location.active ? "; recently written" : ""}. Review required.</p><p>{location.evidence.join("; ")}</p><p>{location.owners.length ? `Associated locations: ${location.owners.join(", ")}` : "Ownership unknown"}</p>{!IS_MOBILE_BUILD && <button type="button" onClick={() => onReveal(location.path)}>Reveal</button>}</div>)}
  </details>)}
  <details open={o.coverage.length > 0}><summary>Coverage and exclusions</summary>{o.coverage.length ? o.coverage.map((c, i) => <p key={`${c.path}:${i}`}>{c.path}: {c.reason}. Unmeasured size is unknown.</p>) : <p>No recorded omissions within selected locations.</p>}</details>
  <div><h3>Growth</h3>{o.comparison.reason ? <p>{o.comparison.reason}</p> : <><p>Change since {time(o.comparison.baseline_at ?? o.started_at)} to {time(o.finished_at)}</p>{o.comparison.changes.map(c => <p key={c.location_id}>{c.path}: {BigInt(c.bytes) > 0n ? "+" : ""}{size(c.bytes, unit)}</p>)}</>}</div>
  {o.history_error && <p role="alert">Current measurement available; history was not saved: {o.history_error}</p>}
 </div>;
}
export function DiskUsagePanel() {
    const [open, setOpen] = useState(false);
    const { status, settings, error, busy, save, start, cancel } = useDiskInventory(open);
    const [selected, setSelected] = useState<string | null>(null);
    const [unit, setUnit] = useState<"GiB" | "GB">("GiB");
    const [draft, setDraft] = useState<string | null>(null);
    const [revealError, setRevealError] = useState<string | null>(null);
    const observations = status?.observations ?? [];
    const observation = observations.find(o => o.volume.id === selected) ?? observations[0];
    return <section className="m-4 rounded border border-border p-3"><button type="button" aria-expanded={open} onClick={() => setOpen(v => !v)}>Disk usage</button>{open && <div className="mt-3 space-y-3">
  <p>Measure selected locations and see what remains unexplained. Scans do not remove files.</p>
  {error && <p role="alert">Disk measurement unavailable: {error}</p>}{status?.error && <p role="alert">{status.error}</p>}{revealError && <p role="alert">{revealError}</p>}
  {!IS_MOBILE_BUILD && <><label className="block">External build and cache folders (one absolute path per line)<textarea value={draft ?? settings?.external_roots.join("\n") ?? ""} onChange={e => setDraft(e.target.value)} disabled={busy || status?.running} className="block w-full rounded border p-2"/></label>
   <button type="button" disabled={busy || status?.running || !settings} onClick={() => void save({ external_roots: (draft ?? settings?.external_roots.join("\n") ?? "").split("\n").map(p => p.trim()).filter(Boolean), additional_locations: settings?.additional_locations ?? false })}>Save locations</button>{" "}
   <button type="button" disabled={busy || status?.running} onClick={() => void start()}>Measure disk usage</button>{" "}<button type="button" disabled={busy || status?.running || !settings} onClick={() => void start(true)}>Scan additional build locations</button>{" "}
   {settings?.additional_locations && <button type="button" disabled={busy || status?.running} onClick={() => void save({ ...settings, additional_locations: false })}>Exclude additional locations from future scans</button>}
   {status?.running && <button type="button" disabled={busy} onClick={() => void cancel()}>Cancel scan</button>}</>}
  {IS_MOBILE_BUILD && <p>Start or stop disk measurements on the desktop.</p>}
  {status?.running && <p role="status">Scanning: {status.visited} entries checked. {status.current_path}</p>}
  <label>Units <select value={unit} onChange={e => setUnit(e.target.value as "GiB" | "GB")}><option>GiB</option><option>GB</option></select></label>
  {observations.length > 1 && <label>Storage scope <select value={observation?.volume.id} onChange={e => setSelected(e.target.value)}>{observations.map(o => <option key={o.volume.id} value={o.volume.id}>{o.volume.label} {o.volume.mount}</option>)}</select></label>}
  {observation ? <Measurement observation={observation} unit={unit} onReveal={path => { setRevealError(null); void claudeRevealPath(path).catch(e => setRevealError(String(e))); }}/> : !status?.running && !error ? <p>No disk measurement yet. Choose locations, then measure disk usage.</p> : null}
 </div>}</section>;
}

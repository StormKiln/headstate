import { useState } from "react";
import {
  usePairedDevices,
  useRevokePairedDevice,
  useSetPairedDeviceAccess,
} from "../api/hooks";
import type { PairedDevice } from "../api/tauri";
import { relativeTime } from "@/lib/time";
import { formatFingerprint } from "@/lib/fingerprint";
import { Dialog, DialogContent, DialogTitle } from "./ui/dialog";

/// The paired-on date, as a date rather than "3 months ago": pairing is
/// a one-off event a person may want to match against a calendar,
/// where last-seen is a recency question.
function pairedOn(iso: string): string {
  return new Date(iso).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

/// One phone's two transcript switches (#1488).
///
/// Transcripts are on and reveal is off by default. Reveal is disabled
/// rather than hidden while transcripts are off: it means nothing then,
/// and a box that vanished would read as a setting that was lost. Its
/// stored value is kept, so turning transcripts back on restores it.
function TranscriptAccess({ device: d }: { device: PairedDevice }) {
  const setAccess = useSetPairedDeviceAccess();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const save = (transcripts: boolean, reveal: boolean) => {
    setBusy(true);
    setError(null);
    setAccess(d.id, transcripts, reveal)
      .catch((e: unknown) =>
        setError(typeof e === "string" ? e : "Could not change this phone's access"),
      )
      .finally(() => setBusy(false));
  };

  return (
    <div className="mt-1 flex flex-col gap-1 text-xs">
      <label className="flex items-start gap-2">
        <input
          type="checkbox"
          className="mt-0.5"
          checked={d.transcripts_allowed}
          disabled={busy}
          onChange={(e) => save(e.target.checked, d.reveal_allowed)}
        />
        <span className="flex flex-col">
          <span className="text-[#e6edf3]">Allow this phone to read session transcripts</span>
          <span className="text-[#8b949e]">
            Claude Code conversation text, transcript search results and opening prompts.
            Likely secrets such as tokens, keys and passwords are hidden before the text
            leaves this computer.
          </span>
        </span>
      </label>
      <label className="flex items-start gap-2">
        <input
          type="checkbox"
          className="mt-0.5"
          checked={d.reveal_allowed}
          disabled={busy || !d.transcripts_allowed}
          onChange={(e) => save(d.transcripts_allowed, e.target.checked)}
        />
        <span className="flex flex-col">
          <span className="text-[#e6edf3]">Allow this phone to reveal hidden text</span>
          <span className="text-[#8b949e]">
            The phone can ask for transcript text with nothing hidden.
          </span>
        </span>
      </label>
      {error ? (
        <p role="alert" className="text-[#f85149]">
          {error}
        </p>
      ) : null}
    </div>
  );
}

/// Settings > Phone > **Paired devices**.
///
/// Revoke asks first. It is the only destructive action on this panel,
/// and the loss is real: the phone has to be paired again, in person,
/// with the QR code. The dialog names the device so a misclick on the
/// wrong row is caught before anything is deleted.
export function PairedDevicesList() {
  const { data: devices, isLoading, error: loadError } = usePairedDevices();
  const revoke = useRevokePairedDevice();
  const [confirming, setConfirming] = useState<PairedDevice | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const confirmRevoke = () => {
    if (!confirming) return;
    setBusy(true);
    setError(null);
    revoke(confirming.id)
      .then(
        () => setConfirming(null),
        (e: unknown) => setError(typeof e === "string" ? e : "Could not revoke this phone"),
      )
      .finally(() => setBusy(false));
  };

  return (
    <div className="mt-5 flex flex-col gap-2">
      <span className="text-sm font-medium">Paired devices</span>
      {isLoading ? (
        <p className="text-xs text-[#8b949e]">Loading…</p>
      ) : loadError ? (
        <p role="alert" className="text-xs text-[#f85149]">
          {typeof loadError === "string" ? loadError : "Could not list paired devices"}
        </p>
      ) : !devices || devices.length === 0 ? (
        <p className="text-xs text-[#8b949e]">No phones paired yet.</p>
      ) : (
        <ul className="flex flex-col divide-y divide-[#30363d] rounded border border-[#30363d]">
          {devices.map((d) => (
            <li key={d.id} className="flex items-center gap-3 px-3 py-2 text-sm">
              <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                <span className="flex items-center gap-2">
                  <span className="truncate font-medium text-[#e6edf3]">{d.name}</span>
                  {/* Which signatures this phone's requests must carry
                      from now on. Shown rather than implied so a
                      pairing that recorded no post-quantum key can be
                      recognised and redone once the phone has one. */}
                  {d.has_mldsa ? (
                    <span
                      title="Paired with a post-quantum signing key (ML-DSA-65)"
                      className="rounded bg-[#1f6feb]/20 px-1.5 py-0.5 text-[10px] text-[#58a6ff]"
                    >
                      post-quantum
                    </span>
                  ) : null}
                </span>
                <span className="text-xs text-[#8b949e]">
                  Paired {pairedOn(d.paired_at)} ·{" "}
                  {d.last_seen === null ? "Never connected" : `Last seen ${relativeTime(d.last_seen)}`}
                </span>
                <code
                  title="Fingerprint (SHA256)"
                  className="truncate font-mono text-[10px] text-[#8b949e]"
                >
                  {formatFingerprint(d.cert_fp)}
                </code>
                <TranscriptAccess device={d} />
              </div>
              <button
                type="button"
                onClick={() => {
                  setError(null);
                  setConfirming(d);
                }}
                aria-label={`Revoke ${d.name}`}
                className="rounded border border-[#f85149]/40 px-2 py-0.5 text-xs text-[#f85149] hover:bg-[#f85149]/10"
              >
                Revoke
              </button>
            </li>
          ))}
        </ul>
      )}

      {confirming ? (
        <Dialog open onOpenChange={(o) => !o && !busy && setConfirming(null)}>
          <DialogContent className="max-w-md">
            <DialogTitle>Revoke {confirming.name}?</DialogTitle>
            <p className="mt-3 text-sm text-[#e6edf3]">
              This phone will be refused on its next connection and any open
              connection is closed now. To use it again you will need to pair it
              again with a new QR code.
            </p>
            {error ? (
              <p role="alert" className="mt-2 text-xs text-[#f85149]">
                {error}
              </p>
            ) : null}
            <div className="mt-5 flex justify-end gap-2">
              <button
                type="button"
                disabled={busy}
                onClick={() => setConfirming(null)}
                className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#21262d] disabled:opacity-50"
              >
                Cancel
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={confirmRevoke}
                className="rounded bg-[#da3633] px-3 py-1.5 text-sm font-medium text-white hover:bg-[#c93c37] disabled:opacity-50"
              >
                {busy ? "Revoking…" : "Revoke"}
              </button>
            </div>
          </DialogContent>
        </Dialog>
      ) : null}
    </div>
  );
}

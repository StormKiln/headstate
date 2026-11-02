import { useEffect, useState } from "react";
import { diskSettings, saveDiskSettings } from "@/api/diskInventory";
import { errorMessage } from "@/lib/queryErrorMessage";
import type { DiskSettings } from "@/types/disk";

/// The disk-usage inventory's external build and cache folders, in
/// Settings → Cleanup. Moved off the Artifacts page, where a configuration
/// box sat above the measurement it configures.
///
/// Desktop only: the phone does not read or write these (`useDiskInventory`
/// skips `diskSettings` on the iOS build), so the host does not render this
/// there.
///
/// Saves only `external_roots`, carrying `additional_locations` through as
/// read, so this box cannot flip the other half of the setting.
export function DiskLocationsSettings() {
  const [settings, setSettings] = useState<DiskSettings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [draft, setDraft] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let live = true;
    diskSettings()
      .then((s) => {
        if (live) {
          setSettings(s);
          setLoadError(null);
        }
      })
      .catch((e) => {
        if (live) setLoadError(errorMessage(e) || "unknown error");
      });
    return () => {
      live = false;
    };
  }, [attempt]);

  const value = draft ?? settings?.external_roots.join("\n") ?? "";
  const save = async () => {
    if (!settings) return;
    const next: DiskSettings = {
      ...settings,
      external_roots: value.split("\n").map((p) => p.trim()).filter(Boolean),
    };
    setBusy(true);
    setSaveError(null);
    try {
      await saveDiskSettings(next);
      setSettings(next);
      setDraft(null);
    } catch (e) {
      setSaveError(errorMessage(e) || "unknown error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="mt-5 flex flex-col gap-1 border-t border-[#30363d] pt-4">
      <label htmlFor="disk-external-roots" className="text-sm font-medium">
        External build and cache folders
      </label>
      <textarea
        id="disk-external-roots"
        value={value}
        onChange={(e) => setDraft(e.target.value)}
        disabled={!settings || busy}
        rows={3}
        spellCheck={false}
        className="rounded border border-[#30363d] bg-[#0d1117] px-2 py-1 font-mono text-sm"
      />
      <p className="text-xs text-[#8b949e]">
        One absolute path per line. Disk usage measures these as well as the repository folders.
      </p>
      {loadError ? (
        <div role="alert" className="text-xs text-[#f85149]">
          Could not load the saved folders: {loadError}
          <button type="button" className="ml-2 underline" onClick={() => setAttempt((n) => n + 1)}>
            Retry loading folders
          </button>
        </div>
      ) : !settings ? (
        <p className="text-xs text-[#8b949e]">Loading the saved folders…</p>
      ) : null}
      {saveError ? (
        <p role="alert" className="text-xs text-[#f85149]">
          {saveError}
        </p>
      ) : null}
      <div>
        <button
          type="button"
          disabled={!settings || busy || draft === null}
          onClick={() => void save()}
          className="rounded border border-[#30363d] px-2 py-1 text-xs disabled:opacity-50"
        >
          Save folders
        </button>
      </div>
    </div>
  );
}

import { useSessionMute } from "@/api/phoneNotify";

/// Mute one session's notifications on this phone (#1486).
///
/// Rendered by the session detail on the mobile build only: the mute is
/// the companion's own setting, kept in its store, and the commands do
/// not exist on the desktop. Muting keeps the session's marks moving, so
/// unmuting announces what happens next rather than what was missed.
export function SessionMuteToggle({ sessionId }: { sessionId: string }) {
  const { muted, set, loaded } = useSessionMute(sessionId);
  return (
    <label className="flex items-center gap-2 text-sm">
      <input
        type="checkbox"
        disabled={!loaded}
        checked={muted ?? false}
        onChange={() => void set(!muted)}
      />
      Mute this session&apos;s notifications on this phone
    </label>
  );
}

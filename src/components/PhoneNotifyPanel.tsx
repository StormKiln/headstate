import { usePhoneNotifyPrefs } from "@/api/phoneNotify";

/// The phone's own notification settings (#789).
///
/// Rendered only on the mobile build -- `SettingsDialog` guards it
/// behind `IS_MOBILE_BUILD`, and a separate component is what keeps
/// `api/phoneNotify` out of the desktop bundle's import graph. The
/// commands it calls do not exist on the desktop, so rendering this
/// there would mean a query that can only ever reject.
///
/// # Why the copy says "that Mac" rather than nothing
///
/// The companion reaches the desktop through the remote surface, so
/// health data on the phone is the DESKTOP's health. An unqualified
/// "battery problems" row here would read as a setting about the phone's
/// own battery, which is a completely different thing and one the user
/// can already see in their status bar. The notifications themselves
/// carry the paired desktop's name (`notify::health_transitions`); this
/// panel has to match, or the setting and the notification it controls
/// appear to be about different machines.
///
/// # Why the best-effort note is not optional
///
/// iOS decides when to grant a background refresh window, so a new pull
/// request surfaces within the hour rather than instantly. A user who
/// expects instant and gets hourly concludes the feature is broken --
/// so the panel says what it actually promises. This is the honest
/// description of local notifications off `BGTaskScheduler`, and #789
/// accepted that tradeoff deliberately rather than building APNs.
export function PhoneNotifyPanel() {
  const { prefs, set } = usePhoneNotifyPrefs();

  // `?? true` everywhere, matching the desktop panel: the value is
  // undefined for one render while the query resolves, and a checkbox
  // that flickers from off to on reads as the app changing a setting by
  // itself. The Rust default is ON, so `true` is what it will be.
  const enabled = prefs?.enabled ?? true;

  return (
    <div className="mt-5 flex flex-col gap-2 border-t border-[#30363d] pt-4">
      <span className="text-sm font-medium">On this phone</span>
      <label className="flex items-center gap-2 text-sm">
        <input
          type="checkbox"
          checked={enabled}
          onChange={() => prefs && void set({ ...prefs, enabled: !prefs.enabled })}
        />
        Phone notifications
      </label>
      {/* Nested and DISABLED rather than hidden when the master switch
          is off, exactly as the desktop panel does it: hiding them makes
          the choices look lost, and they are deliberately preserved so
          turning notifications back on restores what was picked. */}
      <div className="ml-6 flex flex-col gap-2">
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            disabled={!enabled}
            checked={prefs?.new_pr ?? true}
            onChange={() => prefs && void set({ ...prefs, new_pr: !prefs.new_pr })}
          />
          A pull request appears
        </label>
        {/* Both health rows NAME THE MACHINE. The notification does too
            ("Mac mini: Battery at 18%"), and a setting that said only
            "battery problems" would read as being about this phone. */}
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            disabled={!enabled}
            checked={prefs?.health_battery ?? true}
            onChange={() => prefs && void set({ ...prefs, health_battery: !prefs.health_battery })}
          />
          Battery problems on the paired Mac
        </label>
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            disabled={!enabled}
            checked={prefs?.health_cpu ?? true}
            onChange={() => prefs && void set({ ...prefs, health_cpu: !prefs.health_cpu })}
          />
          The paired Mac&apos;s CPU is busy with nothing in particular
        </label>
        {/* #1486. The global switch for Claude Code session notifications;
            a single session is muted from its own detail. */}
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            disabled={!enabled}
            checked={prefs?.sessions ?? true}
            onChange={() => prefs && void set({ ...prefs, sessions: !prefs.sessions })}
          />
          Sessions: a turn finished, one is waiting for you, or one errored
        </label>
        {/* Nested under Sessions and OFF by default: the lock screen
            names the project and the state, and nothing from the
            conversation unless the owner asks for it here. */}
        <label className="ml-6 flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            disabled={!enabled || !(prefs?.sessions ?? true)}
            checked={prefs?.session_snippet ?? false}
            onChange={() =>
              prefs && void set({ ...prefs, session_snippet: !prefs.session_snippet })
            }
          />
          Show the session&apos;s opening prompt in the notification
        </label>
        <p className="ml-6 text-xs text-[#8b949e]">
          Best-effort, delivery can be delayed by iOS. Without the prompt, a session
          notification shows only the project and its state.
        </p>
      </div>
      <p className="text-xs text-[#8b949e]">
        Health notifications are about the Mac you paired with, not this phone, and
        say which machine in the notification itself.
      </p>
      {/* The tradeoff, stated plainly. iOS grants background refresh
          windows when it chooses, so this is "you will find out within
          the hour", not "instantly". */}
      <p className="text-xs text-[#8b949e]">
        Delivered when iOS next wakes the app in the background, so expect these
        within the hour rather than the moment they happen. The first one asks
        permission.
      </p>
    </div>
  );
}

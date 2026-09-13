import { describe, expect, it } from "vitest";
// Vite's `?raw` rather than `node:fs`: the project deliberately
// carries no `@types/node` (see `vite.config.ts`), so a filesystem
// read here would fail `yarn tsc -b`. `?raw` inlines the file at
// transform time and needs no ambient Node types.
import surfaceRs from "../../src-tauri/src/remote/surface.rs?raw";
import tauriTs from "./tauri.ts?raw";

/// The frontend half of the remote surface's safety net.
///
/// `src-tauri/src/remote/surface.rs` has
/// `every_registered_command_has_exactly_one_class`, which guarantees the
/// ALLOWLIST stays complete. Nothing guaranteed the other direction:
/// nothing stopped a component calling a command the desktop classes
/// `Class::Local` and will never forward, so the phone rendered buttons
/// that could only ever reject. That is how #603, #604 and #606 all
/// happened, independently, in the same release.
///
/// This test reads the Rust allowlist directly rather than keeping a
/// second copy of it. A copy would drift, and a drifted copy of a
/// security boundary is worse than none: it would go on passing while
/// describing a surface the desktop no longer has.

/// Commands the desktop refuses to forward, read out of `SURFACE`.
function localCommands(): Set<string> {
  // The table entries are `("name", Class::Local),` one per line. Only
  // the table uses that exact shape; `class_of` and the tests below it
  // spell the class differently.
  const local = new Set<string>();
  for (const m of surfaceRs.matchAll(/\("([a-z0-9_]+)",\s*Class::Local\)/g)) local.add(m[1]);
  return local;
}

/// Every command name the typed wrappers can reach, mapped to the
/// wrapper that reaches it.
function wrappedCommands(): Map<string, string> {
  const found = new Map<string, string>();
  // `export const someName = (...) => call<T>("command_name", ...)`,
  // possibly wrapped across lines by the formatter, so the name and the
  // call are matched separately and paired by position.
  for (const m of tauriTs.matchAll(/export const (\w+)\s*=([\s\S]*?)(?=\nexport |\n\/\/\/|$)/g)) {
    for (const c of m[2].matchAll(/call<[^>]*>\(\s*"([a-z0-9_]+)"/g)) {
      found.set(c[1], m[1]);
    }
  }
  return found;
}

describe("the remote surface's frontend half", () => {
  it("reads a non-empty Class::Local set from surface.rs", () => {
    // Guards the guard: a regex that silently matched nothing would make
    // every assertion below vacuously true.
    const local = localCommands();
    expect(local.size).toBeGreaterThan(5);
    expect(local.has("set_autostart")).toBe(true);
    expect(local.has("issue_pairing_token")).toBe(true);
    // A Read command must not appear in it.
    expect(local.has("get_cached")).toBe(false);
  });

  it("finds the typed wrappers in tauri.ts", () => {
    const wrapped = wrappedCommands();
    expect(wrapped.size).toBeGreaterThan(50);
    expect(wrapped.get("get_cached")).toBeDefined();
  });

  it("names every Class::Local wrapper in the desktop-only list", () => {
    // The list below is the point of the test. A wrapper for a command
    // the desktop will not forward is only safe if it is unreachable
    // from the mobile build, and that is not something this test can
    // check by itself -- so the list is the explicit, reviewed record of
    // which ones exist, and adding to it is a deliberate act.
    //
    // If this fails because a NEW `Class::Local` wrapper appeared: do not
    // just add it here. First make sure whatever calls it is behind
    // `IS_MOBILE_BUILD` (see `src/lib/target.ts`), so the phone never
    // renders a control that can only reject.
    //
    // #626 moved `apply_updates_in_background` off Local once it
    // could be cancelled and its outcome read back after a
    // suspension.
    // If it fails because a command was RECLASSIFIED away from Local
    // (v5.5.0 did that for `assess_worktree`, `pull_checkout`,
    // `docker_start` and `docker_restart`; #625 for `claudify_command`,
    // which only ever built a string), delete its line.
    // Note `diag_log` is absent: it is `Class::Local` but has no
    // wrapper in `tauri.ts` at all, so there is nothing to guard.
    //
    // #915 added the three Claude hook installers. They edit
    // `~/.claude/settings.json`, a config file shared with other tools, and
    // the refusal cases need a human reading an explanation at the machine
    // with the broken file -- so all three are `Class::Local` and the phone
    // cannot reach them at all. `ClaudeIntegrationsPanel` renders the three
    // buttons behind `IS_MOBILE_BUILD`, showing the phone an explanatory
    // sentence in their place; the STATUS read beside them is
    // `claude_hooks_status`, a `Class::Read`, which is why it is absent from
    // this list and visible on both builds.
    const DESKTOP_ONLY_WRAPPERS = [
      "claudeInstallHooks",
      "claudeReinstallHooks",
      "claudeUninstallHooks",
      "getAutostart",
      "getNotifyPrefs",
      "getRemoteEnabled",
      "issuePairingToken",
      "listPairedDevices",
      "respondToPairing",
      "revealLog",
      "revokePairedDevice",
      "setAutostart",
      "setNotifyPrefs",
      "setRemoteEnabled",
      "setWorktreeDirs",
    ].sort();

    const local = localCommands();
    const wrapped = wrappedCommands();
    const actual = [...wrapped.entries()]
      .filter(([command]) => local.has(command))
      .map(([, wrapper]) => wrapper)
      .sort();

    expect(actual).toEqual(DESKTOP_ONLY_WRAPPERS);
  });
});

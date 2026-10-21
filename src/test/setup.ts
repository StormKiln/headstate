import { afterEach, beforeEach, vi } from "vitest";
import { installViewCrashGuard, takeUnexpectedViewCrashes } from "./viewCrashGuard";

/// Shared setup for every test file (`vite.config.ts` `setupFiles`).
///
/// Kept to things that are true of the whole suite. Each block says what
/// it does to a test that did not ask for it, because that is the cost.

/// `listen` with no Tauri IPC installed subscribes to nothing (#1965).
///
/// A test that renders `App` without `mockIPC` -- or after `clearMocks()`
/// -- has no `window.__TAURI_INTERNALS__.transformCallback`, and the real
/// `listen` threw `TypeError: ... transformCallback is not a function`
/// from inside `App`'s notification-click effect. `App` catches that and
/// logs "Could not listen for notification clicks": 76 times per run,
/// none of them about the test that printed it.
///
/// With no IPC there is no backend to emit anything, so an inert
/// subscription is the faithful answer, not a silenced one. Whenever IPC
/// IS installed -- `mockIPC`, with or without `shouldMockEvents` -- the
/// real `listen` runs unchanged, so event tests see exactly what they saw
/// before. A test file's own `vi.mock("@tauri-apps/api/event")` replaces
/// this one.
vi.mock("@tauri-apps/api/event", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@tauri-apps/api/event")>();
  const listen: typeof actual.listen = (event, handler, options) => {
    const internals = (window as { __TAURI_INTERNALS__?: { transformCallback?: unknown } })
      .__TAURI_INTERNALS__;
    if (typeof internals?.transformCallback !== "function") {
      return Promise.resolve(() => {});
    }
    return actual.listen(event, handler, options);
  };
  return { ...actual, listen };
});

let uninstall: (() => void) | undefined;
beforeEach(() => {
  uninstall = installViewCrashGuard();
});
afterEach(() => {
  uninstall?.();
  uninstall = undefined;
  const crashes = takeUnexpectedViewCrashes();
  if (crashes.length > 0) {
    throw new Error(
      `A ViewErrorBoundary caught a render error during this test, which would otherwise ` +
        `pass over the error panel (#1965). Fix the crash (often an incomplete vi.mock), or ` +
        `call expectViewCrash() if the test crashes a view on purpose.\n` +
        crashes.join("\n"),
    );
  }
});

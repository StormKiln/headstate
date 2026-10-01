/// The seam between the typed wrappers in `tauri.ts` and whatever
/// carries them to Rust.
///
/// On the desktop that is the webview's own IPC (`local.ts`). On the
/// mobile companion it is the companion's Rust process (`remote.ts`),
/// which reaches the paired desktop over the network with the same
/// command names and the same event names, so the wrappers and the
/// TanStack Query hooks above this line never learn which one they are
/// on. Nothing else imports `invoke` or `listen` from `@tauri-apps/api`;
/// every command and every poll-loop event goes through here.
///
/// The build picks the transport with `VITE_TARGET` (see
/// `vite.config.ts`, which defaults it to `desktop`). Picking at module
/// load rather than per call keeps the desktop path byte-for-byte what
/// it was: one property lookup, no branch, no promise wrapper.

import { local } from "./local";
import { remote } from "./remote";

/// What `listen` resolves with: call it to stop listening. Shaped
/// exactly like Tauri's own `UnlistenFn`, and re-declared here so the
/// hooks and `unlisten.ts` need no import from `@tauri-apps/api`.
export type UnlistenFn = () => void;

export interface Transport {
  /// Run a Rust command by name. `args` are the command's parameters
  /// under the keys Tauri expects (camelCase, matching `commands.rs`).
  call<T>(name: string, args?: Record<string, unknown>): Promise<T>;
  /// Subscribe to an event by name. The callback sees the payload under
  /// `payload`, as Tauri's does; nothing here reads the other fields.
  listen<T>(event: string, cb: (e: { payload: T }) => void): Promise<UnlistenFn>;
}

// Keep the selection directly on Vite's build constant. Hiding it in a
// function retained the entire remote schema graph in desktop bundles (#711),
// even though that path never validates desktop IPC.
const target = import.meta.env.VITE_TARGET;
if (target !== "desktop" && target !== "mobile") {
  throw new Error(`unknown VITE_TARGET "${target}": expected "desktop" or "mobile"`);
}
const transport = target === "desktop" ? local : remote;

// Plain pass-throughs, NOT `async`: `listen` throws synchronously
// outside a Tauri runtime, and `useUpdateRunOutcome` relies on catching
// that throw. An `async` wrapper would turn it into a rejection and
// change behaviour the tests pin.
export const call: Transport["call"] = (name, args) => transport.call(name, args);
export const listen: Transport["listen"] = (event, cb) => transport.listen(event, cb);

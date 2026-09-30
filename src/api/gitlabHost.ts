// The Rust setting is authoritative because background polling cannot read
// webview storage. The command wrappers live in tauri.ts for the desktop
// reachability invariant; this module groups them for host-aware UI callers.
export { getGitLabHost, setGitLabHost } from "./tauri";

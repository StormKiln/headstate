/// The phone's session notifications (#1486): a compact, content-free
/// status per Claude Code session.
///
/// The phone's background window reads this through the companion's Rust
/// side, which computes the transitions; the webview reaches it only
/// through `claudeSessionDigest` in `tauri.ts`. It carries NO transcript
/// text -- see `claude::digest` on the Rust side.
///
/// The nested interfaces are exported and tagged `@public` for `yarn
/// knip`, for `types/transcript.ts`'s reason: the mirrored-type invariant
/// checks only `export interface` declarations.

/// Mirrors `claude::digest::SessionDigest`.
export interface SessionDigest {
  /// The desktop's clock at the read, RFC 3339.
  as_of: string;
  /// Running sessions first, then the most recently active; bounded.
  sessions: DigestRow[];
  /// How many qualified before the bound. More than `sessions.length`
  /// means rows were dropped.
  total: number;
}

/** @public */
/// Mirrors `claude::digest::DigestRow`.
export interface DigestRow {
  session_id: string;
  /// The working directory's last component. Not transcript text.
  project: string | null;
  liveness: "running" | "dead" | "unknown";
  /// Only while the session is waiting on the user now.
  waiting: DigestWaiting | null;
  /// The turn that most recently ended, while knowable. `null` is "not
  /// known", not "none ended".
  last_turn: LastTurn | null;
}

/** @public */
/// Mirrors `claude::digest::DigestWaiting`.
export interface DigestWaiting {
  /// `idle_prompt` or `permission_prompt`, verbatim.
  kind: string;
  since: string;
}

/** @public */
/// Mirrors `claude::digest::LastTurn`.
export interface LastTurn {
  ended_at: string;
  outcome: { state: "completed" } | { state: "failed"; error_type: string | null };
}

/// Mirrors `claude::sessions::OpeningPrompt`. Transcript text, masked at
/// the remote boundary before it reaches a phone.
export interface OpeningPrompt {
  prompt: string | null;
}

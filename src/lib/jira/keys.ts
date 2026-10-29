/// Jira keys in the frontend (#2035, epic #2031).
///
/// Keys in pull requests, worktrees and branches are found by the desktop
/// (`src-tauri/src/jira/keys.rs`) and read from its index (`links.ts`).
/// What is left here is what a person types.

import type { FoundIn } from "@/types/jira";

export type KeySource = FoundIn;

export interface FoundKey {
  key: string;
  /// Where it was found, shown so a surprising match can be explained.
  source: KeySource;
}

/// What a person typed into a jump box -- `abc-1`, ` ABC-1 `, or a browse
/// URL ending in one -- as a key, or null. No project check: an unknown
/// prefix is reported as such by the caller, not silently dropped.
export function parseTypedKey(input: string): string | null {
  const m = /(?:^|\/browse\/)\s*([A-Za-z][A-Za-z0-9_]{1,9})-([1-9][0-9]{0,8})\s*(?:[?#].*)?$/.exec(input.trim());
  return m ? `${m[1].toUpperCase()}-${m[2]}` : null;
}

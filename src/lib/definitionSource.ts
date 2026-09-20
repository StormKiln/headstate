import type { ClaudeDefinitionSource } from "../api/tauri";

/// A short label for a definition's scope (#1215).
///
/// Lives here rather than in `api/tauri.ts` because that module's
/// exports are transport wrappers and nothing else -- an invariant
/// `transport.test.ts` enforces by requiring a row for every exported
/// function.
///
/// A project is labelled by its DIRECTORY NAME rather than its full
/// path: across ~38 repositories a wall of absolute paths is not
/// readable, and the full path stays available as the badge's `title`
/// so nothing is actually lost.
export function definitionSourceLabel(s: ClaudeDefinitionSource): string {
  if (s.scope === "user") return "~/.claude";
  if (s.scope === "plugin") return s.name;
  return s.path.split("/").filter(Boolean).pop() ?? s.path;
}

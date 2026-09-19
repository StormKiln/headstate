import type { CleanupPrefs } from "@/types/pr";

/// A cleanup category and the specific things it may propose.
///
/// Grouped because the categories are not comparable: "branches" and
/// "artifacts" are different kinds of thing, and a flat list of seven
/// checkboxes made that invisible.
export interface CleanupGroup {
  /// The parent field.
  key: keyof CleanupPrefs;
  label: string;
  /// Whether the automatic pass actually acts on this yet.
  ///
  /// `propose` implements artifacts, virtualenvs, worktrees and
  /// branches (#1141); Docker is stored and shown but does nothing on a
  /// timer. Said in the UI rather than left to look functional — a
  /// setting that silently does nothing is worse than one that is
  /// honestly not ready.
  ///
  /// The label has to stop claiming "pending" the MOMENT it stops being
  /// true, which is why this moves in the same change as the proposer
  /// rather than after it.
  pending?: true;
  /// The children, each a field of its own.
  children: { key: keyof CleanupPrefs; label: string; hint: string }[];
}

/// Every category the automatic pass can act on.
///
/// A child is a SEPARATE claim about what may be deleted without a
/// human present, never a detail of its parent — which is why each one
/// has its own field rather than the parent carrying a mode.
export const CLEANUP_GROUPS: CleanupGroup[] = [
  {
    key: "artifacts",
    label: "Build artifacts",
    children: [],
  },
  {
    key: "venvs",
    label: "Virtualenvs",
    children: [
      {
        key: "venvs_stale",
        label: "Also stale ones",
        hint: "An orphan is a fact — nothing hashes to it. Stale is a threshold about a project that still exists.",
      },
    ],
  },
  {
    key: "branches",
    label: "Merged branches",
    children: [
      {
        key: "branches_ancestor",
        label: "Merged by ancestry",
        hint: "The tip is reachable from the default branch. A graph fact.",
      },
      {
        key: "branches_squash",
        label: "Merged by squash",
        hint: "Found by comparing patch-ids — a content comparison, not a graph one. Most merged branches are these.",
      },
    ],
  },
  {
    key: "worktrees",
    label: "Merged worktrees",
    children: [
      {
        key: "worktrees_safe",
        label: "Merged, clean, and pushed",
        hint: "Nothing is lost by removing one.",
      },
    ],
  },
  {
    key: "docker",
    pending: true,
    label: "Docker images",
    children: [
      {
        key: "docker_dangling",
        label: "Dangling images",
        hint: "Untagged and referenced by nothing.",
      },
    ],
  },
];

/// Whether a parent is on, off, or partly on.
///
/// `mixed` matters: a parent that rendered as plain "on" while only
/// some children were would misstate what the unattended pass will do,
/// which is the one thing these controls exist to be precise about.
export type ParentState = "on" | "off" | "mixed";

export function parentState(prefs: CleanupPrefs, g: CleanupGroup): ParentState {
  if (!prefs[g.key]) return "off";
  if (g.children.length === 0) return "on";
  const on = g.children.filter((c) => prefs[c.key]).length;
  if (on === 0) return "off";
  return on === g.children.length ? "on" : "mixed";
}

/// Ticking a parent ticks everything under it; unticking unticks them.
///
/// Returns the fields to change rather than mutating, so the caller
/// persists once instead of once per checkbox.
export function toggleParent(
  prefs: CleanupPrefs,
  g: CleanupGroup,
): Partial<CleanupPrefs> {
  // A mixed parent turns everything ON: the user clicking it wants
  // more, not less. Turning the ticked ones off would be a surprising
  // way to answer a click meaning "all of these".
  const next = parentState(prefs, g) !== "on";
  const out: Partial<CleanupPrefs> = { [g.key]: next } as Partial<CleanupPrefs>;
  for (const c of g.children) {
    (out as Record<string, boolean>)[c.key as string] = next;
  }
  return out;
}

/// Ticking a child implies its parent; unticking the last one clears it.
///
/// Without this a user could tick a child under an off parent and see
/// nothing happen, because the pass reads the parent.
export function toggleChild(
  prefs: CleanupPrefs,
  g: CleanupGroup,
  childKey: keyof CleanupPrefs,
): Partial<CleanupPrefs> {
  const next = !prefs[childKey];
  const out: Record<string, boolean> = { [childKey as string]: next };
  const anyOn = g.children.some((c) => (c.key === childKey ? next : prefs[c.key]));
  out[g.key as string] = anyOn;
  return out as Partial<CleanupPrefs>;
}

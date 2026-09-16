import type { ClaudeAgentTypes } from "@/types/pr";

/// How many auto-compactions read as sustained context pressure.
///
/// MUST equal `AUTO_COMPACT_PRESSURE` in
/// `src-tauri/src/claude/signals.rs`, which is what actually sets
/// `ClaudeSession.context_pressure`. This copy is what the frontend words
/// the marker AGAINST -- "compacted automatically several times" has to
/// mean the same thing in both languages or the row explains a flag the
/// backend raised on a different rule.
///
/// Pinned by `mirroredConstants.test.ts` rather than left to whoever
/// edits one of them next, which is #850's whole finding: a TS test using
/// this symbolically is self-consistent at any value, so only a test that
/// reads the Rust LITERAL can fail when one side moves.
///
/// Two, not one. A single auto-compaction is an ordinary long session and
/// flagging it would badge a large share of real work; the signal #1065
/// wants is the REPEATED case.
export const AUTO_COMPACT_PRESSURE = 2;

/// Spawns the hook observed for a session, across all types.
///
/// The TypeScript twin of `AgentTypes::spawns`. Counts EVENTS, so an
/// untyped spawn is still a spawn: the hook fired and did not name a
/// type, which is a thing we saw rather than a thing we did not.
export function statedSpawns(types: ClaudeAgentTypes): number {
  return types.untyped + types.stated.reduce((sum, [, n]) => sum + n, 0);
}

/// The sentence to show when the hook and the directory rule do not agree
/// about whether this session spawned subagents (#1066), or `null` when
/// they do.
///
/// # Why this is computed here and not received
///
/// Rust has this exact logic in `AgentTypes::disagreement`, and it is a
/// METHOD rather than a field -- so it is never serialised and the pane
/// cannot simply render what it was sent. The choice is between
/// re-deriving it and inventing a second rule inline in JSX; this file is
/// the first, with its own test, so that the two languages can be read
/// against each other and the asymmetry below is stated once.
///
/// # Only ONE direction is a disagreement
///
/// Hook spawns with no inferred children IS one: the session told us it
/// spawned subagents and the cwd rule found none of them, so they ran
/// somewhere the rule does not look. That is a finding about the
/// inference, and #1066 asks for it to be surfaced rather than silently
/// resolved -- picking a winner would hide the one signal saying the
/// directory assumption has moved.
///
/// Inferred children with no hook spawns is NOT one, and this asymmetry
/// is the whole reason the check does not cry wolf. It is the expected
/// state for every session that predates the install -- which is all of
/// them today -- and for any session that ran while the hook was
/// uninstalled. Reporting it would flag the entire corpus as
/// contradictory on the day this ships, and a guard that fires everywhere
/// is one that gets turned off.
///
/// `null` therefore covers three genuinely-agreeing situations as well as
/// the pre-hook one: both sources found subagents, neither did, or the
/// hook has nothing to say.
export function subagentDisagreement(types: ClaudeAgentTypes | null): string | null {
  if (types === null) return null;
  const spawns = statedSpawns(types);
  if (spawns > 0 && types.inferred_children === 0) {
    return `The hook recorded ${spawns.toLocaleString()} subagent ${
      spawns === 1 ? "start" : "starts"
    } for this session, but none of its subagents were found by their working directory. Subagents may be running somewhere the directory rule does not recognise.`;
  }
  return null;
}

import { useState } from "react";
import { useActiveFilters } from "@/store/filters";
import { useClaudeEffectiveSettings } from "@/api/hooks";
import type { ClaudeResolvedKey, ClaudeSettingsOrigin } from "@/api/tauri";

/// Which file a value came from, in words.
///
/// The PATH shape rather than a label like "user": a reader debugging a
/// denial needs to know which file to open, and "project" does not say
/// `.claude/settings.json` in the repository root.
const ORIGIN_LABEL: Record<ClaudeSettingsOrigin, string> = {
  user: "~/.claude/settings.json",
  project: ".claude/settings.json",
  local: ".claude/settings.local.json",
  // `plugin` is the scope `Origin` grew for the MCP inventory (#1216),
  // and `claude_effective_settings` never returns it -- no settings file
  // yields a plugin scope. The row is here because the table is total
  // over the union, and it names the real thing rather than carrying a
  // placeholder: if it ever DID render, "a plugin's .mcp.json" is true,
  // whereas an empty string or "unknown" would be a silent hole.
  plugin: "a plugin's .mcp.json",
};

/// What Claude Code actually reads for this repository (#1130).
///
/// `events.rs` records every `PermissionDenied` and tallies it by tool,
/// so the EFFECT of a rule was visible while the rule itself was not.
/// A user debugging "why did that tool get denied" had to open three
/// files and merge them mentally.
///
/// Collapsed by default: three file reads for a question asked when
/// something is wrong, not on every visit to Settings.
export function EffectiveSettingsPanel({ enabled }: { enabled: boolean }) {
  const [open, setOpen] = useState(false);
  const repo = useActiveFilters().repo;
  const { data, error } = useClaudeEffectiveSettings(repo, enabled && open);

  if (!enabled) return null;

  return (
    <div className="mt-2 border-t border-[#21262d] pt-2">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        className="tap-target text-xs text-[#58a6ff] hover:underline"
      >
        {open ? "Hide" : "Show"} the settings Claude Code reads here
      </button>
      {!open ? null : !repo ? (
        // A repository is the SUBJECT of two of the three scopes, so
        // without one there is no question to answer. Saying so beats
        // rendering the user scope alone and calling it effective.
        <p className="mt-2 text-xs text-[#8b949e]">
          Pick a repository to see which of its settings files apply.
        </p>
      ) : error ? (
        <p role="alert" className="mt-2 text-xs text-[#f85149]">
          {String(error instanceof Error ? error.message : error)}
        </p>
      ) : !data ? (
        <p className="mt-2 text-xs text-[#8b949e]">Reading…</p>
      ) : (
        <Resolved data={data} />
      )}
    </div>
  );
}

function Resolved({
  data,
}: {
  data: { keys: ClaudeResolvedKey[]; unreadable: { path: string; detail: string }[] };
}) {
  return (
    <div className="mt-2 space-y-2">
      {/* Refusals FIRST and in their own colour. A file Claude Code
          cannot parse is one it ignores entirely, so every rule in it is
          already doing nothing -- which is the more urgent fact than any
          value below it. */}
      {data.unreadable.map((r) => (
        <p key={r.path} role="alert" className="text-xs text-[#f85149]">
          {r.detail}
        </p>
      ))}

      {data.keys.length === 0 ? (
        <p className="text-xs text-[#8b949e]">
          None of the tracked settings are set for this repository.
        </p>
      ) : (
        data.keys.map((k) => (
          <div key={k.key}>
            <p className="text-xs font-semibold text-[#e6edf3]">{k.key}</p>
            {k.undecidable ? (
              // NOT the lower scope's value. A scope that outranks the
              // best readable one could not be parsed, so what it
              // carried is unknown -- and printing the loser as the
              // winner would be a confident wrong answer.
              <p className="text-[11px] text-[#d29922]">
                Cannot say which value wins: a file that would override this could not be
                read.
              </p>
            ) : null}
            <ul className="mt-0.5 space-y-0.5">
              {k.contributions.map((c) => {
                const wins = !k.undecidable && k.winner?.origin === c.origin;
                return (
                  <li
                    key={c.origin}
                    className={`text-[11px] ${wins ? "text-[#e6edf3]" : "text-[#6e7681]"}`}
                  >
                    {/* The winner is marked in TEXT as well as by
                        colour: colour alone is not an answer for a
                        reader who cannot see it. */}
                    <span className="mr-1">{wins ? "[applies]" : "[overridden]"}</span>
                    <span className="mr-1">{ORIGIN_LABEL[c.origin]}</span>
                    <code className="break-all">{JSON.stringify(c.value)}</code>
                  </li>
                );
              })}
            </ul>
          </div>
        ))
      )}
    </div>
  );
}

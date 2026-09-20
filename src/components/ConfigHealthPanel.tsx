import { useState } from "react";
import { useClaudeConfigHealth } from "@/api/hooks";
import type {
  ClaudeHealthFinding,
  ClaudeRepoHealth,
  ClaudeSettingsOrigin,
} from "@/api/tauri";

/// Which file a settings finding is about, as a PATH.
///
/// The same table `EffectiveSettingsPanel` keeps and for the same
/// reason: a reader whose local scope refused needs to know which file to
/// open, and "local" does not say `.claude/settings.local.json`.
const ORIGIN_LABEL: Record<ClaudeSettingsOrigin, string> = {
  user: "~/.claude/settings.json",
  project: ".claude/settings.json",
  local: ".claude/settings.local.json",
  plugin: "a plugin's .mcp.json",
};

/// What each verdict is called, and how it is coloured.
///
/// `unknown` gets its own word and its own colour, never a muted version
/// of `pass`. That is the whole of #1042 on this surface: "we could not
/// check" rendered as a quiet pass is how an unchecked repository becomes
/// a cleared one in a reader's head.
const VERDICT: Record<
  ClaudeRepoHealth["verdict"],
  { label: string; className: string }
> = {
  problem: { label: "broken", className: "text-[#f85149]" },
  unknown: { label: "could not check", className: "text-[#d29922]" },
  pass: { label: "checked, clean", className: "text-[#3fb950]" },
};

/// Silently-broken agent configuration, across every scanned repository
/// (#1217).
///
/// `install.rs` measured the failure this page exists for: a corrupt
/// `settings.json` produced a Claude Code that started normally, answered
/// normally, and never ran a hook — no error, no warning, nothing on
/// stderr. That was detected once, for one file, as a side effect of the
/// install dialog reading it. Nothing asked the same question of the
/// other ~38 checkouts.
///
/// Every row here states a PROOF the backend already had: a parse error
/// with its line and column, or an import resolver's own message. There
/// is no heuristic and no style judgement, because a health page that
/// mixes those with real failures teaches its reader to skim both — and
/// an ignored health page is worse than none, since it looks like
/// coverage.
///
/// Collapsed by default: the sweep parses settings and walks CLAUDE.md
/// trees across every repository, which is not a cost to pay on every
/// visit to Settings.
export function ConfigHealthPanel({ enabled }: { enabled: boolean }) {
  const [open, setOpen] = useState(false);
  const { data, error, isFetching, refetch } = useClaudeConfigHealth(enabled && open);

  // Hooks run unconditionally, so the early return sits below all of
  // them. Nothing here is computed in an effect: the counts are derived
  // during render from `data`, which is the only way they cannot lag
  // behind it.
  if (!enabled) return null;

  return (
    <div className="mt-2 border-t border-[#21262d] pt-2">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        className="tap-target text-xs text-[#58a6ff] hover:underline"
      >
        {open ? "Hide" : "Show"} configuration health across your repositories
      </button>
      {!open ? null : error ? (
        <p role="alert" className="mt-2 text-xs text-[#f85149]">
          {String(error instanceof Error ? error.message : error)}
        </p>
      ) : !data ? (
        <p className="mt-2 text-xs text-[#8b949e]">Sweeping…</p>
      ) : (
        <Sweep data={data} isFetching={isFetching} onRecheck={() => void refetch()} />
      )}
    </div>
  );
}

function Sweep({
  data,
  isFetching,
  onRecheck,
}: {
  data: {
    repos: ClaudeRepoHealth[];
    unreadableRoots: string[];
    userFindings: ClaudeHealthFinding[];
  };
  isFetching: boolean;
  onRecheck: () => void;
}) {
  const problems = data.repos.filter((r) => r.verdict === "problem");
  const unknowns = data.repos.filter((r) => r.verdict === "unknown");
  const passes = data.repos.filter((r) => r.verdict === "pass");

  return (
    <div className="mt-2 space-y-2">
      {/* The three counts stated SEPARATELY. An "unknown" folded into the
          passes would overstate coverage and folded into the problems
          would cry wolf, so it gets its own number. */}
      <p className="text-xs text-[#8b949e]">
        <span className="text-[#f85149]">{problems.length} broken</span>
        {" · "}
        <span className="text-[#d29922]">{unknowns.length} could not be checked</span>
        {" · "}
        <span className="text-[#3fb950]">{passes.length} checked and clean</span>
      </p>

      {/* The shortfall in the CENSUS, not in any one repository. Without
          it "N clean" is a claim about a machine only half looked at. */}
      {data.unreadableRoots.length > 0 ? (
        <div role="alert" className="text-xs text-[#d29922]">
          <p>
            Some scan roots could not be read, so this list may be missing
            repositories entirely:
          </p>
          <ul className="mt-0.5 space-y-0.5">
            {data.unreadableRoots.map((r) => (
              <li key={r} className="break-all text-[11px]">
                {r}
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      {data.userFindings.length > 0 ? (
        <div>
          <p className="text-xs font-semibold text-[#e6edf3]">
            ~/.claude — loaded into every session on this machine
          </p>
          <ul className="mt-0.5 space-y-1">
            {data.userFindings.map((f) => (
              <FindingRow key={`${f.check}:${f.path}`} finding={f} />
            ))}
          </ul>
        </div>
      ) : null}

      {/* Ranked worst first by the backend and rendered in that order.
          Re-sorting here would be a second ordering to keep in step with
          the one `rank` documents. */}
      {data.repos.length === 0 ? (
        <p className="text-xs text-[#8b949e]">
          No repositories were scanned, so nothing was checked.
        </p>
      ) : (
        <ul className="space-y-2">
          {data.repos
            .filter((r) => r.verdict !== "pass")
            .map((r) => (
              <RepoRow key={r.path} repo={r} />
            ))}
        </ul>
      )}

      {problems.length === 0 && unknowns.length === 0 && data.repos.length > 0 ? (
        <p className="text-xs text-[#3fb950]">
          Every settings file parsed and every CLAUDE.md import resolved.
        </p>
      ) : null}

      <button
        type="button"
        onClick={onRecheck}
        disabled={isFetching}
        className="tap-target text-xs text-[#58a6ff] hover:underline disabled:text-[#6e7681]"
      >
        {isFetching ? "Re-checking…" : "Re-check"}
      </button>
    </div>
  );
}

function RepoRow({ repo }: { repo: ClaudeRepoHealth }) {
  const verdict = VERDICT[repo.verdict];
  return (
    <li>
      <p className="text-xs">
        <span className="font-semibold text-[#e6edf3]">{repo.name}</span>{" "}
        {/* The verdict in TEXT as well as in colour: colour alone is not
            an answer for a reader who cannot see it. */}
        <span className={verdict.className}>[{verdict.label}]</span>
      </p>
      <ul className="mt-0.5 space-y-1">
        {repo.findings.map((f) => (
          <FindingRow key={`${f.check}:${f.path}:${f.proof}`} finding={f} />
        ))}
      </ul>
    </li>
  );
}

function FindingRow({ finding }: { finding: ClaudeHealthFinding }) {
  return (
    <li
      className={`text-[11px] ${
        finding.severity === "problem" ? "text-[#f85149]" : "text-[#d29922]"
      }`}
    >
      {/* WHICH scope, named rather than flattened. The remedy is one
          specific file, and a repository whose local scope alone refused
          is not simply "unhealthy" — that distinction is what settings.rs
          was built to preserve. */}
      {finding.scope !== null ? (
        <span className="mr-1 text-[#8b949e]">{ORIGIN_LABEL[finding.scope]}</span>
      ) : null}
      {/* The proof VERBATIM. It carries the parse error's line and column,
          which is the only actionable thing in the whole finding, and
          paraphrasing it here would turn a check into an opinion. */}
      <span className="break-all">{finding.proof}</span>
      {finding.undecidableKeys.length > 0 ? (
        // The consequence, which is why naming the scope matters: with
        // that file unreadable, Headstate cannot say which of these is in
        // force — and neither can Claude Code.
        <span className="ml-1 text-[#8b949e]">
          (so {finding.undecidableKeys.join(", ")} cannot be resolved)
        </span>
      ) : null}
    </li>
  );
}

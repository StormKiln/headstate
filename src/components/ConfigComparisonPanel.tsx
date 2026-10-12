import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useWorktrees } from "@/api/hooks";
import { claudeCompareConfig, type ClaudeConfigCell, type ClaudeConfigComparison } from "@/api/tauri";
import { useActiveFilters } from "@/store/filters";

const statuses = {
  different: "Different", onlyLeft: "Only on the left", onlyRight: "Only on the right",
  cannotCompare: "Cannot compare", same: "Same",
} as const;
const categories = { settings: "Settings", hooks: "Hooks", definitions: "Definitions", claudeMd: "CLAUDE.md" };
const control = "tap-target rounded border border-[#30363d] bg-[#0d1117] px-2 py-1 text-xs text-[#e6edf3]";

export function ConfigComparisonPanel({ enabled }: { enabled: boolean }) {
  const [open, setOpen] = useState(false);
  const active = useActiveFilters().repo;
  const [left, setLeft] = useState(active ?? "");
  const [right, setRight] = useState("");
  if (!enabled) return null;
  return <section className="mt-2 border-t border-[#21262d] pt-2">
    <button type="button" aria-expanded={open} onClick={() => setOpen(!open)} className="tap-target text-xs text-[#58a6ff] hover:underline">
      Compare repository configurations
    </button>
    {open ? <Comparison left={left} right={right} setLeft={setLeft} setRight={setRight} /> : null}
  </section>;
}

function Comparison({ left, right, setLeft, setRight }: {
  left: string; right: string; setLeft: (value: string) => void; setRight: (value: string) => void;
}) {
  const repos = useWorktrees();
  const query = useQuery({
    queryKey: ["claude-config-comparison", left, right],
    queryFn: () => claudeCompareConfig(left, right || null),
    enabled: Boolean(left) && left !== right,
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
    retry: false,
  });
  const options = new Map((repos.data ?? []).map(r => [r.path, r.name]));
  if (left && !options.has(left)) options.set(left, left);
  if (right && !options.has(right)) options.set(right, right);
  return <div className="mt-2 space-y-3">
    <p className="text-xs text-[#8b949e]">Compare checkout configuration with another repository or your user configuration. Use Refresh after editing files.</p>
    <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
      <label className="flex min-w-0 flex-1 flex-col gap-1 text-xs">Repository
        <select className={control} value={left} onChange={e => setLeft(e.target.value)}>
          <option value="">Select a repository</option>
          {[...options].map(([path, name]) => <option key={path} value={path}>{name} — {path}</option>)}
        </select>
      </label>
      <label className="flex min-w-0 flex-1 flex-col gap-1 text-xs">Compare with
        <select className={control} value={right} onChange={e => setRight(e.target.value)}>
          <option value="">User configuration</option>
          {[...options].map(([path, name]) => <option key={path} value={path}>{name} — {path}</option>)}
        </select>
      </label>
    </div>
    {repos.error ? <p role="alert" className="text-xs text-[#d29922]">Could not list repositories: {String(repos.error)}</p> : null}
    {repos.unreadable.map(detail => <p key={detail} role="alert" className="text-xs text-[#d29922]">{detail}</p>)}
    {left === right && left ? <p className="text-xs">Choose two different repositories.</p> : !left ? <p className="text-xs">Select a repository to compare.</p> : <>
      <button type="button" className={control} disabled={query.isFetching} onClick={() => void query.refetch()}>Refresh comparison</button>
      {query.isError ? <p role="alert" className="text-xs text-[#d29922]">{String(query.error)}{query.data ? " Showing the previous comparison; refresh to try again." : " Refresh to try again."}</p> : null}
      {!query.data && query.isFetching ? <p className="text-xs">Reading configurations…</p> : null}
      {query.data ? <Results data={query.data} /> : null}
    </>}
  </div>;
}

function Results({ data }: { data: ClaudeConfigComparison }) {
  return <div className="space-y-3">
    <p className="break-all text-xs">{data.leftLabel} compared with {data.rightLabel}</p>
    {data.limitations.map(detail => <p key={detail} role="alert" className="text-xs text-[#d29922]">{detail}</p>)}
    {data.rows.length === 0 ? <p className="text-xs">No comparable configuration entries were found.</p> : null}
    {Object.entries(statuses).map(([status, label]) => {
      const rows = data.rows.filter(row => row.status === status);
      if (!rows.length) return null;
      return <details key={status} open={status !== "same"} className="rounded border border-[#30363d] p-2">
        <summary className="tap-target cursor-pointer text-xs font-medium">{label} ({rows.length})</summary>
        {rows.map(row => <div key={row.id} className="mt-2 border-t border-[#21262d] pt-2">
          <h4 className="break-all text-xs font-semibold">{categories[row.category]} · {row.name}</h4>
          <div className="grid min-w-0 grid-cols-1 gap-2 sm:grid-cols-2">
            <Cell label={data.leftLabel} cell={row.left} />
            <Cell label={data.rightLabel} cell={row.right} />
          </div>
        </div>)}
      </details>;
    })}
  </div>;
}

function Cell({ label, cell }: { label: string; cell: ClaudeConfigCell }) {
  return <div className="min-w-0 text-xs">
    <p className="break-all text-[#8b949e]">{label}</p>
    {cell.detail ? <p className="break-all text-[#8b949e]">{cell.detail}</p> : null}
    {cell.state === "present" ? <pre className="max-h-48 overflow-auto whitespace-pre-wrap break-all">{cell.value}</pre>
      : <p>{cell.state === "unknown" ? "Could not determine" : "Not present"}</p>}
  </div>;
}

import { useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { current } from "@/lib/ariaCurrent";
import { NO_EPIC, UNRESOLVED, buildTree, rollUp, type Lookup } from "@/lib/jira/tree";
import { CATEGORY_TEXT, describe } from "@/lib/jira/describe";
import { useJiraView } from "@/store/jira";

/// Initiative → epic → issue, epics collapsible (#2037, #2038).
export function JiraTree({
  keys,
  lookup,
  prCount,
}: {
  keys: readonly string[];
  lookup: Lookup;
  prCount: (key: string) => number;
}) {
  const tree = buildTree(keys, lookup);
  return (
    <div className="flex flex-col gap-2">
      {tree.map((g) => (
        <section key={g.initiative ?? "none"} aria-label={g.initiative ?? "Epics"}>
          {g.initiative ? (
            <div className="px-1 pb-1 text-[11px] font-semibold uppercase tracking-wide text-[#8b949e]">
              <span className="font-mono">{g.initiative}</span> {describe(g.initiative, lookup).title}
            </div>
          ) : null}
          {g.epics.map((e) => (
            <EpicGroup key={e.epic} epic={e.epic} issues={e.issues} lookup={lookup} prCount={prCount} />
          ))}
        </section>
      ))}
    </div>
  );
}

function EpicGroup({
  epic,
  issues,
  lookup,
  prCount,
}: {
  epic: string;
  issues: string[];
  lookup: Lookup;
  prCount: (key: string) => number;
}) {
  const [open, setOpen] = useState(true);
  const special = epic === NO_EPIC || epic === UNRESOLVED;
  const label = epic === NO_EPIC ? "No epic" : epic === UNRESOLVED ? "Not loaded yet" : null;
  const d = special ? null : describe(epic, lookup);
  const counts = rollUp(issues, lookup);
  const Chevron = open ? ChevronDown : ChevronRight;
  return (
    <div className="mb-1">
      <div className="flex items-center gap-1">
        <button
          type="button"
          aria-expanded={open}
          aria-label={`${open ? "Collapse" : "Expand"} ${label ?? epic}`}
          onClick={() => setOpen(!open)}
          className="rounded p-0.5 text-[#8b949e] hover:bg-[#21262d]"
        >
          <Chevron className="h-3.5 w-3.5" aria-hidden="true" />
        </button>
        {special ? (
          <span className="flex-1 truncate text-xs font-medium text-[#8b949e]">{label}</span>
        ) : (
          <IssueButton k={epic} title={d!.title} className="flex-1 text-xs font-medium" />
        )}
        <span
          className="shrink-0 text-[10px] text-[#8b949e]"
          title={`${counts.done} done, ${counts.in_progress} in progress, ${counts.new} to do${counts.unknown ? `, ${counts.unknown} not known` : ""}`}
        >
          {counts.done}/{issues.length}
        </span>
      </div>
      {open ? (
        <ul className="ml-5 flex flex-col">
          {issues.map((k) => {
            const r = describe(k, lookup);
            const n = prCount(k);
            return (
              <li key={k} className="flex items-center gap-1">
                <IssueButton k={k} title={r.title} className="min-w-0 flex-1 text-xs" />
                {r.status ? <span className={`shrink-0 text-[10px] ${CATEGORY_TEXT[r.category]}`}>{r.status}</span> : null}
                {n > 0 ? (
                  <span className="shrink-0 rounded bg-[#21262d] px-1 text-[10px] text-[#8b949e]" title={`${n} linked pull request${n === 1 ? "" : "s"}`}>
                    {n}
                  </span>
                ) : null}
              </li>
            );
          })}
        </ul>
      ) : null}
    </div>
  );
}

function IssueButton({ k, title, className }: { k: string; title: string; className: string }) {
  const { selected, select } = useJiraView();
  return (
    <button
      type="button"
      onClick={() => select(k)}
      aria-current={current(selected === k)}
      title={`${k} ${title}`}
      className={`truncate rounded px-1 py-0.5 text-left text-[#e6edf3] hover:bg-[#21262d] aria-[current=true]:bg-[#1f6feb33] ${className}`}
    >
      <span className="font-mono text-[#8b949e]">{k}</span> {title}
    </button>
  );
}

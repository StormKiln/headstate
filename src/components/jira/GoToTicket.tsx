import { useState } from "react";
import { parseTypedKey } from "@/lib/jira/keys";
import type { Links } from "@/lib/jira/tree";
import { useJiraView } from "@/store/jira";
import { openInHeadstate } from "./openInHeadstate";

/// Type a key -- `abc-1`, or paste a Jira link -- and land on its work
/// (#2040).
///
/// One pull request names it: that pull request opens. Otherwise the issue
/// opens here, listing every pull request and worktree that names it, or
/// saying none of yours does. A `type="search"` field, so the app's `/`
/// shortcut reaches it.
export function GoToTicket({ projects, links }: { projects: ReadonlySet<string>; links: ReadonlyMap<string, Links> }) {
  const { select } = useJiraView();
  const [text, setText] = useState("");
  const [problem, setProblem] = useState<string | null>(null);

  const go = () => {
    const key = parseTypedKey(text);
    if (!key) {
      setProblem("Type a Jira key such as ABC-1, or paste a link to an issue.");
      return;
    }
    const project = key.slice(0, key.lastIndexOf("-"));
    if (!projects.has(project)) {
      setProblem(`${project} is not one of your Jira projects.`);
      return;
    }
    setProblem(null);
    setText("");
    const prs = links.get(key)?.prs ?? [];
    if (prs.length === 1) openInHeadstate(prs[0].identity);
    else select(key);
  };

  return (
    <div className="flex flex-col gap-1">
      <input
        type="search"
        value={text}
        spellCheck={false}
        placeholder="Go to ticket, e.g. ABC-1"
        aria-label="Go to ticket"
        aria-invalid={problem !== null}
        onChange={(e) => {
          setText(e.target.value);
          setProblem(null);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            go();
          }
        }}
        className="rounded border border-[#30363d] bg-[#0d1117] px-2 py-1 font-mono text-xs text-[#e6edf3]"
      />
      {problem ? <p className="text-xs text-[#d29922]">{problem}</p> : null}
    </div>
  );
}

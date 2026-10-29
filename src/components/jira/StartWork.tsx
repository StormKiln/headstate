import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { GitBranchPlus } from "lucide-react";
import { startWork, startWorkPlan, type StartMode, type StartPlan, type StartRefusal, type Started } from "@/api/tauri";
import { useWorktrees } from "@/api/hooks";
import { branchName } from "@/lib/jira/branchName";
import { errorMessage } from "@/lib/queryErrorMessage";
import type { JiraIssue } from "@/types/jira";
import { ActingOnDesktop } from "../ActingOnDesktop";
import { ArgvPreview } from "../ArgvPreview";
import { Dialog, DialogContent, DialogTitle } from "../ui/dialog";
import { useJira } from "./jiraState";

/// "Start work" for an assigned issue with no pull request yet (#2045): a
/// branch named from the key, or a worktree on one, in a repository the
/// Worktrees page already tracks.
///
/// The desktop plans and the dialog shows that plan -- the exact argv, the
/// base and the directory -- before anything runs; the desktop runs only
/// that argv, re-checked. `onStarted` is the seam for #2044's "Move to In
/// Progress": run the transition there, after git has succeeded, so a
/// failed Jira write never reads as a failed branch.
export function StartWork({ issue, onStarted }: { issue: JiraIssue; onStarted?: (s: Started) => void }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button
        type="button"
        onClick={() => setOpen(true)}
        className="flex items-center gap-1.5 self-start rounded border border-[#30363d] px-2 py-1 text-xs text-[#e6edf3] hover:bg-[#21262d]"
      >
        <GitBranchPlus className="h-3.5 w-3.5" aria-hidden="true" />
        Start work
      </button>
      {open ? <StartWorkDialog issue={issue} onClose={() => setOpen(false)} onStarted={onStarted} /> : null}
    </>
  );
}

/// The value, once it has stopped changing for `ms`: a plan per keystroke
/// would be a burst of git calls on the desktop for names nobody wanted.
function useSettled<T>(value: T, ms = 300): T {
  const [settled, setSettled] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setSettled(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return settled;
}

const INPUT = "rounded border border-[#30363d] bg-[#0d1117] px-2 py-1 text-sm text-[#e6edf3]";

function StartWorkDialog({
  issue,
  onClose,
  onStarted,
}: {
  issue: JiraIssue;
  onClose: () => void;
  onStarted?: (s: Started) => void;
}) {
  const qc = useQueryClient();
  const template = useJira()?.branchTemplate ?? "";
  const scan = useWorktrees(true);
  const repos = scan.data ?? [];
  const [repo, setRepo] = useState("");
  const [mode, setMode] = useState<StartMode>("worktree");
  const proposed = branchName(template, issue.key, issue.summary);
  const [edited, setEdited] = useState<string | null>(null);
  const branch = edited ?? proposed;
  const settled = useSettled(branch.trim());
  const [running, setRunning] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  const [started, setStarted] = useState<Started | null>(null);

  const plan = useQuery({
    queryKey: ["start-work-plan", repo, settled, mode],
    queryFn: () => startWorkPlan(repo, settled, mode),
    enabled: repo !== "" && settled !== "" && started === null,
    staleTime: 0,
    retry: false,
  });
  // The plan on screen is for what is typed now, or it is not shown as
  // runnable: a plan for the previous name must not be confirmable.
  const current = plan.data && plan.data.branch === branch.trim() && plan.data.mode === mode ? plan.data : null;

  const run = (p: StartPlan) => {
    setRunning(true);
    setFailed(null);
    startWork(repo, p.branch, p.mode, p.args)
      .then((s) => {
        setStarted(s);
        // The Worktrees page, and through its scan the ticket's linked
        // work (#2035); the Branches page for a branch alone.
        void qc.invalidateQueries({ queryKey: ["worktrees"] });
        void qc.invalidateQueries({ queryKey: ["worktree-safety"] });
        void qc.invalidateQueries({ queryKey: ["branches", repo] });
        void qc.invalidateQueries({ queryKey: ["jira-links"] });
        onStarted?.(s);
      })
      .catch((e: unknown) => {
        setFailed(errorMessage(e) ?? "The desktop did not say why.");
        void plan.refetch();
      })
      .finally(() => setRunning(false));
  };

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-w-lg">
        <DialogTitle>
          Start work on <span className="font-mono">{issue.key}</span>
        </DialogTitle>
        <ActingOnDesktop />
        <p className="mt-1 truncate text-sm text-[#8b949e]">{issue.summary}</p>
        {started ? (
          <StartedNote started={started} />
        ) : (
          <div className="mt-3 flex flex-col gap-3">
            <label className="flex flex-col gap-1 text-xs text-[#8b949e]">
              Repository
              {scan.isPending ? (
                <span className="h-7 w-full animate-pulse rounded bg-[#21262d]" aria-label="Finding repositories" />
              ) : scan.isError ? (
                <span className="text-[#f85149]">
                  Could not list your repositories: {errorMessage(scan.error) ?? "the desktop did not say why"}
                </span>
              ) : repos.length === 0 ? (
                <span>No repositories were found in your scan folders. Add one in Settings › Worktrees.</span>
              ) : (
                <select value={repo} onChange={(e) => setRepo(e.target.value)} className={INPUT}>
                  <option value="">Choose a repository</option>
                  {repos.map((r) => (
                    <option key={r.path} value={r.path}>
                      {r.identity ?? r.name}
                    </option>
                  ))}
                </select>
              )}
            </label>
            <fieldset className="flex flex-wrap gap-4 text-sm">
              <legend className="sr-only">Create</legend>
              <label className="flex items-center gap-1.5">
                <input type="radio" checked={mode === "worktree"} onChange={() => setMode("worktree")} />
                New worktree
              </label>
              <label className="flex items-center gap-1.5">
                <input type="radio" checked={mode === "branch"} onChange={() => setMode("branch")} />
                Branch only
              </label>
            </fieldset>
            <label className="flex flex-col gap-1 text-xs text-[#8b949e]">
              Branch
              <input
                value={branch}
                onChange={(e) => setEdited(e.target.value)}
                spellCheck={false}
                className={`${INPUT} font-mono`}
              />
            </label>
            {/* #2044: the "Move to In Progress" checkbox belongs here,
                unchecked unless the user opts in; its write runs from
                `onStarted`, after git has succeeded. */}
            {repo === "" ? null : plan.isError ? (
              <p className="text-xs text-[#f85149]">
                Could not check this: {errorMessage(plan.error) ?? "the desktop did not say why"}
              </p>
            ) : current ? (
              <PlanView plan={current} k={issue.key} />
            ) : settled !== "" ? (
              <span className="h-16 w-full animate-pulse rounded bg-[#21262d]" aria-label="Checking" />
            ) : null}
            {failed ? <p className="text-xs text-[#f85149]">{failed}</p> : null}
          </div>
        )}
        <div className="mt-5 flex justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#21262d]"
          >
            {started ? "Close" : "Cancel"}
          </button>
          {started ? null : (
            <button
              type="button"
              disabled={!current || current.refusal !== null || running}
              onClick={() => current && run(current)}
              className="rounded bg-[#238636] px-3 py-1.5 text-sm font-medium text-white hover:bg-[#1a7f37] disabled:opacity-50"
            >
              {running ? "Creating…" : mode === "worktree" ? "Create worktree" : "Create branch"}
            </button>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}

function PlanView({ plan, k }: { plan: StartPlan; k: string }) {
  const links = plan.keys.includes(k);
  return (
    <div className="flex flex-col gap-1 text-xs text-[#8b949e]">
      {plan.refusal ? <p className="text-[#f85149]">{refusalText(plan.refusal)}</p> : null}
      {plan.base ? (
        <p>
          From <span className="font-mono">{plan.base}</span> as last fetched
          {plan.worktree_path ? (
            <>
              , in <span className="break-all font-mono">{plan.worktree_path}</span>
            </>
          ) : null}
          .
        </p>
      ) : null}
      {links ? null : (
        <p role="status" className="text-[#d29922]">
          This name does not carry {k}, so the branch will not link to the ticket.
        </p>
      )}
      <span>
        Runs in <span className="break-all font-mono">{plan.cwd}</span>:
      </span>
      <ArgvPreview program={plan.program} args={plan.args} />
    </div>
  );
}

/// A refusal as a sentence that says what to do.
function refusalText(r: StartRefusal): string {
  switch (r.state) {
    case "invalid_branch":
      return `Git will not accept this branch name: ${r.detail}`;
    case "branch_exists":
      return `A branch named ${r.branch} already exists in this repository. Choose another name.`;
    case "remote_branch_exists":
      return `origin already has a branch named ${r.branch}. Choose another name.`;
    case "path_exists":
      return `${r.path} already exists. Choose another branch name, or move that directory.`;
    case "path_registered":
      return `Git still lists a worktree at ${r.path}, though the directory is gone. Prune this repository's worktrees on the Worktrees page first.`;
    case "no_base":
      return `${r.base} does not resolve to a commit in this repository, so there is nothing to start from.`;
    case "could_not_check":
      return `Could not check whether this can run: ${r.detail}`;
  }
}

function StartedNote({ started }: { started: Started }) {
  return (
    <p role="status" className="mt-3 text-sm text-[#3fb950]">
      {started.worktree_path ? (
        <>
          Git lists a worktree at <span className="break-all font-mono">{started.worktree_path}</span> on{" "}
        </>
      ) : (
        <>Created branch </>
      )}
      <span className="font-mono">{started.branch}</span>, at{" "}
      <span className="font-mono">{started.head.slice(0, 7)}</span>.
      {started.worktree_path ? null : " It is not checked out anywhere yet."}
    </p>
  );
}

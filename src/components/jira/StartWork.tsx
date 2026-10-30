import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { GitBranchPlus } from "lucide-react";
import { startWork, startWorkPlan, type StartMode, type StartPlan, type StartRefusal, type Started } from "@/api/tauri";
import { useWorktrees } from "@/api/hooks";
import { describeJiraError, failureOf, useJiraProjectStatuses, useJiraTransition } from "@/api/jira";
import { branchName } from "@/lib/jira/branchName";
import { startFix, type Fix } from "@/lib/jira/drift";
import { TRANSITION_PROGRAM, candidateStatuses, isStatusName, transitionArgv, transitionOutcome } from "@/lib/jira/transition";
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
/// that argv, re-checked. An opt-in "Also move ABC-1 to In Progress"
/// (#2044) is confirmed in the same dialog, its argv beside git's, and runs
/// only after git has succeeded, so a failed Jira write never reads as a
/// failed branch. `onStarted` fires once git has.
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
  const jira = useJira();
  const template = jira?.branchTemplate ?? "";
  // The move offered alongside (#2045): toward the project's on-open
  // expectation, never by default, and only while Jira is on.
  const fix = jira ? startFix(issue, jira.expectations) : null;
  const [alsoMove, setAlsoMove] = useState(false);
  const [typedStatus, setTypedStatus] = useState("");
  const moveTo = fix?.kind === "move" ? fix.status : typedStatus.trim();
  const move = alsoMove && fix && isStatusName(moveTo) ? moveTo : null;
  const transition = useJiraTransition();
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

  // `move` is fixed by this render: what was confirmed is what runs once
  // git is done, and nothing runs if git fails.
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
        if (move) transition.mutate({ key: issue.key, status: move });
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
          <>
            <StartedNote started={started} />
            <MoveNote k={issue.key} m={transition} />
          </>
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
            {fix ? (
              <MoveChoice
                issue={issue}
                fix={fix}
                checked={alsoMove}
                onChecked={setAlsoMove}
                typed={typedStatus}
                onTyped={setTypedStatus}
              />
            ) : null}
            {repo === "" ? null : plan.isError ? (
              <p className="text-xs text-[#f85149]">
                Could not check this: {errorMessage(plan.error) ?? "the desktop did not say why"}
              </p>
            ) : current ? (
              <PlanView plan={current} k={issue.key} move={move} />
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
              disabled={!current || current.refusal !== null || running || (alsoMove && move === null)}
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

/// "Also move ABC-1 to …": unchecked unless the reader opts in. A named
/// on-open expectation is the status; a category is not one status, so
/// the reader names it, helped by the statuses seen in the project.
function MoveChoice({
  issue,
  fix,
  checked,
  onChecked,
  typed,
  onTyped,
}: {
  issue: JiraIssue;
  fix: Fix;
  checked: boolean;
  onChecked: (v: boolean) => void;
  typed: string;
  onTyped: (v: string) => void;
}) {
  const choose = fix.kind === "choose";
  const seen = useJiraProjectStatuses(issue.project_key, checked && choose);
  const options = candidateStatuses(seen.data?.statuses ?? [], issue.status.name || null);
  const listId = `start-move-${issue.key}`;
  return (
    <div className="flex flex-col gap-1 text-xs text-[#8b949e]">
      <label className="flex items-center gap-1.5 text-sm text-[#e6edf3]">
        <input type="checkbox" checked={checked} onChange={(e) => onChecked(e.target.checked)} />
        {fix.kind === "move" ? `Also move ${issue.key} to ${fix.status}` : `Also move ${issue.key} in Jira`}
      </label>
      {checked && fix.kind === "choose" ? (
        <>
          <p>
            {issue.project_key} expects {fix.expected} here. Headstate cannot see which moves its workflow allows from{" "}
            {issue.status.name || "this status"}, and Jira refuses one it does not allow.
          </p>
          <label className="flex flex-col gap-1">
            Move to
            <input
              value={typed}
              onChange={(e) => onTyped(e.target.value)}
              list={listId}
              placeholder="as Jira names it"
              className={INPUT}
            />
            <datalist id={listId}>
              {options.map((o) => (
                <option key={o.name} value={o.name} />
              ))}
            </datalist>
          </label>
          {isStatusName(typed.trim()) ? null : <p>Name the status to move {issue.key} to, or untick the box.</p>}
        </>
      ) : null}
    </div>
  );
}

const MOVE_TONE = { ok: "text-[#3fb950]", warn: "text-[#d29922]", fail: "text-[#f85149]" } as const;

/// What the move after git came to: the status Jira reports when read
/// again, or Jira's own reason for refusing.
function MoveNote({ k, m }: { k: string; m: ReturnType<typeof useJiraTransition> }) {
  if (m.isIdle) return null;
  if (m.isPending) {
    return (
      <p role="status" className="mt-2 text-sm text-[#8b949e]">
        Moving {k} in Jira…
      </p>
    );
  }
  if (m.isError) {
    return (
      <p role="alert" className="mt-2 text-sm text-[#f85149]">
        Could not move {k}: {describeJiraError(failureOf(m.error))}
      </p>
    );
  }
  const o = transitionOutcome(m.data);
  return (
    <p role="status" className={`mt-2 text-sm ${MOVE_TONE[o.tone]}`}>
      {o.text}
    </p>
  );
}

function PlanView({ plan, k, move }: { plan: StartPlan; k: string; move: string | null }) {
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
      {move ? (
        <>
          <span>Then, only if that succeeds, in Jira:</span>
          <div aria-label="Jira command to run">
            <ArgvPreview program={TRANSITION_PROGRAM} args={transitionArgv(k, move)} />
          </div>
        </>
      ) : null}
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

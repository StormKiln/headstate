import { describeJiraError, failureOf, useJiraConnection, useJiraLinks } from "@/api/jira";
import { useJiraView, type JiraFilter } from "@/store/jira";
import { needsAttention } from "@/lib/jira/tree";
import { readAge } from "@/lib/jira/age";
import { useNow } from "@/lib/readyAge";
import type { IssueBatch, JiraError } from "@/types/jira";
import { HelpButton } from "../HelpButton";
import { GoToTicket } from "./GoToTicket";
import { JiraTree } from "./JiraTree";
import { useJiraBoard } from "./useJiraBoard";

const FILTERS: { id: JiraFilter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "linked", label: "With PRs" },
  { id: "assigned", label: "Assigned" },
  { id: "attention", label: "Needs attention" },
];

const SKELETON = <span className="block h-4 w-40 animate-pulse rounded bg-[#21262d]" aria-label="Loading" />;

/// The filter tabs and the issue tree (#2038). Rendered in the sidebar on
/// the desktop, and as the page itself on a phone until an issue is picked.
export function JiraBrowser() {
  const board = useJiraBoard();
  const { filter, setFilter } = useJiraView();
  const now = useNow(30_000);
  if (!board.jira) return <NotReady />;

  const assignedOk = board.assigned.isSuccess && board.assigned.data.error === null;
  const keys =
    filter === "linked"
      ? board.linked
      : filter === "assigned"
        ? board.assignedKeys
        : filter === "attention"
          ? board.linked.filter((k) => needsAttention(k, board.links, board.lookup, board.jira?.expectations))
          : [...new Set([...board.linked, ...board.assignedKeys])];
  const wantsAssigned = filter === "all" || filter === "assigned";
  const wantsLinked = filter !== "assigned";

  return (
    <div className="flex min-h-0 flex-col gap-2">
      <GoToTicket projects={board.jira.projects} links={board.links} site={board.jira.site} />
      <div className="flex items-center">
        <div role="group" aria-label="Which issues" className="flex flex-wrap gap-1">
          {FILTERS.map((f) => (
            <button
              key={f.id}
              type="button"
              aria-pressed={filter === f.id}
              onClick={() => setFilter(f.id)}
              className="rounded px-2 py-0.5 text-xs text-[#8b949e] hover:bg-[#21262d] aria-pressed:bg-[#21262d] aria-pressed:text-[#e6edf3]"
            >
              {f.label}
            </button>
          ))}
        </div>
        <HelpButton topic="jira" />
      </div>
      {wantsAssigned && board.assigned.isPending ? SKELETON : null}
      {wantsAssigned ? (
        <AssignedProblem
          data={board.assigned.data}
          // The latest attempt's failure: a rejected refetch outranks the
          // error the data it kept was carrying.
          error={board.assigned.isError ? failureOf(board.assigned.error) : (board.assigned.data?.error ?? null)}
          rejected={board.assigned.isError}
          now={now}
        />
      ) : null}
      {wantsAssigned && assignedOk && board.assigned.data?.truncated ? (
        <p className="text-xs text-[#8b949e]">Showing the {board.assignedKeys.length} most recently updated assigned issues; there are more.</p>
      ) : null}
      {wantsLinked && board.worktreesLoading && board.linked.length === 0 ? SKELETON : null}
      {keys.length > 0 ? (
        <JiraTree keys={keys} lookup={board.lookup} prCount={(k) => board.links.get(k)?.prs.length ?? 0} />
      ) : (
        <Empty filter={filter} assignedOk={assignedOk} worktreesLoading={board.worktreesLoading} />
      )}
    </div>
  );
}

/// What went wrong reading the assigned list, said once (#2036). A list
/// already on screen -- kept from before a failed refetch, or the last one
/// the desktop read -- is said to be old, with its age when that is
/// known; the error alone is for when there is no list at all.
function AssignedProblem({
  data,
  error,
  rejected,
  now,
}: {
  data: IssueBatch | undefined;
  error: JiraError | null;
  rejected: boolean;
  now: Date;
}) {
  if (!error) return null;
  const why = describeJiraError(error);
  const shown = (data?.issues.length ?? 0) > 0;
  // An old list: the desktop served its last one, or this side kept it.
  const old = shown && (rejected || data?.issues.some((i) => i.stale) === true);
  if (old) {
    const age = readAge(data?.list_read_at, now);
    return (
      <p role="status" className="text-xs text-[#d29922]">
        {`Showing the issues assigned to you as ${age ?? "last read"}. The latest read failed: ${why}`}
      </p>
    );
  }
  return (
    <p className="text-xs text-[#f85149]">
      {shown ? "Some of the issues assigned to you could not be read: " : "Could not read the issues assigned to you: "}
      {why}
    </p>
  );
}

/// An empty list is only called empty once what feeds it has answered.
function Empty({ filter, assignedOk, worktreesLoading }: { filter: JiraFilter; assignedOk: boolean; worktreesLoading: boolean }) {
  const linkedSettled = !worktreesLoading;
  const say = (text: string) => <p className="text-xs text-[#8b949e]">{text}</p>;
  if (filter === "assigned") return assignedOk ? say("Nothing is assigned to you that is not done.") : null;
  if (filter === "attention") {
    return linkedSettled ? say("No open pull request of yours disagrees with its Jira issue.") : null;
  }
  if (filter === "linked" || !assignedOk) {
    return linkedSettled
      ? say("None of your open pull requests or worktrees name a Jira issue. A key such as ABC-1 in a title or branch name links one.")
      : null;
  }
  return linkedSettled
    ? say("No Jira issues: none of your open pull requests or worktrees name one, and nothing open is assigned to you.")
    : null;
}

/// Why there is no tree, in terms the reader can act on.
function NotReady() {
  const connection = useJiraConnection(true, 5 * 60_000);
  const signedIn = connection.data?.state === "logged_in";
  const links = useJiraLinks(signedIn);
  if (connection.isPending || (signedIn && links.isPending)) return SKELETON;
  if (signedIn && (links.isError || links.data?.error)) {
    return (
      <p className="text-xs text-[#f85149]">
        {links.data?.error
          ? `Could not list your Jira projects: ${describeJiraError(links.data.error)}`
          : `Could not read which pull requests name Jira issues: ${describeJiraError(failureOf(links.error))}`}
      </p>
    );
  }
  if (connection.isError) return <p className="text-xs text-[#f85149]">Could not ask the desktop about Jira.</p>;
  return <p className="text-xs text-[#d29922]">Jira is not connected. Open Settings → Jira to sign in.</p>;
}

import { useJiraConnection, useJiraProjects } from "@/api/jira";
import { useJiraView, type JiraFilter } from "@/store/jira";
import { needsAttention } from "@/lib/jira/tree";
import { GoToTicket } from "./GoToTicket";
import { JiraTree } from "./JiraTree";
import { useJiraBoard, type BoardPr } from "./useJiraBoard";

const FILTERS: { id: JiraFilter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "linked", label: "With PRs" },
  { id: "assigned", label: "Assigned" },
  { id: "attention", label: "Needs attention" },
];

const SKELETON = <span className="block h-4 w-40 animate-pulse rounded bg-[#21262d]" aria-label="Loading" />;

/// The filter tabs and the issue tree (#2038). Rendered in the sidebar on
/// the desktop, and as the page itself on a phone until an issue is picked.
export function JiraBrowser({ prs }: { prs: readonly BoardPr[] }) {
  const board = useJiraBoard(prs);
  const { filter, setFilter } = useJiraView();
  if (!board.jira) return <NotReady />;

  const assignedOk = board.assigned.isSuccess && board.assigned.data.error === null;
  const keys =
    filter === "linked"
      ? board.linked
      : filter === "assigned"
        ? board.assignedKeys
        : filter === "attention"
          ? board.linked.filter((k) => needsAttention(k, board.links, board.lookup))
          : [...new Set([...board.linked, ...board.assignedKeys])];
  const wantsAssigned = filter === "all" || filter === "assigned";
  const wantsLinked = filter !== "assigned";

  return (
    <div className="flex min-h-0 flex-col gap-2">
      <GoToTicket projects={board.jira.projects} links={board.links} />
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
      {wantsAssigned && board.assigned.isPending ? SKELETON : null}
      {wantsAssigned && (board.assigned.isError || board.assigned.data?.error) ? (
        <p className="text-xs text-[#f85149]">
          Could not read the issues assigned to you: {board.assigned.data?.error ?? String(board.assigned.error)}
        </p>
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
  const projects = useJiraProjects(signedIn);
  if (connection.isPending || (signedIn && projects.isPending)) return SKELETON;
  if (signedIn && (projects.isError || projects.data?.error)) {
    return (
      <p className="text-xs text-[#f85149]">
        Could not list your Jira projects: {projects.data?.error ?? "no answer from acli"}
      </p>
    );
  }
  if (connection.isError) return <p className="text-xs text-[#f85149]">Could not ask the desktop about Jira.</p>;
  return <p className="text-xs text-[#d29922]">Jira is not connected. Open Settings → Jira to sign in.</p>;
}

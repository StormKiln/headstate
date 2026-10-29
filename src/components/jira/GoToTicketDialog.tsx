import { useState } from "react";
import { useJiraFind } from "@/api/jira";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { searchWords, ticketWork, type WorkRow } from "@/lib/jira/ticketWork";
import { useFilters } from "@/store/filters";
import { useGoToTicket } from "@/store/goToTicket";
import { useSourceSelection } from "@/store/sourceSelection";
import { ExternalLink } from "../ExternalLink";
import { browseUrl, jumpToOnlyPr, openIssue } from "./goToTicketActions";
import { GoToTicketButton, TicketField } from "./GoToTicket";
import { useJira, type JiraState } from "./jiraState";
import { openInHeadstate } from "./openInHeadstate";

/// Go to ticket from any page (#2040), opened by Cmd/Ctrl+J or the header
/// button. Deliberately one job -- a key in, its work out -- rather than a
/// general command palette; a palette can host `TicketField` later.
export function GoToTicketDialog() {
  const { open, setOpen } = useGoToTicket();
  const jira = useJira();
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className="max-w-lg content-start border border-[#30363d] bg-[#0d1117] text-[#e6edf3]">
        <DialogTitle className="text-sm">Go to ticket</DialogTitle>
        {jira ? (
          <Body jira={jira} close={() => setOpen(false)} />
        ) : (
          // `null` covers "off", "signed out" and "not read yet" alike, so
          // this names all three rather than guessing one.
          <p className="text-xs text-[#d29922]">
            Jira is not connected, or Headstate has not finished reading your Jira projects. Settings → Jira shows which.
          </p>
        )}
      </DialogContent>
    </Dialog>
  );
}

function Body({ jira, close }: { jira: JiraState; close: () => void }) {
  const [key, setKey] = useState<string | null>(null);
  return (
    <div className="flex min-w-0 flex-col gap-3">
      <TicketField
        projects={jira.projects}
        autoFocus
        onKey={(k) => {
          if (jumpToOnlyPr(k, jira.links.byKey.get(k), jira.site)) close();
          else setKey(k);
        }}
      />
      {key ? <Results key={key} k={key} jira={jira} close={close} /> : null}
    </div>
  );
}

const STATE_CLASS: Record<string, string> = {
  open: "text-[#3fb950]",
  draft: "text-[#8b949e]",
  merged: "text-[#a371f7]",
  closed: "text-[#f85149]",
  "no longer open": "text-[#8b949e]",
};

const time = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

/// Everything that names `k`: a picker when there are several, and the
/// search's answer when Headstate held none.
function Results({ k, jira, close }: { k: string; jira: JiraState; close: () => void }) {
  const [more, setMore] = useState(false);
  const q = useJiraFind(k, true, more);
  const selection = useSourceSelection((s) => s.selection);
  const local = jira.links.byKey.get(k);
  const rows = ticketWork(local, q.data);
  const prs = rows.filter((r) => r.kind === "pr");
  // Found by searching (it opens on its host), per provider, versus
  // already in your lists.
  const found = prs.filter((r) => r.identity === null);
  const shown = (provider: "GitHub" | "GitLab") => found.filter((r) => r.gitlab === (provider === "GitLab")).length;
  const mine = prs.length - found.length;
  const href = browseUrl(jira.site, k);
  const searching = selection === "github" ? "Searching GitHub…" : "Searching GitHub and GitLab…";
  const halves = q.data ? ([["GitHub", q.data.search], ["GitLab", q.data.gitlab]] as const) : [];
  const alreadyKnown = halves.some(([, s]) => s?.state === "not_asked" && s.reason.startsWith("already known"));

  return (
    <div className="flex min-w-0 flex-col gap-2">
      {rows.length === 0 && !q.isPending ? (
        <p className="text-xs text-[#8b949e]">None of your pull requests, worktrees or branches names {k}.</p>
      ) : null}
      {rows.length > 0 ? (
        <ul aria-label={`Work that names ${k}`} className="flex max-h-72 flex-col gap-1 overflow-y-auto">
          {rows.map((r) => (
            <Row key={r.kind === "pr" ? r.url : r.kind === "worktree" ? r.path : `${r.repoPath}\n${r.branch}`} row={r} close={close} />
          ))}
        </ul>
      ) : null}
      {q.isPending ? (
        // Pending: a skeleton AND words, so it never reads as an answer.
        <p role="status" className="flex items-center gap-2 text-xs text-[#8b949e]">
          <span className="h-3 w-24 animate-pulse rounded bg-[#21262d]" aria-hidden="true" />
          {(local?.prs.length ?? 0) === 0 ? searching : `Checking what else names ${k}…`}
        </p>
      ) : q.isError ? (
        <p className="text-xs text-[#f85149]">Could not look for pull requests that name {k}: {String(q.error)}</p>
      ) : (
        <div className="flex flex-col gap-1 text-xs">
          {halves.map(([provider, s]) =>
            s ? (
              <p key={provider} className={s.state === "failed" ? "text-[#f85149]" : "text-[#8b949e]"}>
                {searchWords(provider, s, k, { shown: shown(provider), mine, time })}
              </p>
            ) : null,
          )}
          {alreadyKnown && !more ? (
            <button type="button" onClick={() => setMore(true)} className="self-start text-[#58a6ff] hover:underline">
              Search for more
            </button>
          ) : null}
        </div>
      )}
      <div className="flex flex-wrap gap-x-4 gap-y-1 border-t border-[#30363d] pt-2 text-xs">
        <button
          type="button"
          onClick={() => {
            openIssue(k);
            close();
          }}
          className="text-[#58a6ff] hover:underline"
        >
          Open {k} in My Jira issues
        </button>
        {href ? (
          <ExternalLink href={href} className="text-[#58a6ff] hover:underline">
            Open in Jira
          </ExternalLink>
        ) : null}
      </div>
    </div>
  );
}

function Row({ row, close }: { row: WorkRow; close: () => void }) {
  const cell = "flex flex-wrap items-center gap-x-2 gap-y-0.5 rounded border border-[#30363d] px-2 py-1.5 text-xs";
  if (row.kind === "worktree") {
    return (
      <li className={cell}>
        <span className="text-[#8b949e]">Worktree</span>
        <span className="text-[#e6edf3]">{row.repo}</span>
        <span className="font-mono text-[#e6edf3]">{row.branch}</span>
        <span className="min-w-0 truncate font-mono text-[#8b949e]">{row.path}</span>
        <button
          type="button"
          onClick={() => {
            useFilters.getState().setView("worktrees");
            close();
          }}
          className="ml-auto text-[#58a6ff] hover:underline"
        >
          Show in Worktrees
        </button>
      </li>
    );
  }
  if (row.kind === "branch") {
    return (
      <li className={cell}>
        <span className="text-[#8b949e]">Branch</span>
        <span className="font-mono text-[#e6edf3]">{row.branch}</span>
        <span className="min-w-0 truncate font-mono text-[#8b949e]">{row.repoPath}</span>
      </li>
    );
  }
  const identity = row.identity;
  return (
    <li className={cell}>
      <span className="min-w-0 basis-full truncate text-sm text-[#e6edf3]">{row.title}</span>
      <span className="text-[#8b949e]">{row.ref}</span>
      <span className={STATE_CLASS[row.state]}>{row.state}</span>
      <span className="min-w-0 truncate font-mono text-[#8b949e]">{row.branch}</span>
      <span className="ml-auto flex gap-3">
        {identity ? (
          <button
            type="button"
            onClick={() => {
              openInHeadstate(identity);
              close();
            }}
            className="text-[#58a6ff] hover:underline"
          >
            Open in Headstate
          </button>
        ) : null}
        <ExternalLink href={row.url} className="text-[#58a6ff] hover:underline">
          {row.gitlab ? "Open on GitLab" : "Open on GitHub"}
        </ExternalLink>
      </span>
    </li>
  );
}

/// The header's button: shown wherever Jira is ready, on every page. On a
/// phone, an icon beside the page title -- the phone has no Cmd+J.
export function GoToTicketEntry({ compact }: { compact: boolean }) {
  const jira = useJira();
  const setOpen = useGoToTicket((s) => s.setOpen);
  if (!jira) return null;
  return <GoToTicketButton compact={compact} onClick={() => setOpen(true)} />;
}

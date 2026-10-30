import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { describeJiraError, failureOf, useJiraAssigned, useJiraSearch } from "@/api/jira";
import { gitLabAction, setPrTitle, type TitleOutcome } from "@/api/tauri";
import { ticketState, titleWithKey, type TicketRow } from "@/lib/jira/noTicket";
import { titleWritePreview, type TitleTarget } from "@/lib/jira/titleWrite";
import type { IssueBatch, JiraIssue } from "@/types/jira";
import { ActingOnDesktop } from "../ActingOnDesktop";
import { ArgvPreview } from "../ArgvPreview";
import { Dialog, DialogContent, DialogTitle } from "../ui/dialog";
import { commandError } from "@/lib/errorKind";
import { useJira } from "./jiraState";

/// The muted "no ticket" marker on one of the user's open pull requests
/// (#2046), and from it, linking one. Renders nothing unless Jira is on,
/// Settings › Jira's marker is on, and the index read this pull request
/// as it is now and found no key: a pull request not read yet is pending,
/// and pending is never "no ticket".
export function NoTicketMarker({ row, target }: { row: TicketRow; target: TitleTarget }) {
  const jira = useJira();
  const [open, setOpen] = useState(false);
  if (!jira?.noTicket || ticketState(jira.noTicket, row) !== "none") return null;
  return (
    // Inside a clickable row, and the dialog's events bubble through its
    // portal to here: neither may reach the row.
    <span className="contents" onClick={(e) => e.stopPropagation()} onKeyDown={(e) => e.stopPropagation()}>
      <button
        type="button"
        title="Names no Jira ticket in its title or branch. Link one."
        className="inline-flex shrink-0 items-center rounded-full border border-dashed border-[#30363d] px-2 py-0.5 text-xs text-[#8b949e] hover:bg-[#21262d]"
        onClick={() => setOpen(true)}
      >
        no ticket
      </button>
      {open ? <LinkTicketDialog row={row} target={target} onClose={() => setOpen(false)} /> : null}
    </span>
  );
}

type Phase =
  | { at: "pick" }
  | { at: "confirm"; issue: JiraIssue; title: string }
  | { at: "writing"; issue: JiraIssue; title: string }
  | { at: "done"; ok: boolean; message: string }
  | { at: "failed"; notSent: boolean; message: string };

/// What a GitHub outcome says, in the reader's terms.
function describeOutcome(o: TitleOutcome, wanted: string): { ok: boolean; message: string } {
  switch (o.state) {
    case "written":
      return { ok: true, message: `GitHub now reads: ${o.title}` };
    case "already_set":
      return { ok: true, message: `GitHub already reads: ${wanted}. Nothing was changed.` };
    case "changed":
      return { ok: false, message: `Not changed: the title on GitHub is now "${o.title}". Refresh, then link again.` };
    case "unconfirmed":
      return {
        ok: false,
        message:
          o.title === null
            ? "GitHub accepted the change, but the title could not be read back. Refresh to check it."
            : `GitHub accepted the change, but reads: ${o.title}. Refresh to check it.`,
      };
  }
}

async function writeTitle(target: TitleTarget, expected: string, title: string): Promise<{ ok: boolean; message: string }> {
  if (target.kind === "github") {
    return describeOutcome(await setPrTitle(target.id, target.repo, target.number, expected, title), title);
  }
  const receipt = await gitLabAction({
    identity: target.identity,
    action: "set_title",
    expected_viewer: target.viewer,
    expected_head: target.head,
    body: title,
    expected_title: expected,
  });
  // Verified means the MR was read back with exactly this title.
  return receipt.outcome === "verified"
    ? { ok: true, message: `GitLab now reads: ${title}` }
    : { ok: false, message: receipt.message };
}

function LinkTicketDialog({ row, target, onClose }: { row: TicketRow; target: TitleTarget; onClose: () => void }) {
  const qc = useQueryClient();
  const [text, setText] = useState("");
  const [phase, setPhase] = useState<Phase>({ at: "pick" });
  const searching = text.trim() !== "";
  const assigned = useJiraAssigned(phase.at === "pick" && !searching);
  const search = useJiraSearch(text, phase.at === "pick");
  const q = searching ? search : assigned;

  const pick = (issue: JiraIssue) => {
    const title = titleWithKey(row.title, issue.key);
    if (title === null) setPhase({ at: "done", ok: true, message: `The title already names ${issue.key}. Nothing to change.` });
    else setPhase({ at: "confirm", issue, title });
  };

  const confirm = async (issue: JiraIssue, title: string) => {
    setPhase({ at: "writing", issue, title });
    try {
      const r = await writeTitle(target, row.title, title);
      setPhase({ at: "done", ...r });
      // The list re-reads after a write; the index follows it.
      void qc.invalidateQueries({ queryKey: ["jira-links"] });
    } catch (e) {
      // "Not sent" only when Headstate declined to send it. Any other
      // failure may have followed the write, so it is not called "not
      // changed".
      const f = commandError(e);
      setPhase({ at: "failed", notSent: f.kind === "not-asked", message: f.message });
    }
  };

  return (
    <Dialog open onOpenChange={(o) => { if (!o) onClose(); }}>
      <DialogContent className="max-w-lg">
        <DialogTitle>Link a ticket</DialogTitle>
        <p className="break-words text-sm text-[#8b949e]">{row.title}</p>
        {phase.at === "pick" ? (
          <>
            <input
              aria-label="Search Jira"
              className="w-full rounded border border-[#30363d] bg-[#0d1117] px-2 py-1 text-sm text-[#e6edf3]"
              placeholder="Search Jira, or type a key"
              value={text}
              onChange={(e) => setText(e.target.value)}
            />
            <IssueChoices
              heading={searching ? "Matching issues" : "Assigned to you"}
              empty={searching ? "No issue matches." : "No open issues are assigned to you."}
              pending={q.isPending}
              error={q.isError ? describeJiraError(failureOf(q.error)) : null}
              batch={q.data}
              onPick={pick}
            />
          </>
        ) : null}
        {phase.at === "confirm" || phase.at === "writing" ? (
          <ConfirmTitle
            target={target}
            title={phase.title}
            busy={phase.at === "writing"}
            onBack={() => setPhase({ at: "pick" })}
            onConfirm={() => void confirm(phase.issue, phase.title)}
          />
        ) : null}
        {phase.at === "done" ? (
          <p role="status" className={`break-words text-sm ${phase.ok ? "text-[#3fb950]" : "text-[#d29922]"}`}>{phase.message}</p>
        ) : null}
        {phase.at === "failed" ? (
          <p role="alert" className="break-words text-sm text-[#f85149]">
            {phase.notSent ? `Not sent: ${phase.message}` : `${phase.message} Refresh to check the title before trying again.`}
          </p>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

function IssueChoices({
  heading,
  empty,
  pending,
  error,
  batch,
  onPick,
}: {
  heading: string;
  empty: string;
  pending: boolean;
  error: string | null;
  batch: IssueBatch | undefined;
  onPick: (issue: JiraIssue) => void;
}) {
  if (pending) return <div aria-busy="true" className="h-16 animate-pulse rounded bg-[#21262d]" />;
  if (error) return <p role="alert" className="text-sm text-[#f85149]">{error}</p>;
  if (!batch) return null;
  const issues = batch.issues;
  // A failed read with nothing found is a failure, not "no issues".
  if (issues.length === 0 && batch.error) return <p role="alert" className="text-sm text-[#f85149]">{describeJiraError(batch.error)}</p>;
  return (
    <div className="flex flex-col gap-1">
      <p className="text-xs text-[#8b949e]">
        {heading}
        {batch.truncated ? ` (the first ${issues.length}; there may be more)` : ""}
      </p>
      {batch.error ? <p className="text-xs text-[#d29922]">{describeJiraError(batch.error)}</p> : null}
      {issues.length === 0 ? <p className="text-sm text-[#8b949e]">{empty}</p> : null}
      <ul className="max-h-64 overflow-auto">
        {issues.map((i) => (
          <li key={i.key}>
            <button
              type="button"
              className="flex w-full items-baseline gap-2 rounded px-2 py-1 text-left text-sm hover:bg-[#21262d]"
              onClick={() => onPick(i)}
            >
              <span className="shrink-0 font-mono text-xs text-[#e6edf3]">{i.key}</span>
              <span className="min-w-0 flex-1 truncate text-[#e6edf3]">{i.summary}</span>
              <span className="shrink-0 text-xs text-[#8b949e]">{i.status.name}</span>
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

function ConfirmTitle({
  target,
  title,
  busy,
  onBack,
  onConfirm,
}: {
  target: TitleTarget;
  title: string;
  busy: boolean;
  onBack: () => void;
  onConfirm: () => void;
}) {
  const preview = titleWritePreview(target, title);
  return (
    <div className="flex flex-col gap-2 text-sm">
      <p className="text-[#8b949e]">New title:</p>
      <p className="break-words font-semibold text-[#e6edf3]">{title}</p>
      <p className="text-xs text-[#8b949e]">
        {target.kind === "github" ? "Sent to GitHub:" : "Runs:"}
      </p>
      <div aria-label="Title change">
        <ArgvPreview program={preview.program} args={preview.args} />
      </div>
      {preview.stdin ? (
        <>
          <p className="text-xs text-[#8b949e]">On its input:</p>
          <ArgvPreview program={preview.stdin} args={[]} />
        </>
      ) : null}
      <p className="text-xs text-[#8b949e]">The branch is not renamed.</p>
      <ActingOnDesktop />
      <div className="flex justify-end gap-2">
        <button type="button" className="rounded border border-[#30363d] px-3 py-1.5 hover:bg-[#21262d]" disabled={busy} onClick={onBack}>
          Back
        </button>
        <button
          type="button"
          className="rounded bg-[#238636] px-3 py-1.5 font-medium text-white hover:bg-[#1a7f37] disabled:opacity-40"
          disabled={busy}
          onClick={onConfirm}
        >
          {busy ? "Changing title…" : "Change title"}
        </button>
      </div>
    </div>
  );
}

import { useState } from "react";
import { Ticket } from "lucide-react";
import { isMac } from "@/lib/shortcuts";
import type { Links } from "@/lib/jira/tree";
import { useJiraView } from "@/store/jira";
import { jumpToOnlyPr, resolveTyped } from "./goToTicketActions";

/// Go to ticket (#2040): type a key -- `abc-1`, or paste a Jira link --
/// and land on its work. Reached two ways: the box on My Jira issues, and
/// the dialog any page opens with Cmd/Ctrl+J (`GoToTicketDialog`).
///
/// Exactly one pull request names the key: it opens in My PRs, and a toast
/// still offers the issue in My Jira issues and in Jira. Otherwise the
/// caller shows every pull request, worktree and branch that names it --
/// searching GitHub and GitLab when Headstate holds none.

/// The key field, shared by the page box and the dialog. `onKey` gets a
/// key in one of the user's projects; anything else is explained here.
export function TicketField({
  projects,
  onKey,
  autoFocus = false,
}: {
  projects: ReadonlySet<string>;
  onKey: (key: string) => void;
  autoFocus?: boolean;
}) {
  const [text, setText] = useState("");
  const [problem, setProblem] = useState<string | null>(null);

  const go = () => {
    const typed = resolveTyped(text, projects);
    if ("problem" in typed) {
      setProblem(typed.problem);
      return;
    }
    setProblem(null);
    setText("");
    onKey(typed.key);
  };

  return (
    <div className="flex flex-col gap-1">
      <input
        type="search"
        value={text}
        spellCheck={false}
        autoFocus={autoFocus}
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

/// The box on My Jira issues. One pull request opens; otherwise the issue
/// opens here, where its panel lists and searches for what names it. A
/// `type="search"` field, so the app's `/` shortcut reaches it.
export function GoToTicket({
  projects,
  links,
  site = null,
}: {
  projects: ReadonlySet<string>;
  links: ReadonlyMap<string, Links>;
  site?: string | null;
}) {
  const { select } = useJiraView();
  return <TicketField projects={projects} onKey={(key) => jumpToOnlyPr(key, links.get(key), site) || select(key)} />;
}

const GO_TO_KEYS = isMac() ? "⌘J" : "Ctrl+J";

/// The header's way in, for a pointer or a phone with no keyboard.
export function GoToTicketButton({ onClick, compact }: { onClick: () => void; compact: boolean }) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label="Go to ticket"
      title={`Go to ticket (${GO_TO_KEYS})`}
      className={
        compact
          ? "tap-target flex items-center justify-center rounded text-[#8b949e] hover:bg-[#161b22]"
          : "flex items-center gap-1.5 rounded border border-[#30363d] px-2 py-1 text-xs text-[#8b949e] hover:bg-[#161b22] hover:text-[#e6edf3]"
      }
    >
      <Ticket className="h-3.5 w-3.5" aria-hidden="true" />
      {compact ? null : (
        <>
          Go to ticket <kbd className="font-sans text-[10px] text-[#8b949e]">{GO_TO_KEYS}</kbd>
        </>
      )}
    </button>
  );
}

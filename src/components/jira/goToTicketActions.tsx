import { toast } from "sonner";
import { parseTypedKey } from "@/lib/jira/keys";
import type { Links } from "@/lib/jira/tree";
import { useFilters } from "@/store/filters";
import { useJiraView } from "@/store/jira";
import { ExternalLink } from "../ExternalLink";
import { openInHeadstate } from "./openInHeadstate";

/// What Go to ticket does with a key (#2040), apart from the components
/// that call it.

export type Typed = { key: string } | { problem: string };

/// What the box holds, as a key in one of the user's projects, or why not.
export function resolveTyped(text: string, projects: ReadonlySet<string>): Typed {
  const key = parseTypedKey(text);
  if (!key) return { problem: "Type a Jira key such as ABC-1, or paste a link to an issue." };
  const project = key.slice(0, key.lastIndexOf("-"));
  if (!projects.has(project)) return { problem: `${project} is not one of your Jira projects.` };
  return { key };
}

export const browseUrl = (site: string | null, key: string) => (site ? `https://${site}/browse/${key}` : null);

/// Open the issue on My Jira issues.
export function openIssue(key: string) {
  useFilters.getState().setView("jira");
  useJiraView.getState().select(key);
}

/// Open the one pull request that names `key`, and say so with the two
/// other places the ticket can go. Returns false when there is not exactly
/// one.
export function jumpToOnlyPr(key: string, links: Links | undefined, site: string | null): boolean {
  const prs = links?.prs ?? [];
  if (prs.length !== 1) return false;
  const pr = prs[0];
  openInHeadstate(pr.identity);
  const gitlab = pr.identity.source?.provider === "gitlab";
  const href = browseUrl(site, key);
  toast.success(`Opened ${pr.identity.repo}${gitlab ? "!" : "#"}${pr.identity.number} for ${key}`, {
    duration: 10_000,
    description: (
      <span className="flex flex-wrap gap-x-3">
        <button type="button" onClick={() => openIssue(key)} className="text-[#58a6ff] hover:underline">
          Open {key} in My Jira issues
        </button>
        {href ? (
          <ExternalLink href={href} className="text-[#58a6ff] hover:underline">
            Open in Jira
          </ExternalLink>
        ) : null}
      </span>
    ),
  });
  return true;
}


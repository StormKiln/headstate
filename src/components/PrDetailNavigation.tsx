import { ArrowLeft, ExternalLink as ExternalLinkIcon } from "lucide-react";
import { ExternalLink } from "./ExternalLink";

// Browser navigation depends only on the selected identity, never on a
// successful detail request or the provider's API cooldown (#1623).
export function PrDetailNavigation({ provider, repo, number, href, onBack }: {
  provider: "GitHub" | "GitLab";
  repo: string;
  number: number;
  href: string;
  onBack: () => void;
}) {
  return (
    <nav
      aria-label={provider === "GitHub" ? "Pull request navigation" : "Merge request navigation"}
      className="mb-3 flex flex-wrap items-center gap-3 rounded-md border border-[#30363d] bg-[#0d1117] p-3"
    >
      <button type="button" onClick={onBack} className="tap-target flex items-center gap-1.5 text-sm text-[#8b949e] hover:text-[#e6edf3]">
        <ArrowLeft className="h-4 w-4" aria-hidden="true" />
        Back to list
      </button>
      <span className="min-w-0 flex-1 break-words text-sm text-[#e6edf3]">
        {repo} {provider === "GitHub" ? "#" : "!"}{number}
      </span>
      <ExternalLink href={href} className="tap-target inline-flex shrink-0 items-center justify-center gap-2 rounded-md border border-[#58a6ff]/50 bg-[#58a6ff]/10 px-3 py-2 text-sm font-medium text-[#79c0ff] hover:bg-[#58a6ff]/20">
        <ExternalLinkIcon className="h-4 w-4" aria-hidden="true" />
        Open on {provider}
      </ExternalLink>
    </nav>
  );
}

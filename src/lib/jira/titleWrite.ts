/// Writing a Jira key into a pull request's title (#2046): where the write
/// goes, and how it is shown before it runs.

import type { PrIdentity, Source } from "@/types/identity";

/// Where a title is written: the provider's own write path. GitHub through
/// Headstate's GraphQL client (`set_pr_title`), GitLab through `glab`
/// (`gitlab_action`'s `set_title`). Never the branch: renaming a pushed
/// branch closes a GitHub pull request.
export type TitleTarget =
  | { kind: "github"; id: string; repo: string; number: number }
  | { kind: "gitlab"; identity: PrIdentity & { source: Source }; viewer: string | null; head: string | null };

/// The write, as the provider will receive it, one box per word. For
/// GitLab that is the `glab` argv itself, with the body it reads on
/// stdin. GitHub has no argv -- Headstate sends the mutation itself -- so
/// its boxes are the mutation and its two inputs.
export function titleWritePreview(target: TitleTarget, title: string): { program: string; args: string[]; stdin?: string } {
  if (target.kind === "github") {
    return { program: "updatePullRequest", args: [`pullRequestId: ${target.id}`, `title: ${title}`] };
  }
  const path = `projects/${encodeURIComponent(target.identity.repo)}/merge_requests/${target.identity.number}`;
  return {
    program: "glab",
    args: ["api", "--hostname", target.identity.source.host, "-i", path, "--method", "PUT", "--input", "-", "--header", "Content-Type: application/json"],
    stdin: JSON.stringify({ title }),
  };
}

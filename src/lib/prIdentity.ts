import type { PrIdentity, Source } from "../types/identity";

const GITHUB: Source = { provider: "github", host: "github.com" };

/// A tuple encoding preserves field boundaries even if a name contains a
/// delimiter. Legacy payloads and newly emitted GitHub rows name the same PR.
export function prKey(pr: PrIdentity): string {
  const source = pr.source ?? GITHUB;
  return JSON.stringify([source.provider, source.host, pr.repo, pr.number]);
}

/// Keep only identity in UI selection, including source when it was supplied.
export function prIdentity(pr: PrIdentity): PrIdentity {
  return { ...(pr.source === undefined ? {} : { source: pr.source }), repo: pr.repo, number: pr.number };
}

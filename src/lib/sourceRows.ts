import type { PullRequest } from "../types/pr";
import type { MergeRequest } from "../types/gitlab";
import type { PrIdentity } from "../types/identity";
import type { SourceSelection } from "../store/sourceSelection";
import { prKey } from "./prIdentity";
import { applyFilters, type Filters } from "./derive";
import { filterGitLab } from "./gitlabFilters";

// The combined GitHub and GitLab queue's rows. Apart from `SourceQueue.tsx`
// so that file exports only components (#1965), which is what lets dev
// hot-reload swap it in place.

export type Row = { kind: "github"; value: PullRequest } | { kind: "gitlab"; value: MergeRequest };

export function sourceRepoKey(row: PrIdentity): string {
  const source = row.source ?? { provider: "github", host: "github.com" };
  return JSON.stringify([source.provider, source.host, row.repo]);
}

export function combinedRows(github: PullRequest[], gitlab: MergeRequest[], selection: SourceSelection, repoKey: string | null, query: string, filters: Filters = {}): Row[] {
  const all: Row[] = [
    ...(selection === "gitlab" ? [] : applyFilters(github, filters).map((value): Row => ({ kind: "github", value }))),
    ...(selection === "github" ? [] : filterGitLab(gitlab, filters).map((value): Row => ({ kind: "gitlab", value }))),
  ];
  const needle = query.toLowerCase().trim();
  return all.filter(({ value }) =>
    (repoKey === null || sourceRepoKey(value) === repoKey) &&
    (!needle || value.title.toLowerCase().includes(needle) || value.repo.toLowerCase().includes(needle) || String(value.number) === needle.replace(/^#|^!/, "")),
  ).sort((a, b) => {
    const field = filters.sort === "recently-updated" || filters.sort === "least-recently-updated" ? "updated_at" : "created_at";
    const direction = filters.sort === "oldest" || filters.sort === "least-recently-updated" ? 1 : -1;
    return direction * (Date.parse(a.value[field]) - Date.parse(b.value[field])) || prKey(a.value).localeCompare(prKey(b.value));
  });
}

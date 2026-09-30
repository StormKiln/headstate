import { useGitLabViewer } from "./authAvailability";
import { useRef } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { gitlabStatsTree, gitlabStatsLoad, gitlabStatsBackfill } from "./tauri";
import type { Source } from "../types/identity";

export type GitLabScope = { kind: "mine" } | { kind: "project" | "group" | "person"; path: string };
interface Coverage {
  complete: boolean;
  stop: string;
  pages: number;
  received: number;
  total: number | null;
  rate_remaining: number | null;
  rate_reset: number | null;
}
export interface GitLabStatsTree {
  account?: string | null;
  source: Source;
  viewer: string;
  projects: { path: string; namespace: string }[];
  groups?: string[]; group_coverage?: Coverage | null; group_error?: string | null;
  coverage: Coverage;
}
export interface GitLabStatsReport {
  source: Source;
  viewer: string;
  scope: GitLabScope;
  start: string;
  end: string;
  fetched_at: string;
  coverage: Coverage;
  counts: { created: number; merged: number; closed: number; opened: number };
  series: { day: string; created: number }[];
  authors: { username: string; created: number; merged: number; mean_merge_hours: number | null; timed_merges: number }[];
  reviewers: { username: string; assigned: number }[];
  reviewer_rows_measured: number;
  review_activity: number | null;
  merged_window?: {
    fetched_at: string; coverage: Coverage; count: number;
    series: { day: string; merged: number }[];
    authors: { username: string; merged: number; mean_merge_hours: number | null }[];
    history: GitLabStatsReport["history"];
  } | null;
  merged_error?: string | null;
  activity?: {
    complete: boolean; mrs_checked: number; mrs_total: number; comments: number | null;
    participants: { username: string; comments: number; mrs: number }[];
    mean_first_response_hours: number | null; responded_mrs: number;
    rate_limited?: boolean;
    failures: string[]; rate_remaining: number | null; rate_reset: number | null;
  } | null;
  review_evidence?: {
    mrs_total: number; approvals_checked: number; changes_checked: number;
    approvals_complete: boolean; changes_complete: boolean;
    current_approvals: number | null; current_change_requests: number | null;
    mean_first_current_approval_hours: number | null; timed_approved_mrs: number;
    reviewers: { username: string; approvals: number; change_requests: number }[];
    failures: string[]; rate_remaining: number | null; rate_reset: number | null;
  } | null;
  history: { source: Source; project: string; iid: number; title: string; url: string; author: string; state: string; created_at: string; merged_at: string | null }[];
}
export function useGitLabStatsTree(host: string, revision: number) {
  const account = useGitLabViewer();
  return useQuery({ queryKey: ["stats", "gitlab", host, "tree", revision, account], queryFn: async () => { const tree = await gitlabStatsTree(host); if (account !== undefined && tree.account !== account) throw new Error("GitLab account changed. Refresh account status."); return tree; }, enabled: account !== null, staleTime: 5 * 60_000, gcTime: 30 * 60_000, retry: false });
}
export function useGitLabStats(host: string, viewer: string | undefined, scope: GitLabScope, days: number, forceRefresh = false) {
  const force = useRef(forceRefresh);
  return useQuery({
    queryKey: ["stats", "gitlab", host, viewer, scope, days],
    queryFn: async () => {
      const refresh = force.current;
      force.current = false;
      const report = await gitlabStatsLoad(host, scope, days, refresh);
      // glab credentials can change between discovery and this request. Never
      // put the new account's result into the previous account's query key.
      if (report.viewer !== viewer || report.source.provider !== "gitlab" || report.source.host !== host) {
        throw new Error("GitLab account changed while loading statistics. Refresh to reload the account and scopes.");
      }
      return report;
    },
    enabled: viewer !== undefined, staleTime: 5 * 60_000, gcTime: 30 * 60_000, retry: false,
  });
}

export interface GitLabBackfill {
  source: Source; viewer: string; scope: GitLabScope; requested_days: number;
  complete_days: number; attempted_days: number; slices: (Pick<GitLabStatsReport, "start" | "fetched_at" | "counts" | "coverage"> & { merged_window: Pick<NonNullable<GitLabStatsReport["merged_window"]>, "count" | "coverage" | "fetched_at"> | null })[]; error: string | null;
}
export function useGitLabBackfill(host: string, viewer: string, scope: GitLabScope, days: number) {
  return useMutation({
    mutationFn: async () => {
      const result = await gitlabStatsBackfill(host, scope, days);
      if (result.viewer !== viewer || result.source.host !== host || result.source.provider !== "gitlab") {
        throw new Error("GitLab account changed while loading history. Refresh to reload the account and scopes.");
      }
      return result;
    },
  });
}

import type { JiraStatusCategory } from "@/types/jira";
import type { Lookup } from "./tree";

/// Text colour by status CATEGORY, never by status name.
export const CATEGORY_TEXT: Record<JiraStatusCategory, string> = {
  new: "text-[#8b949e]",
  in_progress: "text-[#58a6ff]",
  done: "text-[#3fb950]",
  unknown: "text-[#8b949e]",
};

/// What a key's answer says, for a heading or a row.
export function describe(key: string, lookup: Lookup): { title: string; status: string | null; category: JiraStatusCategory } {
  const s = lookup(key);
  if (s === undefined) return { title: "Loading…", status: null, category: "unknown" };
  if (s === "failed") return { title: "Could not load", status: null, category: "unknown" };
  if (s.state === "missing") return { title: "Cannot view this issue", status: null, category: "unknown" };
  if (s.state === "unasked") return { title: "Not checked yet", status: null, category: "unknown" };
  return { title: s.issue.summary, status: s.issue.status.name, category: s.issue.status.category };
}


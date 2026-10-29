/// Jira, as `src-tauri/src/jira/model.rs` reports it (epic #2031).

import type { Source } from "./identity";

/// Jira's own grouping of every status in every workflow. `unknown` is a
/// real state and never a stand-in for `new`: a category that did not come
/// back must not read as "To Do".
export type JiraStatusCategory = "new" | "in_progress" | "done" | "unknown";

export interface JiraStatus {
  name: string;
  category: JiraStatusCategory;
}

export interface JiraIssueType {
  name: string;
  /// -1 sub-task, 0 standard, 1 epic, 2+ initiative and above.
  hierarchy_level: number | null;
}

export interface JiraIssue {
  key: string;
  summary: string;
  issue_type: JiraIssueType;
  status: JiraStatus;
  assignee: string | null;
  /// Meaningful only when `parent_checked`.
  parent_key: string | null;
  /// Whether the parent has been read. `acli`'s search cannot return
  /// parents, so the desktop reads them a few at a time: `parent_key: null`
  /// with this false is "not known yet", never "has no epic".
  parent_checked: boolean;
  project_key: string;
  /// On the signed-in site; `null` until the site is known.
  url: string | null;
  updated: string | null;
}

export interface JiraProject {
  key: string;
  name: string;
}

export interface ProjectList {
  projects: JiraProject[];
  error: string | null;
}

/// Per key: found (`issues`), asked and not returned (`missing`), or never
/// asked because the call budget ran out (`unasked`).
export interface IssueBatch {
  issues: JiraIssue[];
  missing: string[];
  unasked: string[];
  error: string | null;
  truncated: boolean;
}

export type JiraConnection =
  | { state: "disabled" }
  | { state: "not_installed" }
  | { state: "not_logged_in"; detail: string }
  | { state: "logged_in"; site: string | null; account: string | null }
  | { state: "cannot_tell"; reason: string };

/// Where a key was found in a pull request or branch.
export type FoundIn = "title" | "branch";

export type PrRole = "authored" | "reviewing";

/// What names a key, from the desktop's index (`jira/index.rs`).
export type LinkTarget =
  | {
      kind: "pr";
      source: Source;
      role: PrRole;
      repo: string;
      number: number;
      title: string;
      url: string;
      head_ref: string;
      is_draft: boolean;
    }
  | { kind: "worktree"; repo: string; path: string; branch: string }
  | { kind: "branch"; repo_path: string; branch: string };

export interface JiraLink {
  key: string;
  found_in: FoundIn;
  /// In the latest read of where it lives. A pull request that is not
  /// current was open earlier and has left its list: merged or closed,
  /// unknown which.
  current: boolean;
  target: LinkTarget;
}

/// Every link whose key is in the user's projects, less exclusions.
export interface LinkIndex {
  links: JiraLink[];
  projects: string[];
  /// Why the project list could not be read; `links` is then empty.
  error: string | null;
}

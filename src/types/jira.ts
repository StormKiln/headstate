/// Jira, as `src-tauri/src/jira/model.rs` reports it (epic #2031).

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

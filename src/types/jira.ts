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

/// Mirrors `jira::model::JiraIssueType`.
export interface JiraIssueType {
  name: string;
  /// -1 sub-task, 0 standard, 1 epic, 2+ initiative and above.
  hierarchy_level: number | null;
  /// Through `acli` this names an internal Atlassian host that may not
  /// load without a session: a failed load is "no icon", not an error.
  /// Optional only so hand-built fixtures need not spell it; the desktop
  /// always sends it, `null` when Jira did not.
  icon_url?: string | null;
}

/// Mirrors `jira::model::JiraIssue`.
export interface JiraIssue {
  key: string;
  /// Jira's numeric id, which survives a key change. Optional only so
  /// hand-built fixtures need not spell it; the desktop always sends it.
  id?: string | null;
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
  /// Never filled through `acli`, whose search refuses the field: `null`
  /// is "not read", never "never updated".
  updated: string | null;
}

export interface JiraProject {
  key: string;
  name: string;
}

/// Why a Jira read did not happen, or did not finish (`JiraError` in
/// `jira/model.rs`; its wire spelling is pinned by
/// `every_error_state_has_its_wire_spelling` in `jira/commands.rs`).
///
/// `not_asked` is "we did not ask" -- Jira is switched off -- and never
/// "Jira did not answer" or "nothing found" (#1050). `rate_limited`,
/// `timed_out` and `not_logged_in` ended a read part-way; what they left
/// is `unasked`, not `missing`. `retry_after_secs: null` is "Jira did not
/// say when", not "retry now".
export type JiraError =
  | { state: "not_asked" }
  | { state: "not_installed"; searched: string[] }
  | { state: "not_logged_in" }
  | { state: "rate_limited"; retry_after_secs: number | null }
  | { state: "timed_out"; secs: number }
  | { state: "failed"; detail: string };

/// Mirrors `jira::model::ProjectList`.
export interface ProjectList {
  projects: JiraProject[];
  error: JiraError | null;
}

/// Per key: found (`issues`), asked and not returned (`missing`), or never
/// asked because the call budget ran out or the read was ended early
/// (`unasked`). Mirrors `jira::model::IssueBatch`.
export interface IssueBatch {
  issues: JiraIssue[];
  missing: string[];
  unasked: string[];
  /// Why the read is incomplete; set alongside whatever it did find.
  error: JiraError | null;
  truncated: boolean;
}

export type JiraConnection =
  | { state: "disabled" }
  | { state: "not_installed"; searched: string[] }
  | { state: "not_logged_in"; detail: string }
  | { state: "logged_in"; site: string | null; account: string | null }
  | { state: "cannot_tell"; reason: string };

/// Where a key was found in a pull request or branch.
export type FoundIn = "title" | "branch";

/// `searched`: found by searching GitHub for the key (#2041), not from the
/// user's lists.
export type PrRole = "authored" | "reviewing" | "searched";

/// Known only for a searched pull request.
export type PrState = "open" | "merged" | "closed";

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
      state: PrState | null;
      author: string | null;
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
/// Mirrors `jira::model::LinkIndex`.
export interface LinkIndex {
  links: JiraLink[];
  projects: string[];
  /// Why the project list could not be read; `links` is then empty.
  error: JiraError | null;
}

/// One provider's half of a ticket lookup (`jira/lookup.rs`). `not_asked`
/// is never "found nothing": it says why no search was made.
///
/// `asked` says what it covered: `scope` is the GitHub owners, or GitLab
/// projects as `host/path`, that were searched; `unsearched` the
/// candidates left out (past the cap, or a project that did not answer).
/// `total` is what the provider counted and `read` how many results were
/// read; `truncated` when more matched than were read -- with `total ===
/// read` when the provider did not count, which is then only a floor.
export type KeySearch =
  | { state: "not_asked"; reason: string }
  | {
      state: "asked";
      at: string;
      total: number;
      read: number;
      truncated: boolean;
      scope: string[];
      unsearched: string[];
    }
  | { state: "failed"; error: string };

export interface KeyLookup {
  key: string;
  links: JiraLink[];
  /// The GitHub half.
  search: KeySearch;
  /// The GitLab half; `null` when the user has no GitLab lists, so there
  /// was nothing of theirs to search.
  gitlab: KeySearch | null;
}

/// The statuses seen in one project's recent issues (`jira_project_statuses`).
/// `acli` cannot list a workflow's statuses, so this is a sample: a status
/// no recent issue holds is not here. `error` set means the read failed,
/// never that the project has no statuses.
export interface ProjectStatuses {
  project: string;
  statuses: JiraStatus[];
  /// The read hit its limit; older issues were not looked at.
  truncated: boolean;
  error: JiraError | null;
}

/// A category a ticket can be expected to be in. Not `unknown`: that is
/// something Jira reports, never something to want.
export type ExpectedCategory = Exclude<JiraStatusCategory, "unknown">;

/// What a ticket's status should be. A category matches any status in it;
/// a named status matches by name. Mirrors `jira::model::StatusExpectation`.
export type StatusExpectation =
  | { kind: "category"; category: ExpectedCategory }
  | { kind: "status"; name: string };

/// One project's expectations. `null` (or absent) is "not set": the drift
/// rules use in progress while open and done once merged.
export interface StatusExpectations {
  on_open?: StatusExpectation | null;
  on_merged?: StatusExpectation | null;
}

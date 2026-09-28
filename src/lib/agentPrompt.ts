import type { PrDetail, PullRequest } from "../types/pr";

/// The subset of a pull request needed to brief an agent.
///
/// Deliberately narrower than either `PullRequest` or `PrDetail`: both
/// satisfy it, so the kebab menu can compose from a list row and the
/// detail view can compose from its richer fetch, without two composers
/// that would drift the way M4's count-vs-list pair did.
export interface AgentContext {
  repo: string;
  number: number;
  title: string;
  url: string;
  head_ref: string;
  base_ref: string;
  merge_status: string;
  unresolved_threads: number;
  /// Only the detail view has per-check names and URLs. When absent the
  /// prompt says so rather than implying CI was clean.
  checks?: { name: string; state: string; url: string }[];
  /// The size of the change, from the detail view. Absent on a list row,
  /// and absent is not zero: the review criteria leave the size out
  /// rather than describing a change nobody measured.
  size?: { additions: number; deletions: number; changed_files: number };
  /// Whether the author wrote a description. Absent when unknown (a list
  /// row carries no body), which the criteria treat as "say nothing".
  has_description?: boolean;
  /// The repository's main checkout on this machine, when the scan found
  /// one (#1455). Named in the prompt so a COPIED prompt says where to
  /// start; a launched one already runs there.
  checkout?: string;
}

/// A check that failed. `state` is a raw GitHub value when unmodelled, so
/// this matches the two that mean failure rather than assuming anything
/// not "success" is broken -- `skipped` and `pending` are neither.
function failing(c: { state: string }): boolean {
  return c.state === "failure" || c.state === "error";
}

/// What this PR needs, as an instruction rather than a description.
///
/// The lead line is what an agent acts on, so it names the task instead
/// of restating the state: "Resolve the merge conflicts" beats "this PR
/// is DIRTY". Conflicts come first because nothing else can proceed
/// until they are resolved.
function lead(pr: AgentContext, failed: number): string {
  const ref = `${pr.repo}#${pr.number}`;
  if (pr.merge_status === "dirty") return `Resolve the merge conflicts on ${ref}`;
  if (failed > 0) return `Fix the failing CI on ${ref}`;
  if (pr.unresolved_threads > 0) return `Address the review feedback on ${ref}`;
  if (pr.merge_status === "behind") return `Update ${ref} with its base branch`;
  return `Review ${ref}`;
}

/// Explicit review criteria, adapted to what is known about the PR (#1455).
///
/// "Review X" on its own leaves the agent to invent a standard, and the
/// one it invents varies from run to run. These are the axes a human
/// reviewer works through, and each says what to LOOK FOR rather than
/// naming a topic -- "security" alone is a heading, not an instruction.
///
/// Adapted only where the data is real. A list row has no size, no body
/// and no per-check detail, so those lines are omitted there rather than
/// guessed: absent is not zero, and describing a change nobody measured
/// is the confident wrong statement the root rules forbid.
function reviewCriteria(pr: AgentContext): string[] {
  const out = ["", "Review it against these criteria, and report findings under each:"];
  out.push(
    "  1. Correctness and edge cases: does the code do what the PR says, including for",
    "     empty, missing, boundary and concurrent inputs?",
  );
  if (pr.has_description === false) {
    out.push("     The PR has no description, so infer the intent from the diff and say so.");
  }
  out.push(
    "  2. Security: untrusted input reaching a shell, path, query or file; secrets or",
    "     personal data in code, logs or fixtures; permission checks bypassed.",
    "  3. Performance: new work in hot paths or loops, unbounded reads, blocking calls",
    "     on async paths, repeated I/O that could be batched.",
    "  4. Tests: is the CHANGED behaviour covered, not merely some test touched? Would",
    "     the tests fail if the change were reverted?",
  );
  const pending = (pr.checks ?? []).filter((c) => c.state === "pending").length;
  if (pending > 0) {
    out.push(
      `     ${pending} check${pending === 1 ? " is" : "s are"} still running, so do not treat CI as passed.`,
    );
  }
  out.push(
    "  5. Documentation and comments: do comments, docs and names still describe what",
    "     the code now does?",
    `  6. Backwards compatibility and migrations: does anything relying on ${pr.base_ref}'s`,
    "     current behaviour, data or configuration break, and is a migration needed?",
    "  7. Error handling: are failures reported rather than swallowed, and is a partial",
    "     result kept rather than discarded?",
  );
  if (pr.size) {
    const { additions, deletions, changed_files } = pr.size;
    out.push(
      "",
      `The change is +${additions}/-${deletions} across ${changed_files} file${changed_files === 1 ? "" : "s"}.`,
    );
  }
  // A review is a report. An agent that "fixes" what it reviewed has
  // done a different job from the one the user pressed the button for.
  out.push("", "Do not push commits or post comments; report what you find.");
  return out;
}

/// Quote a word for a POSIX shell, but only when it needs it.
///
/// The setup lines at the end of the prompt are commands an agent runs,
/// and a branch name is chosen by whoever pushed it: git allows `;`, `$`
/// and quotes in ref names. Single quotes make every character literal
/// except `'`, which is closed, escaped and reopened -- the scheme
/// `worktrees/assess.rs`'s `shell_quote` uses. A plain name stays bare so
/// the common case reads naturally.
export function shellWord(s: string): string {
  return /^[A-Za-z0-9._/@%+=:,-]+$/.test(s) ? s : `'${s.replace(/'/g, "'\\''")}'`;
}

/// A prompt handing a pull request to a coding agent.
///
/// This is the loop the product is built around closing: an agent opens
/// the PR, Headstate surfaces that it broke, and this hands it back with
/// the context needed to fix it -- by clipboard, or since #1455 by
/// Claudify, which starts `claude` on this text in the main checkout.
export function agentPrompt(pr: AgentContext): string {
  const failed = (pr.checks ?? []).filter(failing);
  const task = lead(pr, failed.length);
  const out = [`${task} (${pr.head_ref} → ${pr.base_ref}).`, ""];

  // No separate "Branch:" line -- the lead already names the pair, and a
  // prompt that repeats itself wastes the agent's attention on nothing.
  out.push(`Title: ${pr.title}`, `URL: ${pr.url}`);

  if (failed.length > 0) {
    out.push("", "Failing checks:");
    for (const c of failed) out.push(`  - ${c.name}: ${c.url}`);
  } else if (pr.checks === undefined) {
    // Say nothing rather than implying CI passed: the list row has a CI
    // rollup but not the per-check detail, and a silent omission would
    // read as "nothing failed".
    out.push("", "Failing checks: not loaded — open the PR for details.");
  }

  if (pr.merge_status === "dirty") out.push("", "This branch has merge conflicts with its base.");
  if (pr.unresolved_threads > 0) {
    out.push(
      "",
      `Unresolved review conversations: ${pr.unresolved_threads} (visible on the PR page).`,
    );
  }

  // Criteria only when the task IS a review. Conflicts, CI and feedback
  // each have a concrete job, and a seven-point checklist appended to
  // "fix the build" would bury it.
  if (task.startsWith("Review ")) out.push(...reviewCriteria(pr));

  // HOW to start, last, because it is what the agent does first and a
  // reader scanning back finds it at the end.
  //
  // A worktree rather than a checkout in place: every destructive-
  // adjacent thing this app does works in one, and an agent switching
  // the user's current checkout to a PR branch is a surprise they did
  // not ask for -- especially with uncommitted work present.
  //
  // FROM THE MAIN CHECKOUT (#1455). The hint used to be a bare
  // `../pr-{n}`, relative to wherever the agent happened to start: from
  // inside another worktree it put the new one beside THAT worktree.
  // Claudify now starts the session in the main checkout and a copied
  // prompt names it, so the relative path has one meaning.
  const dir = `../${worktreeName(pr)}`;
  out.push(
    "",
    pr.checkout
      ? `Work in a new git worktree created from the main checkout at ${pr.checkout}, not in the checkout itself:`
      : "Work in a new git worktree created from the repository's main checkout (not from another worktree), not in the checkout itself:",
  );
  if (pr.checkout) out.push(`  cd ${shellWord(pr.checkout)}`);
  out.push(
    `  git fetch origin ${shellWord(pr.head_ref)}`,
    `  git worktree add ${shellWord(dir)} ${shellWord(pr.head_ref)}`,
    `  cd ${shellWord(dir)}`,
  );

  return out.join("\n");
}

/// A directory name for the PR's worktree.
///
/// Derived from the PR number rather than the branch: a branch name can
/// contain slashes, which would nest the worktree somewhere unexpected,
/// and the number is what the prompt already refers to throughout.
///
/// Prefixed with the repository's name (#1455). The worktree is a
/// SIBLING of the main checkout, and every checkout under one code
/// directory shares that parent, so two repositories' `pr-42` would be
/// the same directory. Only the part after the slash, and only when it
/// is a plain name, so a repository name cannot add a path segment.
export function worktreeName(pr: Pick<AgentContext, "repo" | "number">): string {
  const name = pr.repo.split("/").pop() ?? "";
  return /^[A-Za-z0-9._-]+$/.test(name) && !/^\.+$/.test(name)
    ? `${name}-pr-${pr.number}`
    : `pr-${pr.number}`;
}

/// Widen a list row or a detail into the shape `agentPrompt` needs.
///
/// A row has no `checks`, and that absence is meaningful -- see the
/// "not loaded" branch above -- so it is preserved rather than defaulted
/// to an empty array. `size` and `has_description` follow the same rule.
///
/// `checkout` is the main checkout the caller resolved from the scan, if
/// any; undefined leaves the prompt describing "the main checkout"
/// rather than naming a path nobody found.
export function toAgentContext(pr: PullRequest | PrDetail, checkout?: string): AgentContext {
  return {
    repo: pr.repo,
    number: pr.number,
    title: pr.title,
    url: pr.url,
    head_ref: pr.head_ref,
    base_ref: pr.base_ref,
    merge_status: pr.merge_status,
    unresolved_threads: pr.unresolved_threads,
    checks: "checks" in pr ? pr.checks : undefined,
    size:
      "changed_files" in pr
        ? { additions: pr.additions, deletions: pr.deletions, changed_files: pr.changed_files }
        : undefined,
    has_description: "body" in pr ? pr.body.trim() !== "" : undefined,
    checkout,
  };
}

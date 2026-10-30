import { useState } from "react";
import { toast } from "sonner";
import type { UiPrefs } from "@/api/tauri";
import {
  describeJiraError,
  failureOf,
  useJiraAcliVersion,
  useJiraConnection,
  useJiraProjects,
  useJiraProjectStatuses,
} from "@/api/jira";
import type { ExpectedCategory, JiraConnection, StatusExpectation, StatusExpectations } from "@/types/jira";
import { copyText } from "@/lib/clipboard";
import { DEFAULT_MERGE_GRACE_MINS, DEFAULT_ON_MERGED, DEFAULT_ON_OPEN, expectationLabel } from "@/lib/jira/drift";
import { branchName, DEFAULT_BRANCH_TEMPLATE, templateHasKey } from "@/lib/jira/branchName";
import { ArgvPreview } from "./ArgvPreview";
import { ExternalLink } from "./ExternalLink";
import { HelpButton } from "./HelpButton";

const INSTALL_URL = "https://developer.atlassian.com/cloud/acli/guides/install-acli/";
const LOGIN_ARGV = ["acli", "jira", "auth", "login", "--web"];
const LOGIN = LOGIN_ARGV.join(" ");

/// Settings › Jira (#2034, epic #2031).
///
/// A CAPABILITY switch, like Claude Code's: off, the integration spawns
/// nothing and every Jira read reports "not asked". On, this panel says
/// whether `acli` can answer, and when it cannot, the one thing to do about
/// it. Signing in is interactive, so Headstate shows the command rather than
/// running it.
export function JiraSettingsPanel({
  prefs,
  setPrefs,
}: {
  prefs: UiPrefs | undefined;
  setPrefs: (prefs: UiPrefs) => Promise<void>;
}) {
  const enabled = prefs?.jira_enabled ?? false;
  const excluded = prefs?.jira_excluded_projects ?? [];
  const connection = useJiraConnection(enabled);
  const signedIn = connection.data?.state === "logged_in";
  // Installed, as far as the connection check has said: asking the
  // version of an acli that is not there, or before Jira is on, would be
  // a spawn for nothing.
  const state = connection.data?.state;
  const installed = enabled && (state === "logged_in" || state === "not_logged_in" || state === "cannot_tell");
  const projects = useJiraProjects(enabled && signedIn);

  return (
    <div className="mt-5 flex flex-col gap-2">
      <span className="text-sm font-medium">
        Jira
        <HelpButton topic="jira" />
      </span>
      <label className="flex items-center gap-2 text-sm">
        <input
          type="checkbox"
          checked={enabled}
          onChange={() => prefs && void setPrefs({ ...prefs, jira_enabled: !prefs.jira_enabled })}
        />
        Link pull requests to Jira
      </label>
      <p className="text-xs text-[#8b949e]">
        A Jira key in a pull request&apos;s title or branch name, such as{" "}
        <code className="font-mono">ABC-1</code> or{" "}
        <code className="font-mono">feature/abc-1-fix-login</code>, links it to that issue.
        Jira Cloud only, through Atlassian&apos;s <code className="font-mono">acli</code>.
      </p>
      {enabled ? (
        <>
          <ConnectionStatus
            connection={connection.data}
            loading={connection.isPending}
            failed={connection.isError}
          />
          <AcliVersion asked={installed} />
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={prefs?.jira_chips_on_prs ?? true}
              onChange={() => prefs && void setPrefs({ ...prefs, jira_chips_on_prs: !prefs.jira_chips_on_prs })}
            />
            Show tickets on pull request rows
          </label>
          <p className="text-xs text-[#8b949e]">
            A pull request&apos;s own page, and worktree and branch rows, show their tickets either way.
          </p>
          {prefs ? <BranchTemplate key={prefs.jira_branch_template} prefs={prefs} setPrefs={setPrefs} /> : null}
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={prefs?.jira_no_ticket_marker ?? false}
              onChange={() => prefs && void setPrefs({ ...prefs, jira_no_ticket_marker: !prefs.jira_no_ticket_marker })}
            />
            Mark my pull requests that name no ticket
          </label>
          <p className="text-xs text-[#8b949e]">
            Adds a &ldquo;no ticket&rdquo; marker, a No ticket filter in My PRs, and a way to link a ticket by putting its key at
            the start of the title.
          </p>
          {prefs ? <MergeGrace prefs={prefs} setPrefs={setPrefs} /> : null}
          {signedIn ? (
            <div className="flex flex-col gap-1">
              <span className="text-sm font-medium">Projects</span>
              {projects.isPending ? (
                <span className="h-4 w-40 animate-pulse rounded bg-[#21262d]" aria-label="Loading projects" />
              ) : projects.isError || projects.data?.error ? (
                <p className="text-xs text-[#f85149]">
                  Could not list your Jira projects:{" "}
                  {describeJiraError(projects.data?.error ?? failureOf(projects.error))}
                </p>
              ) : (
                <>
                  <p className="text-xs text-[#8b949e]">
                    Keys are matched only for your {projects.data.projects.length} projects, so
                    look-alikes such as <code className="font-mono">UTF-8</code> are never
                    linked. Turn a project off if its key collides with your branch names.
                  </p>
                  <ul className="flex flex-wrap gap-1" aria-label="Jira projects">
                    {projects.data.projects.map((p) => {
                      const on = !excluded.includes(p.key);
                      return (
                        <li key={p.key}>
                          <button
                            type="button"
                            title={p.name}
                            aria-pressed={on}
                            onClick={() =>
                              prefs &&
                              void setPrefs({
                                ...prefs,
                                jira_excluded_projects: on
                                  ? [...excluded, p.key]
                                  : excluded.filter((k) => k !== p.key),
                              })
                            }
                            className={`rounded border px-1.5 py-0.5 font-mono text-xs ${
                              on
                                ? "border-[#30363d] text-[#e6edf3]"
                                : "border-dashed border-[#30363d] text-[#8b949e] line-through"
                            }`}
                          >
                            {p.key}
                          </button>
                        </li>
                      );
                    })}
                  </ul>
                  {prefs ? (
                    <StatusExpectationsEditor
                      projects={projects.data.projects.map((p) => p.key).filter((k) => !excluded.includes(k))}
                      prefs={prefs}
                      setPrefs={setPrefs}
                    />
                  ) : null}
                </>
              )}
            </div>
          ) : null}
        </>
      ) : null}
    </div>
  );
}

/// How "Start work" names a branch (#2045). Saved when the field loses
/// focus, not per keystroke; the proposal stays editable in the dialog.
function BranchTemplate({ prefs, setPrefs }: { prefs: UiPrefs; setPrefs: (prefs: UiPrefs) => Promise<void> }) {
  const [draft, setDraft] = useState(prefs.jira_branch_template);
  const example = branchName(draft, "ABC-1", "Fix login redirect");
  const save = () => {
    if (draft.trim() !== prefs.jira_branch_template) void setPrefs({ ...prefs, jira_branch_template: draft.trim() });
  };
  return (
    <div className="flex flex-col gap-1">
      <label className="flex flex-col gap-1 text-sm">
        Branch name for Start work
        <input
          value={draft}
          placeholder={DEFAULT_BRANCH_TEMPLATE}
          spellCheck={false}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={save}
          onKeyDown={(e) => e.key === "Enter" && save()}
          className="rounded border border-[#30363d] bg-[#0d1117] px-2 py-1 font-mono text-xs text-[#e6edf3]"
        />
      </label>
      <p className="text-xs text-[#8b949e]">
        <code className="font-mono">{"{key}"}</code> is the key in lower case,{" "}
        <code className="font-mono">{"{KEY}"}</code> as Jira writes it, and{" "}
        <code className="font-mono">{"{summary}"}</code> the summary as words joined by hyphens. ABC-1 &ldquo;Fix
        login redirect&rdquo; becomes <code className="font-mono">{example}</code>.
      </p>
      {templateHasKey(draft) ? null : (
        <p role="status" className="text-xs text-[#d29922]">
          Without the key, a branch named this way will not link to its ticket.
        </p>
      )}
    </div>
  );
}

function ConnectionStatus({
  connection,
  loading,
  failed,
}: {
  connection: JiraConnection | undefined;
  loading: boolean;
  failed: boolean;
}) {
  if (loading) {
    return <span className="h-4 w-48 animate-pulse rounded bg-[#21262d]" aria-label="Checking acli" />;
  }
  if (failed || connection === undefined) {
    return <p className="text-xs text-[#f85149]">Could not ask the desktop whether acli is signed in.</p>;
  }
  switch (connection.state) {
    case "logged_in":
      return (
        <p className="text-xs text-[#3fb950]">
          Signed in{connection.site ? ` to ${connection.site}` : ""}
          {connection.account ? ` as ${connection.account}` : ""}.
        </p>
      );
    case "not_installed":
      return (
        <p className="text-xs text-[#d29922]">
          acli is not installed.{" "}
          <ExternalLink href={INSTALL_URL} className="text-[#58a6ff] underline">
            Install Atlassian&apos;s CLI
          </ExternalLink>
          , then sign in.
          {connection.searched.length > 0 ? (
            <span className="block text-[#8b949e]">
              Looked in {connection.searched.join(", ")}. If it is elsewhere, set HEADSTATE_ACLI to its path.
            </span>
          ) : null}
        </p>
      );
    case "not_logged_in":
      return (
        <div className="flex flex-col gap-1 text-xs text-[#d29922]">
          <span>acli is installed but not signed in. Run this in a terminal:</span>
          <div className="flex items-start gap-2">
            <div aria-label="Sign-in command" className="min-w-0 flex-1">
              <ArgvPreview program={LOGIN_ARGV[0]} args={LOGIN_ARGV.slice(1)} />
            </div>
            <button
              type="button"
              className="mt-1 rounded border border-[#30363d] px-1.5 py-0.5 text-[#e6edf3] hover:bg-[#21262d]"
              onClick={() =>
                void copyText(LOGIN).then((err) => (err ? toast.error(err) : toast.success("Copied")))
              }
            >
              Copy
            </button>
          </div>
        </div>
      );
    case "cannot_tell":
      return (
        <p className="text-xs text-[#d29922]">
          Could not tell whether acli is signed in. It said: {connection.reason || "nothing"}
        </p>
      );
    case "disabled":
      // The switch was turned on a moment ago and the desktop has not
      // caught up; the next poll answers.
      return <span className="h-4 w-48 animate-pulse rounded bg-[#21262d]" aria-label="Checking acli" />;
  }
}

/// The installed acli's version. Not asked until the connection check has
/// found acli; renders nothing until then.
function AcliVersion({ asked }: { asked: boolean }) {
  const version = useJiraAcliVersion(asked);
  if (!asked) return null;
  if (version.isPending) {
    return <span className="h-3 w-32 animate-pulse rounded bg-[#21262d]" aria-label="Reading the acli version" />;
  }
  if (version.isError) {
    return <p className="text-xs text-[#8b949e]">Could not read the acli version: {describeJiraError(failureOf(version.error))}</p>;
  }
  return <p className="text-xs text-[#8b949e]">acli {version.data}</p>;
}

/// How long after a merge before "merged, but the ticket did not move" is
/// raised (#2042), so an automation that moves tickets on merge runs first.
/// Committed on blur, so typing "30" never stores "3" on the way.
function MergeGrace({ prefs, setPrefs }: { prefs: UiPrefs; setPrefs: (prefs: UiPrefs) => Promise<void> }) {
  const stored = prefs.jira_merge_grace_mins ?? DEFAULT_MERGE_GRACE_MINS;
  const [draft, setDraft] = useState<string | null>(null);
  const commit = () => {
    if (draft === null) return;
    const n = Number.parseInt(draft, 10);
    setDraft(null);
    if (Number.isFinite(n) && n >= 0 && n <= 1440 && n !== stored) void setPrefs({ ...prefs, jira_merge_grace_mins: n });
  };
  return (
    <label className="flex flex-wrap items-center gap-2 text-sm">
      Wait
      <input
        type="number"
        min={0}
        max={1440}
        value={draft ?? String(stored)}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => e.key === "Enter" && commit()}
        aria-label="Minutes to wait after a merge"
        className="w-16 rounded border border-[#30363d] bg-[#0d1117] px-1 py-0.5 text-xs text-[#e6edf3]"
      />
      minutes after a merge before flagging a ticket that did not move
    </label>
  );
}

const CATEGORIES: ExpectedCategory[] = ["new", "in_progress", "done"];

/// A `<select>` value for an expectation; "" is "not set".
function encode(e: StatusExpectation | null | undefined): string {
  if (!e) return "";
  return e.kind === "category" ? `category:${e.category}` : `status:${e.name}`;
}

function decode(v: string): StatusExpectation | null {
  if (v.startsWith("category:")) return { kind: "category", category: v.slice(9) as ExpectedCategory };
  if (v.startsWith("status:")) return { kind: "status", name: v.slice(7) };
  return null;
}

/// Per-project status expectations (#2034): what a ticket should say while
/// its pull request is open, and once it merged. Unset, the drift rules use
/// their defaults. A project's statuses are read only once it is chosen.
function StatusExpectationsEditor({
  projects,
  prefs,
  setPrefs,
}: {
  projects: string[];
  prefs: UiPrefs;
  setPrefs: (prefs: UiPrefs) => Promise<void>;
}) {
  const [project, setProject] = useState("");
  const all = prefs.jira_status_expectations ?? {};
  const set = (side: keyof StatusExpectations, value: StatusExpectation | null) => {
    const next: StatusExpectations = { ...all[project], [side]: value };
    const rest = Object.fromEntries(Object.entries(all).filter(([k]) => k !== project));
    void setPrefs({
      ...prefs,
      // An entry with nothing set is dropped, so the map holds only choices.
      jira_status_expectations: next.on_open || next.on_merged ? { ...rest, [project]: next } : rest,
    });
  };
  if (projects.length === 0) return null;
  return (
    <div className="mt-2 flex flex-col gap-1">
      <span className="text-sm font-medium">Status expectations</span>
      <p className="text-xs text-[#8b949e]">
        A ticket is flagged when it disagrees with its pull request. By default that means in progress while the
        pull request is open, and done once it merged.
      </p>
      <label className="flex items-center gap-2 text-xs">
        Project
        <select
          value={project}
          onChange={(e) => setProject(e.target.value)}
          className="rounded border border-[#30363d] bg-[#0d1117] px-1 py-0.5 font-mono text-xs text-[#e6edf3]"
        >
          <option value="">Choose a project</option>
          {projects.map((k) => (
            <option key={k} value={k}>
              {k}
              {all[k] ? " (set)" : ""}
            </option>
          ))}
        </select>
      </label>
      {project ? (
        <ProjectExpectations
          project={project}
          current={all[project] ?? {}}
          onChange={set}
        />
      ) : null}
    </div>
  );
}

function ProjectExpectations({
  project,
  current,
  onChange,
}: {
  project: string;
  current: StatusExpectations;
  onChange: (side: keyof StatusExpectations, value: StatusExpectation | null) => void;
}) {
  const seen = useJiraProjectStatuses(project, true);
  const names = [...new Set((seen.data?.statuses ?? []).map((s) => s.name))];
  const sides: [keyof StatusExpectations, string, StatusExpectation][] = [
    ["on_open", "While a pull request is open", DEFAULT_ON_OPEN],
    ["on_merged", "Once it has merged", DEFAULT_ON_MERGED],
  ];
  return (
    <div className="flex flex-col gap-1">
      {seen.isPending ? (
        <span className="h-3 w-48 animate-pulse rounded bg-[#21262d]" aria-label={`Reading ${project}'s statuses`} />
      ) : seen.isError || seen.data?.error ? (
        <p className="text-xs text-[#f85149]">
          The three categories still apply. Could not read the statuses in {project}&apos;s issues:{" "}
          {describeJiraError(seen.data?.error ?? failureOf(seen.error))}
        </p>
      ) : seen.data.truncated ? (
        <p className="text-xs text-[#8b949e]">
          Statuses are those seen in {project}&apos;s most recently updated issues; older issues were not read.
        </p>
      ) : null}
      {sides.map(([side, label, fallback]) => {
        const value = current[side] ?? null;
        // A chosen status no recent issue holds is still offered, so the
        // select shows what is stored rather than silently another value.
        const extra = value?.kind === "status" && !names.includes(value.name) ? [value.name] : [];
        return (
          <label key={side} className="flex flex-wrap items-center gap-2 text-xs">
            {label}
            <select
              value={encode(value)}
              onChange={(e) => onChange(side, decode(e.target.value))}
              className="rounded border border-[#30363d] bg-[#0d1117] px-1 py-0.5 text-xs text-[#e6edf3]"
            >
              <option value="">Default ({expectationLabel(fallback)})</option>
              <optgroup label="Categories">
                {CATEGORIES.map((c) => (
                  <option key={c} value={encode({ kind: "category", category: c })}>
                    {expectationLabel({ kind: "category", category: c })}
                  </option>
                ))}
              </optgroup>
              {names.length + extra.length > 0 ? (
                <optgroup label={`Statuses seen in ${project}'s issues`}>
                  {[...names, ...extra].map((n) => (
                    <option key={n} value={encode({ kind: "status", name: n })}>
                      {n}
                    </option>
                  ))}
                </optgroup>
              ) : null}
            </select>
          </label>
        );
      })}
    </div>
  );
}

import { toast } from "sonner";
import type { UiPrefs } from "@/api/tauri";
import { useJiraConnection, useJiraProjects } from "@/api/jira";
import type { JiraConnection } from "@/types/jira";
import { copyText } from "@/lib/clipboard";
import { ExternalLink } from "./ExternalLink";

const INSTALL_URL = "https://developer.atlassian.com/cloud/acli/guides/install-acli/";
const LOGIN = "acli jira auth login --web";

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
  const connection = useJiraConnection(enabled);
  const signedIn = connection.data?.state === "logged_in";
  const projects = useJiraProjects(enabled && signedIn);

  return (
    <div className="mt-5 flex flex-col gap-2">
      <span className="text-sm font-medium">Jira</span>
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
          {signedIn ? (
            <div className="flex flex-col gap-1">
              <span className="text-sm font-medium">Projects</span>
              {projects.isPending ? (
                <span className="h-4 w-40 animate-pulse rounded bg-[#21262d]" aria-label="Loading projects" />
              ) : projects.isError || projects.data?.error ? (
                <p className="text-xs text-[#f85149]">
                  Could not list your Jira projects: {projects.data?.error ?? "no answer from acli"}
                </p>
              ) : (
                <>
                  <p className="text-xs text-[#8b949e]">
                    Keys are matched only for these {projects.data.projects.length} projects, so
                    look-alikes such as <code className="font-mono">UTF-8</code> are never
                    linked.
                  </p>
                  <ul className="flex flex-wrap gap-1" aria-label="Jira projects">
                    {projects.data.projects.map((p) => (
                      <li
                        key={p.key}
                        title={p.name}
                        className="rounded border border-[#30363d] px-1.5 py-0.5 font-mono text-xs text-[#e6edf3]"
                      >
                        {p.key}
                      </li>
                    ))}
                  </ul>
                </>
              )}
            </div>
          ) : null}
        </>
      ) : null}
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
        </p>
      );
    case "not_logged_in":
      return (
        <div className="flex flex-col gap-1 text-xs text-[#d29922]">
          <span>acli is installed but not signed in. Run this in a terminal:</span>
          <span className="flex items-center gap-2">
            <code className="rounded border border-[#30363d] bg-[#0d1117] px-1 py-0.5 font-mono text-[#e6edf3]">
              {LOGIN}
            </code>
            <button
              type="button"
              className="rounded border border-[#30363d] px-1.5 py-0.5 text-[#e6edf3] hover:bg-[#21262d]"
              onClick={() =>
                void copyText(LOGIN).then((err) => (err ? toast.error(err) : toast.success("Copied")))
              }
            >
              Copy
            </button>
          </span>
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

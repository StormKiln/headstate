import { useSaveGitLabHost, type GitLabAuthCheck } from "@/api/gitlabAuth";
import { gitlabAuthRemedy } from "@/lib/gitlabAuthRemedy";
import { IS_MOBILE_BUILD } from "@/lib/target";
import { useSettingsDialog } from "@/store/settingsDialog";

const BUTTON = "rounded border border-[#30363d] px-1.5 py-0.5 text-[#e6edf3] hover:bg-[#21262d] disabled:opacity-50";

/// The actions for a GitLab sign-in that could not be verified. A retry
/// shows that it is running and when it last finished, so an identical
/// answer still reads as a check that happened rather than a dead button.
export function GitLabAuthRemedy({ auth }: { auth: GitLabAuthCheck }) {
  const save = useSaveGitLabHost();
  const openSettings = useSettingsDialog((s) => s.open);
  const remedy = gitlabAuthRemedy(auth.data, IS_MOBILE_BUILD);
  const checking = auth.isFetching;
  const checkedAt = Math.max(auth.dataUpdatedAt, auth.errorUpdatedAt);
  return (
    <span className="ml-2 inline-flex flex-wrap items-center gap-2">
      {remedy.useHost ? (
        <button type="button" className={BUTTON} disabled={save.isPending} onClick={() => save.mutate(remedy.useHost ?? "")}>
          {save.isPending ? `Saving ${remedy.useHost}…` : `Use ${remedy.useHost}`}
        </button>
      ) : null}
      {remedy.settings ? (
        <button type="button" className={BUTTON} onClick={() => openSettings("gitlab")}>Open GitLab settings</button>
      ) : null}
      {remedy.retry || (checking && auth.data === undefined) ? (
        <button type="button" className={BUTTON} disabled={checking} onClick={() => void auth.refetch()}>
          {checking ? "Checking GitLab authentication…" : "Retry GitLab authentication"}
        </button>
      ) : null}
      {!checking && checkedAt > 0 ? (
        <span className="text-xs opacity-80">Checked at {new Date(checkedAt).toLocaleTimeString()}.</span>
      ) : null}
      {save.isError ? <span role="alert" className="text-xs text-[#f85149]">{String(save.error)}</span> : null}
    </span>
  );
}

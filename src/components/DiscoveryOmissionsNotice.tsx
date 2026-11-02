import { useMemo, useState } from "react";
import { IS_MOBILE_BUILD } from "@/lib/target";
import { useSettingsDialog } from "@/store/settingsDialog";
import { groupOmissions, type Omission } from "./discoveryOmissions";

/// Projects listed per reason, and paths per project, until "Show all".
const PROJECTS_SHOWN = 5;
const PATHS_SHOWN = 3;

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/// The Artifacts page's "Discovery is incomplete" notice (#846, #1151).
///
/// One qualified line, collapsed. Every omitted location used to be a row,
/// and a scan can report hundreds: the notice buried the page it was
/// qualifying. The count stays on the summary line -- the list is still a
/// floor, and the reader is told so -- and the detail is one click away,
/// grouped by reason and project and capped until "Show all".
export function DiscoveryOmissionsNotice({
  omissions,
  scanDirs,
}: {
  omissions: readonly Omission[];
  scanDirs: readonly string[];
}) {
  const [open, setOpen] = useState(false);
  const [all, setAll] = useState(false);
  const openSettings = useSettingsDialog((s) => s.open);
  const groups = useMemo(
    () => (open ? groupOmissions(omissions, scanDirs) : []),
    [open, omissions, scanDirs],
  );
  const capped = groups.some(
    (g) => g.projects.length > PROJECTS_SHOWN || g.projects.some((p) => p.paths.length > PATHS_SHOWN),
  );
  return (
    <div
      className="mx-4 mt-3 rounded-md border border-[#d29922]/40 bg-[#d29922]/5 px-3 py-2 text-xs text-[#d29922]"
      role="status"
    >
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <p className="font-semibold">
          Discovery is incomplete in {plural(omissions.length, "location")}. Unlisted output may still use disk space.
        </p>
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen((v) => !v)}
          className="underline hover:text-[#e6edf3]"
        >
          {open ? "Hide details" : "Show details"}
        </button>
      </div>
      <p className="mt-0.5 text-[#8b949e]">
        Add a deeper folder in Settings → Repositories{IS_MOBILE_BUILD ? " on the desktop" : ""} for a targeted scan.
        {IS_MOBILE_BUILD ? null : (
          <button
            type="button"
            onClick={() => openSettings("repositories")}
            className="ml-2 underline hover:text-[#e6edf3]"
          >
            Open repository settings
          </button>
        )}
      </p>
      {open && (
        <div className="mt-2 space-y-2">
          {groups.map((g) => (
            <div key={g.why}>
              <p className="font-medium">
                {g.why} — {plural(g.count, "location")}
              </p>
              <ul className="mt-0.5 space-y-1 pl-3">
                {(all ? g.projects : g.projects.slice(0, PROJECTS_SHOWN)).map((p) => {
                  // A path no scan folder holds is its own "project": one row.
                  if (p.paths.length === 1 && p.paths[0] === p.project) {
                    return (
                      <li key={p.project} className="break-all text-[#c9d1d9]">
                        {p.project}
                      </li>
                    );
                  }
                  const shown = all ? p.paths : p.paths.slice(0, PATHS_SHOWN);
                  return (
                    <li key={p.project} className="break-all text-[#8b949e]">
                      <span className="text-[#c9d1d9]">{p.project}</span> — {plural(p.paths.length, "location")}
                      <ul className="pl-3">
                        {shown.map((path) => (
                          <li key={path}>{path}</li>
                        ))}
                        {shown.length < p.paths.length && <li>and {p.paths.length - shown.length} more</li>}
                      </ul>
                    </li>
                  );
                })}
                {!all && g.projects.length > PROJECTS_SHOWN && (
                  <li className="text-[#8b949e]">
                    and {plural(g.projects.length - PROJECTS_SHOWN, "more project")}
                  </li>
                )}
              </ul>
            </div>
          ))}
          {capped && (
            <button type="button" onClick={() => setAll((v) => !v)} className="underline hover:text-[#e6edf3]">
              {all ? "Show fewer" : `Show all ${plural(omissions.length, "location")}`}
            </button>
          )}
        </div>
      )}
    </div>
  );
}

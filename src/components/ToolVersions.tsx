import { useToolVersions } from "@/api/hooks";
import type { ToolReport, ToolVersion } from "@/api/tauri";

/// What each tool's state means, in one sentence (#1154).
///
/// Three states, not two, and that is the point: `install.rs:341`
/// already argues it for Claude Code -- "`CannotTell` is the state that
/// must not render as `NotInstalled`" -- and the same holds here. A
/// version we could not READ is not one that is too old, and neither is
/// a missing binary. Three remedies, three sentences.
function say(v: ToolVersion): { text: string; tone: string } {
  switch (v.state) {
    case "ok":
      return { text: v.found, tone: "text-[#8b949e]" };
    case "tooOld":
      // BOTH numbers. "2.30 is older than the 2.41 this needs" is
      // actionable; "your git is too old" is not.
      return {
        text: `${v.found} — older than the ${v.required} this needs`,
        tone: "text-[#d29922]",
      };
    case "notFound":
      return { text: "not found", tone: "text-[#8b949e]" };
    case "cannotTell":
      // Amber, like tooOld, because it is a thing to look at -- but
      // worded so nobody upgrades on the strength of it.
      return { text: `could not tell (${v.detail})`, tone: "text-[#d29922]" };
  }
}

/// The external tools this machine has, and what each is for.
///
/// Headstate detected whether these EXIST and never which version, so a
/// too-old tool failed at the point of use with that tool's own error.
/// The app relies on modern flags, and on an old binary those produce
/// empty output that callers read as absent data.
export function ToolVersions() {
  // Through the hooks layer rather than `useQuery` directly, so the
  // component has one seam its tests can mock -- the same reason every
  // other panel in this dialog reads its data that way.
  const { data, isError } = useToolVersions();

  // A failed probe renders nothing rather than "not found" for
  // everything: that would be four confident wrong answers at once.
  if (isError || !data) return null;

  return (
    <div className="mt-3">
      <p className="text-xs font-semibold text-[#e6edf3]">External tools</p>
      <ul className="mt-1 space-y-1">
        {data.map((t: ToolReport) => {
          const s = say(t.version);
          return (
            <li key={t.name} className="text-xs">
              <span className="text-[#e6edf3]">{t.name}</span>{" "}
              <span className={s.tone}>{s.text}</span>
              {/* What stops working without it. The tools are not equal:
                  no docker costs one page, no git costs the worktree and
                  branch views entirely -- and a user deciding whether to
                  act needs that, not just a version. */}
              {t.version.state !== "ok" ? (
                <span className="ml-1 text-[#8b949e]">— {t.matters}</span>
              ) : null}
              {/* The resolved PATH, and only when it is surprising: a
                  working tool at an expected location is noise. */}
              {t.path && t.version.state !== "ok" ? (
                <span className="ml-1 text-[10px] text-[#6e7681]">{t.path}</span>
              ) : null}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

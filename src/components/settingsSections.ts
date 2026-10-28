/// Settings' left-rail topics, in the order shown. Apart from
/// `SettingsDialog` so that file exports only components (#1965), and so
/// the phone's list can be tested without rendering the dialog.
///
/// Grouped by what a setting is ABOUT rather than by which struct it
/// lives in: "General" holds the poll interval, window behaviour and
/// keyboard notes, which come from three preference sources and are one
/// subject to a person.
const SECTIONS = [
  { id: "general", label: "General" },
  { id: "repositories", label: "Repositories" },
  { id: "gitlab", label: "GitLab" },
  { id: "notifications", label: "Notifications" },
  { id: "cleanup", label: "Cleanup" },
  { id: "phone", label: "Phone" },
  // Its own topic rather than a row under Views, because the switch is a
  // CAPABILITY and not a view preference -- and because the section carries
  // the hook installer, which edits a file outside this app (#915). Placed
  // before Views so the last entry stays the one that is about nothing but
  // what is on screen.
  { id: "claude", label: "Claude Code" },
  { id: "jira", label: "Jira" },
  { id: "views", label: "Views" },
] as const;

export type SectionId = (typeof SECTIONS)[number]["id"];

/// Topics configured only on the desktop: GitLab's host and Jira both lean
/// on a CLI (`glab`, `acli`) that runs there, and the phone holds neither
/// credential (#2047).
const DESKTOP_ONLY: ReadonlySet<SectionId> = new Set<SectionId>(["gitlab", "jira"]);

/// The topics this build offers.
export function settingsSections(mobileBuild: boolean) {
  return SECTIONS.filter((s) => !mobileBuild || !DESKTOP_ONLY.has(s.id));
}

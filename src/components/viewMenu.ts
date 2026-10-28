import { Activity, Bot, Container, Eye, FileText, FolderGit2, FolderTree, GitBranch, GitPullRequest, HardDrive, Package, Ticket } from "lucide-react";
import { type View, viewLabel } from "../store/filters";
import type { UiPrefs } from "../api/tauri";

/// The view menu's tables: groups, entries and the always-offered set.
/// Read by `ViewSwitcher.tsx` and `SettingsDialog.tsx`; apart from the
/// component so that file exports only components (#1965), which is what
/// lets dev hot-reload swap it in place.

/// The menu's groups, in the order their headings appear (#1017).
///
/// Declared as a const tuple so `Group` derives from it, the same way
/// `View` derives from `ALL_VIEWS`: one list, and a type that cannot
/// name a group the menu does not render.
///
/// The order is the menu's order. `VIEWS` is grouped by it at render
/// time rather than being stored pre-sorted, so the two cannot disagree
/// about where a heading goes.
export const GROUPS = [
  { id: "pull-requests", label: "Pull requests" },
  // "Repos", not "Repositories", and #1023 is what forced the choice.
  //
  // #1017 labelled this group "Repositories" while it held two views
  // neither of which was called that. The third one IS (#1011's grouping
  // names it), so the menu would render a "Repositories" heading with a
  // "Repositories" item beneath it -- a heading and one of its own
  // children indistinguishable by name, which is ambiguous to a reader
  // and genuinely unresolvable to a screen reader querying by
  // accessible name.
  //
  // The epic's own grouping spells it "Repos" for exactly this reason:
  //
  //     Repos    Worktrees . Branches . Repositories (NEW)
  //
  // The group renamed rather than the view, because the view's name is
  // the user-facing feature and matches its header, its README section
  // and `SettingsDialog`'s checkbox -- three places #794's rule says must
  // agree. A group label is internal navigation furniture with one call
  // site, so it is the cheaper of the two to move.
  { id: "repos", label: "Repos" },
  { id: "builds", label: "Builds" },
  { id: "ai", label: "AI" },
  // Last, for the reason `system-health` was already last: it is the
  // only entry that is not about the user's code.
  { id: "system", label: "System" },
] as const;

export type Group = (typeof GROUPS)[number]["id"];

/// Every view, in sidebar order, with the label, icon and GROUP each needs.
///
/// Exported because `SettingsDialog` offers these as hide/show
/// checkboxes and previously kept its OWN hand-written list. That list
/// carried four of the nine views there were then, so five could not be
/// hidden at all and nothing said so -- the section simply looked
/// complete (#675). One array, one order, one set of labels, which is why
/// #794's tenth view needed no edit there.
///
/// `group` is REQUIRED rather than optional, and that is the whole guard
/// (#1024). Until #1017 the render was `offered.map(...)`, so every view
/// that survived the filter reached the menu by construction. Grouping
/// removes that property: the loop is now over groups, so a view in no
/// group is a view in no menu, and a partial list looks exactly like a
/// complete one -- which is #675 again, one structure further in. A
/// required field makes the omission a compile error instead of a
/// silently absent entry.
///
/// `SettingsDialog.tsx` still maps this flat array for its checkboxes:
/// the grouping is applied at render, so the array stays one list rather
/// than becoming a nested structure every consumer has to walk.
/// The menu entries, with their labels DERIVED rather than declared.
///
/// `label` used to be written out here beside the icon, which made this
/// the second hand-maintained table over `View` -- and the two drifted
/// on four of twelve entries (#1185). `viewLabel` is now the only place
/// a view's name is written, so a change lands in the menu and the page
/// header together and cannot land in one alone.
///
/// The remaining fields are genuinely this file's: an icon and a group
/// mean nothing to the store.
const VIEW_ENTRIES: { id: View; Icon: typeof GitPullRequest; group: Group }[] = [
  { id: "my-prs", Icon: GitPullRequest, group: "pull-requests" },
  { id: "to-review", Icon: Eye, group: "pull-requests" },
  { id: "jira", Icon: Ticket, group: "pull-requests" },
  { id: "worktrees", Icon: FolderGit2, group: "repos" },
  { id: "branches", Icon: GitBranch, group: "repos" },
  // Third in the Repos group, after the two views about a checkout's
  // STATE (#1023, epic #1011). This one answers "what is in it", which is
  // why it sits with them rather than anywhere else -- the group is the
  // set of views over the same checkouts.
  //
  // `group` is required as of #1024, and that requirement is what makes
  // this entry safe to add: a new view whose group was forgotten does not
  // silently vanish from a grouped menu, the compiler demands it.
  { id: "repositories", Icon: FolderTree, group: "repos" },
  { id: "docker", Icon: Container, group: "builds" },
  { id: "artifacts", Icon: HardDrive, group: "builds" },
  { id: "packages", Icon: Package, group: "builds" },
  { id: "claude-md", Icon: FileText, group: "ai" },
  // Offered only while `claude_integrations_enabled` is on -- see the
  // `capabilityOff` check below for why that is not a `hidden_views` entry.
  // With grouping this is also what can empty the "AI" group, which is
  // why the render drops a group with no visible members (#1018).
  { id: "claude-code", Icon: Bot, group: "ai" },
  // Last, and deliberately so: it is the only entry that is not about
  // the user's code at all. Grouping it with the repo-scoped views
  // would imply it takes a repository, which it does not -- and now that
  // the menu has headings, its own group says that outright.
  { id: "system-health", Icon: Activity, group: "system" },
];

/// The menu, with each entry's label resolved from the one table.
///
/// Derived here rather than at every call site so `VIEWS` keeps the
/// shape its readers already expect -- `ViewSwitcher` itself, and the
/// tests that assert against `.label`.
export const VIEWS: { id: View; label: string; Icon: typeof GitPullRequest; group: Group }[] =
  VIEW_ENTRIES.map((e) => ({ ...e, label: viewLabel(e.id) }));


/// Views that are offered whatever `hidden_views` says.
///
/// Only "my-prs": it is the default view and the app's whole premise,
/// so hiding it would leave someone with no way back to what they
/// installed this for. The CURRENT view is also always offered, but
/// that is a function of where the user happens to be rather than a
/// property of the view, so it stays a separate check below.
///
/// Exported so `SettingsDialog` does not offer a checkbox that cannot
/// do anything: unhideable here means no toggle there, rather than a
/// control that appears to work and silently does not.
export const ALWAYS_OFFERED: ReadonlySet<View> = new Set<View>(["my-prs"]);

/// Whether `id` is a view whose CAPABILITY is switched off -- not hidden by
/// preference, but with no page behind it. Read by `ViewSwitcher` (no menu
/// entry) and `App` (move off it), so the two cannot disagree. See the
/// `capabilityOff` note in `ViewSwitcher` for why this overrides both of
/// that file's escape hatches (#916).
export function capabilityOff(id: View, prefs: UiPrefs | undefined): boolean {
  return (
    (id === "claude-code" && !prefs?.claude_integrations_enabled) ||
    (id === "jira" && !prefs?.jira_enabled)
  );
}

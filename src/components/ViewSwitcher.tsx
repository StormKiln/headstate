import { Activity, BarChart3, Bot, ChevronDown, Container, Eye, FileText, FolderGit2, FolderTree, GitBranch, GitPullRequest, HardDrive, Package } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { MOBILE_HIDDEN_VIEWS, type View, useFilters, viewLabel } from "../store/filters";
import { useUiPrefs } from "../api/hooks";
import { IS_MOBILE_BUILD } from "../lib/target";
import { current } from "../lib/ariaCurrent";
import { useIsMobile } from "../lib/useIsMobile";

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
  // "Pull requests" leads because `my-prs` is the default view and the
  // app's premise, and because #823's PR Stats-first rule is a statement
  // about this group's contents leading the menu.
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
  // FIRST in the menu, per #823 -- and still first once grouped, because
  // it leads the group that leads `GROUPS`. See `ALL_VIEWS` in
  // `store/filters.ts` for why this leads; note that since #1017 the two
  // lists are no longer kept in a shared order, so what a user sees is
  // this array read group by group rather than top to bottom.
  //
  // "PR Stats", not "Stats" (#794). The bare word had the sidebar's
  // context to lean on -- it sat under a list of repositories with open
  // pull requests in them. In a flat menu beside "System health" it
  // would read as stats about the machine, which is the one thing it is
  // not about.
  { id: "pr-stats", Icon: BarChart3, group: "pull-requests" },
  { id: "my-prs", Icon: GitPullRequest, group: "pull-requests" },
  { id: "to-review", Icon: Eye, group: "pull-requests" },
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

/// The top-level view control, at the head of the sidebar.
///
/// Collapsed it names the CURRENT view; expanded it lists them all. It
/// replaces the "Awaiting your review" entry that was pinned to the
/// sidebar's bottom, which was a flat list masquerading as a peer of the
/// repo rows.
///
/// PR Stats joined this menu in #794, reversing the rule that used to
/// stand here: that Stats was a panel of My PRs rather than a view, and
/// that listing it here would imply it has its own repo sidebar. Both
/// halves were true; the conclusion was wrong.
///
/// What decided it is that the pinned row was the only navigation in the
/// app that was not in this menu, so "where do I go to see something
/// else" had two answers -- and the bottom-left corner is where a
/// reader looks last. Being a sub-page of My PRs was an implementation
/// fact (`panel`), not something the user could see: nothing about the
/// stats page is scoped to the My PRs list, and the rest of `panel`
/// (Docker's images-versus-builds) is a genuine tab pair in a way
/// list-versus-whole-account-summary never was.
///
/// The implication about the sidebar was the real question, and it has now
/// been answered twice. #794 gave PR Stats the `RepoSidebar` it had
/// inherited as a panel, on the reasoning that an inert repo list was a
/// smaller lie than the blank column the issue offered -- while recording
/// that `StatsPage` did not read `filters.repo`, so the rows were
/// continuity and "a future scope hook, not a live filter", and that this
/// was "worth revisiting if PR Stats is ever scoped per repo, at which
/// point these rows stop being decoration".
///
/// #825 is that revisit, and the prediction held: PR Stats now has its own
/// `StatsSidebar`, and the rows are live. The column is a GitHub-sourced
/// hierarchy of organisations, their repositories and their members, so
/// "how is my team doing?" (#823's second audience) is askable from it. The
/// old rows could not express that -- they came from `repoCounts(prs)`,
/// repositories where the viewer has an OPEN PR, which holds neither an
/// organisation nor a person and omits any repository that is quiet today.
///
/// What survives from #794 unchanged: a selection writes to `pr-stats`'s
/// OWN filter set, which is why the view has an entry in `EMPTY_FILTERS`.
/// The keys are `statsScopeKind` / `statsScopeValue` / `statsSubject` now
/// rather than `repo`, and they are navigation rather than filters -- see
/// `activeFilterCount`, which excludes them for the same reason it excludes
/// `repo`.
export function ViewSwitcher({ counts }: { counts?: Partial<Record<View, number>> }) {
  const { view: storedView, setView } = useFilters();
  // The SAME fallback `App.tsx` applies, and it has to be the same or the
  // collapsed control names a page that is not on screen: the companion
  // renders My PRs for a stored `pr-stats` (the view is declined, not
  // rewritten, so the desktop sharing the store keeps it), and a button
  // reading "PR Stats" above the PR list is worse than either.
  //
  // Derived here rather than passed in as a prop: five sidebars render
  // this component, and a prop would be five call sites that have to
  // remember. One rule, read from the store, in both places that route on
  // it.
  const view =
    IS_MOBILE_BUILD && MOBILE_HIDDEN_VIEWS.has(storedView) ? "my-prs" : storedView;
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const currentView = VIEWS.find((v) => v.id === view) ?? VIEWS[0];
  // A LAYOUT question, so the hook rather than `IS_MOBILE_BUILD` (#1020).
  const isMobile = useIsMobile();
  const { prefs } = useUiPrefs();
  // Two views are never hidden, whatever is stored:
  //
  // - "my-prs" is the default view and the app's whole premise. Hiding
  //   it would leave someone with no way back to what they installed
  //   this for.
  // - The CURRENT view, even when hidden, or the app would show a page
  //   its own switcher says does not exist -- with no way off it.
  const hidden = new Set(prefs?.hidden_views ?? []);
  // The build-time set is checked FIRST and overrides both escape
  // hatches above. A view the companion does not ship is not hidden by
  // preference -- it does not exist in this bundle, so "but it is the
  // current view" cannot make it offerable: the phone has no page to
  // show behind the entry. `App.tsx` is what keeps `view` off such a
  // value in the first place, so the two cannot disagree about what is
  // on screen.
  // A CAPABILITY check, in the same position and for the same reason as
  // the build-time set above: it overrides both escape hatches (#916).
  //
  // The distinction from `hidden_views` is the whole point. That list means
  // "I do not want to see this", so honouring it loosely is correct -- a
  // user sitting on a view they hid keeps it rather than being thrown off
  // mid-task. A switched-off integration is not that: there is no page
  // behind the entry, so "but it is the current view" must not make it
  // offerable, exactly as for a view the companion does not ship.
  //
  // Routing this through `hidden_views` instead would also clobber a real
  // preference -- someone who hid the Claude Code view for their own
  // reasons would lose that the first time the capability toggled.
  const capabilityOff = (id: View) =>
    id === "claude-code" && !prefs?.claude_integrations_enabled;
  const offered = VIEWS.filter(({ id }) =>
    (IS_MOBILE_BUILD && MOBILE_HIDDEN_VIEWS.has(id)) || capabilityOff(id)
      ? false
      : ALWAYS_OFFERED.has(id) || id === view || !hidden.has(id),
  );

  // Group AFTER filtering, and drop a group with no surviving members
  // (#1018).
  //
  // The order matters and is the whole of the issue. The natural refactor
  // -- iterate the groups, collect each one's views, filter inside the
  // loop -- renders the heading before it knows whether anything is left
  // under it, and an "AI" heading with nothing beneath it is a menu
  // telling the user about a feature they cannot reach. Filtering first
  // and keeping only non-empty groups makes that state unrepresentable
  // rather than merely unlikely.
  //
  // This is reachable without any capability. `hidden_views` can empty
  // Repositories, Builds and System outright -- none of their members is
  // in `ALWAYS_OFFERED`. Only Pull requests is protected, and only
  // because `my-prs` is. The Claude capability is the second route to
  // the same state, not the only one.
  //
  // The saving property is that the CURRENT view is always offered, so
  // the group the user is standing in can never be the empty one. That
  // is what makes "render nothing" safe: dropping a group can never drop
  // the way back to where you are.
  //
  // A view whose `group` is not in `GROUPS` cannot exist -- `Group` is
  // derived from `GROUPS` -- so this loses nothing the type permits.
  const visibleGroups = GROUPS.map((g) => ({
    ...g,
    members: offered.filter((v) => v.group === g.id),
  })).filter(({ members }) => members.length > 0);

  // Dismiss on Escape and on a click elsewhere. Without both, the menu
  // stays open behind whatever the user does next.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    const onClick = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("mousedown", onClick);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("mousedown", onClick);
    };
  }, [open]);

  return (
    <div ref={ref} className="relative mb-2">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        aria-haspopup="menu"
        className="flex w-full items-center gap-2 rounded px-3 py-2 text-sm font-semibold text-[#e6edf3] hover:bg-[#161b22]"
      >
        <currentView.Icon className="h-4 w-4 shrink-0" aria-hidden="true" />
        <span className="truncate">{currentView.label}</span>
        <ChevronDown
          className={`ml-auto h-3.5 w-3.5 shrink-0 transition-transform ${
            open ? "rotate-180" : ""
          }`}
          aria-hidden="true"
        />
      </button>

      {open ? (
        <div
          role="menu"
          className="absolute left-0 right-0 top-full z-20 mt-1 rounded border border-[#30363d] bg-[#161b22] p-1 shadow-lg"
        >
          {visibleGroups.map(({ id: groupId, label: groupLabel, members }) => {
            const headingId = `view-group-${groupId}`;
            return (
              <div
                key={groupId}
                // `role="group"` with `aria-labelledby`, NOT a bare `<h3>`
                // (#1022). `role="menu"`'s content model admits `menuitem`,
                // `menuitemradio`, `menuitemcheckbox`, `group` and
                // `separator` -- a heading element is not among them, so an
                // `<h3>` dropped straight into the menu is a child a screen
                // reader is entitled to ignore, taking the label with it.
                // Wrapping the members in a `group` and pointing it at the
                // heading keeps the label attached to the items it names.
                role="group"
                aria-labelledby={headingId}
              >
                {/* Painted on every layout (#1403). #1020 hid these on the
                    narrow layout, reasoning that "the phone's sheet has
                    288px" of vertical space for five headings to crowd --
                    but 288px is the sheet's WIDTH (`w-72` in `App.tsx`); a
                    left sheet spans the full viewport height. Five headings
                    cost ~100px of it, and they are what makes a 13-entry
                    menu scannable, most of all on a phone.

                    Slightly tighter on the narrow layout -- less space above
                    each heading -- which is the one part of #1020's instinct
                    worth keeping. `useIsMobile()` rather than
                    `IS_MOBILE_BUILD`, per `lib/target.ts`: this is layout,
                    and a desktop window dragged narrow gets the same. */}
                <h3
                  id={headingId}
                  className={`px-2 pb-0.5 ${isMobile ? "pt-1" : "pt-1.5"} text-xs font-semibold uppercase tracking-wide text-[#8b949e]`}
                >
                  {groupLabel}
                </h3>
                {members.map(({ id, label, Icon }) => (
                  <button
                    key={id}
                    type="button"
                    role="menuitem"
                    // `current(...)` from `@/lib/ariaCurrent` (#977, adopted
                    // here in #1022). This was `aria-current={id === view}`,
                    // which React serialises to the literal string "false"
                    // on every non-current item -- announced by some
                    // readers, where the attribute's ABSENCE is how "not
                    // current" is spelled. `ViewSwitcher` was the one
                    // navigation list that never adopted the shared helper,
                    // because it already had the attribute and so did not
                    // look broken.
                    aria-current={current(id === view)}
                    onClick={() => {
                      setView(id);
                      setOpen(false);
                    }}
                    className={`flex w-full items-center gap-2 rounded px-2 py-1.5 text-sm ${
                      id === view ? "bg-[#1f6feb] text-white" : "text-[#e6edf3] hover:bg-[#21262d]"
                    }`}
                  >
                    <Icon className="h-4 w-4 shrink-0" aria-hidden="true" />
                    <span className="truncate">{label}</span>
                    {counts?.[id] ? (
                      <span className="ml-auto text-xs tabular-nums">{counts[id]}</span>
                    ) : null}
                  </button>
                ))}
              </div>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}

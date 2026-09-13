import { Activity, BarChart3, Bot, ChevronDown, Container, Eye, FileText, FolderGit2, GitBranch, GitPullRequest, HardDrive, Package } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { MOBILE_HIDDEN_VIEWS, type View, useFilters } from "../store/filters";
import { useUiPrefs } from "../api/hooks";
import { IS_MOBILE_BUILD } from "../lib/target";

/// Every view, in sidebar order, with the label and icon each needs.
///
/// Exported because `SettingsDialog` offers these as hide/show
/// checkboxes and previously kept its OWN hand-written list. That list
/// carried four of the nine views there were then, so five could not be
/// hidden at all and nothing said so -- the section simply looked
/// complete (#675). One array, one order, one set of labels, which is why
/// #794's tenth view needed no edit there.
export const VIEWS: { id: View; label: string; Icon: typeof GitPullRequest }[] = [
  // FIRST in the menu, per #823 -- the order here is what the user sees,
  // and `ALL_VIEWS` in `store/filters.ts` is kept in step so the two read
  // alike. See that list for why this leads.
  //
  // "PR Stats", not "Stats" (#794). The bare word had the sidebar's
  // context to lean on -- it sat under a list of repositories with open
  // pull requests in them. In a flat menu beside "System health" it
  // would read as stats about the machine, which is the one thing it is
  // not about.
  { id: "pr-stats", label: "PR Stats", Icon: BarChart3 },
  { id: "my-prs", label: "My pull requests", Icon: GitPullRequest },
  { id: "to-review", label: "To review", Icon: Eye },
  { id: "worktrees", label: "Worktrees", Icon: FolderGit2 },
  { id: "branches", label: "Branches", Icon: GitBranch },
  { id: "docker", label: "Docker", Icon: Container },
  { id: "artifacts", label: "Artifacts", Icon: HardDrive },
  { id: "packages", label: "Package updates", Icon: Package },
  { id: "claude-md", label: "CLAUDE.md", Icon: FileText },
  // Offered only while `claude_integrations_enabled` is on -- see the
  // `capabilityOff` check below for why that is not a `hidden_views` entry.
  { id: "claude-code", label: "Claude Code", Icon: Bot },
  // Last, and deliberately so: it is the only entry that is not about
  // the user's code at all. Grouping it with the repo-scoped views
  // would imply it takes a repository, which it does not.
  { id: "system-health", label: "System health", Icon: Activity },
];

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
  const current = VIEWS.find((v) => v.id === view) ?? VIEWS[0];
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
        <current.Icon className="h-4 w-4 shrink-0" aria-hidden="true" />
        <span className="truncate">{current.label}</span>
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
          {offered.map(({ id, label, Icon }) => (
            <button
              key={id}
              type="button"
              role="menuitem"
              aria-current={id === view}
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
      ) : null}
    </div>
  );
}

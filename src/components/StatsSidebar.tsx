import { useState } from "react";
import {
  AlertTriangle,
  Archive,
  ChevronDown,
  ChevronRight,
  Users,
} from "lucide-react";
import { type View, useActiveFilters, useFilters } from "@/store/filters";
import { ViewSwitcher } from "@/components/ViewSwitcher";
import { useStatsTree } from "@/api/hooks";
import type { Filters } from "@/lib/derive";
import type { MemberRow, OrgTree, RepoRow } from "@/types/pr";

/// The PR Stats sidebar: the organisations, repositories and people a stats
/// question can be asked about (#825).
///
/// ```text
/// Organizations
///   <org>
///     Repos
///       All repos
///       <one line per repo>
///     Members
///       <one line per member>
/// Personal
///   All repos
///   <one line per repo>
/// ```
///
/// # What this replaces
///
/// `RepoSidebar`, which PR Stats inherited as a deliberate fall-through in
/// #794. That column listed `repoCounts(prs)` -- repositories where the
/// VIEWER HAS AN OPEN PR -- and `ViewSwitcher`'s doc comment recorded both
/// that the rows were "continuity and a future scope hook, not a live
/// filter" and that it was "worth revisiting if PR Stats is ever scoped per
/// repo, at which point these rows stop being decoration". This is that
/// revisit, and the rows are live now.
///
/// The old list could not express the feature's second audience. It has no
/// organisations and no people in it, so "how is my team doing?" (#823) was
/// unaskable; and being derived from open pull requests, a repository with
/// no current PR was simply absent even though its history is exactly what
/// a lead wants to read. #825 requirement 4: absence of PRs is not absence
/// of repo.
///
/// Nothing here consults local git (#825 requirement 3). No worktree roots,
/// no `git remote` -- where you happen to have cloned something has no
/// bearing on whose statistics you want.
///
/// # Discovery is cheap; measurement waits for a click
///
/// `useStatsTree` runs on mount and costs 2 rate-limit points for the whole
/// hierarchy (measured; see `github::stats::tree`). It fetches NO
/// statistics. Clicking a row writes a scope to the store and that is all
/// this component does -- #826 renders the numbers. That is
/// `hooks.ts:712-717`'s split, and it is why arriving at this view is free
/// while a click is not.
///
/// # A selection is a scope, and a member row is a SUBJECT
///
/// Both axes write through `setStatsScope` in one atomic update. A Members
/// row sets the subject AND keeps the organisation scope, because "this
/// person, in this org" is the question that row asks -- a person's activity
/// is only meaningful somewhere. Every other row clears the subject, so
/// clicking a repository after clicking a colleague is a question about the
/// repository rather than about that colleague inside it.
/// How many organisations one "Everything" search may name.
///
/// Mirrors `ORG_UNION_CAP` in `src-tauri/src/github/stats/scope.rs`. The
/// Rust side trims to this too, so a mismatch makes the sidebar's claim
/// wrong rather than the search -- which is why the number is stated in
/// both places and named the same in both.
const ACCOUNT_ORG_CAP = 20;

export function StatsSidebar({
  viewCounts,
}: {
  /// Badge counts for the switcher, e.g. how many PRs await review.
  viewCounts?: Partial<Record<View, number>>;
}) {
  const filters = useActiveFilters();
  const { setStatsScope, view } = useFilters();
  // Gated on actually being the PR Stats view. This component is only
  // mounted for `pr-stats` today, but the gate is what stops a future
  // fall-through (the thing this very component replaces) from silently
  // spending two points on a view that does not use the tree.
  const enabled = view === "pr-stats";
  const { data, isPending, error } = useStatsTree(enabled);
  // `login,orgA,orgB` for the account-wide row. Empty-safe: a viewer in no
  // organisations yields their login alone, which is a narrow but VALID
  // qualifier -- the failure mode is a board that is too small, never one
  // that searches every repository on GitHub (#1114).
  const accountScope = data
    ? [data.viewer, ...data.orgs.map((o) => o.login)].join(",")
    : undefined;
  // How many organisations "Everything" actually spans, against how many
  // the viewer has. The union is capped (`ORG_UNION_CAP`, 20) BELOW the
  // sidebar's own list cap (50), so a viewer in 30 organisations would see
  // all 30 listed while the account row silently covered 20. Said out
  // loud, for the same reason every other count in this sidebar is: a
  // figure drawn from a sample must not look like a total.
  const accountOrgs = data ? Math.min(data.orgs.length, ACCOUNT_ORG_CAP) : 0;
  const accountShort = data ? data.orgsTotal - accountOrgs : 0;

  // Which org sections are open. Collapsed by DEFAULT, and that is the
  // list-length decision -- see `OrgSection`.
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const toggle = (login: string) =>
    setOpen((o) => ({ ...o, [login]: !o[login] }));

  const selected: IsSelected = (kind, value, subject) =>
    filters.statsScopeKind === kind &&
    filters.statsScopeValue === value &&
    filters.statsSubject === subject;

  return (
    <nav className="flex w-64 shrink-0 flex-col border-r border-[#30363d] p-3">
      <ViewSwitcher counts={viewCounts} />
      <div className="min-h-0 flex-1 overflow-y-auto">
        {/* Three states, never collapsed into two. An empty tree under a
            heading reads as "you belong to nothing", which is the #769
            failure at the top level -- so pending says it is working and an
            error says it failed, rather than both rendering as absence. */}
        {isPending && enabled ? (
          <p className="px-3 py-2 text-xs text-[#8b949e]">
            Finding your organizations...
          </p>
        ) : error ? (
          <p className="px-3 py-2 text-xs text-[#f85149]">
            Could not list your organizations: {String(error)}
          </p>
        ) : !data ? null : (
          <>
            {/* The union "Everything" spans, spelled `login,orgA,orgB` and
                carried in the `scope_value` this kind leaves unused.

                Sent EXPLICITLY because GitHub will not infer it. An absent
                repository qualifier does not mean "everywhere this token
                can see" -- it means the whole of GitHub, which returned
                606,016 pull requests for a single day and made the
                backfill's five-slice document 502 every tick (#1114).

                Derived from the same tree the rows below are drawn from,
                so the account page and the organisations under it cannot
                disagree about which organisations exist. */}
            {/* The account-wide page, FIRST and outside every heading
                (#826's reopening).

                This is the row #829 removed the page behind. It is at the top
                because it is the only scope that needs no knowledge of the
                hierarchy below it -- and because it is the DEFAULT the page
                renders with nothing selected, so its position should match
                where the user already is rather than being buried under two
                headings.

                # Why it is not "All repos" under Personal

                Because that row is a different and much smaller number, and
                the difference is measured. `user:<login>` covers the viewer's
                OWN repositories; this carries no repository qualifier at all,
                which is the only shape that spans organisations the viewer
                contributes to without owning. MEASURED live 2026-09-11 over a
                30-day window: account-wide is **893** merged pull requests
                against **317** for Personal / All repos -- 35%, with the
                other 576 in FNX-Labs (494) and Stohic (82). Two rows,
                because they are two answers.

                Selected by `kind: "all"`, the variant `Filters` has carried
                since #825 ("the widest scope was deliberately clicked") with
                nothing selecting it until now -- so the highlight, the store
                and the page agree without a new axis. `value` is `undefined`
                because `scopeIsLoadable` exempts this kind from needing one:
                "everything" has nothing to name. */}
            <Row
              depth={0}
              label="Everything"
              detail={
                accountShort > 0
                  ? `${accountOrgs} of ${accountOrgs + accountShort} organizations`
                  : "your account"
              }
              active={selected("all", accountScope, undefined)}
              onClick={() => setStatsScope("all", accountScope, undefined)}
            />

            {/* The heading is conditional on there BEING organisations
                (#825's "if any"): a solo user gets a sidebar of their own
                repositories rather than an empty "Organizations" heading
                above nothing, which reads as a failed load. */}
            {data.orgs.length > 0 && (
              <>
                <Heading>Organizations</Heading>
                {data.orgs.map((org) => (
                  <OrgSection
                    key={org.login}
                    org={org}
                    open={open[org.login] ?? false}
                    onToggle={() => toggle(org.login)}
                    selected={selected}
                    onSelect={setStatsScope}
                  />
                ))}
                {/* The org list itself can be truncated, at 50. Said out
                    loud for the same reason every other count here is:
                    a list that is a sample must not look complete. */}
                {data.orgsTotal > data.orgs.length && (
                  <Note>
                    Showing {data.orgs.length} of {data.orgsTotal} organizations
                  </Note>
                )}
              </>
            )}

            {/* Personal, also conditional: an account whose every
                repository lives in an org should not get an empty heading.
                `personalTotal` rather than `personal.length` so a section
                that exists but arrived truncated still appears. */}
            {(data.personal.length > 0 || data.personalTotal > 0) && (
              <>
                <Heading>Personal</Heading>
                <Row
                  depth={1}
                  label="All repos"
                  active={selected("user", data.viewer, undefined)}
                  onClick={() => setStatsScope("user", data.viewer, undefined)}
                />
                <RepoList
                  repos={data.personal}
                  total={data.personalTotal}
                  depth={1}
                  selected={selected}
                  onSelect={setStatsScope}
                />
              </>
            )}

            {/* Refusals that were not a whole organisation. Surfaced rather
                than swallowed: the alternative is a quietly short list,
                which is the defect `client.rs:1140-1190` exists for. */}
            {data.refusedFields > 0 && (
              <Note>
                GitHub refused {data.refusedFields} field
                {data.refusedFields === 1 ? "" : "s"}; some rows may be missing.
              </Note>
            )}
          </>
        )}
      </div>
    </nav>
  );
}

function Heading({ children }: { children: React.ReactNode }) {
  return (
    <p className="px-3 pt-3 pb-1 text-[11px] font-semibold uppercase tracking-wide text-[#8b949e]">
      {children}
    </p>
  );
}

/// A line of explanation, for anything the rows above cannot say
/// themselves: a truncated list, a refused field, an org that would not
/// open.
function Note({ children }: { children: React.ReactNode }) {
  return <p className="px-3 py-1 text-[11px] text-[#8b949e]">{children}</p>;
}

/// One row. `depth` indents rather than nesting scroll containers, so the
/// whole tree scrolls as one column.
function Row({
  depth,
  label,
  detail,
  active,
  onClick,
  icon,
  title,
}: {
  depth: number;
  label: string;
  /// The right-hand annotation: a relative date, a count, "archived".
  detail?: string;
  active: boolean;
  onClick: () => void;
  icon?: React.ReactNode;
  title?: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      // `aria-current` rather than only a colour: the selection is
      // navigation state, and a screen reader reading a list of repository
      // names has no other way to know which one is open.
      aria-current={active ? "true" : undefined}
      className={`flex w-full items-center justify-between gap-2 rounded px-3 py-1.5 text-sm ${
        active ? "bg-[#1f6feb] text-white" : "text-[#e6edf3] hover:bg-[#161b22]"
      }`}
      style={{ paddingLeft: `${depth * 0.75 + 0.75}rem` }}
    >
      <span className="flex min-w-0 items-center gap-1.5">
        {icon}
        <span className="truncate">{label}</span>
      </span>
      {detail && (
        <span
          className={`shrink-0 text-[11px] ${active ? "text-white/80" : "text-[#8b949e]"}`}
        >
          {detail}
        </span>
      )}
    </button>
  );
}

/// Which scope a row selects, as the store spells it.
///
/// One alias shared by every level of the tree, rather than each child
/// narrowing to the kinds it happens to emit. Narrow signatures read as
/// tighter typing but force the parent's wider function through an `as` cast
/// at each boundary -- and a cast in the selection path is precisely where a
/// genuine kind/value mismatch would hide, since that is the one place the
/// compiler has been told to stop checking.
type ScopeKind = NonNullable<Filters["statsScopeKind"]>;

/// Whether a row is the current selection.
type IsSelected = (
  kind: ScopeKind,
  value: string | undefined,
  subject: string | undefined,
) => boolean;

/// Select a row: a scope, and a person within it when the row names one.
type OnSelect = (
  kind: ScopeKind,
  value: string | undefined,
  subject: string | undefined,
) => void;

/// How many repository rows render before the rest go behind "Show all".
///
/// # The list-length decision, made deliberately (#825 asks for it)
///
/// 49 repositories in one organisation is real (MEASURED on this account),
/// and two organisations plus a personal section is ~65 rows before a single
/// member. Three options were considered:
///
/// - **Render all of them.** Rejected for the org sections: the sidebar
///   becomes a scroll surface where the *second* organisation is below the
///   fold and Members -- the rows that make the team audience reachable at
///   all -- is off-screen entirely. The feature's headline capability would
///   be invisible on arrival.
/// - **Paginate or filter.** Rejected as premature: a search box over 49
///   rows that are already ordered by recency is a control for a problem
///   the ordering mostly solves, and it adds state this component does not
///   otherwise need. Worth adding if someone actually has 300 repositories
///   in one org -- which nothing in reach does.
/// - **Cap with an explicit expansion.** Taken.
///
/// 12 because it is what fits beside the other sections without pushing
/// Members off a laptop screen, and because the ordering makes the cut
/// meaningful rather than arbitrary: these are the twelve most recently
/// pushed, so the hidden tail is the dead end of the list, not a random
/// slice. MEASURED against the live data: the twelfth FNX-Labs repository
/// was pushed 2026-07-21 and the tail reaches 2022 -- so the cut lands
/// exactly where a list stops describing current work.
///
/// The count is always stated ("Show all 49"), never silently cut. That is
/// #824 item 8's rule applied to a row list, and #802 and #790 both shipped
/// the other behaviour.
export const REPO_PREVIEW = 12;

/// Repository rows, with the tail behind an explicit expansion.
function RepoList({
  repos,
  total,
  depth,
  selected,
  onSelect,
}: {
  repos: RepoRow[];
  /// What GitHub says the true count is, which may exceed `repos.length`.
  total: number;
  depth: number;
  selected: IsSelected;
  onSelect: OnSelect;
}) {
  const [expanded, setExpanded] = useState(false);
  const shown = expanded ? repos : repos.slice(0, REPO_PREVIEW);

  return (
    <>
      {shown.map((r) => (
        <Row
          key={r.nameWithOwner}
          depth={depth + 1}
          // The owner is already the section heading, so the row shows the
          // repository name alone -- 49 rows all starting with the same
          // twelve characters truncate into indistinguishability in a
          // 256px column. The full `owner/name` stays in the tooltip and
          // is what the scope is keyed on.
          label={
            r.nameWithOwner.split("/").slice(1).join("/") || r.nameWithOwner
          }
          title={r.nameWithOwner}
          detail={r.isArchived ? "archived" : relativeDay(r.pushedAt)}
          icon={
            r.isArchived ? (
              <Archive
                className="h-3 w-3 shrink-0 text-[#8b949e]"
                aria-hidden="true"
              />
            ) : undefined
          }
          active={selected("repo", r.nameWithOwner, undefined)}
          onClick={() => onSelect("repo", r.nameWithOwner, undefined)}
        />
      ))}
      {!expanded && repos.length > REPO_PREVIEW && (
        <button
          type="button"
          onClick={() => setExpanded(true)}
          className="w-full px-3 py-1 text-left text-[11px] text-[#58a6ff] hover:underline"
          style={{ paddingLeft: `${(depth + 1) * 0.75 + 0.75}rem` }}
        >
          Show all {repos.length}
        </button>
      )}
      {/* A list SHORTER than GitHub's own count: the connection is capped
          at 100 nodes and `totalCount` keeps telling the truth above it
          (MEASURED: 100 returned, totalCount 559). Expanding cannot reveal
          these, so the note is the only honest thing available. */}
      {total > repos.length && (
        <Note>
          Showing {repos.length} of {total} repositories
        </Note>
      )}
    </>
  );
}

/// One organisation: a disclosure holding Repos and Members.
///
/// Collapsed by default. With two organisations of 49 and 10 repositories,
/// expanding everything puts ~70 rows in a 256px column and the second
/// organisation's Members -- the rows that make #823's team audience
/// reachable -- below the fold. Collapsed, the whole hierarchy is one
/// screen and the user opens the one they mean.
///
/// The heading itself is NOT a scope. "All repos" inside is, deliberately:
/// a disclosure triangle that also navigates is a control where a click
/// meant to reveal the list instead loads an expensive org-wide scope.
/// Separating them costs one row and removes the misfire.
function OrgSection({
  org,
  open,
  onToggle,
  selected,
  onSelect,
}: {
  org: OrgTree;
  open: boolean;
  onToggle: () => void;
  selected: IsSelected;
  onSelect: OnSelect;
}) {
  return (
    <>
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        className="flex w-full items-center gap-1.5 rounded px-3 py-1.5 text-sm text-[#e6edf3] hover:bg-[#161b22]"
      >
        {open ? (
          <ChevronDown className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
        ) : (
          <ChevronRight className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
        )}
        <span className="truncate">{org.login}</span>
        {/* An org that could not be read is marked on the COLLAPSED row
            too. The detail is inside, but a user who never expands it must
            still be able to see that this organisation's numbers are not
            available -- otherwise the honest degradation is hidden behind
            a disclosure. */}
        {!org.readable && (
          <AlertTriangle
            className="ml-auto h-3.5 w-3.5 shrink-0 text-[#d29922]"
            aria-label="could not be read"
          />
        )}
      </button>

      {open &&
        (org.readable ? (
          <>
            <Heading>Repos</Heading>
            {/* `All repos` is its OWN scope, not a sum of the rows below
                (#825 requirement 5). An org-wide search is capped at 1,000
                results and sliced (#827's `slice.rs`); a per-repository
                connection is uncapped. The two are answered by different
                GitHub APIs, so they are not expected to agree to the unit,
                and the rows exclude archived repositories from aggregates
                while this scope covers everything GitHub has. */}
            <Row
              depth={2}
              label="All repos"
              detail={
                archivedCount(org.repos) > 0
                  ? `${org.reposTotal} incl. archived`
                  : String(org.reposTotal)
              }
              active={selected("org", org.login, undefined)}
              onClick={() => onSelect("org", org.login, undefined)}
            />
            <RepoList
              repos={org.repos}
              total={org.reposTotal}
              depth={2}
              selected={selected}
              onSelect={onSelect}
            />

            <Heading>Members</Heading>
            {org.members.length === 0 ? (
              // Readable AND empty is a real, if odd, state -- and it is
              // only sayable because `readable` distinguishes it from the
              // refused case above. Without that flag this line would be
              // what a permission failure rendered as, which is exactly
              // the #769 defect.
              <Note>No members listed for this organization.</Note>
            ) : (
              org.members.map((m: MemberRow) => (
                <Row
                  key={m.login}
                  depth={2}
                  label={m.name ?? m.login}
                  // The login even when a display name exists: the
                  // statistics are keyed on `author:<login>`, and a row
                  // showing only "Jane Doe" cannot be checked against
                  // GitHub's own UI.
                  detail={m.name ? m.login : undefined}
                  title={m.login}
                  icon={
                    <Users
                      className="h-3 w-3 shrink-0 text-[#8b949e]"
                      aria-hidden="true"
                    />
                  }
                  // A member row keeps the ORG scope and sets the subject:
                  // "this person, in this org". A person's activity is
                  // only meaningful somewhere.
                  active={selected("org", org.login, m.login)}
                  onClick={() => onSelect("org", org.login, m.login)}
                />
              ))
            )}
            {org.membersTotal > org.members.length && (
              <Note>
                Showing {org.members.length} of {org.membersTotal} members
              </Note>
            )}
          </>
        ) : (
          // THE honest-degradation path (#825 requirement 2). An empty
          // Members list would read as "this org has no members" --
          // silence read as success, the #769 lesson. The counts come from
          // the organisations query, which succeeds even when the detail
          // query is refused, so the row can say how big the org is while
          // admitting it could not be read.
          // The SCOPE fix leads, SSO follows (#840). Both causes produce
          // the same null alias and the app cannot tell them apart from
          // the response body (`tree.rs`'s `readable` doc explains why),
          // so the message has to name both -- and the order is the whole
          // point: `gh auth refresh -s read:org` is one command the
          // reader runs themselves, while SAML authorization may need an
          // administrator. Leading with SSO sent people to ask for help
          // with the expensive cause when the cheap one was theirs to fix.
          <Note>
            Could not read this organization ({org.reposTotal} repositories,{" "}
            {org.membersTotal} members). Most often the token is missing the{" "}
            <code>read:org</code> scope -- <code>gh auth refresh -s read:org</code>{" "}
            adds it. If this org uses SAML single sign-on, the token also has to
            be authorized for it.
          </Note>
        ))}
    </>
  );
}

/// How many of these rows GitHub marks archived.
///
/// Counted from the rows rather than asked for separately, because the rows
/// already carry `isArchived` for free. It annotates `All repos` as
/// "incl. archived" so the two numbers a user can see -- the org total and
/// the rows -- do not appear to disagree: archived repositories are listed
/// and are inside the org scope's total, but are excluded from any
/// per-repository aggregate #826 builds over the rows.
///
/// This counts only the rows PRESENT, so on a truncated list it is a floor.
/// That is why it only ever gates the word "incl." rather than being shown
/// as a figure of its own.
///
/// Not exported: the only caller is `OrgSection` below, and `knip` fails the
/// lint on an export nothing imports. #826 may well want this beside a
/// per-repository aggregate, and exporting it then is one word.
function archivedCount(repos: RepoRow[]): number {
  return repos.filter((r) => r.isArchived).length;
}

/// "3d", "2mo", "2y" -- how long ago, in the smallest unit that is not a
/// silly number.
///
/// Shown because the ordering is most-recently-active and that is invisible
/// otherwise: a user cannot tell whether the twelfth row is a week stale or
/// three years dead. An absolute date would be exact and useless -- the
/// question a reader has here is "is this live?", not "what day was it".
///
/// `null` for an absent timestamp rather than a fabricated one. A repository
/// that has never been pushed to genuinely has no value, and rendering the
/// epoch would claim 1970 for a repository created this morning.
export function relativeDay(
  iso: string | null,
  now = Date.now(),
): string | undefined {
  if (!iso) return undefined;
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return undefined;
  const days = Math.floor((now - then) / 86_400_000);
  // Clamped at zero rather than rendering "-1d": a `pushedAt` a few seconds
  // in the future is a clock-skew artefact, not information.
  if (days <= 0) return "today";
  if (days < 30) return `${days}d`;
  if (days < 365) return `${Math.floor(days / 30)}mo`;
  return `${Math.floor(days / 365)}y`;
}

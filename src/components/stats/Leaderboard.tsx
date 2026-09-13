import { Card } from "@/components/ui/card";
import type { AuthorRow, StatsReviewers } from "@/types/pr";

/// How many rows a leaderboard shows.
///
/// Must equal the Rust `board::TOP_N`, which is what actually cuts the list
/// -- this is the number the HEADING quotes. Re-cutting here would produce a
/// "top five" heading over three rows, or a top-three whose fourth place was
/// decided by a tie-break this side never saw.
///
/// That equality is asserted by `src/lib/mirroredConstants.test.ts`, which
/// reads the literal out of `github/stats/board.rs` (#850). Nothing asserted
/// it before: `top_n_is_five` in `board.rs` only sees Rust, and every test in
/// this directory uses `TOP_N` SYMBOLICALLY -- `expect(rows.length).toBe(TOP_N)`
/// and `` getByText(`Top ${TOP_N} ...`) `` are self-consistent at any value,
/// so setting this to 3 passed the whole suite while the page rendered "Top 3
/// pull request authors" over a Rust-ranked list of five.
export const TOP_N = 5;

/// The label for the lines-changed measure, and it is load-bearing.
///
/// #823 settled the code metric as raw `additions`/`deletions` with the
/// honest label AS the mitigation: "this metric is gameable by a large
/// generated diff, and people who know they are ranked will notice. The
/// honest label is the mitigation, not a fix."
///
/// So the words "including generated files" are not decoration and must not
/// be shortened for layout. A reader who takes this for a measure of effort
/// has been misled by the chart, and there is nothing else on screen to
/// correct them. Exported so the one phrasing is reused wherever the measure
/// appears rather than re-worded per component.
export const LINES_CHANGED_LABEL = "lines changed, including generated files";

/// What the review count on the AUTHOR board measures.
///
/// RECEIVED, not given. It reads `reviews { totalCount }` off a pull request
/// the author WROTE, so it counts how much review their work attracted --
/// close to the opposite of what "top reviewers" would suggest to a reader
/// skimming.
///
/// Both boards now ship, which makes this label MORE load-bearing rather than
/// less: with two review charts on one screen, the only thing separating them
/// is what each says it counts. #829 was right to refuse the requested title
/// over this measure; the fix was never to re-word this one, it was to build
/// the other.
export const REVIEWS_LABEL = "reviews received on their pull requests";

/// What the reviews-GIVEN board measures.
///
/// GIVEN, and from a different search: `reviewed-by:<login>`, one per person,
/// which finds pull requests by ANYONE that this person reviewed. The phrase
/// names the population ("by anyone") because that is exactly the half a
/// reader could otherwise assume away -- the natural misreading of a reviewer
/// board is that it counts reviews within some narrower set.
///
/// MEASURED, and the two boards genuinely name different people on this
/// account's own data (live API, 2026-09-11): a `reviewed-by:<viewer>` search
/// over an org window returned two pull requests, both AUTHORED BY SOMEONE
/// ELSE and each carrying one review. So the AUTHOR leads the received board
/// and the REVIEWER the given one, on the same two rows of data. That is why
/// both labels are exported constants asserted by tests rather than strings
/// typed into two components.
export const REVIEWS_GIVEN_LABEL = "reviews they gave on pull requests by anyone";

/// One ranked list, as horizontal bars.
///
/// Bars rather than a recharts chart, deliberately, and not for lack of the
/// library: `ActivityChart` uses recharts because a 30-point time series
/// needs axes, a tooltip and interpolation. A five-row ranking needs a label,
/// a proportion and a number, which is exactly what `RepoTable` already draws
/// with a span and a width percentage -- so this reuses THAT idiom. #826 asks
/// that the existing components be reused rather than a second charting idiom
/// introduced, and the second idiom would have been a recharts bar chart
/// beside a hand-drawn bar table, not a chart beside no chart.
///
/// # The bar is a share of the LEADER, not of the total
///
/// A share of the total would make every bar short as soon as the population
/// is large -- in a 40-person org the leader's bar would be 8% wide and the
/// ranking unreadable. Against the leader, the shape answers the question a
/// ranking is for: how far ahead is first place. The numbers are printed
/// beside the bars so the absolute figures are never only a bar length.
function Ranked<T extends { login: string }>({
  title,
  hint,
  rows,
  value,
  format,
  emptyNote,
}: {
  title: string;
  /// What the measure IS. Never optional: every measure here is either
  /// gameable (lines changed) or easy to misread (reviews received against
  /// reviews given), and the hint is where that is said.
  hint: string;
  /// Generic over the row, so the reviews-GIVEN board draws through THIS
  /// component rather than a second ranked-bar idiom beside it. The
  /// constraint is `{ login: string }` and nothing more: everything else a
  /// ranking needs is reached through the two accessors below, which is the
  /// same widening `Outliers` took in #829 for the same reason (#826 asks
  /// that existing components be reused).
  ///
  /// The rows cannot be a union of the two shapes instead, because the two
  /// boards are ranked on fields that do not both exist -- `ReviewerRow` has
  /// no `prs` and `AuthorRow` has no reviews-given count at all, which is the
  /// whole point of their being separate queries.
  rows: T[];
  value: (r: T) => number;
  format: (r: T) => string;
  /// What to say when nobody qualifies. Distinct per measure, because "no
  /// pull requests" and "no lines changed" are different facts.
  emptyNote: string;
}) {
  // The LEADER's value, which is the bar scale. Taken from the rows rather
  // than assumed to be the first, so this is correct even if a caller hands
  // over an unsorted list.
  const leader = rows.reduce((m, r) => Math.max(m, value(r)), 0);

  return (
    <Card className="px-4">
      <div className="text-sm font-semibold">{title}</div>
      <div className="text-xs text-[#8b949e]">{hint}</div>
      {rows.length === 0 ? (
        <div className="py-8 text-center text-sm text-[#8b949e]">{emptyNote}</div>
      ) : (
        <ol className="mt-3 flex flex-col gap-1">
          {rows.map((r, i) => {
            const pct = leader === 0 ? 0 : Math.round((value(r) / leader) * 100);
            return (
              <li
                key={r.login}
                className="flex items-center gap-3 rounded px-2 py-1.5 text-sm"
              >
                {/* The rank as a number. A ranking whose order is carried
                    only by vertical position is unreadable to anyone
                    hearing it read out, and `ol` markup alone does not
                    surface the index in most screen readers. */}
                <span className="w-4 shrink-0 text-right text-xs tabular-nums text-[#8b949e]">
                  {i + 1}
                </span>
                {/* `min-w-0 flex-1` rather than `w-40 shrink-0`, which
                    is `Outliers.tsx`' pattern in this same directory
                    for the same row shape: the field that CAN be long
                    absorbs the slack and truncates, the fields that
                    cannot are bare `shrink-0` with no width (#942).

                    The old `w-40` was the larger half of a 340px row
                    minimum in a 326px card, so the row overflowed an
                    `overflow-hidden` Card by 14px at 390px and the bar
                    between these two columns was squeezed to ZERO --
                    first place and fifth place drew identical bars,
                    which is the one thing this component's own doc says
                    the bar length is for. `title` already carried the
                    full login, so truncating sooner loses nothing. */}
                <span className="min-w-0 flex-1 truncate text-left" title={r.login}>
                  {r.login}
                </span>
                <span
                  // `basis-16 shrink-0` rather than `flex-1`: as a
                  // second `flex-1` beside the login above, the bar
                  // would split the slack evenly with it and both would
                  // be starved at 390px. A fixed 64px basis is the
                  // narrowest width at which "how far ahead is first
                  // place" is still legible, and it is the column that
                  // must never reach zero again.
                  className="relative h-1.5 basis-16 shrink-0 overflow-hidden rounded bg-[#21262d]"
                  // The bar is decoration over a number that is already
                  // printed beside it, so it is hidden rather than given an
                  // ARIA value that would read the same figure twice.
                  aria-hidden="true"
                >
                  <span
                    className="absolute inset-y-0 left-0 rounded bg-[#58a6ff]"
                    style={{ width: `${pct}%` }}
                  />
                </span>
                {/* Bare `shrink-0`, no width: the figure sizes itself,
                    which is what `Outliers` does and what keeps this
                    column from claiming 112px it does not always need. */}
                <span className="shrink-0 text-right tabular-nums text-xs">
                  {format(r)}
                </span>
              </li>
            );
          })}
        </ol>
      )}
    </Card>
  );
}

/// The three leaderboards #826 asks for: top authors, top reviewers, top by
/// code volume.
///
/// # Everyone who can open a scope sees this
///
/// No role gating, which #823 settled with its consequence recorded rather
/// than assumed away: "this publishes a peer-visible ranking of colleagues,
/// not just a lead-facing one". It is repeated here because this component is
/// where that decision becomes visible, and a future reader wondering whether
/// the missing permission check is an oversight should find the answer at the
/// code rather than in an issue thread.
///
/// # Ranking over a partial board
///
/// A ranking is far less forgiving of missing data than a count: a total 5%
/// short is slightly wrong, while a top-five 5% short can have the wrong
/// person in first place. So `complete` is a REQUIRED prop and a partial
/// board renders the caveat above the boards rather than beside one of them
/// -- it applies to all three, and #826's rule is "never a confident
/// top-five over a sample".
export function Leaderboards({
  rows,
  complete,
  caveat,
  reviewers,
  reviewersPending = false,
  reviewersError = false,
  reviewersAvailable = false,
  reviewersTruncated = false,
}: {
  /// Every author in scope. Ranked and cut here, per measure, because the
  /// rankings disagree -- the most prolific author is rarely the one with the
  /// most lines.
  rows: AuthorRow[];
  complete: boolean;
  /// The reviews-GIVEN board, from its own query (#826's reopening).
  ///
  /// A SEPARATE prop rather than a field on `rows`, because it is a different
  /// search over a different population: the rows are authors in the window,
  /// and a reviewer need not have authored anything at all. It also lands
  /// independently, so the three board-derived rankings do not wait on it --
  /// this page's progressive rule, one more part.
  ///
  /// `undefined` means "not here yet or not asked for", which the three flags
  /// below disambiguate. They are separate rather than one status enum for
  /// `useScopedCounts`' reason (`hooks.ts`): a pending board, a failed one and
  /// one nothing enumerated are three different things to say, and collapsing
  /// them is how a failure comes to render as a zero.
  reviewers?: StatsReviewers;
  /// In flight. Renders a loading board, never an empty one -- see
  /// `ReviewsGiven`.
  reviewersPending?: boolean;
  /// The query failed. Distinct from an empty result, which is the rule this
  /// whole feature is built on.
  reviewersError?: boolean;
  /// Whether a roster existed to ask about at all.
  ///
  /// False on a scope with no membership -- a repository, Personal,
  /// Everything -- where the board is ABSENT rather than empty. "Nobody
  /// reviewed" and "nothing enumerated the reviewers" are different claims,
  /// and only one of them is ours to make.
  reviewersAvailable?: boolean;
  /// Whether the ROSTER this board ranks was itself cut short (#851).
  ///
  /// True when `OrgTree::members_truncated()` is -- the org has more members
  /// than `tree::PAGE` returned, so the board asked about a SUBSET of the
  /// population and ranked that.
  ///
  /// A separate prop from `reviewersAvailable` because it is a different
  /// claim: that one is "was there a roster at all", this one is "was the
  /// roster complete". Both can be true, and the honest board says so.
  reviewersTruncated?: boolean;
  /// Why the board is partial, in the caller's words -- the caller knows
  /// which of the three partiality channels applied and this component does
  /// not.
  ///
  /// OPTIONAL, because `StatsPage` carries the detail in a page-level banner
  /// that also covers the Mine view's figures, and two copies of one warning
  /// read as two different problems. Omitted, the short reminder below still
  /// renders: a reader taking a name off a ranking needs the caveat where
  /// their eye is, not only at the top of the page.
  caveat?: string;
}) {
  const top = (value: (r: AuthorRow) => number) =>
    [...rows]
      // A zero has no rank. Padding a top-five with zeroes presents people
      // as ranked on a measure they do not appear in at all -- which for
      // "top reviewers" would list colleagues as reviewed when nobody
      // reviewed them.
      .filter((r) => value(r) > 0)
      // Ties break on LOGIN so the boards do not reorder between loads when
      // nothing changed. The Rust side sorts the same way for the same
      // reason; doing it here too means a caller that re-sorts locally
      // cannot reintroduce the flicker.
      .sort((a, b) => value(b) - value(a) || a.login.localeCompare(b.login))
      .slice(0, TOP_N);

  return (
    <div className="flex flex-col gap-3">
      {!complete ? (
        <div className="rounded-md border border-[#d29922]/40 bg-[#d29922]/10 px-3 py-2 text-xs text-[#d29922]">
          {/* Above the boards, not inside one: the partiality applies to
              every ranking below it, and a note attached to one board would
              read as though the others were complete.

              Rendered on `!complete` ALONE, with or without a reason. An
              unexplained warning is worth far more than a silent confident
              top-five, and the reason is optional precisely because the page
              may be carrying it elsewhere. */}
          These rankings are incomplete{caveat ? `. ${caveat}` : ", so the order may be wrong."}
        </div>
      ) : null}
      {/* Two columns at `md`, not three: a fourth board arrived with the
          reviews-given ranking, and three columns would leave one board alone
          on a second row at every width. Two pairs read as two pairs. The
          login column inside `Ranked` is a fixed 10rem, so three boards on a
          laptop truncated every name -- which on a leaderboard of colleagues
          is the one field that must stay readable. */}
      <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
        <Ranked
          title={`Top ${TOP_N} pull request authors`}
          hint="pull requests in this window"
          rows={top((r) => r.prs)}
          value={(r) => r.prs}
          format={(r) => `${r.prs.toLocaleString()} PRs`}
          emptyNote="No pull requests in this window."
        />
        <Ranked
          title={`Top ${TOP_N} by code volume`}
          hint={LINES_CHANGED_LABEL}
          rows={top((r) => r.additions + r.deletions)}
          value={(r) => r.additions + r.deletions}
          // The file count rides along, because it is what distinguishes a
          // generated diff from a refactor and it is a free scalar. Without
          // it the measure's one honest defence is a sentence nobody reads
          // twice; with it the reader can see 40,000 lines in 3 files for
          // themselves.
          format={(r) =>
            `${(r.additions + r.deletions).toLocaleString()} · ${r.changedFiles.toLocaleString()} files`
          }
          emptyNote="No lines changed in this window."
        />
        {/* Still NOT titled "Top reviewers", and the title is the whole
            point. `reviews { totalCount }` hangs off a pull request the
            author WROTE, so it counts review their work ATTRACTED -- the
            person at the top of a board titled "top reviewers" would be the
            one whose code was reviewed MOST, not the one who reviewed most.
            #829 refused to ship that and was right to.

            This board is KEPT rather than replaced, because it answers a
            different and legitimate question: whose work draws the most
            attention. With the reviews-given board now beside it, the two
            titles are what keep them apart -- which is why both labels are
            exported constants with tests on them. */}
        <Ranked
          title={`Top ${TOP_N} most-reviewed`}
          hint={REVIEWS_LABEL}
          rows={top((r) => r.reviewsReceived)}
          value={(r) => r.reviewsReceived}
          format={(r) => `${r.reviewsReceived.toLocaleString()} reviews`}
          emptyNote="No reviews received in this window."
        />
        {/* The board #826 actually asked for, built on `reviewed-by:<login>`
            (#826's reopening). Absent entirely when nothing enumerated a
            roster, rather than rendered empty. */}
        {reviewersAvailable && (
          <ReviewsGiven
            reviewers={reviewers}
            pending={reviewersPending}
            failed={reviewersError}
            truncated={reviewersTruncated}
          />
        )}
      </div>
    </div>
  );
}

/// The reviews-GIVEN board: who reviewed the most, whoever wrote it.
///
/// # Why this is its own component rather than a fourth `Ranked` call
///
/// Not for the chart -- it draws through `Ranked` like the other three. It is
/// for the four STATES a separately-fetched board has that a field on the
/// shared rows does not: pending, failed, measured-empty, and partial with
/// named people missing. Folding those into the call site would put four
/// branches inside a JSX list, and the distinctions are exactly the ones this
/// feature exists to keep ("partial data SAYS SO", "empty means empty").
///
/// # Why a reviewer with zero reviews is not on the board
///
/// `Leaderboards`' own rule, applied here: a zero has no rank. Rust returns
/// measured zeroes deliberately (`fetch::Reviewers::rows`) so the UI can tell
/// them from unmeasured ones, and the UI then declines to RANK them -- listing
/// a colleague in a top five on the strength of reviewing nothing presents
/// them as ranked on a measure they do not appear in.
///
/// This matters more than it sounds on real data. MEASURED on this account
/// 2026-09-11: `org:FNX-Labs` over a 30-day window holds 569 merged pull
/// requests and **zero** carrying a review by any of its four members -- the
/// account merges without human review. So "No reviews given in this window"
/// is the TRUE answer here, and that is precisely why it must be
/// distinguishable from a failure: the state this board is usually in is the
/// one that looks most like a broken query.
function ReviewsGiven({
  reviewers,
  pending,
  failed,
  truncated = false,
}: {
  reviewers?: StatsReviewers;
  pending: boolean;
  failed: boolean;
  /// Whether the roster was cut short before this board ever asked (#851).
  truncated?: boolean;
}) {
  const title = `Top ${TOP_N} reviewers`;

  // Pending first. A board that printed its empty note while in flight would
  // state "no reviews given" as a fact for the second before the answer
  // arrives -- and on this account that transient is INDISTINGUISHABLE from
  // the real answer, so a reader could not tell which they were looking at.
  if (pending || (!reviewers && !failed)) {
    return (
      <Card className="px-4">
        <div className="text-sm font-semibold">{title}</div>
        <div className="text-xs text-[#8b949e]">{REVIEWS_GIVEN_LABEL}</div>
        <div className="py-8 text-center text-sm text-[#8b949e]">
          Counting reviews...
        </div>
      </Card>
    );
  }

  // Failed, and said as a failure. NOT as zero and not as empty: one search
  // per person means this board can fail while the other three succeeded, and
  // rendering "no reviews given" over a query that never answered is the
  // exact #802/#790 confusion this feature is built to prevent.
  if (failed || !reviewers) {
    return (
      <Card className="px-4">
        <div className="text-sm font-semibold">{title}</div>
        <div className="text-xs text-[#8b949e]">{REVIEWS_GIVEN_LABEL}</div>
        <div className="py-8 text-center text-sm text-[#d29922]">
          Could not count reviews. This is a separate query from the other
          boards, so they may be complete while this is not.
        </div>
      </Card>
    );
  }

  const ranked = [...reviewers.rows]
    // A zero has no rank -- see the doc above.
    .filter((r) => r.reviews > 0)
    // Ranked here as well as in Rust, for `board.rs`' reason: a caller that
    // re-sorted locally must not be able to reintroduce the flicker that
    // out-of-order chunk completion causes. Ties break on login, the same
    // tie-break the author boards use, so a tie orders identically on both.
    .sort((a, b) => b.reviews - a.reviews || a.login.localeCompare(b.login))
    .slice(0, TOP_N);

  return (
    <div className="flex flex-col gap-2">
      <Ranked
        title={title}
        hint={REVIEWS_GIVEN_LABEL}
        rows={ranked}
        value={(r) => r.reviews}
        format={(r) => `${r.reviews.toLocaleString()} reviews`}
        // "Given", explicitly, because the board beside this one has an empty
        // note about reviews RECEIVED and two charts reading "No reviews in
        // this window" would be one fact said twice rather than two facts.
        emptyNote="No reviews given in this window."
      />
      {/* The people this ranking could not measure, NAMED. A ranking missing
          one person can have the wrong name in first place, and "2 could not
          be measured" does not say whether the leader is one of them. */}
      {reviewers.unmeasured.length > 0 && (
        <p className="text-xs text-[#d29922]">
          {reviewers.unmeasured.length === 1 ? "1 person" : `${reviewers.unmeasured.length} people`}{" "}
          could not be counted and {reviewers.unmeasured.length === 1 ? "is" : "are"}{" "}
          absent from this ranking rather than shown as zero:{" "}
          {reviewers.unmeasured.join(", ")}.
        </p>
      )}
      {/* #851: the roster itself was cut short, so this ranking is over a
          SUBSET of the organisation and the leader may not be in it.
          Deliberately in the same register as the `unmeasured` line above --
          "this ranking could be missing the leader" -- because it is the same
          kind of claim arriving one layer earlier: that line is about people
          who were asked and did not answer, this one about people who were
          never asked.

          Worse than the repository truncation the module documents, and said
          so rather than softened: `tree::PAGE`'s doc reasons that "because
          the order is most-recently-active, a truncation at 100 drops the
          DEADEST repositories". `membersWithRole` takes no `orderBy` at all,
          so the members cut is ARBITRARY -- on a 224-member org (the size the
          module records verifying against) the top reviewer can be among the
          124 never asked about, and nothing about the cut makes that less
          likely. */}
      {truncated && (
        <p className="text-xs text-[#d29922]">
          This organization has more members than the roster could list, so
          this ranking covers only the members that were listed -- and the
          member list is not ordered by anything, so the people it left out
          are an arbitrary slice rather than the least active. Somebody absent
          here may have reviewed more than anybody shown.
        </p>
      )}
      {reviewers.refusedFields > 0 && (
        <p className="text-xs text-[#d29922]">
          GitHub refused {reviewers.refusedFields} field
          {reviewers.refusedFields === 1 ? "" : "s"} on this ranking -- the
          token may be missing the <code>read:org</code> scope
          (<code>gh auth refresh -s read:org</code>), or this organization may
          use SAML single sign-on and need the token authorized for it.
        </p>
      )}
      {/* Who is IN scope for this board, stated because it is a real and
          unfixable gap rather than a caveat for form's sake. The roster comes
          from the organisation's Members list, so somebody who reviewed
          without being a member -- an outside collaborator, a bot -- is not
          counted. Nothing enumerated them: a pull request node says how many
          reviews it has, never who wrote them, so there is no cheaper
          population to ask about. */}
      <p className="text-xs text-[#8b949e]">
        Counts reviews by this organization's {reviewers.rows.length + reviewers.unmeasured.length}{" "}
        listed member
        {reviewers.rows.length + reviewers.unmeasured.length === 1 ? "" : "s"}.
        Reviews by anyone else -- an outside collaborator, a bot -- are not
        counted here.
      </p>
    </div>
  );
}

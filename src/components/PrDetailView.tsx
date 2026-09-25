import { ExternalLink } from "./ExternalLink";
import { ArrowLeft, Trash2, Check, CircleDot, CircleSlash, ExternalLink as ExternalLinkIcon, X } from "lucide-react";
import { toast } from "sonner";
import {
  useClaudeSessionsForPr,
  useCommentOnPr,
  useDeleteHeadBranch,
  usePrDetail,
  useRerunChecks,
  useReviewGates,
  useReviewPr,
  useViewer,
} from "../api/hooks";
import { gateVerdict } from "../lib/reviewGates";
import { useState } from "react";
import type { ReviewVerdictName } from "../api/tauri";
import type { ClaudePrLink } from "../types/pr";
import { useFilters } from "../store/filters";
import { rerunnableRun } from "../lib/rerun";
import { useIsMobile } from "../lib/useIsMobile";
import { Markdown } from "./Markdown";
import { PrClaudifyButton } from "./PrClaudify";
import { CommentRow } from "./CommentRow";
import { ReviewThreads } from "./ReviewThreads";
import { PrDates } from "./PrDates";
import { Section } from "./Section";
import { PrActions } from "./PrActions";
import { StackBadge } from "./StackBadge";
import { ReviewBox } from "./ReviewBox";
import { QueryError, errorMessage } from "./QueryError";
import { scopeEffect } from "../lib/branchDelete";
import { Dialog, DialogContent, DialogTitle } from "./ui/dialog";

/// One check, with its outcome and a link to the run.
function CheckRow({ name, state, url }: { name: string; state: string; url: string }) {
  const Icon =
    state === "success" ? Check : state === "failure" ? X : state === "pending" ? CircleDot : CircleSlash;
  const tone =
    state === "success"
      ? "text-[#3fb950]"
      : state === "failure"
        ? "text-[#f85149]"
        : state === "pending"
          ? "text-[#d29922]"
          : "text-[#8b949e]";

  const body = (
    <>
      <Icon className={`h-3.5 w-3.5 shrink-0 ${tone}`} aria-hidden="true" />
      <span className="min-w-0 flex-1 truncate">{name}</span>
      <span className={`shrink-0 text-xs ${tone}`}>{state}</span>
    </>
  );

  // No link when GitHub gives no URL, rather than an anchor that goes
  // nowhere.
  return url ? (
    <ExternalLink
      href={url}
      className="flex items-center gap-2 rounded px-2 py-1.5 text-sm hover:bg-[#161b22]"
    >
      {body}
    </ExternalLink>
  ) : (
    <div className="flex items-center gap-2 px-2 py-1.5 text-sm">{body}</div>
  );
}

/// The pull request detail view.
///
/// Modelled on GitHub's PR page minus what does not belong in a triage
/// tool: no file diff, no commit history, no posting comments. Headstate
/// is for deciding and acting; reviewing code belongs in GitHub or an
/// editor, and "View on GitHub" covers the rest.
/// The Claude sessions that produced this pull request (#1211).
///
/// The `pr-link` record has been read since #1132 and surfaced in one
/// direction only: a session names its pull requests, and a pull
/// request named nothing. The reverse is the more useful half -- a PR
/// fails CI, and its session's transcript is one click away rather than
/// a search through 1,453 rows whose titles collide.
///
/// # Absent means something specific, and it is not "no session"
///
/// A pull request with no linked session means THIS MACHINE holds no
/// transcript for it. It may have been opened by a teammate, by CI, or
/// by a session whose transcript has since been pruned. Rendering that
/// as "no session" would be a confident wrong answer about someone
/// else's work, so the panel is absent entirely rather than empty --
/// the same choice `SessionPullRequests` makes for the same reason.
function PrSessions({ repo, number }: { repo: string; number: number }) {
  const q = useClaudeSessionsForPr(repo, number, true);
  // Rows without a usable `session_id` are DROPPED rather than rendered
  // (#1288).
  //
  // The contract is asserted where a contract can be asserted -- in
  // Rust, by `PrLink`'s `pr_link_serialises_snake_case` and by
  // `invariants.rs`'s
  // `every_mirrored_type_agrees_with_its_rust_wire_spelling`, which
  // compare the real serialised keys against this file's type. This
  // filter is not a second spelling of that contract and deliberately
  // does NOT read `sessionId`: accepting both spellings would make the
  // wire unfalsifiable and let the next drift through in silence.
  //
  // What it buys is proportionality. `PrLink` serialised `sessionId`
  // for a release and `l.session_id.slice(0, 8)` threw on `undefined`,
  // which took down the ENTIRE pull request page -- title, checks,
  // diff, review -- over a provenance footnote. This panel already
  // treats a failed lookup as silent for exactly that reason: it is not
  // the subject of the page. A malformed row now costs its own line and
  // nothing else.
  const links = (q.data ?? []).filter(
    (l: ClaudePrLink) => typeof l?.session_id === "string" && l.session_id.length > 0,
  );

  // Nothing to say: no link recorded here, or the lookup failed. A
  // failed lookup is deliberately silent rather than an error panel --
  // this is provenance, not the subject of the page, and a red box
  // about a secondary join would crowd out the PR the user came for.
  if (links.length === 0) return null;

  return (
    <Section title="Written by" count={links.length}>
      <ul className="space-y-0.5">
        {links.map((l: ClaudePrLink) => (
          <li key={l.session_id} className="text-xs">
            <button
              type="button"
              onClick={() => {
                // `setView` FIRST, and the order is load-bearing: it
                // resets `claudePage` to "overview" and clears
                // `claudeSelected`, so the natural-reading order --
                // select, then page, then view -- lands on the overview
                // with nothing selected.
                //
                // This is #920's bug in a new place. The store's own
                // `showClaudeSessions` comment records it: "that jump
                // had to call `setView` BEFORE `setFilter`... the
                // natural-reading order filed the value under the page
                // being left and the destination opened on its
                // default." A test asserting only the view would not
                // have noticed; the one asserting all three caught it.
                const st = useFilters.getState();
                st.setView("claude-code");
                st.setClaudePage("sessions");
                st.selectClaudeSession(l.session_id);
              }}
              className="text-[#58a6ff] hover:underline"
            >
              {l.session_id.slice(0, 8)}
            </button>
            {l.first_seen_at ? (
              <span className="ml-2 text-[#8b949e]">first linked {l.first_seen_at}</span>
            ) : null}
          </li>
        ))}
      </ul>
    </Section>
  );
}

export function PrDetailView({
  repo,
  number,
  onBack,
}: {
  repo: string;
  number: number;
  onBack: () => void;
}) {
  // `isPlaceholderData` is true while this is the clicked row's own data
  // standing in for the fetch (#790). `isLoading` is false in that state
  // -- TanStack reports `success` on placeholder data -- which is exactly
  // what lets the real view render immediately; the spinner branch below
  // is now only for a pull request with no cached row to seed from.
  const {
    data: pr,
    isLoading,
    isPlaceholderData,
    isError,
    error,
    refetch,
  } = usePrDetail(repo, number);
  const deleteBranch = useDeleteHeadBranch();
  const review = useReviewPr();
  const comment = useCommentOnPr();
  // Undefined until the login lands, and undefined FOREVER if it fails.
  // ReviewBox reads that as "might not be mine" rather than "is mine",
  // so a failed viewer fetch never silently removes the approve button.
  const { data: viewer } = useViewer();
  // The base branch's review rules (#1451, #1454). Undefined while pending
  // and when unreadable alike -- both render nothing new -- so `gate` is
  // all-null until a rule is actually READ.
  const { data: gates } = useReviewGates(pr, isPlaceholderData);
  const gate = pr
    ? gateVerdict(gates, pr, viewer, isPlaceholderData)
    : { approveWontCount: null, approveCaveat: null, mergeBlocked: null };
  const [reviewing, setReviewing] = useState<ReviewVerdictName | null>(null);
  const rerun = useRerunChecks();
  const [rerunning, setRerunning] = useState(false);
  /// Whether the remote-branch deletion is awaiting confirmation (#845).
  ///
  /// A boolean rather than a nullable ref, because the dialog can only
  /// ever be about this view's own head ref -- there is one branch on
  /// this page, and it is `pr.head_ref`. The gate it renders behind
  /// re-reads `pr.head_ref_id`, so a detail refetch that loses the ref
  /// closes the question rather than leaving a dialog offering to delete
  /// something the view can no longer name.
  const [deleting, setDeleting] = useState(false);
  const rerunnable = pr ? rerunnableRun(pr.checks) : null;
  // The same actions in a different arrangement: on a phone the sticky
  // header stacks the action buttons under the back link, and the
  // footer wraps, rather than dropping anything.
  const isMobile = useIsMobile();
  // The viewer's own verdict, read the same way ReviewBox reads it: the
  // pull request's aggregate `review` says CHANGES_REQUESTED when
  // somebody ELSE blocked it, which says nothing about whether this
  // user approved. DISMISSED is deliberately not an approval -- GitHub
  // dismisses a review when the branch changes under it.
  const approvedByViewer =
    viewer !== undefined &&
    pr?.latest_reviews?.some((r) => r.author === viewer && r.state === "APPROVED") === true;

  /// Lifted out of the ReviewBox JSX so the sticky header can submit
  /// the same way. Two call sites for one mutation, and a second inline
  /// copy would be the kind of duplication that drifts.
  ///
  /// `pr` is non-null at every call site (both are inside the loaded
  /// branch), but this closure is defined above the guard, so the guard
  /// is restated rather than asserted away.
  const submitReview = (verdict: ReviewVerdictName, body: string) => {
    if (!pr) return;
    setReviewing(verdict);
    const done = () => setReviewing(null);
    const label =
      verdict === "approve"
        ? "Approved"
        : verdict === "request_changes"
          ? "Changes requested on"
          : "Commented on";
    // "Comment" posts a CONVERSATION comment, not a COMMENT
    // review. They are different nodes: addComment creates an
    // IssueComment, addPullRequestReview creates a
    // PullRequestReview with state COMMENTED. The list above
    // renders IssueComments -- so routing this through the review
    // mutation would post something the user then could not see.
    const submit =
      verdict === "comment"
        ? comment(pr.id, pr.repo, pr.number, body)
        : review(pr.id, pr.repo, pr.number, verdict, body);
    submit.then(
      () => {
        done();
        // The after-approve state: an approval that will not count toward
        // merging still reads "Approved", so the toast says what it means
        // (#1451).
        toast.success(`${label} ${pr.repo}#${pr.number}`, {
          description:
            verdict === "approve" && gate.approveWontCount ? gate.approveWontCount : undefined,
        });
      },
      (e: unknown) => {
        done();
        // GitHub's refusal is the useful part -- "Can not approve
        // your own pull request" tells the user exactly what
        // happened where a generic message would not.
        toast.error(`Could not review #${pr.number}`, {
          description: typeof e === "string" ? e : undefined,
        });
      },
    );
  };

  const back = (
    <button
      type="button"
      onClick={onBack}
      className="mb-3 flex items-center gap-1.5 text-sm text-[#8b949e] hover:text-[#e6edf3]"
    >
      <ArrowLeft className="h-4 w-4" aria-hidden="true" />
      Back to list
    </button>
  );

  // Only reachable with NO cached row to seed from: a cold launch
  // straight into a detail view, or a pull request in neither list.
  // Every click from a list now skips this entirely and renders the
  // seeded view below -- see `usePrDetail` (#790).
  if (isLoading) {
    return (
      <div>
        {back}
        <div className="rounded-md border border-[#30363d] px-4 py-12 text-center text-sm text-[#8b949e]">
          Loading pull request…
        </div>
      </div>
    );
  }

  if (isError || !pr) {
    return (
      <div>
        {back}
        <QueryError
          title="Could not load this pull request"
          message={errorMessage(error)}
          onRetry={() => void refetch()}
        />
      </div>
    );
  }

  /// The pinned actions, built once so the phone and desktop headers
  /// place the same elements rather than two copies that drift.
  const pinnedActions = (
    <>
      {/* The two the user actually reaches for, in the order they
          reach for them. Approve is absent: it needs the comment box
          that only makes sense in the body, and a bare approve
          button here would submit an empty review from a header the
          user may have scrolled past without reading. */}
      {/* Approve, pinned.

          Deliberately omitted at first because it submits a review
          with no comment from a bar the user may have scrolled
          past. Added on request -- and GitHub allows an empty
          approval, so the objection was about accident, not
          validity. The guards that make it safe are the same ones
          ReviewBox applies: hidden on your own pull request, which
          GitHub refuses outright, and showing "Approved" rather
          than offering a second one once your approval is on
          record. */}
      {viewer !== undefined && viewer !== pr.author ? (
        <button
          type="button"
          disabled={approvedByViewer || reviewing !== null}
          onClick={() => submitReview("approve", "")}
          title={
            approvedByViewer
              ? "You have already approved this pull request"
              : (gate.approveWontCount ?? "Approve without a comment")
          }
          className={`rounded px-2.5 py-1 text-sm font-medium ${
            approvedByViewer || reviewing !== null
              ? "border border-[#30363d] text-[#8b949e] opacity-50"
              : "bg-[#238636] text-white hover:bg-[#1a7f37]"
          }`}
        >
          {reviewing === "approve"
            ? "Working…"
            : approvedByViewer
              ? "Approved"
              : "Approve"}
        </button>
      ) : null}
      {/* The header has no room for the sentence, so it carries the short
          form beside the button, with the full one in its title and in
          the body's review box (#1451). */}
      {viewer !== undefined && viewer !== pr.author && gate.approveWontCount ? (
        <span className="text-xs font-medium text-[#d29922]" title={gate.approveWontCount}>
          Won't count toward merging
        </span>
      ) : null}
      <PrActions pr={pr} compact conversations={gate.mergeBlocked} />
    </>
  );

  return (
    // `max-w-4xl mx-auto`: the body is prose and prose needs a measure.
    // At full window width a description ran the entire monitor, which
    // is both hard to read and what made every section feel crammed
    // against its neighbour. The sticky header opts out via `-mx-4` so
    // it still spans the panel.
    <div className="mx-auto flex max-w-4xl flex-col gap-3">
      {/* NOTE: the body's own `back` button is deliberately not rendered
          here. The sticky header carries one that is always visible, and
          two "back" controls a few pixels apart is worse than one. The
          loading and error branches above still use `back`, since they
          have no header to hang it on. */}
      {/* Sticky, because the actions were unreachable from where the
          decision gets made. Reading a long PR put "Back to list" far
          above the viewport and "View on GitHub" far below it, so
          approving meant scrolling to the top and opening it on GitHub
          meant scrolling to the bottom.

          The scroll container is `<main>` in App, which is this
          element's scrolling ancestor -- that is what makes `sticky`
          work here at all.

          `top-0` was correct once and is NOT any more (#1278). This
          comment used to read "the app header above scrolls away with
          the content rather than being sticky itself, so nothing
          overlaps"; #623 made that app header `sticky top-0 z-20` for
          the phone layout, in this same scroll container. At `top-0`
          this bar still pinned -- it just pinned to the exact band the
          app header occupies, one z-layer down and behind its opaque
          background. Sticky was never broken here; the bar was pinned
          and invisible, which looks identical to not sticking at all.

          `--app-header-h` is the app header's measured height, written
          onto `<main>` by `useStickyHeaderOffset`. Measured rather than
          hardcoded because the phone's header is taller than the
          desktop's (a 44px `.tap-target` hamburger), so one number
          would be wrong on one of the two layouts this component
          serves. The fallback keeps the bar pinned somewhere sane if
          the variable is ever missing -- in jsdom, for instance, where
          nothing publishes it. */}
      <div
        className={
          isMobile
            ? "sticky z-10 -mx-4 flex flex-wrap items-center gap-2 border-b border-[#30363d] bg-[#0d1117] px-4 py-2"
            : "sticky z-10 -mx-4 flex items-center gap-2 border-b border-[#30363d] bg-[#0d1117] px-4 py-2"
        }
        style={{ top: "var(--app-header-h, 0px)" }}
      >
        <button
          type="button"
          onClick={onBack}
          className="flex shrink-0 items-center gap-1.5 text-sm text-[#8b949e] hover:text-[#e6edf3]"
        >
          <ArrowLeft className="h-4 w-4" aria-hidden="true" />
          Back to list
        </button>
        {/* No title or number here on purpose.

            Both are already in the <h2> immediately below, so putting
            them in the header repeats the same words at the top of the
            page and makes them ambiguous to a screen reader, which
            reads every copy. Only one pull request is ever open, so
            the pinned buttons cannot be about a different one. */}
        <div className="ml-auto flex shrink-0 items-center gap-2">
          {isMobile ? null : pinnedActions}
          <ExternalLink
            href={pr.url}
            className="flex items-center gap-1.5 rounded border border-[#30363d] px-2.5 py-1 text-sm hover:bg-[#161b22]"
          >
            <ExternalLinkIcon className="h-3.5 w-3.5" aria-hidden="true" />
            GitHub
          </ExternalLink>
        </div>
        {/* A second, full-width line on the phone: back and GitHub
            fit beside each other, but Approve and Merge on the same
            line would push GitHub off the screen. */}
        {isMobile ? (
          <div className="flex basis-full flex-wrap items-center gap-2">{pinnedActions}</div>
        ) : null}
      </div>

      <div>
        <h2 className="text-lg font-semibold leading-snug text-[#e6edf3]">
          {pr.title} <span className="font-normal text-[#8b949e]">#{pr.number}</span>
        </h2>
        {/* ONE metadata line, not two stacked paragraphs. The branch
            pair and the diff size are the same kind of fact about the
            same pull request, and splitting them across two lines was
            half the vertical noise above the fold. */}
        <p className="mt-1 flex flex-wrap items-center gap-x-1.5 text-xs text-[#8b949e]">
          <span>
            {pr.author} wants to merge <span className="font-mono">{pr.head_ref}</span> into{" "}
            <span className="font-mono">{pr.base_ref}</span>
          </span>
          <span aria-hidden="true">·</span>
          <span>{pr.repo}</span>
          {pr.is_draft ? (
            <>
              <span aria-hidden="true">·</span>
              <span>draft</span>
            </>
          ) : null}
          {/* Opened, ready for review, last commit (#1457); each omitted
              when it cannot be read. */}
          <PrDates pr={pr} />
          <StackBadge stack={pr.stack} />
          {/* The ONE metadata fact the list row does not carry: the list
              query does not select additions, deletions or changedFiles
              (see `PRS_QUERY`). So while this is the seeded placeholder
              the three are zero, and printing "+0 −0 across 0 files" on
              a real pull request would be a lie the user cannot tell
              from a genuinely empty diff. Omitted until it arrives; the
              header reflows by one item rather than showing a wrong
              number. A real zero-line pull request is possible and also
              shows nothing here, which is the right way round: silence
              costs a fact, a zero invents one (#790). */}
          {isPlaceholderData && pr.changed_files === 0 ? null : (
            <>
              <span aria-hidden="true">·</span>
              <span className="tabular-nums">
                +{pr.additions.toLocaleString()} −{pr.deletions.toLocaleString()} across{" "}
                {pr.changed_files} file{pr.changed_files === 1 ? "" : "s"}
              </span>
            </>
          )}
          {pr.unresolved_threads > 0 ? (
            <>
              <span aria-hidden="true">·</span>
              <span className="text-[#d29922]">
                {pr.unresolved_threads} unresolved conversation
                {pr.unresolved_threads === 1 ? "" : "s"}
              </span>
            </>
          ) : null}
        </p>
      </div>

      <PrActions pr={pr} conversations={gate.mergeBlocked} />

      {/* Available on EVERY pull request, not only the review queue.
          Gating this on which list you arrived from would mean the same
          pull request offers different actions depending on how you
          navigated to it -- and commenting on your own work is normal.
          Approving your own is the one case GitHub refuses, and
          ReviewBox handles that itself. */}
      <ReviewBox
        viewer={viewer}
        author={pr.author}
        latestReviews={pr.latest_reviews}
        approveWontCount={gate.approveWontCount}
        approveCaveat={gate.approveCaveat}
        busy={reviewing}
        onSubmit={submitReview}
      />

      {pr.body.trim() ? (
        // Open by default: the description is what the pull request IS,
        // and collapsing it would hide the thing you opened the view to
        // read.
        <Section title="Description">
          <Markdown>{pr.body}</Markdown>
        </Section>
      ) : isPlaceholderData ? (
        // "No description." would be WRONG here, not merely early: the
        // body is the one thing the seeded row cannot carry, and the
        // sections below it are hidden while empty -- so a user who
        // clicked a pull request to read its description would be told
        // there isn't one, and nothing on the page would correct that
        // until the fetch landed. This is also the only place the view
        // says a fetch is still running, which is why it names the two
        // things the wait is actually for (#790).
        <p className="text-sm text-[#8b949e]">Loading the description and checks…</p>
      ) : (
        <p className="text-sm text-[#8b949e]">No description.</p>
      )}

      {pr.checks.length > 0 ? (
        // COLLAPSED when everything passed. A wall of twenty green
        // check rows is the single largest block on a healthy pull
        // request and tells you nothing you did not already learn from
        // the CI pill -- but it stays open the moment anything is not
        // passing, which is when you actually need the names.
        <Section
          title="Checks"
          count={pr.checks.length}
          // Open when anything is not passing -- and open when the list
          // is CAPPED, because an all-green page of 300 that is missing
          // 112 contexts is the one case where the collapsed "everything
          // passed" summary is the least trustworthy (#790).
          defaultOpen={
            pr.checks.some((c) => c.state !== "success") || pr.checks_total > pr.checks.length
          }
          // Offered only when something FAILED and that failure belongs
          // to an Actions workflow run. A status context and a
          // non-Actions check both have no run to re-run, so the button
          // would 404 rather than help.
          aside={
            rerunnable !== null ? (
              <button
                type="button"
                disabled={rerunning}
                onClick={() => {
                  setRerunning(true);
                  rerun(pr.repo, pr.number, rerunnable).then(
                    () => {
                      setRerunning(false);
                      toast.success(`Re-running failed checks on #${pr.number}`);
                    },
                    (e: unknown) => {
                      setRerunning(false);
                      // GitHub's refusal is the useful part: "This
                      // workflow run cannot be retried" says exactly why
                      // where a generic message would not.
                      toast.error(`Could not re-run checks on #${pr.number}`, {
                        description: typeof e === "string" ? e : undefined,
                      });
                    },
                  );
                }}
                className="rounded border border-[#30363d] px-2 py-1 text-xs font-normal text-[#e6edf3] hover:bg-[#161b22] disabled:opacity-50"
              >
                {rerunning ? "Working…" : "Re-run failed"}
              </button>
            ) : null
          }
        >
          {pr.checks.map((c) => (
            <CheckRow key={c.name} {...c} />
          ))}
          {/* HONEST TRUNCATION, the same shape the comments list and the
              PR list's "showing 100 of 137" use. The Rust side pages the
              rollup up to a budget (#790 cut it from 20 serial requests
              to 3), so a pull request with hundreds of contexts arrives
              capped -- and the reason that budget is a tradeoff and not a
              free win is precisely that a short check list does not LOOK
              short: it renders a wall of green on a pull request whose
              rollup says FAILURE. Saying so is what keeps the cap safe.

              `>` rather than a subtraction against a possibly-larger
              length: the two numbers come from different pages of a
              rollup that can grow mid-fetch, so the total can legitimately
              be the smaller one and that is not a shortfall. */}
          {pr.checks_total > pr.checks.length ? (
            <p className="px-2 py-1.5 text-xs text-[#d29922]">
              Showing {pr.checks.length} of {pr.checks_total} checks. A failure could be among
              the rest — see them on GitHub.
            </p>
          ) : null}
        </Section>
      ) : null}

      {/* ABOVE the comments: a conversation waiting on an answer is
          more urgent than the discussion thread, and the header's
          unresolved count points at this section. */}
      {/* `total` so a truncated list can say so (#802). Passed down rather
          than annotated here, because this section's header and its rows
          both live inside `ReviewThreads` -- the notice belongs next to
          the rows it qualifies. */}
      <ReviewThreads
        threads={pr.review_threads}
        total={pr.review_threads_total}
        repo={pr.repo}
        number={pr.number}
      />

      {/* WHICH SESSION WROTE THIS (#1211). The reverse of the link the
          Claude Code page has shown since #1132, and the more useful
          direction: a PR that broke sends you looking for the session,
          and finding it by title fails -- 286 of 1,438 sessions share a
          title with another.

          Below the review threads because it answers "where did this
          come from" rather than "what is wrong with it", and the second
          question is the one a reader opens a failing PR to ask. */}
      <PrSessions repo={pr.repo} number={pr.number} />

      {pr.comments.length > 0 ? (
        // COLLAPSED past a handful. Fifty comments is the longest block
        // in this view by far, and scrolling past all of it to reach
        // the footer links was most of the "shoved together" problem.
        // A short thread stays open, because collapsing three comments
        // hides nothing worth a click.
        <Section title="Comments" count={pr.comment_count}>
          <div className="flex flex-col gap-2">
          {/* At the TOP, and naming which ones are missing (#1453). The
              query fetches the newest comments, so what is cut is the
              oldest, and the reader should know that before scrolling
              rather than after. */}
          {pr.comment_count > pr.comments.length ? (
            <p className="text-xs text-[#8b949e]">
              Showing the newest {pr.comments.length} of {pr.comment_count} — older ones are on
              GitHub.
            </p>
          ) : null}
          {/* Each comment collapses on its OWN, rather than the whole
              block collapsing together. One section for fifty comments
              meant finding a particular one required expanding all of
              them and scrolling; the collapsed row carries a body
              preview so it can be picked out without opening it.

              A lone comment opens by default -- there is nothing to
              scan past, so collapsing it only adds a click. */}
          {pr.comments.map((c, i) => (
            <CommentRow
              key={`${c.author}-${c.created_at}-${i}`}
              author={c.author}
              createdAt={c.created_at}
              body={c.body}
              defaultOpen={pr.comments.length === 1}
            />
          ))}
          </div>
        </Section>
      ) : null}

      <div className={isMobile ? "flex flex-wrap items-center gap-2" : "flex items-center gap-2"}>
        <ExternalLink
          href={pr.url}
          className="flex w-fit items-center gap-1.5 rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#161b22]"
        >
          <ExternalLinkIcon className="h-3.5 w-3.5" aria-hidden="true" />
          View on GitHub
        </ExternalLink>
        {/* Claudify (#1455), which replaced "Copy for agent". Worth more
            here than on a row: this view has the per-check names and
            URLs, the size and the description, so the prompt names the
            jobs that actually failed and adapts its review criteria. */}
        <PrClaudifyButton pr={pr} />

        {/* Only once the PR has MERGED, and only while the branch still
            exists. 31 of the last 60 merged PRs on a real account still
            held a live remote branch -- the app's own thesis (agents
            create branches, PRs merge, leftovers stay) applied to the
            one domain where it did nothing.

            Deleting the head ref of an OPEN pull request closes it off,
            so the gate is re-checked on the Rust side too. */}
        {pr.state === "MERGED" && pr.head_ref_id ? (
          <button
            type="button"
            // ASKS, rather than deleting (#845). This fired
            // `deleteBranch(..., true)` straight from `onClick` -- a
            // REMOTE deletion, the one operation in this app that no
            // reflog can undo, on one click. `BranchesPage` states the
            // rule for the very same operation: "Local deletion is
            // recoverable from the reflog; a remote deletion is not, so
            // it is never what a distracted Enter press does." This view
            // made it exactly that.
            onClick={() => setDeleting(true)}
            // DESTRUCTIVE styling, the classes every other destructive
            // button in the app uses. The old `className` was
            // BYTE-IDENTICAL to "View on GitHub" and "Copy for agent"
            // directly above it -- two actions that change nothing --
            // so the control that destroyed a shared ref was the one
            // thing in the row with no visual warning at all.
            className="flex w-fit items-center gap-1.5 rounded border border-[#f85149]/40 px-3 py-1.5 text-sm text-[#f85149] hover:bg-[#f85149]/10"
          >
            <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
            {/* The ellipsis says a question comes first, as on
                `BranchesPage`'s "Delete {n}…" and the orphan row's
                "Delete…". */}
            Delete branch…
          </button>
        ) : null}
      </div>

      {/* The confirmation (#845).

          Reuses `scopeEffect("remote")` rather than wording its own
          warning. That sentence is `BranchesPage`'s, lifted into
          `lib/branchDelete` so there is exactly one of it: two wordings
          of "no local reflog can undo that" is two claims about one
          operation, and `scopeLabel`'s comment makes the same argument
          for the confirm button's text.

          Deliberately NOT a scope questionnaire like `BranchesPage`'s.
          There is no choice to offer -- this view knows only the head
          ref on GitHub, and `deleteBranch(..., true)` is hardwired to
          the remote -- and a question with one answer trains people to
          click through questions that have several. What it borrows is
          the WARNING, not the form. */}
      {deleting && pr.head_ref_id ? (
        <Dialog open onOpenChange={(o) => !o && setDeleting(false)}>
          <DialogContent className="max-w-lg">
            <DialogTitle>Delete {pr.head_ref} on the remote?</DialogTitle>
            {/* The REF, spelled out in mono. A branch name in prose is
                easy to skim past, and this is the only identifier of
                what is about to go. */}
            <p className="mt-3 break-all font-mono text-xs text-[#8b949e]">
              {pr.repo} · {pr.head_ref}
            </p>
            <p className="mt-3 text-sm text-[#f85149]">{scopeEffect("remote")}</p>
            {/* What is actually lost, which the scope sentence cannot
                say on its own: the ref is the only NAMED handle on the
                pre-merge history. The commits survive in the merge, so
                overstating this as "the work is gone" would be the kind
                of wrongness that teaches users to discount the red. */}
            <p className="mt-2 text-sm text-[#8b949e]">
              The commits are already in the merge. What goes is the only named
              reference to the branch as it stood before it.
            </p>
            <div className="mt-5 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setDeleting(false)}
                className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#21262d]"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => {
                  const refId = pr.head_ref_id as string;
                  setDeleting(false);
                  deleteBranch(refId, pr.repo, pr.number, pr.head_ref, true).then(
                    () => toast.success(`Deleted ${pr.head_ref}`),
                    (e: unknown) =>
                      toast.error(`Could not delete ${pr.head_ref}`, {
                        description: typeof e === "string" ? e : undefined,
                      }),
                  );
                }}
                className="rounded bg-[#da3633] px-3 py-1.5 text-sm font-medium text-white hover:bg-[#c93c37]"
              >
                {/* `scopeLabel`'s own wording for this scope, minus the
                    count: "on the remote" is the half that carries the
                    warning into the button, which is where a distracted
                    Enter press lands. */}
                Delete on the remote
              </button>
            </div>
          </DialogContent>
        </Dialog>
      ) : null}
    </div>
  );
}

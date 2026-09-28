import { prKey } from "@/lib/prIdentity";
import { CircleCheck, GitCommitHorizontal, MessageCircleWarning } from "lucide-react";
import type { PullRequest } from "@/types/pr";
import { type Filters, readyForReview, sortReadyForReview } from "@/lib/derive";
import { useActiveFilters, useFilters } from "@/store/filters";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ExternalLink } from "./ExternalLink";
import { READY_TONE_CLASS, readyAge, useNow } from "@/lib/readyAge";
import { useReadyPushers } from "@/api/hooks";
import {
  approvalWontCount,
  type MyPushesMode,
  partitionReady,
  partitionSummary,
  type ReadyPusher,
} from "@/lib/readyPusher";
import { type ReadyRow, lastPusherOf, readyListMarkdown } from "@/lib/readyMarkdown";
import { CopyMarkdownButton } from "./CopyMarkdownButton";
import { ReadyClaudify } from "./ReadyClaudify";

/// Both labels name the FIELD, not just the direction (#1277).
///
/// "Oldest first" is ambiguous between "opened longest ago" and "waiting
/// for review longest", and on anything that spent time as a draft those
/// are different pull requests. Since #1407 the strip sorts on when each
/// became READY, so the labels say `ready`. Renaming these back to bare
/// directions would put the ambiguity straight back.
///
/// The values keep their persisted `-opened` spelling; see `readySort`.
const READY_SORT_OPTIONS: {
  value: NonNullable<Filters["readySort"]>;
  label: string;
}[] = [
  { value: "oldest-opened", label: "Oldest ready first" },
  { value: "newest-opened", label: "Newest ready first" },
];

/// Re-read the clock once a minute. The coarsest unit shown is minutes,
/// and a threshold crossing is at most this late.
const AGE_TICK_MS = 60_000;

/// How long a row has been ready for review (#1407).
///
/// The TEXT carries the age, so colour is never the only signal. The
/// exact time is in the `title` for a pointer and in visually hidden text
/// for a screen reader, which reads it as part of the row's name.
///
/// Unknown renders as a neutral "age unknown" -- never green, never a
/// number. Absent is not zero.
function ReadyAgeChip({ readyAt, now }: { readyAt: PullRequest["ready_at"]; now: Date }) {
  const age = readyAge(readyAt, now);
  const chip = `shrink-0 whitespace-nowrap rounded-full border px-1.5 py-0.5 text-xs tabular-nums ${READY_TONE_CLASS[age.tone]}`;
  if (age.since === null) {
    return (
      <>
        <span data-ready-age={age.tone} title="Ready-for-review time unknown" className={chip} aria-hidden="true">
          {age.text}
        </span>
        <span className="sr-only">, ready-for-review time unknown</span>
      </>
    );
  }
  const since = age.since.toLocaleString();
  return (
    <>
      <time
        data-ready-age={age.tone}
        dateTime={readyAt ?? undefined}
        title={`Ready for review since ${since}`}
        className={chip}
      >
        {age.text}
      </time>
      <span className="sr-only">, ready for review since {since}</span>
    </>
  );
}

/// Open review conversations on a row the strip calls ready (#1577).
///
/// `readyForReview` does not consider conversations, so without this a
/// pull request with open questions reads as clean. Amber, as on the main
/// list's row: unanswered questions are not a failure.
///
/// The count is a FLOOR when the list query's thread page came back full
/// (#802), and then prints as "N+" and reads as "at least N". Absent
/// (a payload from an older desktop) is treated the same: qualified,
/// never an exact-looking total that might be low.
///
/// Nothing renders for zero. The text carries the count, the icon and
/// colour only repeat it, and the accessible name spells it out.
function UnresolvedChip({
  count,
  floor,
}: {
  count: number;
  floor: PullRequest["unresolved_threads_floor"];
}) {
  if (!(count > 0)) return null;
  const mayBeShort = floor !== false;
  const noun = `unresolved conversation${count === 1 ? "" : "s"}`;
  const label = mayBeShort ? `at least ${count} ${noun}` : `${count} ${noun}`;
  return (
    <>
      <span
        data-unresolved={mayBeShort ? "floor" : "exact"}
        title={label.charAt(0).toUpperCase() + label.slice(1)}
        aria-hidden="true"
        className="inline-flex shrink-0 items-center gap-1 whitespace-nowrap rounded-full border border-[#d29922]/40 px-1.5 py-0.5 text-xs tabular-nums text-[#d29922]"
      >
        <MessageCircleWarning className="h-3 w-3" aria-hidden="true" />
        {mayBeShort ? `${count}+` : count}
      </span>
      <span className="sr-only">, {label}</span>
    </>
  );
}

/// The viewer pushed this row's head commit (#1576).
///
/// Shown ONLY when the activity log named the viewer as the pusher of the
/// head the row shows. A row not checked, or checked and undecided, shows
/// nothing -- not a "someone else" it does not know. The text carries the
/// fact, the colour only repeats it, and the accessible name says it, with
/// the consequence when the base's rules were read as requiring someone
/// else's approval of the last push.
function PushedByYouChip({ pusher }: { pusher: ReadyPusher }) {
  if (pusher.pusher.state !== "viewer") return null;
  const label = approvalWontCount(pusher)
    ? "you pushed the latest commit, so your approval won't count here"
    : "you pushed the latest commit";
  return (
    <>
      <span
        data-pushed-by-you
        title={label.charAt(0).toUpperCase() + label.slice(1)}
        aria-hidden="true"
        className="inline-flex shrink-0 items-center gap-1 whitespace-nowrap rounded-full border border-[#4493f8]/40 px-1.5 py-0.5 text-xs text-[#4493f8]"
      >
        <GitCommitHorizontal className="h-3 w-3" aria-hidden="true" />
        your push
      </span>
      <span className="sr-only">, {label}</span>
    </>
  );
}

/// The choice for rows the viewer pushed last, and what it hid (#1576).
const MY_PUSHES_OPTIONS: { value: MyPushesMode; label: string; short: string }[] = [
  { value: "auto", label: "Hide where my approval can't count", short: "auto" },
  { value: "hide", label: "Hide all I pushed last", short: "hidden" },
  { value: "show", label: "Show all", short: "shown" },
];

/// The strip's last-pusher control and its status line (#1576).
///
/// Under the header rather than in it: the header carries the strip's
/// list-wide actions, and this is a filter over the rows below.
///
/// The status line says how many rows are hidden and how many the filter
/// could not decide, as counts. While the first answer is on its way it
/// says so, rather than counting every row as "not checked".
function MyPushesBar({
  mode,
  onMode,
  checking,
  summary,
}: {
  mode: MyPushesMode;
  onMode: (m: MyPushesMode) => void;
  checking: boolean;
  summary: string | null;
}) {
  const current = MY_PUSHES_OPTIONS.find((o) => o.value === mode) ?? MY_PUSHES_OPTIONS[0];
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-[#3fb950]/20 px-4 py-1 text-xs text-[#8b949e]">
      <DropdownMenu>
        <DropdownMenuTrigger
          render={
            <Button
              variant="ghost"
              size="sm"
              className="h-6 px-1.5 text-xs font-normal"
              aria-label={`Pull requests you pushed last: ${current.label}`}
            >
              Pushed last by you: {current.short}
            </Button>
          }
        />
        <DropdownMenuContent>
          <DropdownMenuRadioGroup
            value={mode}
            onValueChange={(value) => onMode(value as MyPushesMode)}
          >
            {MY_PUSHES_OPTIONS.map((opt) => (
              <DropdownMenuRadioItem key={opt.value} value={opt.value}>
                {opt.label}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </DropdownMenuContent>
      </DropdownMenu>
      <span role="status" data-my-pushes-status>
        {mode !== "show" && checking ? "Checking who pushed last…" : summary}
      </span>
    </div>
  );
}

/// Pinned above the review queue: what is ready to review right now.
///
/// The counterpart to `PrioritiesStrip` on My pull requests. That one
/// says what is blocked on you as an author; this says what a reviewer
/// can pick up without wasting anyone's time -- not a draft, checks
/// passed, no conflicts, nobody has reviewed it yet.
///
/// `readyForReview` is the single source of truth for the predicate.
/// Re-deriving it here would risk a second, drifting copy of a rule
/// that decides what a reviewer sees first.
///
/// The empty state is one quiet line rather than a card, matching the
/// attention strip: a section that shouts when there is nothing in it
/// stops being read, and then it fails on the day it matters.
///
/// Ordered OLDEST READY FIRST by default (#1277, #1407). Working top to
/// bottom through a review queue should mean working through it in the
/// order the pull requests became reviewable, and newest-first buries the
/// three-day-old one whose author is blocked. The sort control offers the
/// other order, and `sortReadyForReview` documents why `ready_at` and not
/// `created_at`.
///
/// Each row carries its age since it became ready (`ReadyAgeChip`), kept
/// current by a once-a-minute clock rather than a re-fetch.
///
/// The preference lives in the per-view filter store under `readySort`,
/// where every other view preference already lives and where `partialize`
/// persists it without a second mechanism. Per-view rather than global
/// because this strip only renders on To Review.
export function ReadyStrip({
  prs,
  onOpen,
}: {
  prs: PullRequest[];
  /// Open a pull request's detail view. Optional so a caller with
  /// nowhere to send the user does not get a row that LOOKS clickable
  /// and is not -- the entry falls back to a plain link.
  onOpen?: (pr: PullRequest) => void;
}) {
  // Read unconditionally, above the early return: hooks cannot sit below
  // one, and the empty state needs no sort but the rules of hooks do not
  // care.
  const { readySort, readyMyPushes } = useActiveFilters();
  const setFilter = useFilters((s) => s.setFilter);
  const now = useNow(AGE_TICK_MS);

  // `all` is every row the predicate admits; `ready` is what the viewer's
  // last-push choice leaves showing (#1576). The header counts and lists
  // `ready`; the bar under it says how many were hidden and how many
  // could not be decided.
  const all = sortReadyForReview(prs.filter(readyForReview), readySort);
  const pushers = useReadyPushers(all);
  const mode: MyPushesMode = readyMyPushes ?? "auto";
  const part = partitionReady(all, pushers.of, mode);
  const ready = part.shown;

  // The rows as SHOWN -- after the last-push filter, in the sort. The
  // list below and "Copy as markdown" both read this, so the copy cannot
  // disagree with the screen (#1578). A count with no floor flag (an
  // older desktop's payload) is a floor, as the chip reads it.
  const shown: ReadyRow[] = ready.map((pr) => ({
    pr,
    lastPusher: lastPusherOf(pushers.of(pr).pusher),
    unresolvedIsFloor: pr.unresolved_threads_floor !== false,
  }));

  if (all.length === 0) {
    return <p className="px-4 py-2 text-xs text-[#8b949e]">Nothing ready to review.</p>;
  }

  return (
    <section className="mb-4 rounded-md border border-[#3fb950]/40 bg-[#3fb950]/5">
      <h2 className="flex items-center gap-2 border-b border-[#3fb950]/30 px-4 py-2 text-sm font-semibold text-[#3fb950]">
        <CircleCheck className="h-4 w-4" aria-hidden="true" />
        Ready for review ({ready.length})
        {/* Beside the sort, and built on click from `shown` -- the rows
            on screen, in their order (#1578). */}
        <CopyMarkdownButton
          label="Copy as markdown"
          markdown={() => readyListMarkdown(shown, { now: new Date() })}
          copied={() => ({
            title: `Copied ${shown.length} ${shown.length === 1 ? "pull request" : "pull requests"} as markdown`,
            description: "Paste it into a chat or a Claude session.",
          })}
          className="ml-auto font-normal"
        />
        {/* The same rows, handed to Claude to approve and merge what is
            eligible (#1579). It only starts Claude; see ReadyClaudify. */}
        <ReadyClaudify rows={shown} />
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button variant="ghost" size="sm" className="font-normal">
                {/* The current order is spelled out rather than hidden
                    behind a bare "Sort" until it is changed. This list
                    having a non-obvious default is the whole point, and a
                    default nobody can see is one nobody can trust. */}
                Sort: {
                  READY_SORT_OPTIONS.find(
                    (opt) => opt.value === (readySort ?? "oldest-opened"),
                  )?.label
                }
              </Button>
            }
          />
          <DropdownMenuContent>
            <DropdownMenuRadioGroup
              value={readySort ?? "oldest-opened"}
              onValueChange={(value) =>
                setFilter("readySort", value as Filters["readySort"])
              }
            >
              {READY_SORT_OPTIONS.map((opt) => (
                <DropdownMenuRadioItem key={opt.value} value={opt.value}>
                  {opt.label}
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      </h2>
      <MyPushesBar
        mode={mode}
        onMode={(m) => setFilter("readyMyPushes", m)}
        checking={pushers.isPending}
        summary={partitionSummary(part, mode)}
      />
      <ul>
        {shown.map(({ pr }) => (
          <li key={prKey(pr)} className="text-sm">
            {onOpen ? (
              <div
                role="button"
                tabIndex={0}
                onClick={() => onOpen(pr)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    onOpen(pr);
                  }
                }}
                className="flex cursor-pointer items-baseline gap-3 px-4 py-2 hover:bg-[#3fb950]/10"
              >
                <span className="min-w-0 flex-1">
                  <span className="text-[#e6edf3]">{pr.title}</span>
                  <span className="ml-2 text-xs text-[#8b949e]">
                    {pr.repo}#{pr.number} · {pr.author}
                  </span>
                </span>
                <PushedByYouChip pusher={pushers.of(pr)} />
                <UnresolvedChip count={pr.unresolved_threads} floor={pr.unresolved_threads_floor} />
                <ReadyAgeChip readyAt={pr.ready_at} now={now} />
              </div>
            ) : (
              <div className="flex items-baseline gap-3 px-4 py-2">
                <span className="min-w-0 flex-1">
                  <ExternalLink href={pr.url} className="text-[#e6edf3] hover:text-[#4493f8]">
                    {pr.title}
                  </ExternalLink>
                  <span className="ml-2 text-xs text-[#8b949e]">
                    {pr.repo}#{pr.number} · {pr.author}
                  </span>
                </span>
                <PushedByYouChip pusher={pushers.of(pr)} />
                <UnresolvedChip count={pr.unresolved_threads} floor={pr.unresolved_threads_floor} />
                <ReadyAgeChip readyAt={pr.ready_at} now={now} />
              </div>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}

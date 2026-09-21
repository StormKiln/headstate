import { CircleCheck } from "lucide-react";
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

/// Both labels name the FIELD, not just the direction (#1277).
///
/// "Oldest first" is ambiguous between "opened longest ago" and "waiting
/// for review longest", and on anything that spent time as a draft those
/// are different pull requests. The app has only `created_at` to sort on,
/// so the label says `opened` and the reader can tell which question they
/// are being answered. Renaming these back to bare directions would put
/// the ambiguity straight back.
const READY_SORT_OPTIONS: {
  value: NonNullable<Filters["readySort"]>;
  label: string;
}[] = [
  { value: "oldest-opened", label: "Oldest opened first" },
  { value: "newest-opened", label: "Newest opened first" },
];

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
/// Ordered OLDEST OPENED FIRST by default (#1277). Working top to bottom
/// through a review queue should mean working through it in the order the
/// pull requests arrived, and newest-first buries the three-day-old one
/// whose author is blocked. The sort control offers the other order, and
/// `sortReadyForReview` documents why `created_at` and not a
/// ready-for-review timestamp.
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
  const { readySort } = useActiveFilters();
  const setFilter = useFilters((s) => s.setFilter);

  const ready = sortReadyForReview(prs.filter(readyForReview), readySort);

  if (ready.length === 0) {
    return <p className="px-4 py-2 text-xs text-[#8b949e]">Nothing ready to review.</p>;
  }

  return (
    <section className="mb-4 rounded-md border border-[#3fb950]/40 bg-[#3fb950]/5">
      <h2 className="flex items-center gap-2 border-b border-[#3fb950]/30 px-4 py-2 text-sm font-semibold text-[#3fb950]">
        <CircleCheck className="h-4 w-4" aria-hidden="true" />
        Ready for review ({ready.length})
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button variant="ghost" size="sm" className="ml-auto font-normal">
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
      <ul>
        {ready.map((pr) => (
          <li key={`${pr.repo}#${pr.number}`} className="text-sm">
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
                className="cursor-pointer px-4 py-2 hover:bg-[#3fb950]/10"
              >
                <span className="text-[#e6edf3]">{pr.title}</span>
                <span className="ml-2 text-xs text-[#8b949e]">
                  {pr.repo}#{pr.number} · {pr.author}
                </span>
              </div>
            ) : (
              <div className="px-4 py-2">
                <ExternalLink href={pr.url} className="text-[#e6edf3] hover:text-[#4493f8]">
                  {pr.title}
                </ExternalLink>
                <span className="ml-2 text-xs text-[#8b949e]">
                  {pr.repo}#{pr.number} · {pr.author}
                </span>
              </div>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}

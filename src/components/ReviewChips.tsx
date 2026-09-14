import type { PullRequest } from "@/types/pr";
import type { Filters } from "@/lib/derive";
import { needsMyReview } from "@/lib/derive";
import { useActiveFilters, useFilters } from "@/store/filters";
import { chipPressed } from "@/lib/chipPressed";

/// Triage chips for the review queue.
///
/// A separate set from `TriageChips` on purpose. Those count states that
/// are the AUTHOR's problem -- needs rebase, red CI, ready to queue -- and
/// none of them is actionable when the PR is someone else's. What is
/// actionable here is "have I reviewed this yet".
///
/// The pressed-state predicate is `TriageChips`'s, via
/// `@/lib/chipPressed`. This file used the bare subset test that file's
/// comment describes as the defect, so typing a search left a chip
/// reading pressed over a list it no longer described, and clicking it to
/// un-press discarded the search (#971). The chip SET stays separate --
/// only the predicate is shared.
const CHIPS: {
  key: string;
  label: string;
  preset: Filters;
  count: (prs: PullRequest[]) => number;
  tone: string;
}[] = [
  {
    key: "mine",
    label: "Awaiting my review",
    preset: { needsMyReviewOnly: true },
    count: (prs) => prs.filter(needsMyReview).length,
    tone: "text-[#58a6ff] border-[#58a6ff]/40",
  },
  {
    key: "drafts",
    label: "Draft",
    preset: { draftsOnly: true },
    count: (prs) => prs.filter((p) => p.is_draft).length,
    tone: "text-[#8b949e] border-[#8b949e]/40",
  },
];

export function ReviewChips({ prs }: { prs: PullRequest[] }) {
  const filters = useActiveFilters();
  const { applyPreset } = useFilters();
  const active = CHIPS.filter((c) => c.count(prs) > 0);
  if (active.length === 0) return null;

  return (
    <div className="mb-3 flex flex-wrap gap-2">
      {active.map((chip) => {
        const isOn = chipPressed(chip.preset, filters);
        return (
          <button
            key={chip.key}
            type="button"
            aria-pressed={isOn}
            onClick={() =>
              applyPreset(
                isOn
                  ? filters.repo
                    ? { repo: filters.repo }
                    : {}
                  : { ...chip.preset, ...(filters.repo ? { repo: filters.repo } : {}) },
              )
            }
            className={`rounded-full border px-3 py-1 text-xs ${chip.tone} ${
              isOn ? "bg-[#161b22]" : "hover:bg-[#161b22]"
            }`}
          >
            <span className="font-semibold tabular-nums">{chip.count(prs)}</span> {chip.label}
          </button>
        );
      })}
    </div>
  );
}

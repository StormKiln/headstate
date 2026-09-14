import type { Filters } from "@/lib/derive";

/// Whether a triage chip should read as pressed.
///
/// EQUALITY, not a subset test. `applyPreset` replaces the whole filter
/// set while `setFilter` merges into it, so after clicking a chip and then
/// typing a search the chip's own keys are still set -- and a subset test
/// kept reading as pressed while the list beneath it had been narrowed by
/// something else. The chip then announced itself as the active filter
/// when it was not, and clicking it to un-press silently discarded the
/// search.
///
/// Shared by `TriageChips` and `ReviewChips` because the guard was written
/// for the first and never reached the second: `onlyThisChip` and
/// `OTHER_FILTERS` were local and unexported, so the sibling could not
/// have inherited the fix even by importing, and a user who learned on My
/// PRs that a chip un-presses when it stops describing the list found the
/// rule false on To review (#971). The chip SETS stay separate -- they
/// count different people's problems -- and only this predicate is shared.
///
/// `repo` is excluded because it is navigation rather than a filter -- the
/// same rule `hasActiveFilters` and the store's `reset` apply.
/// Every filter key that narrows the list, excluding `repo` -- which is
/// navigation, not a filter, per the store's `reset` and
/// `hasActiveFilters`. Listed explicitly rather than derived from the
/// type so that adding a filter is a deliberate decision about whether a
/// chip may coexist with it.
/// Not exported: `chipPressed` is the whole interface, and an exported
/// list invites a second reader to re-derive the predicate from it --
/// which is how the two chip components came to disagree in the first
/// place. `knip` flags it as an unused export, correctly.
const OTHER_FILTERS = [
  "query",
  "unresolvedOnly",
  "needsMyReviewOnly",
  "readyOnly",
  "draftsOnly",
  "ci",
  "review",
  "includeLabels",
  "excludeLabels",
  "needsAttentionOnly",
  "staleOnly",
  "inMergeQueueOnly",
  "awaitingReviewOnly",
  "readyToQueueOnly",
] as const satisfies readonly (keyof Filters)[];

function isSet(v: Filters[keyof Filters]): boolean {
  return Array.isArray(v) ? v.length > 0 : Boolean(v);
}

/// True when `preset`'s keys are all set AND nothing outside the preset is
/// narrowing the list as well.
export function chipPressed(preset: Filters, filters: Filters): boolean {
  const onlyThisChip = !OTHER_FILTERS.some((k) =>
    k in preset ? false : isSet(filters[k]),
  );
  return (
    Object.keys(preset).every((k) => filters[k as keyof Filters] === true)
    && onlyThisChip
  );
}

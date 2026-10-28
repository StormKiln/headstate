import { useFilters } from "@/store/filters";
import { useSourceSelection } from "@/store/sourceSelection";
import type { PrIdentity } from "@/types/identity";

/// Open a pull request in Headstate's own detail view. The source
/// selection is widened only when it would hide this pull request.
export function openInHeadstate(id: PrIdentity) {
  const provider = id.source?.provider ?? "github";
  const sel = useSourceSelection.getState();
  if (sel.selection !== "both" && sel.selection !== provider) sel.setSelection(provider);
  useFilters.getState().setView("my-prs");
  useFilters.getState().selectPr(id);
}

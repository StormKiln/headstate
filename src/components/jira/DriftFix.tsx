import { useState } from "react";
import type { Fix } from "@/lib/jira/drift";
import type { JiraIssue } from "@/types/jira";
import { JiraTransition } from "./JiraTransition";

/// The action under a drift raised as fact (#2042): "Move ABC-1 to In
/// Review" when the project's expectation names a status, else the
/// chooser -- a category is many statuses, and Headstate does not pick
/// one for the reader.
///
/// Mount it whenever the issue is shown, passing `fix` null when there is
/// no drift: once the reader has started a move it stays until they
/// dismiss it, so the move clearing the drift does not also take away the
/// sentence saying what Jira did. Render only while Jira is on.
export function DriftFix({
  issue,
  site,
  fix,
  pickerAbove = false,
}: {
  issue: JiraIssue;
  site: string | null;
  fix: Fix | null;
  /// The page already offers an unpreset "Move to…": a second one is not
  /// rendered, and the reader is pointed at it.
  pickerAbove?: boolean;
}) {
  const [held, setHeld] = useState<Fix | null>(null);
  const shown = held ?? fix;
  if (!shown) return null;
  if (shown.kind === "choose") {
    const why = `${issue.project_key} expects ${shown.expected} here.`;
    if (pickerAbove) return <p className="text-xs text-[#8b949e]">{why} Choose one with Move to… above.</p>;
    return (
      <div className="flex flex-col gap-1 text-xs" onClickCapture={() => setHeld((h) => h ?? shown)}>
        <p className="text-[#8b949e]">{why} Choose the status to move {issue.key} to.</p>
        <JiraTransition issue={issue} site={site} onClose={() => setHeld(null)} />
      </div>
    );
  }
  return (
    <div className="flex flex-col" onClickCapture={() => setHeld((h) => h ?? shown)}>
      <JiraTransition issue={issue} site={site} target={shown.status} onClose={() => setHeld(null)} />
    </div>
  );
}

import { useState } from "react";
import { ChevronRight } from "lucide-react";
import { supersededLabel } from "@/lib/supersededComments";
import type { PrComment } from "@/types/pr";
import { CommentRow } from "./CommentRow";

/// The older copies of one repeated comment, collapsed under its newest
/// (#1581). The rule for what counts as a copy is in
/// `lib/supersededComments.ts`.
///
/// Collapsed by default: these are the comments the newest one replaced,
/// and the point of folding them is that they stop costing a scroll. One
/// click opens them, each still its own `CommentRow`, in the order they
/// were posted.
///
/// `truncated` when the fetch did not return every comment on the pull
/// request. The copies here are then only those that arrived, so the
/// label says "at least".
export function SupersededGroup({
  comments,
  truncated,
}: {
  comments: readonly PrComment[];
  truncated: boolean;
}) {
  const [open, setOpen] = useState(false);
  const label = supersededLabel(comments.length, truncated);

  return (
    <div className="ml-4 flex flex-col gap-2 border-l border-[#30363d] pl-3">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        // `tap-target`: the 44px floor on a phone, and nothing on the
        // desktop (the class is scoped to the phone breakpoint).
        className="tap-target flex w-fit items-center gap-1.5 rounded px-1 py-0.5 text-xs text-[#8b949e] hover:bg-[#161b22] hover:text-[#e6edf3]"
      >
        <ChevronRight
          className={`h-3.5 w-3.5 shrink-0 transition-transform ${open ? "rotate-90" : ""}`}
          aria-hidden="true"
        />
        {label}
      </button>
      {open
        ? comments.map((c, i) => (
            <CommentRow
              key={`${c.author}-${c.created_at}-${i}`}
              author={c.author}
              createdAt={c.created_at}
              body={c.body}
            />
          ))
        : null}
    </div>
  );
}

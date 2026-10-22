import { createContext, useContext } from "react";
import type { ReactNode } from "react";

// The list elements `PROSE` (in `markdownProse.tsx`) renders. Their own
// file so that one exports no components beside its non-component exports
// (#1965), which is what lets dev hot-reload swap both in place.

/// A list nested in another list must not add the outer list's
/// vertical margin -- see `List`.
const InList = createContext(false);

/// A NESTED list drops its vertical margin: `my-2` inside the parent's
/// `li` stacks on that item's own spacing and opens a gap in the middle
/// of a list that should read as one block.
///
/// The bullet is decided per ITEM, by `Item`, not here -- see there.
///
/// `className` is dropped (this computes its own) but everything else
/// is passed through: an `<ol>` starting at `3.` carries `start`, and
/// swallowing it would silently renumber the list from 1.
export function List({ ordered, start, children }: ListProps) {
  const nested = useContext(InList);
  const Tag = ordered ? "ol" : "ul";
  return (
    <InList value={true}>
      <Tag
        start={start}
        className={`${nested ? "mt-1" : "my-2"} ${ordered ? "list-decimal" : "list-disc"} space-y-1 pl-5`}
      >
        {children}
      </Tag>
    </InList>
  );
}

/// A task item is a checkbox, not a bullet.
///
/// GFM renders `- [ ]` as an `<input type="checkbox">` and marks the
/// item `task-list-item`; the list's `list-disc` then gave it a bullet
/// AND a box. Suppressing the marker on the LIST is wrong -- GFM puts
/// `contains-task-list` on a list with even one task item, and a mixed
/// list would lose the bullets from its ordinary items too. Only the
/// task items themselves drop their marker.
export function Item({ className, ...props }: { className?: string; children?: ReactNode }) {
  const task = (className ?? "").includes("task-list-item");
  return <li {...props} className={task ? "list-none -ml-5 pl-5" : undefined} />;
}

type ListProps = {
  ordered: boolean;
  className?: string;
  start?: number;
  children?: ReactNode;
};

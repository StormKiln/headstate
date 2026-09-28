import { createContext, useContext } from "react";
import type { ReactNode } from "react";
import type { Components } from "react-markdown";

/// What `Markdown` (GitHub bodies) and `TranscriptMarkdown` (Claude Code
/// transcripts, #1482) share: how prose LOOKS. The two differ in what
/// they let through -- raw HTML, images, links, code -- and that stays in
/// each renderer; the spacing, lists, headings and tables live here once.

/// A list nested in another list must not add the outer list's
/// vertical margin -- see `List`.
const InList = createContext(false);

/// Strips react-markdown's `node` before it reaches the DOM.
///
/// Every component override receives the hast AST node as a `node`
/// prop. Spreading props onto an intrinsic element hands React an
/// attribute it does not know, and React 19 stringifies it: every
/// element `Markdown.tsx` overrode carried a literal
/// `node="[object Object]"` in the rendered markup.
export function clean<P extends { node?: unknown }>(props: P): Omit<P, "node"> {
  const rest = { ...props };
  delete rest.node;
  return rest;
}

/// The prose elements both renderers share: spacing, lists, headings,
/// tables.
export const PROSE: Components = {
  // VERTICAL RHYTHM. `prose-headstate` on the wrapper was
  // never defined anywhere -- it appears once, on that div, with
  // no matching rule -- so every block element fell back to the
  // CSS reset, which strips margins. The blank lines in the
  // source WERE parsed; the resulting <p>s just had nothing
  // between them.
  p: (props) => <p {...clean(props)} className="my-2" />,
  ul: (props) => <List {...clean(props)} ordered={false} />,
  ol: (props) => <List {...clean(props)} ordered={true} />,
  li: (props) => <Item {...clean(props)} />,
  blockquote: (props) => (
    <blockquote
      {...clean(props)}
      className="my-2 border-l-2 border-[#30363d] pl-3 text-[#8b949e]"
    />
  ),
  hr: (props) => <hr {...clean(props)} className="my-4 border-[#30363d]" />,
  // A heading needs more space ABOVE than below: it belongs to
  // the text that follows it, and equal margins make it float
  // between two sections instead of introducing one.
  h1: (props) => <h1 {...clean(props)} className="mb-2 mt-5 text-base font-semibold" />,
  h2: (props) => <h2 {...clean(props)} className="mb-2 mt-5 text-sm font-semibold" />,
  h3: (props) => <h3 {...clean(props)} className="mb-1 mt-4 text-sm font-semibold" />,
  // A comment body starts at whatever level its author felt
  // like. Without these, h4-h6 fall through to the CSS reset
  // and render at body size and body weight -- a heading
  // indistinguishable from the paragraph under it.
  h4: (props) => <h4 {...clean(props)} className="mb-1 mt-4 text-sm font-semibold" />,
  h5: (props) => <h5 {...clean(props)} className="mb-1 mt-4 text-sm font-semibold" />,
  h6: (props) => (
    <h6 {...clean(props)} className="mb-1 mt-4 text-sm font-semibold text-[#8b949e]" />
  ),
  table: (props) => (
    // `border-collapse`, or every cell's border doubles against
    // its neighbour's and the table reads as a heavy grid.
    <div className="my-3 overflow-x-auto">
      <table {...clean(props)} className="w-full border-collapse text-xs" />
    </div>
  ),
  td: (props) => <td {...clean(props)} className="border border-[#30363d] px-2 py-1" />,
  th: (props) => (
    <th {...clean(props)} className="border border-[#30363d] px-2 py-1 font-semibold" />
  ),
};

/// A NESTED list drops its vertical margin: `my-2` inside the parent's
/// `li` stacks on that item's own spacing and opens a gap in the middle
/// of a list that should read as one block.
///
/// The bullet is decided per ITEM, by `Item`, not here -- see there.
///
/// `className` is dropped (this computes its own) but everything else
/// is passed through: an `<ol>` starting at `3.` carries `start`, and
/// swallowing it would silently renumber the list from 1.
function List({ ordered, start, children }: ListProps) {
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
function Item({ className, ...props }: { className?: string; children?: ReactNode }) {
  const task = (className ?? "").includes("task-list-item");
  return <li {...props} className={task ? "list-none -ml-5 pl-5" : undefined} />;
}

type ListProps = {
  ordered: boolean;
  className?: string;
  start?: number;
  children?: ReactNode;
};

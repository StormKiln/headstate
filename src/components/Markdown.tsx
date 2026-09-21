import { createContext, useContext } from "react";
import type { ReactNode } from "react";
import { ExternalLink } from "./ExternalLink";
import rehypeSanitize from "rehype-sanitize";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

/// Inside a fenced block, `code` is the block's text, not a chip.
///
/// HOW THE TWO ARE TOLD APART. react-markdown 10 gives the `code`
/// component no `inline` prop -- it was removed in v9 -- and the
/// `language-*` class it does pass is only present when the fence
/// carries a language tag, so ``` with no tag looks exactly like
/// inline code from the props alone. The one signal that holds for
/// every fenced block is the `<pre>` wrapping it, and `pre` renders
/// as this component's own parent, so it can simply say so.
const InPre = createContext(false);

/// A list nested in another list must not add the outer list's
/// vertical margin -- see `List`.
const InList = createContext(false);

/// Strips react-markdown's `node` before it reaches the DOM.
///
/// Every component override receives the hast AST node as a `node`
/// prop. Spreading props onto an intrinsic element hands React an
/// attribute it does not know, and React 19 stringifies it: every
/// element this file overrides carried a literal
/// `node="[object Object]"` in the rendered markup.
function clean<P extends { node?: unknown }>(props: P): Omit<P, "node"> {
  const rest = { ...props };
  delete rest.node;
  return rest;
}

/// Renders untrusted Markdown from GitHub.
///
/// Bodies and comments are written by other people, and this app holds a
/// token in memory, so the rendering is deliberately constrained:
///
/// - `rehype-sanitize` strips scripts, event handlers and iframes. A
///   maintained sanitiser, not a hand-rolled regex.
/// - Links open in the SYSTEM BROWSER via the opener plugin, never in the
///   app webview, so a link can never navigate the app itself.
/// - The token lives in Rust memory and is never exposed to the webview,
///   so rendered content has nothing to read even if it could run.
///
/// Remote images ARE loaded, which is a deliberate call: screenshots in
/// PR descriptions are most of the value, at the cost of a hostile
/// comment learning the reader's IP.
export function Markdown({ children }: { children: string }) {
  return (
    <div className="text-sm leading-relaxed text-[#e6edf3]">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[rehypeSanitize]}
        components={{
          // `href` is optional in react-markdown's props but required
          // by ExternalLink, and an anchor with no target is not a link
          // -- render it as plain text rather than inventing a URL.
          a: ({ href, children }) =>
            href ? (
              <ExternalLink href={href} className="text-[#4493f8] hover:underline">
                {children}
              </ExternalLink>
            ) : (
              <span>{children}</span>
            ),
          code: (props) => <Code {...clean(props)} />,
          pre: (props) => (
            <InPre value={true}>
              <pre
                {...clean(props)}
                className="my-3 overflow-x-auto rounded border border-[#30363d] bg-[#161b22] p-3 text-xs"
              />
            </InPre>
          ),
          img: (props) => (
            <img {...clean(props)} alt={props.alt ?? ""} className="max-w-full rounded" />
          ),
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
        }}
      >
        {children}
      </ReactMarkdown>
    </div>
  );
}

/// The block's chrome belongs to the `<pre>`, not to this.
///
/// Applying the inline chip's `px-1 py-0.5` inside a `<pre>` is the
/// #1279 symptom: `<code>` is inline, so the horizontal padding lands
/// at the start of the first line and after the last rather than
/// around the box, and every fenced block read as one space indented.
/// The background and rounding doubled against the `<pre>`'s own there
/// too.
function Code({ className, ...props }: { className?: string; children?: ReactNode }) {
  const inPre = useContext(InPre);
  return inPre ? (
    // `className` is kept: remark puts `language-js` there, and
    // dropping it would take the language with it.
    <code {...props} className={className} />
  ) : (
    <code {...props} className="rounded bg-[#161b22] px-1 py-0.5 font-mono text-xs" />
  );
}

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

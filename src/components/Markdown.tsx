import { createContext, useContext } from "react";
import type { ReactNode } from "react";
import { ExternalLink } from "./ExternalLink";
import { PROSE, clean } from "./markdownProse";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import type { Options as SanitizeSchema } from "rehype-sanitize";
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

/// The sanitiser's schema: GitHub's own default, tightened in two ways.
///
/// - `open` is removed from every element. A `<details>` block must start
///   COLLAPSED at every depth (#1456): CI bots nest large reports three
///   levels deep, and an author's `<details open>` would unfold all of it
///   the moment the comment is expanded. The reader opens what they want.
/// - `<style>` is STRIPPED, contents and all. The default schema only
///   unwraps an element it does not allow, which would print a style
///   sheet's source as body text.
///
/// Everything else -- the tag allowlist, the `javascript:`-refusing
/// protocol list, the absence of every `on*` and `style` attribute, the
/// dropping of HTML comments -- is the default's, unchanged.
const SCHEMA: SanitizeSchema = {
  ...defaultSchema,
  strip: [...(defaultSchema.strip ?? []), "style"],
  attributes: {
    ...defaultSchema.attributes,
    "*": (defaultSchema.attributes?.["*"] ?? []).filter((a) => a !== "open"),
  },
};

/// Renders untrusted Markdown from GitHub.
///
/// Bodies and comments are written by other people, and this app holds a
/// token in memory, so the rendering is deliberately constrained:
///
/// - `rehype-sanitize` strips scripts, event handlers and iframes. A
///   maintained sanitiser, not a hand-rolled regex.
/// - Raw HTML IS parsed (`rehype-raw`), so `<details>` and `<summary>`
///   render as real collapsible elements (#1456). It runs BEFORE the
///   sanitiser, so everything it produces passes through the same
///   allowlist as markdown-generated HTML. The order is the security
///   property: raw HTML parsed after sanitising would be unsanitised.
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
        rehypePlugins={[rehypeRaw, [rehypeSanitize, SCHEMA]]}
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
          ...PROSE,
          // Collapsed by default: `open` is never passed, and the schema
          // has already removed any the author wrote. The browser owns
          // the toggle from there, so each level opens on its own.
          details: ({ children }) => (
            <details className="my-2 rounded border border-[#30363d] px-3 py-1">
              {children}
            </details>
          ),
          summary: ({ children }) => (
            <summary className="cursor-pointer select-none py-1 font-semibold">{children}</summary>
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

import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { Check, Copy, ImageOff } from "lucide-react";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import rehypeSanitize from "rehype-sanitize";
import remarkGfm from "remark-gfm";
import { toast } from "sonner";
import { copyText } from "../lib/clipboard";
import { highlight } from "../lib/highlight";
import type { Outcome } from "../lib/highlight";
import { countLines, grammarFor, oversize } from "../lib/highlightLangs";
import type { Tree } from "../lib/highlightCore";
import { useIsMobile } from "../lib/useIsMobile";
import { useSeen } from "../lib/useSeen";
import { ExternalLink } from "./ExternalLink";
import { PROSE, clean } from "./markdownProse";

/// Renders Claude Code transcript text (#1482).
///
/// NOT `Markdown`. That renderer is for GitHub bodies, which are authored
/// markup, and it deliberately parses raw HTML and loads remote images.
/// Transcript text is neither authored nor trusted: it is tool output,
/// fetched web pages and repository files, all of which an outsider can
/// influence. So this one differs in exactly what it lets through:
///
/// - **Raw HTML renders as TEXT.** There is no `rehype-raw`; an HTML
///   node becomes its own source, visible, rather than an element.
/// - **No image is ever loaded.** An image renders as a placeholder that
///   shows its address. A remote image would tell its host the reader's
///   IP and that the transcript was opened -- from the phone too -- so
///   no `<img>` element is created at all, for any source.
/// - **Links show where they go.** Through `ExternalLink`, to the system
///   browser, with the host printed beside the text so a link whose
///   words say one thing cannot quietly point at another. Anything that
///   is not http(s) is not a link.
/// - **Code blocks** get a language label, a copy button, lazy bounded
///   highlighting (`../lib/highlight.ts` records how the highlighter was
///   chosen) and, on a phone, wrapped lines.
///
/// The sanitiser still runs, on markdown-generated HTML alone. Nothing
/// here needs it today; it is the backstop if a parser ever emits
/// something it should not.
export function TranscriptMarkdown({ children }: { children: string }) {
  return (
    <div className="text-sm leading-relaxed text-[#e6edf3]">
      <ReactMarkdown remarkPlugins={REMARK} rehypePlugins={REHYPE} components={COMPONENTS}>
        {children}
      </ReactMarkdown>
    </div>
  );
}

/// Module constants, not literals in the render.
///
/// A component map written inline is a NEW set of component types on
/// every render, and React unmounts a subtree whose type changed. A live
/// transcript re-renders as it grows, so every code block would be torn
/// down and rebuilt on each new line: its highlighting discarded and
/// redone, its copy state lost. Hoisted, a block keeps its identity for
/// as long as its position in the text does.
const REMARK = [remarkGfm];
const REHYPE = [rawAsText, rehypeSanitize];
const COMPONENTS: Components = {
  ...PROSE,
  a: ({ href, children }) => <TranscriptLink href={href}>{children}</TranscriptLink>,
  img: ({ src, alt }) => <ImagePlaceholder src={typeof src === "string" ? src : ""} alt={alt} />,
  // Every fenced block is handled whole by `pre`, so a `code` that
  // reaches here is inline.
  code: (props) => (
    <code {...clean(props)} className="rounded bg-[#161b22] px-1 py-0.5 font-mono text-xs" />
  ),
  pre: ({ node }) => {
    const block = fencedBlock(node);
    return <CodeBlock code={block.code} lang={block.lang} />;
  },
};

/// A hast node, as far as this file reads one.
type HNode = {
  type: string;
  value?: string;
  tagName?: string;
  properties?: { className?: unknown };
  children?: HNode[];
};

/// Turns every raw HTML node into a text node carrying its source.
///
/// react-markdown does the same by itself, but only AFTER the rehype
/// plugins run, and the sanitiser drops a node type it does not know --
/// so without this, `<b>hi</b>` would vanish from the transcript rather
/// than read as the four characters the tool printed.
function rawAsText() {
  const walk = (node: HNode) => {
    for (const child of node.children ?? []) {
      if (child.type === "raw") child.type = "text";
      else walk(child);
    }
  };
  return walk;
}

/// The text and language of a fenced block, from its `<pre>` node.
///
/// Read from the tree rather than from rendered children: the block is
/// re-rendered by `CodeBlock`, so the text must be the source string.
/// The trailing newline is the one mdast-util-to-hast appends to every
/// block, not the author's.
function fencedBlock(node: HNode | undefined): { code: string; lang: string | undefined } {
  const code = node?.children?.find((c) => c.type === "element" && c.tagName === "code");
  const classes = code?.properties?.className;
  const lang = Array.isArray(classes)
    ? classes
        .map(String)
        .find((c) => c.startsWith("language-"))
        ?.slice("language-".length)
    : undefined;
  const text = textOf(code).replace(/\n$/, "");
  return { code: text, lang };
}

function textOf(node: HNode | undefined): string {
  if (!node) return "";
  if (node.type === "text") return node.value ?? "";
  return (node.children ?? []).map(textOf).join("");
}

/// The host a link points at, or null when it is not an http(s) link.
function httpHost(href: string): string | null {
  try {
    const url = new URL(href);
    return url.protocol === "http:" || url.protocol === "https:" ? url.host : null;
  } catch {
    return null;
  }
}

function TranscriptLink({ href, children }: { href?: string; children?: ReactNode }) {
  const host = href ? httpHost(href) : null;
  // A relative path, `file:`, `mailto:` and whatever the URL filter
  // emptied all land here. None of them has a browser to open in, so
  // none is offered as a link.
  if (!href || host === null) return <span>{children}</span>;
  return (
    <ExternalLink href={href} title={href} className="text-[#4493f8] hover:underline">
      {children}
      <span className="ml-1 text-xs text-[#8b949e]">({host})</span>
    </ExternalLink>
  );
}

/// Where an image would have been. Never an `<img>`: see the module docs.
function ImagePlaceholder({ src, alt }: { src: string; alt?: string }) {
  return (
    <span
      role="img"
      aria-label={alt ? `Image not loaded: ${alt}` : "Image not loaded"}
      className="my-1 inline-flex max-w-full items-center gap-2 rounded border border-dashed border-[#30363d] px-2 py-1 text-xs text-[#8b949e]"
    >
      <ImageOff className="size-3.5 shrink-0" aria-hidden="true" />
      {alt ? <span className="text-[#e6edf3]">{alt}</span> : null}
      <span className="break-all font-mono">{src || "(no address)"}</span>
    </span>
  );
}

/// One fenced block: label, copy, and highlighting once it is seen.
function CodeBlock({ code, lang }: { code: string; lang: string | undefined }) {
  const ref = useRef<HTMLDivElement>(null);
  const seen = useSeen(ref);
  const mobile = useIsMobile();
  const grammar = grammarFor(lang);
  const tooBig = oversize(code);
  // Kept WITH the code it was computed for. A live transcript grows its
  // last block while the reader watches; a tree from the previous text
  // would show stale code until the new one arrived, so a result for
  // any other text is not a result for this one.
  const [result, setResult] = useState<{ code: string; outcome: Outcome } | null>(null);
  const outcome = result?.code === code ? result.outcome : null;
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!seen || grammar === null || tooBig !== null) return;
    const abort = new AbortController();
    void highlight(code, grammar, abort.signal).then((o) => {
      if (!abort.signal.aborted) setResult({ code, outcome: o });
    });
    return () => abort.abort();
  }, [seen, code, grammar, tooBig]);

  const copy = () => {
    void copyText(code).then((failure) => {
      if (failure === null) {
        setCopied(true);
        setTimeout(() => setCopied(false), 1500);
      } else {
        toast.error("Could not copy the code", { description: failure });
      }
    });
  };

  const tree = outcome?.kind === "tree" ? outcome.tree : null;
  // Said when the reader would otherwise wonder why this block, of all
  // of them, has no colour. A grammar that failed to load is not said:
  // nothing about the block explains it and nothing the reader does
  // changes it, and the code itself is all there.
  const plainNote =
    tooBig === "lines"
      ? `${countLines(code).toLocaleString()} lines, shown without highlighting`
      : tooBig === "chars" || outcome?.kind === "too-slow"
        ? "shown without highlighting"
        : null;

  return (
    <div
      ref={ref}
      data-highlighted={tree ? "true" : undefined}
      className="my-3 overflow-hidden rounded border border-[#30363d] bg-[#161b22]"
    >
      <div className="flex items-center gap-2 border-b border-[#30363d] px-3 py-1 text-xs text-[#8b949e]">
        <span className="font-mono">{lang ?? "text"}</span>
        {plainNote !== null ? <span>· {plainNote}</span> : null}
        <button
          type="button"
          onClick={copy}
          aria-label={copied ? "Copied" : "Copy code"}
          className="ml-auto inline-flex items-center gap-1 rounded px-1.5 py-0.5 hover:bg-[#30363d] hover:text-[#e6edf3]"
        >
          {copied ? <Check className="size-3.5" aria-hidden="true" /> : <Copy className="size-3.5" aria-hidden="true" />}
          <span>{copied ? "Copied" : "Copy"}</span>
        </button>
      </div>
      <pre
        className={`p-3 font-mono text-xs ${
          // Wrapped on the phone: a horizontal scroller inside a
          // vertically scrolling transcript fights the thumb for every
          // gesture. The desktop keeps columns aligned and scrolls.
          mobile ? "whitespace-pre-wrap break-words" : "overflow-x-auto"
        }`}
      >
        <code>{tree ? renderTree(tree) : code}</code>
      </pre>
    </div>
  );
}

/// The highlighter's tree as React elements.
///
/// Walked, never serialised: only text and `<span class>` survive, so
/// nothing the highlighter emits is ever parsed as HTML.
function renderTree(tree: Tree): ReactNode {
  return (tree.children as HNode[]).map(renderNode);
}

function renderNode(node: HNode, i: number): ReactNode {
  if (node.type === "text") return node.value ?? "";
  if (node.type !== "element") return null;
  const classes = Array.isArray(node.properties?.className)
    ? (node.properties.className as unknown[]).map(String)
    : [];
  return (
    <span key={i} className={tokenClass(classes)}>
      {(node.children ?? []).map(renderNode)}
    </span>
  );
}

/// Prism token types to this app's palette (GitHub dark, as `Markdown`).
///
/// Spelled out in full so Tailwind's scanner finds every class.
const TOKEN_COLOURS: [string[], string][] = [
  [["comment", "prolog", "doctype", "cdata"], "token text-[#8b949e] italic"],
  [["inserted"], "token text-[#3fb950]"],
  [["deleted"], "token text-[#f85149]"],
  [["keyword", "atrule", "important", "rule"], "token text-[#ff7b72]"],
  [["string", "char", "template-string", "attr-value", "url"], "token text-[#a5d6ff]"],
  [["regex"], "token text-[#7ee787]"],
  [["tag", "selector"], "token text-[#7ee787]"],
  [["function", "function-variable"], "token text-[#d2a8ff]"],
  [["class-name", "builtin", "namespace", "variable"], "token text-[#ffa657]"],
  [["number", "boolean", "constant", "symbol", "property", "attr-name", "entity"], "token text-[#79c0ff]"],
];

function tokenClass(classes: string[]): string {
  for (const [types, cls] of TOKEN_COLOURS) {
    if (types.some((t) => classes.includes(t))) return cls;
  }
  return "token";
}

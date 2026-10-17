import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { PrFindExpanded } from "./prFindContext";

type Hit = { range: Range; element: HTMLElement; offset: number };

// Read only rendered, sanitized text. Inline formatting stays contiguous;
// separate Markdown blocks receive a separator, never their HTML attributes.
function findHits(root: HTMLElement, query: string): Hit[] {
  const hits: Hit[] = [];
  if (!query) return hits;
  for (const element of root.querySelectorAll<HTMLElement>("[data-pr-find-text]")) {
    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
    const nodes: { node: Text; start: number }[] = [];
    let text = "";
    let node: Node | null;
    let block: Element | null = null;
    while ((node = walker.nextNode())) {
      const parent = node.parentElement;
      const control = parent?.closest("button, input, textarea, [aria-hidden=true]");
      if (control && element.contains(control)) continue;
      const nextBlock = parent?.closest("p, li, pre, h1, h2, h3, h4, summary, tr") ?? null;
      if (text && nextBlock !== block) text += "\n";
      block = nextBlock;
      nodes.push({ node: node as Text, start: text.length });
      text += node.textContent ?? "";
    }
    // Case-insensitive matching without changing string length (Unicode lower
    // casing can expand characters and invalidate DOM range offsets).
    const escaped = query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    const pattern = new RegExp(escaped, "giu");
    for (const match of text.matchAll(pattern)) {
      const start = match.index;
      const end = start + match[0].length;
      const first = nodes.find(n => n.start <= start && n.start + n.node.length > start);
      const last = nodes.find(n => n.start < end && n.start + n.node.length >= end);
      if (!first || !last) continue;
      const range = document.createRange();
      range.setStart(first.node, start - first.start);
      range.setEnd(last.node, end - last.start);
      hits.push({ range, element, offset: start });
    }
  }
  return hits;
}

export function PrFind({ children, partial = false }: { children: ReactNode; partial?: boolean }) {
  const frame = useRef<HTMLDivElement>(null);
  const toolbar = useRef<HTMLDivElement>(null);
  const root = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const returnFocus = useRef<HTMLElement | null>(null);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<Hit[]>([]);
  const [index, setIndex] = useState(0);
  const active = useRef<Hit | undefined>(undefined);
  const show = () => {
    if (!open) returnFocus.current = document.activeElement as HTMLElement;
    setOpen(true);
    input.current?.focus();
    input.current?.select();
  };
  const close = () => {
    setOpen(false);
    setQuery("");
    returnFocus.current?.focus();
  };
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (open && event.key === "Escape" && !document.querySelector('[role="dialog"][data-open]')) {
        event.preventDefault(); event.stopImmediatePropagation(); close();
        return;
      }
      if ((event.ctrlKey || event.metaKey) && !event.altKey && event.key.toLowerCase() === "f") {
        event.preventDefault(); event.stopImmediatePropagation(); show();
      }
    };
    window.addEventListener("keydown", key, true);
    return () => window.removeEventListener("keydown", key, true);
  });
  useEffect(() => { if (open) input.current?.focus(); }, [open]);
  useLayoutEffect(() => {
    const bar = toolbar.current;
    const container = frame.current;
    if (!bar || !container) return;
    const measure = () => container.style.setProperty("--pr-find-h", `${open ? bar.getBoundingClientRect().height + 12 : 0}px`);
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(bar);
    return () => observer.disconnect();
  }, [open]);
  useEffect(() => {
    if (!root.current) return;
    const refresh = () => {
      const next = findHits(root.current!, open ? query : "");
      const previous = active.current;
      const retained = next.findIndex(hit => hit.element === previous?.element && hit.offset === previous.offset);
      setHits(previous => previous.length === next.length && previous.every((hit, i) =>
        hit.element === next[i].element && hit.offset === next[i].offset &&
        hit.range.startContainer === next[i].range.startContainer && hit.range.startOffset === next[i].range.startOffset &&
        hit.range.endContainer === next[i].range.endContainer && hit.range.endOffset === next[i].range.endOffset,
      ) ? previous : next);
      setIndex(old => retained >= 0 ? retained : Math.min(old, Math.max(0, next.length - 1)));
    };
    refresh();
    const observer = new MutationObserver(refresh);
    observer.observe(root.current, { childList: true, characterData: true, subtree: true });
    return () => observer.disconnect();
  }, [open, query]);
  useEffect(() => {
    const hit = hits[index];
    const previous = active.current;
    active.current = hit;
    if (!hit) return;
    const opened = new Set<HTMLDetailsElement>();
    for (let node = hit.range.startContainer.parentElement; node && node !== root.current; node = node.parentElement) {
      if (node instanceof HTMLDetailsElement && !node.open) { node.setAttribute("open", ""); opened.add(node); }
    }
    const manuallyToggled = (event: Event) => {
      const target = event.target;
      if (target instanceof Element) {
        const details = target.closest("summary")?.parentElement;
        if (details instanceof HTMLDetailsElement) opened.delete(details);
      }
    };
    const container = root.current;
    container?.addEventListener("click", manuallyToggled);
    hit.element.setAttribute("data-pr-find-active", "true");
    if (!previous || previous.element !== hit.element || previous.offset !== hit.offset) {
      hit.range.startContainer.parentElement?.scrollIntoView?.({ block: "center", behavior: "instant" });
    }
    // CSS Custom Highlights does not rewrite React-owned text nodes or HTML.
    const registry = globalThis.CSS?.highlights;
    if (registry && typeof Highlight !== "undefined") {
      registry.set("pr-find", new Highlight(...hits.map(h => h.range)));
      registry.set("pr-find-active", new Highlight(hit.range));
    }
    return () => {
      hit.element.removeAttribute("data-pr-find-active");
      container?.removeEventListener("click", manuallyToggled);
      for (const node of opened) node.removeAttribute("open");
      registry?.delete("pr-find"); registry?.delete("pr-find-active");
    };
  }, [hits, index]);
  const move = (delta: number) => setIndex(i => hits.length ? (i + delta + hits.length) % hits.length : 0);
  return <PrFindExpanded value={open && query.length > 0}>
    <div ref={frame} className="flex flex-col gap-3">
      <div ref={toolbar} className={`${open ? "sticky z-20" : ""} flex flex-wrap items-center gap-2 rounded border border-[#30363d] bg-[#0d1117] p-2`}
        style={{ top: "var(--app-header-h, 0px)" }}
        onKeyDown={event => { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); } }}>
        <button type="button" className="tap-target rounded px-2 py-1 text-sm" onClick={show}>Find in pull request</button>
        {open && <>
          <input ref={input} type="search" aria-label="Find in pull request" value={query}
            className="min-w-0 basis-full flex-1 rounded border border-[#30363d] bg-[#161b22] p-2 text-sm sm:basis-0"
            onChange={event => { setQuery(event.target.value); setIndex(0); active.current = undefined; }}
            onKeyDown={event => {
              if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); }
              if (event.key === "Enter") { event.preventDefault(); move(event.shiftKey ? -1 : 1); }
            }} />
          <span role="status" className="text-xs">{hits.length ? `${index + 1} of ${hits.length}` : "0 matches"}</span>
          <button type="button" className="tap-target px-2" aria-label="Previous match" disabled={!hits.length} onClick={() => move(-1)}>↑</button>
          <button type="button" className="tap-target px-2" aria-label="Next match" disabled={!hits.length} onClick={() => move(1)}>↓</button>
          <button type="button" className="tap-target px-2" aria-label="Close find" onClick={close}>×</button>
          <p className="w-full text-xs text-[#8b949e]">Loaded text only. {query && "Sections stay open while searching. "}{partial ? " Some content is not loaded; results may be incomplete." : ""}</p>
        </>}
      </div>
      <div ref={root} className="flex flex-col gap-3">{children}</div>
    </div>
  </PrFindExpanded>;
}

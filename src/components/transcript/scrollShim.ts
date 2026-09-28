/// A scroll and layout shim for testing the transcript viewer in jsdom
/// (#1479). TEST-ONLY: imported by `*.test.tsx` files and nothing else.
///
/// # Why it exists
///
/// jsdom does no layout. `virtualWindow.ts` lists what that means: no
/// `ResizeObserver`, every height `0`, no `scrollTo`. The viewer's
/// behaviour -- open at the last anchor, follow at the bottom, hold still
/// when scrolled up -- is ALL geometry, so without this a test could only
/// assert that rows exist, never where the reader is.
///
/// This is the approach `@shadcn/react`'s own jsdom suite takes for the
/// same component (its `message-scroller.test.tsx` stubs rects and
/// heights), keyed here on the vendored wrapper's `data-slot`s rather
/// than test ids so the production markup needs nothing test-only.
///
/// # The model
///
/// - The viewport (`data-slot="message-scroller-viewport"`) is
///   `viewportHeight` tall and sits at `top: 0`. `setViewportHeight`
///   stands in for something below it taking height (the composer, the
///   on-screen keyboard); it keeps `scrollTop` unless the new maximum is
///   lower, as a browser does, and fires nothing until `resize()`.
/// - Rows (`[data-message-id]` children of the content) stack top to
///   bottom with no gap or padding, each `rowHeight` tall unless
///   `setRowHeight` says otherwise.
/// - The scroller's tail spacer adds its inline `height` when shown.
/// - `scrollTop` is stored per element and clamped to
///   `[0, scrollHeight - clientHeight]`, as a browser clamps it. A change
///   fires `scroll` one frame later, as a browser does.
/// - `ResizeObserver` observes nothing by itself: `resize()` fires every
///   live observer, standing in for a row changing height.
///
/// # Stated limits -- what a passing test here does NOT show
///
/// - **No real layout.** Heights are what the test declares. Text
///   wrapping, fonts, `content-visibility`, and CSS padding and gaps do
///   not exist.
/// - **No native scroll anchoring** (`overflow-anchor`). A prepend or a
///   removal above the viewport moves nothing unless the component
///   corrects `scrollTop` itself -- which is what these tests check, and
///   is also Safari's behaviour, so it is the stricter case.
/// - **No smooth scrolling.** `behavior: "smooth"` lands instantly; the
///   last requested behavior is recorded for assertions.
/// - **No `IntersectionObserver`**, so the scroller uses its layout
///   fallback for visibility.
/// - **Frames are timers.** `requestAnimationFrame` is a 16 ms fake
///   timer; `flush()` advances them inside `act`.
///
/// Scroll feel, frame rate and memory are the browser harness's job
/// (`docs/transcript-performance.md`), not this file's.

import { act } from "@testing-library/react";
import { vi } from "vitest";

const VIEWPORT = "message-scroller-viewport";
const CONTENT = "message-scroller-content";

export interface ScrollShim {
  /// The mounted viewport, or throws.
  viewport(): HTMLElement;
  /// Rows currently mounted, in order.
  rows(): HTMLElement[];
  setRowHeight(id: string, height: number): void;
  setViewportHeight(height: number): void;
  /// Fire every `ResizeObserver`, then let frames run.
  resize(): Promise<void>;
  /// Let pending frames and timers run.
  flush(): Promise<void>;
  /// Scroll as a reader does: a wheel gesture, then the position.
  userScrollTo(top: number): Promise<void>;
  /// The largest `scrollTop` the viewport can take now.
  maxScrollTop(): number;
  /// A row's top relative to the viewport's top.
  rowTop(id: string): number;
  /// The `behavior` of the last `scrollTo` call.
  lastBehavior(): ScrollBehavior | undefined;
  restore(): void;
}

export function installScrollShim({
  viewportHeight = 200,
  rowHeight = 40,
}: { viewportHeight?: number; rowHeight?: number } = {}): ScrollShim {
  vi.useFakeTimers();
  const heights = new Map<string, number>();
  const scrollTops = new WeakMap<Element, number>();
  const observers = new Set<ShimResizeObserver>();
  let lastBehavior: ScrollBehavior | undefined;

  const proto = HTMLElement.prototype;
  const saved = {
    rect: proto.getBoundingClientRect,
    scrollTo: Object.getOwnPropertyDescriptor(Element.prototype, "scrollTo"),
    scrollTop: Object.getOwnPropertyDescriptor(Element.prototype, "scrollTop"),
    clientHeight: Object.getOwnPropertyDescriptor(Element.prototype, "clientHeight"),
    scrollHeight: Object.getOwnPropertyDescriptor(Element.prototype, "scrollHeight"),
    resizeObserver: (window as { ResizeObserver?: unknown }).ResizeObserver,
  };

  const isViewport = (el: Element) => el instanceof HTMLElement && el.dataset.slot === VIEWPORT;
  const isContent = (el: Element) => el instanceof HTMLElement && el.dataset.slot === CONTENT;
  const rowsOf = (content: Element) =>
    Array.from(content.children).filter(
      (c): c is HTMLElement => c instanceof HTMLElement && c.dataset.messageId !== undefined,
    );
  const heightOf = (row: HTMLElement) => heights.get(row.dataset.messageId ?? "") ?? rowHeight;
  const contentOf = (el: Element) =>
    isContent(el) ? el : el.querySelector(`[data-slot="${CONTENT}"]`);
  const viewportOf = (el: Element) =>
    isViewport(el) ? el : el.closest(`[data-slot="${VIEWPORT}"]`);

  // Row offsets, cached per content element. The scroller reads every
  // row's rect in a loop, so recomputing offsets per rect is quadratic
  // and a 500-row window takes seconds. Invalidated by any change to the
  // declared heights or to which rows are mounted. The last child is the
  // scroller's spacer, so the row before it is compared too: a row
  // swapped for another at the tail (a pending message replaced by its
  // record, #1491) keeps the count and both ends.
  let heightsVersion = 0;
  interface Layout {
    version: number;
    first: Element | null;
    last: Element | null;
    beforeLast: Element | null;
    count: number;
    offsets: Map<Element, number>;
    total: number;
  }
  const layouts = new WeakMap<Element, Layout>();
  const layoutOf = (content: Element): Layout => {
    const cached = layouts.get(content);
    if (
      cached &&
      cached.version === heightsVersion &&
      cached.count === content.childElementCount &&
      cached.first === content.firstElementChild &&
      cached.last === content.lastElementChild &&
      cached.beforeLast === (content.lastElementChild?.previousElementSibling ?? null)
    )
      return cached;
    const offsets = new Map<Element, number>();
    let total = 0;
    for (const r of rowsOf(content)) {
      offsets.set(r, total);
      total += heightOf(r);
    }
    const layout = {
      version: heightsVersion,
      first: content.firstElementChild,
      last: content.lastElementChild,
      beforeLast: content.lastElementChild?.previousElementSibling ?? null,
      count: content.childElementCount,
      offsets,
      total,
    };
    layouts.set(content, layout);
    return layout;
  };

  const contentHeight = (content: Element): number => {
    let h = layoutOf(content).total;
    const spacer = content.querySelector<HTMLElement>("[data-message-scroller-spacer]");
    if (spacer && !spacer.hidden) h += Number.parseFloat(spacer.style.height) || 0;
    return h;
  };
  const scrollHeightOf = (el: Element): number => {
    const content = contentOf(el);
    return content ? Math.max(contentHeight(content), viewportHeight) : 0;
  };
  const maxTop = (el: Element) => Math.max(0, scrollHeightOf(el) - viewportHeight);

  const rect = (top: number, height: number): DOMRect =>
    ({
      top,
      bottom: top + height,
      height,
      left: 0,
      right: 0,
      width: 0,
      x: 0,
      y: top,
      toJSON: () => ({}),
    }) as DOMRect;

  const pendingScroll = new WeakSet<Element>();
  const setTop = (el: Element, value: number) => {
    const next = Math.min(Math.max(0, value), maxTop(el));
    const prev = scrollTops.get(el) ?? 0;
    scrollTops.set(el, next);
    if (next !== prev && !pendingScroll.has(el)) {
      pendingScroll.add(el);
      window.requestAnimationFrame(() => {
        pendingScroll.delete(el);
        el.dispatchEvent(new Event("scroll"));
      });
    }
  };

  Object.defineProperty(Element.prototype, "scrollTop", {
    configurable: true,
    get(this: Element) {
      return isViewport(this) ? (scrollTops.get(this) ?? 0) : 0;
    },
    set(this: Element, v: number) {
      if (isViewport(this)) setTop(this, v);
    },
  });
  Object.defineProperty(Element.prototype, "clientHeight", {
    configurable: true,
    get(this: Element) {
      if (isViewport(this)) return viewportHeight;
      if (this instanceof HTMLElement && this.dataset.messageId !== undefined)
        return heightOf(this);
      return 0;
    },
  });
  Object.defineProperty(Element.prototype, "scrollHeight", {
    configurable: true,
    get(this: Element) {
      return isViewport(this) || isContent(this) ? scrollHeightOf(this) : 0;
    },
  });
  Object.defineProperty(Element.prototype, "scrollTo", {
    configurable: true,
    writable: true,
    value(this: Element, a?: ScrollToOptions | number, b?: number) {
      const top = typeof a === "number" ? b : a?.top;
      if (typeof a === "object") lastBehavior = a.behavior;
      if (typeof top === "number") setTop(this, top);
    },
  });
  proto.getBoundingClientRect = function (this: HTMLElement) {
    if (isViewport(this)) return rect(0, viewportHeight);
    const viewport = viewportOf(this);
    if (!viewport) return saved.rect.call(this);
    const scrollTop = scrollTops.get(viewport) ?? 0;
    if (isContent(this)) return rect(-scrollTop, contentHeight(this));
    if (this.dataset.messageId !== undefined && this.parentElement) {
      const offset = layoutOf(this.parentElement).offsets.get(this) ?? 0;
      return rect(offset - scrollTop, heightOf(this));
    }
    return saved.rect.call(this);
  };

  class ShimResizeObserver {
    constructor(private readonly callback: ResizeObserverCallback) {
      observers.add(this);
    }
    observe() {}
    unobserve() {}
    disconnect() {
      observers.delete(this);
    }
    fire() {
      this.callback([], this as unknown as ResizeObserver);
    }
  }
  (window as { ResizeObserver?: unknown }).ResizeObserver = ShimResizeObserver;

  const flush = async () => {
    for (let i = 0; i < 6; i++) {
      await act(async () => {
        await vi.advanceTimersByTimeAsync(20);
      });
    }
  };

  const viewport = () => {
    const el = document.querySelector<HTMLElement>(`[data-slot="${VIEWPORT}"]`);
    if (!el) throw new Error("no transcript viewport is mounted");
    return el;
  };

  return {
    viewport,
    rows: () => {
      const content = document.querySelector(`[data-slot="${CONTENT}"]`);
      return content ? rowsOf(content) : [];
    },
    setRowHeight: (id, height) => {
      heights.set(id, height);
      heightsVersion++;
    },
    setViewportHeight: (height) => {
      viewportHeight = height;
      const el = document.querySelector(`[data-slot="${VIEWPORT}"]`);
      if (el) setTop(el, scrollTops.get(el) ?? 0);
    },
    resize: async () => {
      await act(async () => {
        for (const o of observers) o.fire();
      });
      await flush();
    },
    flush,
    userScrollTo: async (top) => {
      const el = viewport();
      await act(async () => {
        el.dispatchEvent(new WheelEvent("wheel", { bubbles: true, deltaY: top - el.scrollTop }));
        el.scrollTop = top;
      });
      await flush();
    },
    maxScrollTop: () => maxTop(viewport()),
    rowTop: (id) => {
      const row = document.querySelector<HTMLElement>(`[data-message-id="${id}"]`);
      if (!row) throw new Error(`row ${id} is not mounted`);
      return row.getBoundingClientRect().top;
    },
    lastBehavior: () => lastBehavior,
    restore: () => {
      proto.getBoundingClientRect = saved.rect;
      for (const [key, d] of [
        ["scrollTo", saved.scrollTo],
        ["scrollTop", saved.scrollTop],
        ["clientHeight", saved.clientHeight],
        ["scrollHeight", saved.scrollHeight],
      ] as const) {
        if (d) Object.defineProperty(Element.prototype, key, d);
        else delete (Element.prototype as unknown as Record<string, unknown>)[key];
      }
      (window as { ResizeObserver?: unknown }).ResizeObserver = saved.resizeObserver;
      vi.useRealTimers();
    },
  };
}

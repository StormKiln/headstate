import { useEffect, useRef, type RefObject } from "react";

/// The CSS variable the app header publishes its own height into.
///
/// Read by every OTHER sticky element inside `<main>` as its `top`, so
/// each one pins directly below the app header instead of underneath it.
export const APP_HEADER_HEIGHT_VAR = "--app-header-h";

/// Publish the app header's height so the stickies below it can clear it.
///
/// #1278: the app header in `App` is `sticky top-0 z-20` inside `<main>`,
/// the app's one scroll container. Everything else that pins in that
/// container is also `top-0`, at a lower `z-index` and with an opaque
/// background -- so those elements DO stick, exactly where the app header
/// is already pinned, and are painted over by it. The symptom is
/// indistinguishable from `position: sticky` not working, which is why
/// #1278 was reported as the PR header "not staying put": it stays put
/// perfectly, one layer down and out of sight.
///
/// The header was not always sticky. `PrDetailView`'s comment still says
/// "the app header above scrolls away with the content rather than being
/// sticky itself, so nothing overlaps", which was true when that header
/// was pinned (#331) and stopped being true when the phone layout pinned
/// the app header (#623). This is the missing half of that change.
///
/// MEASURED rather than a constant, because the two layouts genuinely
/// differ: the phone's hamburger is a `.tap-target` with a 44px minimum
/// and the desktop header is a `text-sm` line, so any single number is
/// wrong on one of them. A `ResizeObserver` also covers the header
/// reflowing under a long view label or a wrapped control, which a token
/// fixed at build time could not.
///
/// Written to `<main>` rather than to `:root` so the variable is scoped
/// to the scroll container whose top it describes -- a sticky element in
/// some other container would get a meaningless offset from a global.
export function useStickyHeaderOffset(
  scrollRef: RefObject<HTMLElement | null>,
  headerRef: RefObject<HTMLElement | null>,
): void {
  // The last value written, so an observer callback that reports the
  // same height does not touch the DOM again. `ResizeObserver` fires on
  // every layout that involves the box, not only on ones that change it.
  const lastRef = useRef<number | null>(null);

  useEffect(() => {
    const scroll = scrollRef.current;
    const header = headerRef.current;
    if (!scroll || !header) return;

    // Reset per effect run, not just per mount: the cache exists to skip
    // redundant DOM writes within one subscription, and carrying a value
    // across a re-run would let a height that is still current for the
    // OLD node suppress the first write for the new one.
    lastRef.current = null;

    const publish = () => {
      // `getBoundingClientRect` rather than `offsetHeight`: the header's
      // height is fractional (a 1px border on a `py-3` line lands on
      // 43.5px at the default zoom), and rounding it down leaves a
      // half-pixel of the pinned element showing above the header.
      const h = header.getBoundingClientRect().height;
      if (h === lastRef.current) return;
      lastRef.current = h;
      scroll.style.setProperty(APP_HEADER_HEIGHT_VAR, `${h}px`);
    };

    publish();

    // Guarded because jsdom has no ResizeObserver: the unit tests render
    // this tree, and an unguarded constructor would throw during their
    // mount rather than in the browser this is for.
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(publish);
    ro.observe(header);
    return () => {
      ro.disconnect();
    };
  }, [scrollRef, headerRef]);
}

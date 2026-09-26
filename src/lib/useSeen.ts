import { useEffect, useState } from "react";
import type { RefObject } from "react";

/// Whether `ref`'s element has come within `margin` of the viewport yet.
///
/// LATCHED: once true it stays true. The caller does expensive one-off
/// work on first sight (syntax highlighting, #1482), and undoing it when
/// the element scrolls away would only mean doing it again on the way
/// back.
///
/// WITHOUT `IntersectionObserver` -- jsdom, which every test here runs
/// in, and nothing this app ships to -- the answer is "seen" at once.
/// The work then happens for every MOUNTED element rather than every
/// visible one, which is still bounded by whatever mounts: a virtualized
/// list mounts a window, not the transcript. Answering "never seen"
/// instead would leave the work undone forever, which is the Pending
/// that nothing moves on (#1042).
export function useSeen(ref: RefObject<Element | null>, margin = "200px"): boolean {
  const [seen, setSeen] = useState(() => typeof IntersectionObserver === "undefined");
  useEffect(() => {
    const el = ref.current;
    if (seen || el === null || typeof IntersectionObserver === "undefined") return;
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          setSeen(true);
          io.disconnect();
        }
      },
      { rootMargin: margin },
    );
    io.observe(el);
    return () => io.disconnect();
  }, [ref, margin, seen]);
  return seen;
}

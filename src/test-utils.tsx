import { QueryClientProvider } from "@tanstack/react-query";
import { createQueryClient } from "./api/queryClient";
import { render } from "@testing-library/react";
import type { ReactElement } from "react";

/// Render with a QueryClient.
///
/// Components below the list level now reach for the query cache -- the
/// kebab menu invalidates `prs` after acting -- so a bare `render` throws
/// "No QueryClient set" for a component whose test has nothing to do with
/// fetching. Built by production's own factory, so a test sees the
/// app's retry policy (off unless a query opts in, #1955) rather than a
/// test-only `retry: false` that would hide a change to it.
export function renderWithQuery(ui: ReactElement) {
  const client = createQueryClient();
  return render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
}

/// Pretend the viewport is `width` pixels wide, for `useIsMobile`.
///
/// jsdom has no `matchMedia`, so the hook reads every test as desktop
/// unless told otherwise. This installs one that answers `max-width`
/// queries from the given width and fires `change` on `resize`. Pass
/// `null` to remove it again, which is what `afterEach` should do.
export function stubViewport(width: number | null): { resize: (width: number) => void } {
  if (width === null) {
    delete (window as { matchMedia?: unknown }).matchMedia;
    return { resize: () => {} };
  }
  let current = width;
  type Listener = (e: MediaQueryListEvent) => void;
  const lists = new Set<{ query: string; listeners: Set<Listener> }>();
  const matches = (query: string) => {
    const m = /\(max-width:\s*(\d+)px\)/.exec(query);
    return m !== null && current <= Number(m[1]);
  };
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    writable: true,
    value: (query: string): MediaQueryList => {
      const listeners = new Set<Listener>();
      lists.add({ query, listeners });
      const mql = {
        media: query,
        get matches() {
          return matches(query);
        },
        onchange: null,
        addEventListener: (_: string, cb: Listener) => listeners.add(cb),
        removeEventListener: (_: string, cb: Listener) => listeners.delete(cb),
        addListener: (cb: Listener) => listeners.add(cb),
        removeListener: (cb: Listener) => listeners.delete(cb),
        dispatchEvent: () => true,
      };
      return mql as unknown as MediaQueryList;
    },
  });
  return {
    resize: (next: number) => {
      current = next;
      for (const { query, listeners } of lists) {
        const e = { matches: matches(query), media: query } as MediaQueryListEvent;
        for (const cb of listeners) cb(e);
      }
    },
  };
}

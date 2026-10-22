import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import type { ReactNode } from "react";

// `background-degraded` was emitted by the backend and heard by nobody
// (#1937): the notice waited for the next 30 s poll to learn what had
// already been announced.
const ipc = vi.hoisted(() => ({
  handlers: new Map<string, () => void>(),
  stop: vi.fn(),
}));
vi.mock("./transport", async (orig) => ({
  ...(await orig<Record<string, unknown>>()),
  listen: vi.fn(async (name: string, handler: () => void) => {
    ipc.handlers.set(name, handler);
    return ipc.stop;
  }),
}));
const health = vi.hoisted(() => vi.fn(() => Promise.resolve([])));
vi.mock("./tauri", async (orig) => ({
  ...(await orig<Record<string, unknown>>()),
  backgroundHealth: health,
}));

const { useBackgroundHealth } = await import("./hooks");

it("re-reads background health the moment a loop reports itself degraded", async () => {
  const client = new QueryClient();
  const { unmount } = renderHook(useBackgroundHealth, {
    wrapper: ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    ),
  });
  await waitFor(() => expect(health).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(ipc.handlers.has("background-degraded")).toBe(true));

  await act(async () => {
    ipc.handlers.get("background-degraded")?.();
  });
  // Well inside the 30 s poll: this is the event, not the interval.
  await waitFor(() => expect(health).toHaveBeenCalledTimes(2));

  unmount();
  expect(ipc.stop).toHaveBeenCalledOnce();
});

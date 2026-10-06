import { act, cleanup, render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, it, vi } from "vitest";
import publications from "../../src-tauri/tests/fixtures/targeted-fact-publications.json";
const boundary = vi.hoisted(() => ({ listeners: new Map<string, (event: { payload: unknown }) => void>() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: async () => [] }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async (name: string, callback: (event: { payload: unknown }) => void) => { boundary.listeners.set(name, callback); return () => boundary.listeners.delete(name); } }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(async () => {}) }));
import { useSourceRefresh } from "../api/sourceRefreshHooks";
import { remoteEventError } from "../api/wireContract";
import { ReadyStrip } from "./ReadyStrip";
import { GitHubQueueFreshness } from "./GitHubQueueFreshness";
const clients: QueryClient[] = [];
function Observer() {
  const receipt = useSourceRefresh("reviewing");
  return <><ReadyStrip prs={receipt.prs ?? []} onOpen={() => {}} /><footer data-testid="footer"><GitHubQueueFreshness github={{ list: "reviewing", receipt }} /></footer></>;
}
afterEach(() => { cleanup(); clients.splice(0).forEach(client => client.clear()); boundary.listeners.clear(); vi.restoreAllMocks(); });
it.each(["clean", "unrelated", "new_head"] as const)("renders native %s targeted publication without inventing field freshness", async name => {
  vi.spyOn(Date, "now").mockReturnValue(Date.parse("2026-10-01T12:01:00Z"));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } }); clients.push(client);
  client.setQueryData(["viewer"], "synthetic-viewer");
  render(<QueryClientProvider client={client}><Observer /></QueryClientProvider>);
  const frame = publications.frames[name];
  expect(remoteEventError("source-poll-status", frame)).toBeNull();
  await act(async () => boundary.listeners.get("source-poll-status")?.({ payload: frame }));
  const footer = within(screen.getByTestId("footer"));
  if (name === "clean") {
    expect(footer.getByText("Review requests checked")).toBeTruthy();
    expect(screen.getByText("Synthetic review 1")).toBeTruthy();
    expect(screen.queryByText("Readiness last known")).toBeNull();
    expect(screen.queryByText("Unconfirmed", { exact: true })).toBeNull();
  } else {
    expect(footer.getByText("Review requests need checking")).toBeTruthy();
    expect(screen.getByText("Nothing ready to review.")).toBeTruthy();
    expect(footer.getByLabelText(/1 rows have unconfirmed or last-known readiness/)).toBeTruthy();
    if (name === "unrelated") expect(footer.getByLabelText(/1 rows have last-known list membership/)).toBeTruthy();
  }
});

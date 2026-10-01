import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { RemoteTranscriptWindow, TranscriptMessage } from "@/types/transcript";
import { call, output, UNKNOWN } from "./fixtures";
import { installScrollShim, type ScrollShim } from "./scrollShim";

const state = vi.hoisted(() => ({
  pages: {} as Record<string, unknown>,
  listeners: new Set<(e: { payload: unknown }) => void>(),
  read: vi.fn(), watch: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => new Promise(() => {})) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async (event, cb) => {
  if (event === "claude-transcript-activity") state.listeners.add(cb);
  return () => state.listeners.delete(cb);
}) }));
vi.mock("@/api/tauri", async (original) => ({
  ...(await original<object>()), claudeTranscriptPage: state.read, claudeTranscriptWatch: state.watch,
}));
import { DesktopTranscript } from "./DesktopTranscript";
import { PhoneTranscript } from "./phone/PhoneTranscript";

function page(path: string, child?: string): RemoteTranscriptWindow {
  const m: TranscriptMessage = {
    id: path, id_source: "uuid", turn_id: path, kind: { kind: "assistant" }, timestamp: null,
    model: null, api_message_id: null, usage: null, duration_ms: null, is_meta: false,
    is_sidechain: false, offset: 0, oversized_bytes: null,
    blocks: child ? [call("Task", { tool: "task", description: "look", subagent_type: child, prompt: "go", truncated: false },
      output({ subagent: { agent_id: child, agent_type: child, status: "running", transcript_found: true, transcript_path: child } }))]
      : [{ kind: "text", index: 0, text: "generic fixture", clip: null }],
  };
  return {
    page: { messages: [m], file_bytes: 100, bytes_read: 100, truncated: false, machinery_records: [], unparseable_records: 0, duplicate_records: 0 },
    start: { offset: 0, behind_digest: "" }, end: { offset: 100, behind_digest: "d" },
    at_start: true, at_end: true, rewritten: false, bytes_scanned: 0,
    position: { first: 1, last: 1, total: 1, exact: true, basis: "whole_file" },
    seam: { first_model: null, last_model: null },
    masking: { hidden: 1, withheld: false, reveal_allowed: true, revealed: false },
  };
}
let shim: ScrollShim;
const settle = () => act(() => vi.advanceTimersByTimeAsync(1));
const nudge = async (id: string) => {
  act(() => { for (const cb of state.listeners) cb({ payload: { watch_id: id, size: 101, seq: 1 } }); });
  await settle();
};
beforeEach(() => {
  vi.useFakeTimers(); shim = installScrollShim();
  Object.defineProperty(document, "visibilityState", { configurable: true, value: "visible" });
  state.pages = { main: page("main", "child"), child: page("child", "nested"), nested: page("nested") };
  state.listeners.clear(); state.read.mockReset(); state.watch.mockReset();
  state.read.mockImplementation(async (path, _a, _d, _l, reveal) => {
    const p = state.pages[path] as RemoteTranscriptWindow;
    return { ...p, masking: { ...p.masking, revealed: !!reveal } };
  });
  state.watch.mockImplementation(async (path) => ({ watch_id: path, expires_in_ms: 30000 }));
});
afterEach(() => { cleanup(); shim.restore(); vi.useRealTimers(); });

it.each(["desktop", "phone"])("%s mounts only the active nested child watch, restores it on back, and stops on close", async (host) => {
  const view = render(host === "desktop" ? <DesktopTranscript path="main" liveness={UNKNOWN} /> : <PhoneTranscript path="main" liveness={UNKNOWN} />);
  await settle(); expect(state.watch).not.toHaveBeenCalled();
  if (host === "phone") { fireEvent.click(screen.getByRole("button", { name: "Agent: look, running" })); await settle(); }
  fireEvent.click(screen.getByRole("button", { name: /open the transcript of subagent child/i }));
  await settle(); expect(state.watch).toHaveBeenLastCalledWith("child");
  if (host === "phone") { fireEvent.click(screen.getByRole("button", { name: "Agent: look, running" })); await settle(); }
  fireEvent.click(screen.getByRole("button", { name: /open the transcript of subagent nested/i }));
  await settle(); expect(state.watch).toHaveBeenLastCalledWith("nested");
  state.read.mockClear();
  await nudge("child"); expect(state.read).not.toHaveBeenCalled();
  await nudge("nested"); expect(state.read).toHaveBeenCalledTimes(1);
  await act(() => vi.advanceTimersByTimeAsync(10001));
  expect(state.watch.mock.calls.filter(([p]) => p === "child")).toHaveLength(1);
  if (host === "desktop") fireEvent.click(screen.getByRole("button", { name: /back to the previous subagent/i }));
  else fireEvent.click(screen.getAllByRole("button", { name: "Close" }).at(-1)!);
  await settle(); expect(state.watch).toHaveBeenLastCalledWith("child");
  view.unmount(); const count = state.watch.mock.calls.length;
  await act(() => vi.advanceTimersByTimeAsync(40000));
  expect(state.watch).toHaveBeenCalledTimes(count); expect(state.listeners.size).toBe(0);
});

it("phone Reveal transfers child nudges to the revealed owner and Hide transfers them back", async () => {
  render(<PhoneTranscript path="child" liveness={UNKNOWN} watchActivity />);
  await settle();
  fireEvent.click(screen.getByRole("button", { name: "Reveal" })); await settle();
  state.read.mockClear(); await nudge("child");
  expect(state.read).toHaveBeenCalledTimes(1); expect(state.read.mock.calls[0][4]).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Hide it again" })); await settle();
  state.read.mockClear(); await nudge("child");
  expect(state.read).toHaveBeenCalledTimes(1); expect(state.read.mock.calls[0][4]).toBe(false);
});

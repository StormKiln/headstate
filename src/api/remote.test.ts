import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  invoke: vi.fn<(cmd: string, args?: unknown) => Promise<unknown>>(() =>
    Promise.resolve(undefined),
  ),
  listen: vi.fn<(event: string, cb: unknown) => Promise<() => void>>(() =>
    Promise.resolve(() => {}),
  ),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: tauri.listen }));

import { remote } from "./remote";
import { PR_FIXTURES } from "../fixtures/prs";
import { GitLabQueueState } from "./gitlabQueueState";
import type { SourcePollUpdate } from "./tauri";

function setVisibility(state: DocumentVisibilityState) {
  Object.defineProperty(document, "visibilityState", { value: state, configurable: true });
  document.dispatchEvent(new Event("visibilitychange"));
}

beforeEach(() => {
  tauri.invoke.mockClear();
  tauri.listen.mockClear();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("remote transport: commands", () => {
  it("keeps native markdown export on the phone, never remote_call", async () => {
    tauri.invoke.mockResolvedValueOnce("presented");
    await expect(remote.call("save_markdown", {markdown: "# masked"})).resolves.toBe("presented");
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("save_markdown", {markdown: "# masked"});
  });
  it("rejects a malformed remote reply before the caller receives it", async () => {
    tauri.invoke.mockResolvedValueOnce([{ number: 1347 }]);
    await expect(remote.call("get_cached")).rejects.toThrow(/get_cached.*incompatible|incompatible.*get_cached/);
  });

  it("forwards a desktop command through remote_call with an object of args", async () => {
    tauri.invoke.mockResolvedValueOnce(PR_FIXTURES);
    await expect(remote.call("get_cached")).resolves.toBe(PR_FIXTURES);
    expect(tauri.invoke).toHaveBeenCalledWith("remote_call", { command: "get_cached", args: {} });

    tauri.invoke.mockResolvedValueOnce({ points: [], week_current: 0, week_previous: 0, opened_week_current: 0, opened_week_previous: 0, month_current: 0, month_previous: 0 });
    await remote.call("get_history", { days: 14 });
    expect(tauri.invoke).toHaveBeenLastCalledWith("remote_call", {
      command: "get_history",
      args: { days: 14 },
    });
  });

  it("invokes the companion's own commands directly, arity preserved", async () => {
    tauri.invoke.mockResolvedValueOnce({
      state: "connected",
      desktop: "octocat's laptop",
      last_poll: null,
      protocol_version: 1,
      stale: false,
    });
    await expect(remote.call("connection_state")).resolves.toMatchObject({ state: "connected" });
    expect(tauri.invoke).toHaveBeenCalledWith("connection_state");
    expect(tauri.invoke.mock.calls[0]).toHaveLength(1);

    tauri.invoke.mockResolvedValueOnce("octocat's laptop");
    await expect(remote.call("pair_from_qr", { payload: "{}" })).resolves.toBe("octocat's laptop");
    expect(tauri.invoke).toHaveBeenLastCalledWith("pair_from_qr", { payload: "{}" });

    for (const name of ["unpair", "subscribe_events"]) {
      await remote.call(name);
      expect(tauri.invoke).toHaveBeenLastCalledWith(name);
    }
  });

  it("passes the companion's refusal through as the rejection", async () => {
    const refusal = "octocat's laptop is unreachable; actions are disabled until it is back";
    tauri.invoke.mockRejectedValueOnce(refusal);
    await expect(remote.call("act_on_pr", { id: "PR_1" })).rejects.toBe(refusal);
  });
});

describe("remote transport: events", () => {
  it("listens locally and opens the stream once, then again on each return to the foreground", async () => {
    const cb = () => {};
    const un = await remote.listen("prs-updated", cb);
    await remote.listen("poll-state", cb);
    expect(tauri.listen).toHaveBeenNthCalledWith(1, "prs-updated", expect.any(Function));
    expect(tauri.listen).toHaveBeenNthCalledWith(2, "poll-state", expect.any(Function));
    expect(typeof un).toBe("function");
    const subscribes = () =>
      tauri.invoke.mock.calls.filter(([cmd]) => cmd === "subscribe_events").length;
    expect(subscribes()).toBe(1);

    setVisibility("hidden");
    expect(subscribes()).toBe(1);
    setVisibility("visible");
    expect(subscribes()).toBe(2);
  });

  it("does not let a refused subscription surface", async () => {
    tauri.invoke.mockRejectedValueOnce("not paired with a desktop");
    setVisibility("visible");
    await Promise.resolve();
    expect(tauri.invoke).toHaveBeenCalledWith("subscribe_events");
  });
});


describe("remote response compatibility", () => {
  it("preserves optional omissions and future fields without cloning", async () => {
    const value = [{ ...PR_FIXTURES[0], future_desktop_field: { secret: "not logged" } }];
    delete value[0].ready_at;
    tauri.invoke.mockResolvedValueOnce(value);
    expect(await remote.call("get_cached")).toBe(value);
  });

  it("rejects nested optional/array/union errors and account error envelopes safely", async () => {
    const secret = "/private/token-DO-NOT-LOG";
    for (const value of [
      [{ ...PR_FIXTURES[0], ready_at: 123 }],
      [{ ...PR_FIXTURES[0], labels: [{ name: secret, color: 9 }] }],
      [{ ...PR_FIXTURES[0], ci: secret }],
      { error: secret },
    ]) {
      tauri.invoke.mockResolvedValueOnce(value);
      const error = await remote.call("get_cached").catch(e => e as Error);
      expect(error).toBeInstanceOf(Error);
      expect(String(error)).toContain("get_cached");
      expect(String(error)).not.toContain(secret);
    }
  });

  it("accepts both supported refresh shapes", async () => {
    for (const name of ["refresh_now", "get_reviewing"]) {
      for (const value of [PR_FIXTURES, { request_id: "r", update: {
        source: { provider: "github", host: "github.com" }, list: "authored", phase: "ready", error: null,
      } }]) {
        tauri.invoke.mockResolvedValueOnce(value);
        expect(await remote.call(name)).toBe(value);
      }
    }
  });

  it("accepts null/undefined void acknowledgements but rejects an error object", async () => {
    for (const value of [null, undefined]) {
      tauri.invoke.mockResolvedValueOnce(value);
      expect(await remote.call("remove_worktree")).toBe(value);
    }
    tauri.invoke.mockResolvedValueOnce({ error: "bad" });
    await expect(remote.call("remove_worktree")).rejects.toThrow(/remove_worktree/);
  });

  it("does not echo unknown command names", async () => {
    tauri.invoke.mockResolvedValueOnce({});
    await expect(remote.call("secret-command-name")).rejects.toThrow("no remote response contract");
  });
});

describe("remote event validation", () => {
  it("drops malformed frames, accepts the next valid frame, and returns the actual unlisten", async () => {
    const warning = vi.spyOn(console, "warn").mockImplementation(() => {});
    const unlisten = vi.fn();
    tauri.listen.mockResolvedValueOnce(unlisten);
    const consumer = vi.fn();
    expect(await remote.listen("prs-updated", consumer)).toBe(unlisten);
    const callback = tauri.listen.mock.calls.at(-1)![1] as (e: { payload: unknown }) => void;
    const secret = "/private/secret-prompt";
    callback({ payload: [{ title: secret }] });
    callback({ payload: [{ title: secret }] });
    expect(consumer).not.toHaveBeenCalled();
    const event = { payload: PR_FIXTURES };
    callback(event);
    expect(consumer).toHaveBeenCalledExactlyOnceWith(event);
    expect(warning).toHaveBeenCalledTimes(1);
    expect(warning.mock.calls.flat().join(" ")).not.toContain(secret);
  });

  it("retains old progress and source shapes but checks present optional fields", async () => {
    const warning = vi.spyOn(console, "warn").mockImplementation(() => {});
    for (const [event, valid, invalid] of [
      ["worktree-removal-progress", { done: 1, total: 2 }, { done: 1, total: 2, removed: "yes" }],
      ["source-poll-status", { source: { provider: "github", host: "github.com" }, list: "authored", phase: "ready", error: null },
        { source: { provider: "github", host: "github.com" }, list: "authored", phase: "ready", error: null, mrs: "wrong" }],
    ] as const) {
      const consumer = vi.fn();
      await remote.listen(event, consumer);
      const callback = tauri.listen.mock.calls.at(-1)![1] as (e: { payload: unknown }) => void;
      callback({ payload: valid });
      callback({ payload: invalid });
      expect(consumer).toHaveBeenCalledTimes(1);
    }
    expect(warning).toHaveBeenCalledTimes(2);
  });

  it("preserves synchronous listener-install errors and local events", async () => {
    const error = new Error("no webview");
    tauri.listen.mockImplementationOnce(() => { throw error; });
    expect(() => remote.listen("poll-state", () => {})).toThrow(error);
    const consumer = vi.fn();
    await remote.listen("connection-state", consumer);
    expect(tauri.listen).toHaveBeenLastCalledWith("connection-state", consumer);
    const callback = tauri.listen.mock.calls.at(-1)![1] as (e: { payload: unknown }) => void;
    const frame = { payload: { arbitrary_local_shape: true } };
    callback(frame);
    expect(consumer).toHaveBeenCalledExactlyOnceWith(frame);
  });
});


// Exercise the real phone event delivery seam and the GitLab consumer model.
// Old GitHub compatibility must never admit a partial GitLab receipt.
describe("remote GitLab queue event contract", () => {
  it.each([
    "session", "revision", "receipt_revision", "completed_request",
    "last_received_at", "mrs", "coverage",
  ])("drops a GitLab frame missing %s without poisoning the next valid receipt", async (missing) => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const model = new GitLabQueueState("gitlab.com", "authored");
    const accept = vi.fn((frame: SourcePollUpdate) => model.accept(frame));
    await remote.listen<SourcePollUpdate>("source-poll-status", ({ payload }) => {
      if (payload.source.provider === "gitlab") accept(payload);
    });
    const deliver = tauri.listen.mock.calls.at(-1)![1] as (event: { payload: unknown }) => void;
    const first: SourcePollUpdate = {
      source: { provider: "gitlab", host: "gitlab.com" }, list: "authored",
      phase: "ready", error: null, session: "desktop-current", revision: 1,
      receipt_revision: 1, completed_request: null,
      last_received_at: "2026-10-01T00:00:00Z", mrs: [], coverage: "complete",
    };
    deliver({ payload: first });
    expect(accept).toHaveBeenCalledTimes(1);
    const established = model.snapshot();
    const malformed: Record<string, unknown> = {
      ...first, revision: 2, receipt_revision: 2,
      phase: "failed", error: "malformed frame must not reach the queue",
    };
    delete malformed[missing];
    deliver({ payload: malformed });
    expect(accept).toHaveBeenCalledTimes(1);
    expect(model.snapshot()).toBe(established);
    const latest: SourcePollUpdate = {
      ...first, revision: 3, receipt_revision: 3,
      phase: "failed", error: "Latest valid provider status", mrs: [], coverage: "unknown",
    };
    deliver({ payload: latest });
    expect(accept).toHaveBeenCalledTimes(2);
    expect(model.snapshot().rows).toBe(latest.mrs);
    expect(model.snapshot().coverage).toBe("unknown");
    expect(model.snapshot().error).toBe("Latest valid provider status");
  });
});


it("validates watch replies and drops malformed activity frames without retiring the listener", async () => {
  tauri.invoke.mockResolvedValueOnce({ watch_id: "opaque", expires_in_ms: 30000 });
  await expect(remote.call("claude_transcript_watch", { path: "/generic/child.jsonl" })).resolves.toEqual({ watch_id: "opaque", expires_in_ms: 30000 });
  expect(tauri.invoke).toHaveBeenLastCalledWith("remote_call", { command: "claude_transcript_watch", args: { path: "/generic/child.jsonl" } });
  tauri.invoke.mockResolvedValueOnce({ watch_id: "opaque" });
  await expect(remote.call("claude_transcript_watch")).rejects.toThrow();
  const cb = vi.fn();
  const stop = await remote.listen("claude-transcript-activity", cb);
  const deliver = tauri.listen.mock.calls.find(([name]) => name === "claude-transcript-activity")![1] as (e: { payload: unknown }) => void;
  deliver({ payload: { watch_id: "opaque", size: 10, seq: 1 } });
  deliver({ payload: { watch_id: "opaque", size: "private-text", seq: 2 } });
  deliver({ payload: { watch_id: "opaque", size: 11, seq: 3 } });
  expect(cb).toHaveBeenCalledTimes(2);
  expect(cb).toHaveBeenLastCalledWith({ payload: { watch_id: "opaque", size: 11, seq: 3 } });
  stop();
});

it("accepts old registered stats boards and old progress at the real remote boundary", async () => {
  const legacy = { viewer:"alice", scopeKey:"scope", backfill:{state:"registered",lastFrame:null}, rows:[], total:null, retrieved:0, complete:false, truncatedSlices:[], refusedFields:0, slices:0, rounds:0, spend:{points:0,requests:0,unmetered:0,remaining:null,resetAt:null}, slowest:[],largest:[],repoCounts:[],accumulated:0,accumulating:false,daysCovered:0,daysTotal:30 };
  tauri.invoke.mockResolvedValueOnce(legacy);
  expect(await remote.call("stats_board")).toBe(legacy);
  const receive=vi.fn(); await remote.listen("stats-backfill-progress",receive);
  const handler=tauri.listen.mock.calls.find(([name])=>name==="stats-backfill-progress")?.[1] as (event:{payload:unknown})=>void;
  const frame={scopeKey:"scope",daysCovered:1,daysTotal:30,collected:17,total:20,phase:{kind:"working"},nextTickAtMs:null};
  handler({payload:frame});expect(receive).toHaveBeenCalledWith({payload:frame});
});

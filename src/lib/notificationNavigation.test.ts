import { describe, expect, it, vi } from "vitest";
import { connectNotificationNavigation } from "./notificationNavigation";
import { useFilters } from "@/store/filters";
import { useSourceSelection } from "@/store/sourceSelection";

describe("notification navigation", () => {
  it("subscribes before draining a startup click, switches provider and leaves local views", async () => {
    const order: string[] = [];
    useFilters.getState().setView("claude-code");
    const target = { source: { provider: "gitlab" as const, host: "gitlab.example" }, repo: "team/sub/project", number: 7 };
    const disconnect = await connectNotificationNavigation(async () => { order.push("listen"); return () => {}; }, async () => { order.push("take"); return target; });
    expect(order).toEqual(["listen", "take"]);
    expect(useFilters.getState().view).toBe("my-prs");
    expect(useFilters.getState().selectedPr).toEqual(target);
    expect(useSourceSelection.getState().selection).toBe("gitlab");
    disconnect();
  });
  it("consumes subsequent clicks without replaying an already drained click", async () => {
    let wake = () => {};
    const target = { source: { provider: "github" as const, host: "github.com" }, repo: "sample/project", number: 8 };
    const take = vi.fn().mockResolvedValueOnce(null).mockResolvedValueOnce(target).mockResolvedValue(null);
    const unlisten = vi.fn<() => void>();
    const disconnect = await connectNotificationNavigation(async (callback) => { wake = callback; return unlisten; }, take);
    wake();
    await vi.waitFor(() => expect(useFilters.getState().selectedPr).toEqual(target));
    useFilters.getState().selectPr(null);
    wake();
    await vi.waitFor(() => expect(take).toHaveBeenCalledTimes(3));
    expect(useFilters.getState().selectedPr).toBeNull();
    disconnect();
    expect(unlisten).toHaveBeenCalledOnce();
  });
  it("does not consume a startup click for an effect unmounted during listener registration", async () => {
    const controller = new AbortController();
    const take = vi.fn<() => Promise<null>>().mockResolvedValue(null);
    const unlisten = vi.fn<() => void>();
    const connected = connectNotificationNavigation(async () => {
      controller.abort();
      return unlisten;
    }, take, controller.signal);
    await connected;
    expect(take).not.toHaveBeenCalled();
    expect(unlisten).toHaveBeenCalledOnce();
  });

  it("keeps a click already taken while its effect is being torn down", async () => {
    const controller = new AbortController();
    const target = { repo: "example/project", number: 9 };
    await connectNotificationNavigation(async () => () => {}, async () => {
      controller.abort();
      return target;
    }, controller.signal);
    expect(useFilters.getState().selectedPr).toEqual(target);
  });

});

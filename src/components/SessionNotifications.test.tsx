import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  pollSessionToasts: vi.fn<(viewing: string | undefined) => Promise<unknown[]>>(() =>
    Promise.resolve([]),
  ),
  takeNotificationSession: vi.fn<() => Promise<string | null>>(() => Promise.resolve(null)),
}));
vi.mock("@/api/phoneNotify", () => api);

const toast = vi.hoisted(() => vi.fn());
vi.mock("sonner", () => ({ toast }));

import { useFilters } from "@/store/filters";
import { openFromNotification, TOAST_MS, TOAST_POLL_MS } from "@/lib/sessionNotify";
import { SessionNotifications } from "./SessionNotifications";

/// Let the mocked promises settle inside `act`.
const settle = () => act(async () => {});

beforeEach(() => {
  api.pollSessionToasts.mockReset().mockResolvedValue([]);
  api.takeNotificationSession.mockReset().mockResolvedValue(null);
  toast.mockReset();
  useFilters.setState({
    view: "my-prs",
    claudeSelected: undefined,
    claudeSessionTab: "details",
  });
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

/// #1486: the phone's session notifications while the app is open.
describe("session notifications in the app", () => {
  /// **The deep link resolves.** A tapped notification's session opens
  /// that session's transcript screen, from whatever view the app was on.
  it("opens the transcript of the session whose notification was tapped", async () => {
    api.takeNotificationSession.mockResolvedValueOnce("s-1");
    render(<SessionNotifications />);
    await settle();
    const s = useFilters.getState();
    expect(s.view).toBe("claude-code");
    expect(s.claudeSelected).toBe("s-1");
    // On its Transcript tab (#1546).
    expect(s.claudeSessionTab).toBe("transcript");
    // At the "since you left" marker, not the newest message (#1484).
    expect(s.claudeTranscriptAt).toBe("marker");
    // Opening it any other way is at the newest.
    useFilters.getState().openClaudeTranscript("s-1");
    expect(useFilters.getState().claudeTranscriptAt).toBe("latest");
  });

  it("opens nothing when no notification was tapped", async () => {
    render(<SessionNotifications />);
    await settle();
    expect(useFilters.getState().view).toBe("my-prs");
  });

  /// A tap from the lock screen arrives as the page becoming visible.
  it("takes the tapped session again when the app returns to the foreground", async () => {
    render(<SessionNotifications />);
    await settle();
    api.takeNotificationSession.mockResolvedValueOnce("s-2");
    document.dispatchEvent(new Event("visibilitychange"));
    await settle();
    expect(useFilters.getState().claudeSelected).toBe("s-2");
    expect(useFilters.getState().claudeSessionTab).toBe("transcript");
  });

  /// Toasts are for OTHER sessions: the one on screen is passed as
  /// `viewing`, and the Rust side leaves it out.
  it("tells the companion which session is on screen", async () => {
    openFromNotification("s-on-screen");
    render(<SessionNotifications />);
    await settle();
    expect(api.pollSessionToasts).toHaveBeenCalledWith("s-on-screen");
  });

  it("toasts each transition, keyed by session, with an action that opens it", async () => {
    vi.useFakeTimers();
    render(<SessionNotifications />);
    await settle();
    expect(toast).not.toHaveBeenCalled();

    api.pollSessionToasts.mockResolvedValueOnce([
      { session_id: "s-4", title: "hello-world", body: "Waiting for your input" },
    ]);
    await act(async () => {
      vi.advanceTimersByTime(TOAST_POLL_MS);
    });
    expect(toast).toHaveBeenCalledTimes(1);
    const [title, opts] = toast.mock.calls[0] as [
      string,
      { id: string; description: string; duration: number; action: { onClick: () => void } },
    ];
    expect(title).toBe("hello-world");
    expect(opts.description).toBe("Waiting for your input");
    expect(opts.id).toBe("session-s-4");
    expect(opts.duration).toBe(TOAST_MS);
    opts.action.onClick();
    expect(useFilters.getState().claudeSelected).toBe("s-4");
    expect(useFilters.getState().claudeSessionTab).toBe("transcript");
  });

  /// A desktop that is away costs the toasts, not the app.
  it("shows nothing when the poll is refused", async () => {
    api.pollSessionToasts.mockRejectedValueOnce(new Error("desktop unreachable"));
    render(<SessionNotifications />);
    await settle();
    expect(toast).not.toHaveBeenCalled();
  });
});

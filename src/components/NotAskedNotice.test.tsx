import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NotAskedNotice } from "./NotAskedNotice";

const build = vi.hoisted(() => ({ mobile: false }));
vi.mock("@/lib/target", () => ({
  get IS_MOBILE_BUILD() { return build.mobile; },
  get IS_DESKTOP_BUILD() { return !build.mobile; },
}));
const relaunch = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch }));

afterEach(() => {
  build.mobile = false;
  relaunch.mockClear();
});

const NO_CREDENTIAL = "not authenticated: run `gh auth login`";
const BUDGET = "operation attempt allowance is spent";

describe("NotAskedNotice", () => {
  it("says the app did not ask, rather than that GitHub did not answer", () => {
    render(<NotAskedNotice message={NO_CREDENTIAL} />);
    expect(screen.getByText(/has not asked GitHub/)).toBeTruthy();
  });

  /// The load-bearing assertion. #1050 shipped a retry that could not
  /// work; offering one for a missing credential would reproduce it --
  /// even when the caller has a retry to hand over (#2004).
  it("offers no retry when there is no credential, even if the caller has one", () => {
    render(<NotAskedNotice message={NO_CREDENTIAL} onRetry={vi.fn()} />);
    expect(screen.queryByRole("button", { name: /try again/i })).toBeNull();
  });

  /// `role="status"`, not `role="alert"`: nothing went wrong, so a
  /// screen reader should not announce this as an error.
  it("is a status rather than an alert", () => {
    render(<NotAskedNotice message={NO_CREDENTIAL} />);
    expect(screen.getByRole("status")).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  /// #2004: the action that CAN fix a missing credential is signing in and
  /// relaunching -- the token is read once, at startup.
  it("offers sign-in and a relaunch on the desktop", () => {
    render(<NotAskedNotice message={NO_CREDENTIAL} />);
    expect(screen.getByText(/Sign in with `gh auth login`, then relaunch Headstate\./)).toBeTruthy();
    screen.getByRole("button", { name: "Relaunch Headstate" }).click();
    expect(relaunch).toHaveBeenCalledOnce();
  });

  /// The phone cannot relaunch its desktop, so it is told where to sign in
  /// and offered no button that would do something else.
  it("on the phone, names the desktop and offers no relaunch", () => {
    build.mobile = true;
    render(<NotAskedNotice message={NO_CREDENTIAL} />);
    expect(screen.getByText(/On the desktop, sign in with `gh auth login`/)).toBeTruthy();
    expect(screen.queryByRole("button")).toBeNull();
  });

  /// A budget or admission refusal CAN clear, so its retry is offered --
  /// and it is not told to sign in, which would be the wrong remedy.
  it("offers Try again for a budget refusal", () => {
    const onRetry = vi.fn();
    render(<NotAskedNotice message={BUDGET} onRetry={onRetry} />);
    screen.getByRole("button", { name: /try again/i }).click();
    expect(onRetry).toHaveBeenCalledOnce();
    expect(screen.queryByText(/sign in/i)).toBeNull();
  });

  it("offers nothing for a source that is not set up to be fetched", () => {
    render(<NotAskedNotice message=": fetching is not enabled for this source" onRetry={vi.fn()} />);
    expect(screen.queryByRole("button")).toBeNull();
  });
});

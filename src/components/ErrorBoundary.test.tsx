import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ErrorBoundary } from "./ErrorBoundary";
import { initSplash } from "../splash";

/// Regression tests for a black window on any render-time throw (#245).
///
/// There was no error boundary anywhere in the app, so a throw during
/// render unmounted the whole tree and left an EMPTY window: no message,
/// no recovery, no clue what failed. That is how #244 presented -- the
/// cause was a one-line undefined read, but the symptom was
/// indistinguishable from a hang, and diagnosing it needed the dev-server
/// console. A user on a release build has nothing to report but "it's
/// black".
function Boom({ message = "kaboom" }: { message?: string }): never {
  throw new Error(message);
}

/// React logs caught errors via console.error. Silencing keeps the suite
/// readable without hiding real failures: the assertions below are what
/// prove the boundary worked.
let spy: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  spy = vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => {
  spy.mockRestore();
  cleanup();
});

describe("ErrorBoundary", () => {
  it("renders children untouched when nothing throws", () => {
    render(
      <ErrorBoundary>
        <p>the app</p>
      </ErrorBoundary>,
    );
    expect(screen.getByText("the app")).toBeTruthy();
  });

  it("shows a message instead of a blank window when a child throws", () => {
    render(
      <ErrorBoundary>
        <Boom />
      </ErrorBoundary>,
    );
    // The whole point: SOMETHING is on screen.
    expect(screen.getByRole("alert")).toBeTruthy();
    expect(screen.getByText(/something went wrong/i)).toBeTruthy();
  });

  it("surfaces the failure text, so a user can report more than 'it's black'", () => {
    render(
      <ErrorBoundary>
        <Boom message="undefined is not an object (evaluating 'filters.sort')" />
      </ErrorBoundary>,
    );
    expect(screen.getByText(/filters\.sort/)).toBeTruthy();
  });

  // The #244 crash came from PERSISTED state, so it reproduced on every
  // launch. A reload button alone would loop forever -- reload, crash,
  // reload. Clearing the stored state is the only escape.
  it("offers a reset that clears persisted state before reloading", () => {
    const onReset = vi.fn();
    render(
      <ErrorBoundary onReset={onReset}>
        <Boom />
      </ErrorBoundary>,
    );
    fireEvent.click(screen.getByRole("button", { name: /reset/i }));
    expect(onReset).toHaveBeenCalledOnce();
  });

  // The splash is a fixed, inset-0, z-index-9999 overlay dismissed only by
  // AuthGate's settled-auth effect. A crash before that point leaves the
  // boundary's message rendered perfectly UNDERNEATH it -- which is
  // exactly the v1.0.0 hang the splash failsafe was written for.
  it("uncovers the window, so its message is not hidden by the splash", () => {
    vi.useFakeTimers();
    try {
      // Re-arm the timing against the fake clock, exactly as splash.test
      // does -- dismissal is floored at MIN_VISIBLE_MS, so it defers via
      // setTimeout rather than hiding on the spot.
      initSplash(Date.now());
      const splash = document.createElement("div");
      splash.id = "splash";
      document.body.appendChild(splash);

      render(
        <ErrorBoundary>
          <Boom />
        </ErrorBoundary>,
      );

      // Past the MIN_VISIBLE_MS floor and the fade, the overlay is
      // REMOVED, not merely faded: a fixed inset-0 element left in place
      // swallows every click even at zero opacity, including the reset
      // button this screen exists to offer.
      vi.advanceTimersByTime(3000 + 600);
      expect(document.getElementById("splash")).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });
});

/// "Report this" on the crash panel (#1148).
describe("ErrorBoundary offers a report", () => {
  it("offers Report this beside the reset", () => {
    // A crash is the error most worth reporting and the one the user is
    // least able to describe: their only route was to recall a red box
    // from memory.
    render(
      <ErrorBoundary>
        <Boom />
      </ErrorBoundary>,
    );
    expect(screen.getByRole("link", { name: /report this/i })).toBeTruthy();
    // Beside, not instead of: retrying is still the remedy.
    expect(screen.getByRole("button", { name: /reset/i })).toBeTruthy();
  });

  it("attaches the component stack to the report URL", () => {
    // THE point of this change. The stack was `console.error`'d and
    // nothing else -- invisible on a release build, where nobody has a
    // console open.
    render(
      <ErrorBoundary>
        <Boom />
      </ErrorBoundary>,
    );
    const href = screen.getByRole("link", { name: /report this/i }).getAttribute("href") ?? "";
    const body = decodeURIComponent(href);
    expect(body).toContain("### Where");
    // React names the throwing component in the stack it supplies.
    expect(body).toContain("Boom");
  });
});

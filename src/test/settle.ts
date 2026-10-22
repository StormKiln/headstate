import { act } from "@testing-library/react";
import { vi } from "vitest";

/// Let the loads a component starts on mount settle, inside `act()`,
/// before the test asserts (#1965).
///
/// A component that asks the backend for something on mount -- a query,
/// an `invoke` in an effect -- commits the answer a few ticks later. A
/// synchronous test that renders and asserts at once ends before that,
/// so it checks a frame the user only sees for a moment, and the late
/// commit lands outside `act()` (React's "not wrapped in act" warning,
/// which `src/test/setup.ts` now fails the test on).
///
/// One macrotask, not a microtask: react-query hands results to its
/// observers through `setTimeout(0)`, so a microtask flush returns
/// before the component has heard anything. `rounds` covers a load that
/// waits on another one (a viewer login, then the rows keyed by it).
/// Under fake timers it runs only what is due now, so a test's clock
/// does not move.
///
/// Not for a test that means to assert the in-flight frame: that test
/// should keep the load pending (a promise that never settles), so the
/// frame it checks is the one the user actually sees until the answer
/// arrives.
export async function settle(rounds = 3): Promise<void> {
  for (let i = 0; i < rounds; i++) {
    await act(async () => {
      if (vi.isFakeTimers()) await vi.advanceTimersByTimeAsync(0);
      else await new Promise<void>((resolve) => setTimeout(resolve, 0));
    });
  }
}

import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { HealthConditions } from "./SystemHealthPage";

const NOW = 1_700_000_000_000;
/// Old enough to trip the stale arm.
const OLD = NOW - 90 * 60_000;

const base = {
  alerts: [],
  failed: false,
  error: undefined,
  onRetry: vi.fn(),
  sampledAt: OLD,
  now: NOW,
};

/// The stale banner names a CAUSE, and there are two of them (#1145).
describe("the stale-reading banner", () => {
  it("says the app was not watching when the sampler is fine", () => {
    // The previous wording, kept: this is the common cause, and it is
    // correct whenever the sampler is not the problem.
    render(<HealthConditions {...base} samplerDegraded={false} />);
    expect(screen.getByText(/The app was not watching/)).toBeTruthy();
  });

  it("says the app WAS running when the sampler is the reason", () => {
    // The correction. An identical-looking gap is produced by an app
    // that ran the whole time and could not record, and the banner
    // asserted the reassuring cause unconditionally.
    render(<HealthConditions {...base} samplerDegraded />);
    expect(screen.getByText(/The app was running/)).toBeTruthy();
    expect(screen.queryByText(/The app was not watching/)).toBeNull();
  });

  it("keeps the previous wording when the cause is not known", () => {
    // Defaulting to the more common cause rather than to silence: the
    // sentence is still true most of the time, and dropping it would
    // lose the warning that a gap hides problems.
    render(<HealthConditions {...base} />);
    expect(screen.getByText(/The app was not watching/)).toBeTruthy();
  });
});

import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Periods } from "@/types/pr";
import { DeltaCards } from "./DeltaCards";

const periods: Periods = {
  week_current: 183,
  week_previous: 110,
  opened_week_current: 190,
  opened_week_previous: 120,
  month_current: 571,
  month_previous: 515,
};

/// The one card whose label matches, scoped to `[data-slot="card"]`.
///
/// `querySelectorAll("div")` also matches the render container, whose
/// `innerHTML` holds all four cards -- so a glyph assertion against it sees
/// every card's icon and cannot distinguish them. The existing colour
/// assertions survive that looseness only because they check a colour one
/// card alone has.
function card(container: HTMLElement, label: string): HTMLElement {
  const found = Array.from(container.querySelectorAll<HTMLElement>('[data-slot="card"]')).find(
    (c) => c.textContent?.startsWith(label),
  );
  expect(found, `no card labelled ${label}`).toBeTruthy();
  return found as HTMLElement;
}

describe("DeltaCards", () => {
  it("shows the merged count and its week-over-week change", () => {
    render(<DeltaCards periods={periods} />);
    expect(screen.getByText("183")).toBeTruthy();
    expect(screen.getByText("+66%")).toBeTruthy();
  });

  it("names the comparison window so the number is interpretable", () => {
    render(<DeltaCards periods={periods} />);
    expect(screen.getAllByText(/vs previous 7 days/i).length).toBeGreaterThan(0);
  });

  it("marks a decline with a negative percentage", () => {
    render(
      <DeltaCards periods={{ ...periods, week_current: 110, week_previous: 183 }} />,
    );
    expect(screen.getByText("-40%")).toBeTruthy();
  });

  it("renders a dash when both periods are empty", () => {
    render(<DeltaCards periods={{ ...periods, week_current: 0, week_previous: 0 }} />);
    expect(screen.getAllByText("--").length).toBeGreaterThan(0);
  });

  // #950: growth from zero is a RISE, and the glyph has to agree with the
  // word. `pctChange` returns `Infinity` when the previous period is zero
  // and the current one is not -- which is every card in a new user's
  // first week -- and the icon branch used to lump that in with "no
  // reading" and draw `Minus`. So the first thing the app said about
  // someone's work was a minus sign in front of the word "new".
  //
  // Asserted on the ARROW, not only on the text: the text was already
  // right, which is exactly why this shipped unnoticed.
  it("marks growth from zero as a rise, not a decrease", () => {
    const { container } = render(
      <DeltaCards periods={{ ...periods, week_current: 12, week_previous: 0 }} />,
    );
    expect(screen.getByText("new")).toBeTruthy();
    // lucide renders the icon name as a class, so the glyph is checkable.
    const merged = card(container, "Merged this week");
    expect(merged.innerHTML).toContain("lucide-arrow-up");
    expect(merged.innerHTML).not.toContain("lucide-minus");
  });

  // The no-reading case keeps `Minus`, and that is the point of splitting
  // them: `previous === 0` AND `current === 0` has no direction at all, so
  // an arrow either way would invent one.
  it("keeps the dash glyph when there is genuinely nothing to compare", () => {
    const { container } = render(
      <DeltaCards periods={{ ...periods, week_current: 0, week_previous: 0 }} />,
    );
    const merged = card(container, "Merged this week");
    expect(merged.innerHTML).toContain("lucide-minus");
    expect(merged.innerHTML).not.toContain("lucide-arrow");
  });

  // Colour follows direction, but polarity still governs: a brand-new
  // user's rising INTAKE must not be cheered green just because it is new.
  it("does not paint a new open count green", () => {
    const { container } = render(
      <DeltaCards
        periods={{ ...periods, opened_week_current: 40, opened_week_previous: 0 }}
      />,
    );
    expect(card(container, "Opened this week").innerHTML).not.toContain("#3fb950");
  });

  // A rising intake is not good news on its own: the activity chart's own
  // comment names the GAP between opened and merged as the signal. One
  // polarity rule across all four cards painted a growing backlog green.
  it("does not paint a rising open count green", () => {
    const { container } = render(
      <DeltaCards
        periods={{ ...periods, opened_week_current: 300, opened_week_previous: 120 }}
      />,
    );
    const opened = Array.from(container.querySelectorAll("div")).find(
      (d) => d.textContent?.startsWith("Opened this week"),
    );
    expect(opened).toBeTruthy();
    expect(opened?.innerHTML).not.toContain("#3fb950");
  });

  // Merged throughput still earns colour -- more IS better there.
  it("still paints rising merges green", () => {
    const { container } = render(<DeltaCards periods={periods} />);
    const merged = Array.from(container.querySelectorAll("div")).find(
      (d) => d.textContent?.startsWith("Merged this week"),
    );
    expect(merged?.innerHTML).toContain("#3fb950");
  });

  // Replaces a second copy of "Median cycle time", which already appears
  // with a p90 in the insight row and had no prior period to compare to.
  it("shows net backlog instead of duplicating cycle time", () => {
    render(<DeltaCards periods={periods} />);
    expect(screen.getByText(/net backlog/i)).toBeTruthy();
    expect(screen.getByText("+7")).toBeTruthy(); // 190 opened - 183 merged
    expect(screen.queryByText(/median cycle time/i)).toBeNull();
  });
});

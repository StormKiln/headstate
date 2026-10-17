import { render, screen, cleanup } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import { CorpusProgress } from "./CorpusProgress";
import type { PopulationProgress } from "../types/pr";
afterEach(cleanup);
it.each([
  ["cancelled", "Scan cancelled"],
  ["timed_out", "Scan reached its time limit"],
  ["deferred", "Scan has work remaining"],
  ["failed", "Scan encountered failures"],
  ["invalidated", "Files changed during the scan"],
] as const)("reports %s without a complete denominator", (status, label) => {
  const progress: PopulationProgress = { status, total: 5, total_known: false, completed: 2, unchanged: 1, failed: 1, remaining: 2 };
  render(<CorpusProgress progress={progress} />);
  const text = screen.getByRole("status").textContent;
  expect(text).toContain(label);
  expect(text).toContain("2 sessions current; total not yet known");
  expect(text).toContain("2 remaining");
  expect(text).toContain("1 failed");
});

it("distinguishes discovery from measured usage while running", () => {
  render(<CorpusProgress progress={{ phase: "discovery", status: "running", total: 6, total_known: false, completed: 2, unchanged: 0, failed: 0, remaining: 4 }} />);
  expect(screen.getByRole("status").textContent).toContain("Discovering sessions");
  expect(screen.getByRole("status").textContent).toContain("2 sessions inspected; total not yet known");
  expect(screen.getByRole("status").textContent).not.toContain("sessions current");
});

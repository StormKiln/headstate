import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ViewErrorBoundary } from "../components/ViewErrorBoundary";
import { expectViewCrash, takeUnexpectedViewCrashes } from "./viewCrashGuard";

function Boom(): React.ReactElement {
  throw new Error("render exploded");
}

/// The guard keys on the boundary's log line, so these render the REAL
/// boundary: rewording that line fails here instead of quietly turning
/// the suite-wide guard off.
describe("viewCrashGuard", () => {
  it("sees a crash the real ViewErrorBoundary catches", () => {
    const seen = expectViewCrash();
    render(
      <ViewErrorBoundary view="Worktrees">
        <Boom />
      </ViewErrorBoundary>,
    );
    expect(screen.getByText("Something went wrong in Worktrees")).toBeTruthy();
    expect(seen.caught).toHaveLength(1);
    expect(seen.caught[0]).toContain("Worktrees");
    expect(seen.caught[0]).toContain("render exploded");
  });

  it("reports an undeclared crash, and only once", () => {
    // Through the guard setup.ts installed for this test. Taking the
    // crash here also clears it, so the afterEach that would otherwise
    // fail this test finds nothing -- which is the "only once" half.
    console.error("Unhandled render error in Docker:", new Error("dead"), "stack");
    expect(takeUnexpectedViewCrashes()).toEqual(["Unhandled render error in Docker: dead"]);
    expect(takeUnexpectedViewCrashes()).toEqual([]);
  });

  it("ignores console errors that are not a boundary catch", () => {
    console.error("Could not do the thing", new Error("nope"));
    expect(takeUnexpectedViewCrashes()).toEqual([]);
  });
});

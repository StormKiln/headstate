import { render, screen } from "@testing-library/react";
import { useEffect, useState } from "react";
import { describe, expect, it } from "vitest";
import { ViewErrorBoundary } from "../components/ViewErrorBoundary";
import { settle } from "./settle";
import { expectViewCrash, takeActWarnings, takeUnexpectedViewCrashes } from "./viewCrashGuard";

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
    expect(takeActWarnings()).toEqual([]);
  });
});

/// A component whose mount-time "load" commits a tick after mounting:
/// the shape behind every act() warning #1965 counted.
function LateLoad() {
  const [text, setText] = useState("loading");
  useEffect(() => {
    void Promise.resolve().then(() => setText("loaded"));
  }, []);
  return <p>{text}</p>;
}

/// The act() half keys on React's own warning text, so these provoke a
/// REAL warning from React: a reworded warning fails here rather than
/// silently turning the suite-wide failure off.
describe("act() warnings", () => {
  it("records the update React reports outside act(), by component", async () => {
    render(<LateLoad />);
    // Let the load land OUTSIDE act(): exactly what a sync test that
    // renders and returns does.
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    // Taking the warning here also clears it, so the afterEach that
    // would otherwise fail this test finds nothing.
    expect(takeActWarnings()).toEqual(["An update to LateLoad was not wrapped in act()"]);
    expect(takeActWarnings()).toEqual([]);
  });

  it("records nothing when the test awaits the settled state", async () => {
    render(<LateLoad />);
    await settle();
    expect(screen.getByText("loaded")).toBeTruthy();
    expect(takeActWarnings()).toEqual([]);
  });
});

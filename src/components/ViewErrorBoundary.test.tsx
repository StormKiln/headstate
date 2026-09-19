import { render, screen, fireEvent } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ViewErrorBoundary } from "./ViewErrorBoundary";
// `?raw` rather than `node:fs`: this project deliberately carries no
// `@types/node`, so `readFileSync` does not typecheck here. Vite's raw
// import gives the same bytes and is what `App.lazy.test.tsx` uses.
import boundarySource from "./ViewErrorBoundary.tsx?raw";

/// React logs every caught error to `console.error` itself, on top of
/// the boundary's own log. Silenced so a deliberate throw does not look
/// like a failing run, and restored after.
let errorSpy: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => {
  errorSpy.mockRestore();
});

function Boom({ error }: { error: Error }): React.ReactElement {
  throw error;
}

describe("ViewErrorBoundary", () => {
  it("renders its children when nothing throws", () => {
    render(
      <ViewErrorBoundary view="Docker images">
        <p>the view</p>
      </ViewErrorBoundary>,
    );
    expect(screen.getByText("the view")).toBeTruthy();
  });

  it("names the view that failed", () => {
    // The point of the smaller blast radius: the chrome around this is
    // still alive, so "something went wrong" alone does not say which
    // pane is dead.
    render(
      <ViewErrorBoundary view="Docker images">
        <Boom error={new Error("render blew up")} />
      </ViewErrorBoundary>,
    );
    expect(screen.getByText(/Docker images/)).toBeTruthy();
    expect(screen.getByText("render blew up")).toBeTruthy();
  });

  it("says a failed lazy chunk needs a relaunch, not a reset", () => {
    // A rejected `import()` throws past `Suspense` to here, and the
    // remedy is a relaunch after an update -- telling the user to reset
    // state sends them to do something that cannot help.
    const chunk = new Error("Failed to fetch dynamically imported module: /assets/Docker.js");
    render(
      <ViewErrorBoundary view="Docker images">
        <Boom error={chunk} />
      </ViewErrorBoundary>,
    );
    expect(screen.getByText(/could not be downloaded/)).toBeTruthy();
    expect(screen.getByText(/relaunching/)).toBeTruthy();
    // And NOT the generic arm -- the two messages are different claims.
    expect(screen.queryByText(/Something went wrong/)).toBeNull();
  });

  it("recognises a ChunkLoadError by name, not only by message", () => {
    // Different bundlers word this differently; the `name` is the part
    // that is stable.
    const e = new Error("boom");
    e.name = "ChunkLoadError";
    render(
      <ViewErrorBoundary view="Artifacts">
        <Boom error={e} />
      </ViewErrorBoundary>,
    );
    expect(screen.getByText(/could not be downloaded/)).toBeTruthy();
  });

  it("remounts the view on retry", () => {
    // The whole reason this boundary exists rather than leaning on the
    // root one: it recovers the broken view IN PLACE, keeping every
    // other view and the chrome around it.
    //
    // `broken` is external state flipped by the test, NOT a
    // render-counter. React 19 retries a failed concurrent render
    // synchronously before handing the error to a boundary, so a child
    // that throws only on its first render succeeds on React's own
    // retry and the panel never appears -- the test would then pass
    // while proving nothing about the retry button.
    let broken = true;
    function Flaky(): React.ReactElement {
      if (broken) throw new Error("transient");
      return <p>recovered</p>;
    }
    render(
      <ViewErrorBoundary view="Branches">
        <Flaky />
      </ViewErrorBoundary>,
    );
    expect(screen.getByText("transient")).toBeTruthy();
    // Still broken: retrying must re-render, and re-throw.
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(screen.getByText("transient")).toBeTruthy();
    broken = false;
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(screen.getByText("recovered")).toBeTruthy();
  });

  it("remounts rather than re-rendering, discarding the view's own state", () => {
    // What the `attempt` key buys, and the case a bare
    // `setState({error: null})` does NOT recover: a view holding state
    // that is ITSELF the problem. Re-rendering hands the component back
    // the same bad state and it throws again; only a remount re-runs
    // the initialiser.
    //
    // `poison` is external and flipped by the test rather than derived
    // from a mount counter: React 19 retries a failed concurrent render
    // synchronously, re-running `useState` initialisers, so a counter
    // would "recover" on React's own retry and never reach the
    // boundary at all.
    let poison = true;
    let inits = 0;
    function Stateful(): React.ReactElement {
      const [bad] = useState(() => {
        inits += 1;
        return poison;
      });
      if (bad) throw new Error("bad state");
      return <p>fresh state</p>;
    }
    render(
      <ViewErrorBoundary view="Docker images">
        <Stateful />
      </ViewErrorBoundary>,
    );
    expect(screen.getByText("bad state")).toBeTruthy();

    // The source of the bad state is gone, but a MOUNTED component
    // would still be holding its copy. Only a remount re-reads it.
    poison = false;
    const before = inits;
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(screen.getByText("fresh state")).toBeTruthy();
    expect(inits).toBeGreaterThan(before);
  });

  it("never reloads the window", () => {
    // Asserted against the SOURCE, not a spy: jsdom defines both
    // `window.location` and its `reload` non-configurable, so neither
    // can be replaced or spied on, and a reload there is a silent
    // no-op that a behavioural test cannot distinguish from success.
    //
    // This is the load-bearing difference from the root boundary --
    // reloading would throw away every working view to recover one --
    // so it gets a guard rather than a comment.
    const code = boundarySource
      .replace(/\/\/.*$/gm, "")
      .replace(/\/\*[\s\S]*?\*\//g, "");
    expect(code).not.toMatch(/location\s*\.\s*reload/);
    expect(code).not.toMatch(/location\s*\.\s*(href|assign|replace)\s*=/);
  });

  it("does not clear persisted state", () => {
    // The root boundary's remedy drops `PERSIST_KEY`. This one must
    // not: the user's filters had nothing to do with the throw, and a
    // per-view failure is not a reason to lose them.
    localStorage.setItem("headstate-filters", "{}");
    render(
      <ViewErrorBoundary view="Worktrees">
        <Boom error={new Error("nope")} />
      </ViewErrorBoundary>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(localStorage.getItem("headstate-filters")).toBe("{}");
    localStorage.removeItem("headstate-filters");
  });

  it("clears the error when the parent switches views", () => {
    // `App` keys this by view. Without the key the boundary's error
    // state would survive the switch and pin the dead panel over a
    // perfectly good page.
    function Switcher(): React.ReactElement {
      const [view, setView] = useState("Docker images");
      return (
        <>
          <button type="button" onClick={() => setView("Branches")}>
            go
          </button>
          <ViewErrorBoundary key={view} view={view}>
            {view === "Docker images" ? <Boom error={new Error("dead")} /> : <p>branches page</p>}
          </ViewErrorBoundary>
        </>
      );
    }
    render(<Switcher />);
    expect(screen.getByText("dead")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "go" }));
    expect(screen.getByText("branches page")).toBeTruthy();
    expect(screen.queryByText("dead")).toBeNull();
  });

  it("logs the component stack", () => {
    // On a release build the console is the only record of where the
    // throw came from.
    render(
      <ViewErrorBoundary view="Packages">
        <Boom error={new Error("traced")} />
      </ViewErrorBoundary>,
    );
    const logged = errorSpy.mock.calls.some(
      (c: unknown[]) => typeof c[0] === "string" && c[0].includes("Packages"),
    );
    expect(logged).toBe(true);
  });
});

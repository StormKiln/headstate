import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { NOT_ASKED } from "@/lib/notAsked";
import { QueryError } from "./QueryError";

describe("QueryError", () => {
  it("shows a failure as an alert, with a retry", () => {
    render(<QueryError title="Could not load" message="timed out" onRetry={() => {}} />);
    expect(screen.getByRole("alert")).toBeTruthy();
    expect(screen.getByRole("button", { name: /try again/i })).toBeTruthy();
  });

  /// #1124: the delegation. A rejection the app never issued must not
  /// render as a failed request, wherever it surfaces -- and every page
  /// that renders a query failure routes through here, which is why the
  /// decision lives at this seam rather than at a dozen call sites.
  it("renders a not-asked rejection as a status, not a failure", () => {
    render(
      <QueryError
        title="Could not load"
        message={`${NOT_ASKED} not authenticated: run \`gh auth login\``}
        onRetry={() => {}}
      />,
    );
    expect(screen.getByRole("status")).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  /// The retry is withheld even though `onRetry` was passed. The caller
  /// does not know the difference; this component does.
  it("withholds a retry that cannot work", () => {
    const onRetry = vi.fn();
    render(
      <QueryError title="Could not load" message={`${NOT_ASKED} not authenticated`} onRetry={onRetry} />,
    );
    expect(screen.queryByRole("button", { name: /try again/i })).toBeNull();
  });

  /// The marker is a wire detail. `cancelled.ts` exists because one
  /// reached a toast.
  it("never shows the marker itself", () => {
    render(<QueryError title="Could not load" message={`${NOT_ASKED} not authenticated`} />);
    expect(screen.queryByText(new RegExp(NOT_ASKED))).toBeNull();
  });
});

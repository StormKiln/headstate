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

/// The opt-in report link (#1148).
describe("QueryError and Report this", () => {
  it("offers no report by default", () => {
    // OPT-IN on purpose: "no network" and "your token expired" are the
    // user's to fix, and a Report link on those invites issues that can
    // only be closed with "this is working as intended".
    render(<QueryError title="Could not load" message="offline" onRetry={() => {}} />);
    expect(screen.queryByRole("link", { name: /report this/i })).toBeNull();
  });

  it("offers one when the caller asks", () => {
    render(<QueryError title="Could not load" message="boom" report onRetry={() => {}} />);
    expect(screen.getByRole("link", { name: /report this/i })).toBeTruthy();
  });

  it("puts the retry first, because it is the remedy to try first", () => {
    render(<QueryError title="Could not load" message="boom" report onRetry={() => {}} />);
    const retry = screen.getByRole("button", { name: /try again/i });
    const report = screen.getByRole("link", { name: /report this/i });
    // `DOCUMENT_POSITION_FOLLOWING` === 4: report comes after retry.
    expect(retry.compareDocumentPosition(report) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("reports the message, not the title", () => {
    // The title is the app's framing; the message is what actually
    // failed, and it is what a maintainer needs.
    render(<QueryError title="Could not load" message="ECONNREFUSED 127.0.0.1" report />);
    const href = screen.getByRole("link", { name: /report this/i }).getAttribute("href") ?? "";
    expect(decodeURIComponent(href)).toContain("ECONNREFUSED");
  });

  it("falls back to the title when there is no message", () => {
    // A report saying only "an error occurred" is worse than one naming
    // the panel the user was looking at.
    render(<QueryError title="Could not load the sessions" report />);
    const href = screen.getByRole("link", { name: /report this/i }).getAttribute("href") ?? "";
    expect(decodeURIComponent(href)).toContain("Could not load the sessions");
  });

  it("carries the view and the diagnostics state when given them", () => {
    render(
      <QueryError
        title="t"
        message="boom"
        report
        reportView="Docker images"
        reportDiagnostics={false}
      />,
    );
    const body = decodeURIComponent(
      screen.getByRole("link", { name: /report this/i }).getAttribute("href") ?? "",
    );
    expect(body).toContain("On the Docker images view");
    expect(body).toContain("Diagnostic logging was off");
  });

  it("says nothing about diagnostics when the caller does not know", () => {
    // Three states, not two (#1042): unknown is not off.
    render(<QueryError title="t" message="boom" report reportView="Docker images" />);
    const body = decodeURIComponent(
      screen.getByRole("link", { name: /report this/i }).getAttribute("href") ?? "",
    );
    expect(body).not.toContain("Diagnostic logging");
  });
});

import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { NotAskedNotice } from "./NotAskedNotice";

describe("NotAskedNotice", () => {
  it("says the app did not ask, rather than that GitHub did not answer", () => {
    render(<NotAskedNotice message="not authenticated: run `gh auth login`" />);
    expect(screen.getByText(/has not asked GitHub/)).toBeTruthy();
  });

  /// The load-bearing assertion. #1050 shipped a retry that could not
  /// work; offering one here would reproduce it.
  it("offers no retry", () => {
    render(<NotAskedNotice message="not authenticated" />);
    expect(screen.queryByRole("button", { name: /try again/i })).toBeNull();
  });

  /// `role="status"`, not `role="alert"`: nothing went wrong, so a
  /// screen reader should not announce this as an error.
  it("is a status rather than an alert", () => {
    render(<NotAskedNotice message="not authenticated" />);
    expect(screen.getByRole("status")).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("shows the remedy", () => {
    render(<NotAskedNotice message="not authenticated: run `gh auth login`" />);
    expect(screen.getByText(/gh auth login/)).toBeTruthy();
  });
});

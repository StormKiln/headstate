import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { PartialScanNotice } from "./PartialScanNotice";

describe("PartialScanNotice", () => {
  it("keeps thousands of diagnostics collapsed and makes every reason reachable in bounded pages", () => {
    const reasons = Array.from({ length: 1583 }, (_, i) => `folder-${i}: discovery depth limit reached`);
    render(<PartialScanNotice unreadable={reasons} consequence="counts may be incomplete." />);
    expect(screen.queryAllByRole("listitem")).toHaveLength(0);
    expect(screen.getByRole("alert").textContent).toContain("Scan incomplete");
    expect(screen.queryByText(/could not be read/)).toBeNull();
    const toggle = screen.getByRole("button", { name: "Show details" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(toggle);
    expect(screen.getAllByRole("listitem")).toHaveLength(50);
    const diagnostics = screen.getByRole("list", { name: "Scan diagnostics" });
    expect(diagnostics.tabIndex).toBe(0);
    diagnostics.focus();
    expect(document.activeElement).toBe(diagnostics);
    expect(screen.getByText(reasons[0])).toBeTruthy();
    for (let page = 1; page <= 31; page++) {
      fireEvent.click(screen.getByRole("button", { name: "Next" }));
      expect(screen.getAllByRole("listitem").length).toBeLessThanOrEqual(50);
    }
    expect(screen.getByText(reasons[1582])).toBeTruthy();
    expect((screen.getByRole("button", { name: "Next" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Previous" }));
    expect(screen.getByText(reasons[1500])).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Hide details" }));
    expect(screen.queryAllByRole("listitem")).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: "Show details" }));
    expect(screen.getByText(reasons[0])).toBeTruthy();
  });

  it("retains small reports and check-specific wording", () => {
    render(<PartialScanNotice unreadable={["check: permission denied"]} checks={{ total: 4 }} consequence="advice is incomplete." />);
    expect(screen.getByText(/1 of 4 checks could not run/)).toBeTruthy();
    expect(screen.getByText("check: permission denied")).toBeTruthy();
  });

  it("keeps the selected page valid when a refresh reduces the report", () => {
    const reasons = Array.from({ length: 120 }, (_, i) => `path-${i}: unavailable`);
    const { rerender } = render(<PartialScanNotice unreadable={reasons} consequence="counts may be incomplete." />);
    fireEvent.click(screen.getByRole("button", { name: "Show details" }));
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    rerender(<PartialScanNotice unreadable={reasons.slice(0, 4)} consequence="counts may be incomplete." />);
    expect(screen.getAllByRole("listitem")).toHaveLength(4);
    expect(screen.getByText(reasons[0])).toBeTruthy();
    rerender(<PartialScanNotice unreadable={[]} consequence="counts may be incomplete." />);
    expect(screen.queryByRole("alert")).toBeNull();
  });
});


it("describes a grouped traversal boundary without claiming unreadable files", () => {
  const message = "root: 128 depth boundaries; choose a narrower scan root";
  render(<PartialScanNotice unreadable={[message]} diagnostics={[{ root: "root", path: "root/deep", kind: "depthLimit", count: 128, message }]} consequence="counts may be incomplete." compact />);
  expect(screen.getByText("Discovery limited. Some repositories may be missing.")).toBeTruthy();
  expect(screen.queryByText(message)).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Show details" }));
  expect(screen.getByText(message)).toBeTruthy();
});

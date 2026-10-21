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

describe("a scan root that is itself a repository (#1948)", () => {
  const boundary = {
    root: "/home/me/code",
    path: "/home/me/code",
    kind: "repositoryBoundary" as const,
    count: 1,
    message: "/home/me/code: this scan root is itself a repository",
  };

  it("qualifies the count with a note, not an alert, when nothing was unreadable", () => {
    render(<PartialScanNotice unreadable={[]} diagnostics={[boundary]} consequence="the counts below are floors rather than totals." />);
    expect(screen.queryByRole("alert")).toBeNull();
    const note = screen.getByRole("note");
    expect(note.textContent).toContain(
      "The scan root /home/me/code is itself a repository, and repositories nested inside it are not listed, so the counts below are floors rather than totals.",
    );
    expect(note.textContent).toContain("add its folder as a scan root in Settings > Repositories");
  });

  it("names the root briefly in a compact sidebar", () => {
    render(<PartialScanNotice unreadable={[]} diagnostics={[boundary]} consequence="unused" compact />);
    expect(screen.getByRole("note").textContent).toContain(
      "The scan root code is itself a repository; repositories nested inside it are not listed.",
    );
  });

  it("does not count the boundary as a read failure beside real limits", () => {
    const message = "root: 128 depth boundaries";
    render(
      <PartialScanNotice
        unreadable={[message]}
        diagnostics={[{ root: "root", path: "root/deep", kind: "depthLimit", count: 128, message }, boundary]}
        consequence="counts may be incomplete."
      />,
    );
    expect(screen.getByRole("alert").textContent).toContain("Discovery limited");
    expect(screen.getByRole("note")).toBeTruthy();
  });

  it("renders nothing when there is neither a shortfall nor a boundary", () => {
    const { container } = render(<PartialScanNotice unreadable={[]} diagnostics={[]} consequence="x" />);
    expect(container.textContent).toBe("");
  });
});

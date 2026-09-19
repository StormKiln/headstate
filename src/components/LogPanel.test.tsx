import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const copyFn = vi.hoisted(() => vi.fn(() => Promise.resolve(null as string | null)));
const refetchFn = vi.hoisted(() => vi.fn());
const toastSuccess = vi.hoisted(() => vi.fn());
const toastError = vi.hoisted(() => vi.fn());
const state = vi.hoisted(() => ({
  data: undefined as unknown,
  isLoading: false,
  isError: false,
  isFetching: false,
  error: undefined as unknown,
  /// Whether the hook was asked to run, which is what proves the panel
  /// does not read the log until it is opened.
  enabled: false,
}));

vi.mock("@/api/hooks", () => ({
  useLogTail: (enabled: boolean) => {
    state.enabled = enabled;
    return {
      data: state.data,
      isLoading: state.isLoading,
      isError: state.isError,
      isFetching: state.isFetching,
      error: state.error,
      refetch: refetchFn,
    };
  },
}));
vi.mock("@/lib/clipboard", () => ({ copyText: copyFn }));
vi.mock("sonner", () => ({ toast: { success: toastSuccess, error: toastError } }));

import { LogPanel } from "./LogPanel";

const tail = (over: Record<string, unknown> = {}) => ({
  text: "a log line\nanother\n",
  offset: 0,
  total: 19,
  truncated: false,
  path: "/Users/acme/Library/Logs/headstate/headstate.log",
  ...over,
});

beforeEach(() => {
  Object.assign(state, {
    data: tail(),
    isLoading: false,
    isError: false,
    isFetching: false,
    error: undefined,
    enabled: false,
  });
  copyFn.mockClear();
  refetchFn.mockClear();
  toastSuccess.mockClear();
});

describe("LogPanel", () => {
  it("reads nothing until it is opened", () => {
    // Collapsed by default is not a styling choice: reading the tail
    // costs a file read and, on a paired phone, a payload over the
    // transport. A user who never opens it should pay for neither.
    render(<LogPanel />);
    expect(state.enabled).toBe(false);
    expect(screen.queryByText(/a log line/)).toBeNull();
  });

  it("reads and shows the log once opened", () => {
    render(<LogPanel />);
    fireEvent.click(screen.getByRole("button", { name: /read the log here/i }));
    expect(state.enabled).toBe(true);
    expect(screen.getByText(/a log line/)).toBeTruthy();
  });

  it("says a truncated tail is an excerpt, and of how much", () => {
    // THE claim this panel must get right. A panel that silently shows
    // the tail is one where a user scrolls to the top, sees no error
    // and concludes there was none.
    state.data = tail({ truncated: true, offset: 4_194_304 - 65_536, total: 4_194_304 });
    render(<LogPanel />);
    fireEvent.click(screen.getByRole("button", { name: /read the log here/i }));
    expect(screen.getByText(/Showing the last 64 KB of 4\.0 MB/)).toBeTruthy();
  });

  it("says so when it is showing the whole file", () => {
    // The other half: an untruncated log must NOT be described as an
    // excerpt, or the user goes looking for lines that do not exist.
    render(<LogPanel />);
    fireEvent.click(screen.getByRole("button", { name: /read the log here/i }));
    expect(screen.getByText(/Showing all 19 B/)).toBeTruthy();
    expect(screen.queryByText(/Showing the last/)).toBeNull();
  });

  it("distinguishes an empty log from a truncated one", () => {
    state.data = tail({ text: "", total: 0 });
    render(<LogPanel />);
    fireEvent.click(screen.getByRole("button", { name: /read the log here/i }));
    expect(screen.getByText(/The log is empty/)).toBeTruthy();
  });

  it("names the file it is showing", () => {
    // So a user filing a bug can say which machine and which file, and
    // so the reveal button and this panel are talking about the same
    // thing.
    render(<LogPanel />);
    fireEvent.click(screen.getByRole("button", { name: /read the log here/i }));
    expect(screen.getByText(/headstate\.log/)).toBeTruthy();
  });

  it("copies the text, not the heading", () => {
    // The copy button exists to feed a bug report; pasting "Showing the
    // last 64 KB of…" into one would be noise.
    render(<LogPanel />);
    fireEvent.click(screen.getByRole("button", { name: /read the log here/i }));
    fireEvent.click(screen.getByRole("button", { name: /^copy$/i }));
    expect(copyFn).toHaveBeenCalledWith("a log line\nanother\n");
  });

  it("reports a read failure rather than an empty panel", () => {
    // An empty <pre> would read as "the log is empty", which is a
    // different claim from "the log could not be read".
    state.isError = true;
    state.error = "No log has been written yet (/tmp/x). It appears once something is logged.";
    state.data = undefined;
    render(<LogPanel />);
    fireEvent.click(screen.getByRole("button", { name: /read the log here/i }));
    expect(screen.getByText(/Could not show the log/)).toBeTruthy();
    expect(screen.getByText(/No log has been written yet/)).toBeTruthy();
  });

  it("offers a manual refresh rather than polling", () => {
    render(<LogPanel />);
    fireEvent.click(screen.getByRole("button", { name: /read the log here/i }));
    fireEvent.click(screen.getByRole("button", { name: /^refresh$/i }));
    expect(refetchFn).toHaveBeenCalledOnce();
  });

  it("collapses again, and stops reading", () => {
    render(<LogPanel />);
    fireEvent.click(screen.getByRole("button", { name: /read the log here/i }));
    expect(state.enabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: /hide the log text/i }));
    expect(state.enabled).toBe(false);
  });
});

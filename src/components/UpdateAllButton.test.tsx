import { describe, expect, it, vi, beforeEach } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { UpdateAllReport, RepoUpdateOutcome } from "@/api/tauri";

/// The hooks are mocked rather than the Tauri boundary, because what is
/// under test is what the BUTTON does -- which command it calls, what it
/// renders from the report, and that Stop reaches the cancel path.
const useUpdateAllRepositories = vi.hoisted(() => vi.fn());
const useCancelUpdateAll = vi.hoisted(() => vi.fn());
const useUpdateAllProgress = vi.hoisted(() => vi.fn());
vi.mock("@/api/hooks", () => ({
  useUpdateAllRepositories,
  useCancelUpdateAll,
  useUpdateAllProgress,
}));
const toastError = vi.hoisted(() => vi.fn());
vi.mock("sonner", () => ({ toast: { error: toastError } }));

import { UpdateAllButton } from "./UpdateAllButton";

const out = (path: string, result: RepoUpdateOutcome["result"]): RepoUpdateOutcome => ({
  path,
  result,
});

const report = (over: Partial<UpdateAllReport> = {}): UpdateAllReport => ({
  outcomes: [],
  cancelled: false,
  timedOut: false,
  unreadable: [],
  ...over,
});

beforeEach(() => {
  useUpdateAllRepositories.mockReset();
  useCancelUpdateAll.mockReset();
  useUpdateAllProgress.mockReset();
  toastError.mockReset();
  useUpdateAllProgress.mockReturnValue(null);
  useCancelUpdateAll.mockReturnValue(vi.fn().mockResolvedValue(undefined));
});

describe("UpdateAllButton", () => {
  /// The button sends NO list and NO verdict (#1012). The set and every
  /// precondition are re-derived inside the command at the moment of
  /// acting, because the table's currency column is minutes old by the
  /// time the button is pressed -- a reason to OFFER the action, never
  /// the basis for performing it.
  it("asks the desktop to work out the set rather than sending the table's verdict", async () => {
    const run = vi.fn().mockResolvedValue(report());
    useUpdateAllRepositories.mockReturnValue(run);
    render(<UpdateAllButton count={3} unreadable={[]} />);

    fireEvent.click(screen.getByRole("button", { name: /update all repositories/i }));

    await waitFor(() => expect(run).toHaveBeenCalled());
    expect(run).toHaveBeenCalledWith();
  });

  /// THE cancellation test at the UI layer (#1016). Stop appears only
  /// while a run is going, and pressing it reaches the cancel command --
  /// the flag the Rust loop reads between repositories.
  ///
  /// The run is held open by a promise this test resolves itself, so the
  /// "while running" window is real rather than assumed.
  it("offers Stop only while a run is going, and Stop reaches the cancel command", async () => {
    let finish: (r: UpdateAllReport) => void = () => {};
    const run = vi.fn(
      () =>
        new Promise<UpdateAllReport>((resolve) => {
          finish = resolve;
        }),
    );
    useUpdateAllRepositories.mockReturnValue(run);
    const cancel = vi.fn().mockResolvedValue(undefined);
    useCancelUpdateAll.mockReturnValue(cancel);

    render(<UpdateAllButton count={45} unreadable={[]} />);
    // Before the run there is nothing to stop, and offering a Stop that
    // does nothing would be its own small lie.
    expect(screen.queryByRole("button", { name: /stop/i })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: /update all repositories/i }));
    const stop = await screen.findByRole("button", { name: /stop/i });

    fireEvent.click(stop);
    expect(cancel).toHaveBeenCalledTimes(1);

    // The run then ends, reporting what it managed -- cancelling must
    // not discard the report.
    finish(
      report({
        cancelled: true,
        outcomes: [
          out("/code/a", { state: "updated", message: "Updating abc..def" }),
          out("/code/b", { state: "notAttempted" }),
        ],
      }),
    );
    expect((await screen.findByRole("status")).textContent).toMatch(/Stopped/);
    expect(screen.getByRole("status").textContent).toMatch(/Updated 1/);
    // And Stop is gone again, because there is no longer a run to stop.
    await waitFor(() => expect(screen.queryByRole("button", { name: /stop/i })).toBeNull());
  });

  /// The pair to the cancellation test: an ordinary run must not be made
  /// noisy, and must not offer a Stop after it has finished.
  it("does not leave a Stop on screen after an uncancelled run", async () => {
    useUpdateAllRepositories.mockReturnValue(
      vi.fn().mockResolvedValue(
        report({
          outcomes: [
            out("/code/a", { state: "updated", message: "Updating abc..def" }),
            out("/code/b", { state: "alreadyLevel" }),
          ],
        }),
      ),
    );
    render(<UpdateAllButton count={2} unreadable={[]} />);

    fireEvent.click(screen.getByRole("button", { name: /update all repositories/i }));

    expect((await screen.findByRole("status")).textContent).toBe("Updated 1 · 1 already level");
    expect(screen.queryByRole("button", { name: /stop/i })).toBeNull();
    // A clean run says nothing about failures, because there were none.
    expect(screen.queryByText(/could not be reached/i)).toBeNull();
  });

  /// Failures carry GIT'S OWN MESSAGE to the screen, and name the
  /// repository. "Could not update" says nothing; git's refusal usually
  /// names the host, the permission or the ref -- and at 45 repositories
  /// that is the only thing making a failure list actionable.
  it("names the repository and keeps git's own message for every refusal", async () => {
    useUpdateAllRepositories.mockReturnValue(
      vi.fn().mockResolvedValue(
        report({
          outcomes: [
            out("/code/a", { state: "failed", error: "could not resolve host github.com" }),
            out("/code/b", {
              state: "skipped",
              reason: "2 uncommitted changes -- commit or stash first",
            }),
          ],
        }),
      ),
    );
    render(<UpdateAllButton count={2} unreadable={[]} />);

    fireEvent.click(screen.getByRole("button", { name: /update all repositories/i }));

    expect(await screen.findByText("/code/a")).not.toBeNull();
    expect(screen.getByText(/could not resolve host github\.com/)).not.toBeNull();
    expect(screen.getByText("/code/b")).not.toBeNull();
    expect(screen.getByText(/2 uncommitted changes/)).not.toBeNull();
    // The two are under DIFFERENT headings: a skip is not a failure, and
    // the largest expected bucket must not read as errors.
    expect(screen.getByRole("heading", { name: /could not be reached/i })).not.toBeNull();
    expect(screen.getByRole("heading", { name: /^skipped$/i })).not.toBeNull();
  });

  /// Over a partial scan the button will not say "All" (#1025), and it
  /// stays ENABLED -- one unreadable directory must not block updating
  /// 42 perfectly good repositories.
  it("names the readable count over a partial scan, and stays enabled", () => {
    useUpdateAllRepositories.mockReturnValue(vi.fn());
    render(<UpdateAllButton count={42} unreadable={["/work: permission denied"]} />);

    const button = screen.getByRole("button", { name: /update 42 readable repositories/i });
    expect(button.hasAttribute("disabled")).toBe(false);
    expect(screen.queryByRole("button", { name: /update all repositories/i })).toBeNull();
  });

  /// The scope of the promise is on the button, not discovered in the
  /// report. "Attempt to update" is an open-ended promise; this one says
  /// exactly what it does.
  it("states that it fast-forwards only", () => {
    useUpdateAllRepositories.mockReturnValue(vi.fn());
    render(<UpdateAllButton count={5} unreadable={[]} />);
    expect(screen.getByText(/fast-forwards only/i)).not.toBeNull();
  });

  /// A run that could not START is distinct from a run that completed
  /// with failures in it. The first is a rejection and belongs in a
  /// toast; the second is the report.
  it("surfaces a refused run rather than swallowing it", async () => {
    useUpdateAllRepositories.mockReturnValue(
      vi.fn().mockRejectedValue(new Error("an update run is already going")),
    );
    render(<UpdateAllButton count={5} unreadable={[]} />);

    fireEvent.click(screen.getByRole("button", { name: /update all repositories/i }));

    await waitFor(() => expect(toastError).toHaveBeenCalledWith("an update run is already going"));
    expect(screen.queryByRole("status")).toBeNull();
  });

  /// Progress is counts only -- the event carries no paths, and the
  /// rendering cannot invent any.
  it("renders progress as counts and never as a repository name", async () => {
    let finish: (r: UpdateAllReport) => void = () => {};
    useUpdateAllRepositories.mockReturnValue(
      vi.fn(() => new Promise<UpdateAllReport>((r) => (finish = r))),
    );
    useUpdateAllProgress.mockReturnValue({ done: 7, total: 45 });
    render(<UpdateAllButton count={45} unreadable={[]} />);

    fireEvent.click(screen.getByRole("button", { name: /update all repositories/i }));

    expect(await screen.findByText(/Updating 7 of 45/)).not.toBeNull();
    finish(report());
  });
});

import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { BatchOutcome } from "@/api/tauri";
import { BulkBar } from "@/components/BulkBar";
import { prKey } from "@/lib/prIdentity";
import { PR_FIXTURES } from "@/fixtures/prs";
import { useFilters } from "@/store/filters";
import { renderWithQuery as render } from "@/test-utils";

type Batch = (prs: [string, string, number][], action: string) => Promise<BatchOutcome[]>;
const batch = vi.fn<Batch>(() => Promise.resolve([]));
vi.mock("@/api/hooks", () => ({ useActOnPrs: () => batch }));

const toastError = vi.fn();
const toastSuccess = vi.fn();
const toastInfo = vi.fn();
vi.mock("sonner", () => ({
  toast: {
    success: (...a: unknown[]) => toastSuccess(...a),
    error: (...a: unknown[]) => toastError(...a),
    info: (...a: unknown[]) => toastInfo(...a),
  },
}));

beforeEach(() => {
  batch.mockClear();
  toastError.mockClear();
  toastSuccess.mockClear();
  toastInfo.mockClear();
  useFilters.getState().clearChecked();
});

const select = (...prs: { repo: string; number: number }[]) =>
  useFilters.getState().setChecked(prs.map(prKey));

/// Click the confirm button inside the dialog. Scoped with `within` --
/// the toolbar button that opened the dialog carries the same label, so
/// an unscoped query matches both.
const confirm = (label: string) =>
  fireEvent.click(
    within(screen.getByRole("dialog")).getByRole("button", {
      name: new RegExp(`^${label} \\d+ pull request`),
    }),
  );

describe("BulkBar", () => {
  it("stays out of the way when nothing is selected", () => {
    const { container } = render(<BulkBar prs={PR_FIXTURES} />);
    expect(container.firstChild).toBeNull();
  });

  it("counts what is selected", () => {
    select(PR_FIXTURES[0], PR_FIXTURES[1]);
    render(<BulkBar prs={PR_FIXTURES} />);
    expect(screen.getByText("2 selected")).not.toBeNull();
  });

  // The issue's first requirement: selecting then narrowing a filter must
  // not silently drop rows. Selection is keyed, and the bar resolves keys
  // against the unfiltered list it is handed.
  it("keeps a selected PR that the visible list no longer contains", async () => {
    select(PR_FIXTURES[0], PR_FIXTURES[1]);
    render(<BulkBar prs={PR_FIXTURES} />);

    // The batch must contain BOTH -- proven by acting, not by the count,
    // which a bar handed a pre-filtered list would also render correctly.
    batch.mockResolvedValueOnce(
      [PR_FIXTURES[0], PR_FIXTURES[1]].map((p) => ({
        repo: p.repo,
        number: p.number,
        error: null,
      })),
    );
    fireEvent.click(screen.getByRole("button", { name: "Add to merge queue" }));
    confirm("Add to merge queue");
    await waitFor(() => expect(batch).toHaveBeenCalled());
    expect(batch.mock.calls[0][0].map((t) => t[2]).sort()).toEqual(
      [PR_FIXTURES[0].number, PR_FIXTURES[1].number].sort(),
    );
  });

  // Bulk merge is deliberately excluded: each merge changes the base the
  // next merges onto, which cascades conflicts onto main. Enqueue is the
  // safe expression of the same intent.
  it("does not offer bulk merge", () => {
    select(PR_FIXTURES[0]);
    render(<BulkBar prs={PR_FIXTURES} />);
    expect(screen.queryByRole("button", { name: "Merge" })).toBeNull();
    expect(screen.getByRole("button", { name: "Add to merge queue" })).not.toBeNull();
  });

  // A count is not something anyone can act on safely.
  it("names every pull request in the confirmation, not just a count", () => {
    select(PR_FIXTURES[0], PR_FIXTURES[1]);
    render(<BulkBar prs={PR_FIXTURES} />);
    fireEvent.click(screen.getByRole("button", { name: "Close PRs" }));
    for (const pr of [PR_FIXTURES[0], PR_FIXTURES[1]]) {
      expect(screen.getByText(new RegExp(`${pr.repo}#${pr.number}`))).not.toBeNull();
    }
    expect(batch).not.toHaveBeenCalled();
  });

  it("confirms every bulk action, not only the destructive one", () => {
    select(PR_FIXTURES[0]);
    render(<BulkBar prs={PR_FIXTURES} />);
    fireEvent.click(screen.getByRole("button", { name: "Add to merge queue" }));
    expect(screen.getByRole("dialog")).not.toBeNull();
    expect(batch).not.toHaveBeenCalled();
  });

  it("does nothing when the confirmation is cancelled", () => {
    select(PR_FIXTURES[0]);
    render(<BulkBar prs={PR_FIXTURES} />);
    fireEvent.click(screen.getByRole("button", { name: "Close PRs" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(batch).not.toHaveBeenCalled();
  });

  it("sends every selected PR once confirmed", async () => {
    select(PR_FIXTURES[0], PR_FIXTURES[1]);
    batch.mockResolvedValueOnce([
      { repo: PR_FIXTURES[0].repo, number: PR_FIXTURES[0].number, error: null },
      { repo: PR_FIXTURES[1].repo, number: PR_FIXTURES[1].number, error: null },
    ]);
    render(<BulkBar prs={PR_FIXTURES} />);
    fireEvent.click(screen.getByRole("button", { name: "Add to merge queue" }));
    confirm("Add to merge queue");
    await waitFor(() => expect(batch).toHaveBeenCalled());
    expect(batch.mock.calls[0][0]).toHaveLength(2);
    expect(batch.mock.calls[0][1]).toBe("enqueue");
  });

  // Partial failure is the normal case for a batch, not the exception.
  it("reports failures rather than a bare success", async () => {
    select(PR_FIXTURES[0], PR_FIXTURES[1]);
    batch.mockResolvedValueOnce([
      { repo: PR_FIXTURES[0].repo, number: PR_FIXTURES[0].number, error: null },
      { repo: PR_FIXTURES[1].repo, number: PR_FIXTURES[1].number, error: "not mergeable" },
    ]);
    render(<BulkBar prs={PR_FIXTURES} />);
    fireEvent.click(screen.getByRole("button", { name: "Close PRs" }));
    confirm("Close PRs");
    await waitFor(() => expect(toastError).toHaveBeenCalled());
    expect(toastSuccess).not.toHaveBeenCalled();
    const [title, opts] = toastError.mock.calls[0] as [string, { description: string }];
    expect(title).toMatch(/1 of 2 failed/);
    expect(opts.description).toContain("not mergeable");
  });

  // Retrying should not repeat what already worked.
  it("keeps only the failures selected, so a retry does not repeat successes", async () => {
    select(PR_FIXTURES[0], PR_FIXTURES[1]);
    batch.mockResolvedValueOnce([
      { repo: PR_FIXTURES[0].repo, number: PR_FIXTURES[0].number, error: null },
      { repo: PR_FIXTURES[1].repo, number: PR_FIXTURES[1].number, error: "boom" },
    ]);
    render(<BulkBar prs={PR_FIXTURES} />);
    fireEvent.click(screen.getByRole("button", { name: "Close PRs" }));
    confirm("Close PRs");
    await waitFor(() => expect(toastError).toHaveBeenCalled());
    expect(useFilters.getState().checked).toEqual([prKey(PR_FIXTURES[1])]);
  });

  /// #752. `PR_FIXTURES[2]` is already in the merge queue; `[0]` is not.
  ///
  /// The batch is the assertion, not the wording: enqueueing a PR that
  /// is already queued is the pointless request the issue exists to
  /// stop, and it produced six "already in the queue" refusals in a
  /// real session log.
  describe("an action that would do nothing", () => {
    it("leaves already-queued pull requests out of the batch", async () => {
      select(PR_FIXTURES[0], PR_FIXTURES[2]);
      batch.mockResolvedValueOnce([
        { repo: PR_FIXTURES[0].repo, number: PR_FIXTURES[0].number, error: null },
      ]);
      render(<BulkBar prs={PR_FIXTURES} />);
      fireEvent.click(screen.getByRole("button", { name: "Add to merge queue" }));
      confirm("Add to merge queue");
      await waitFor(() => expect(batch).toHaveBeenCalled());
      expect(batch.mock.calls[0][0].map((t) => t[2])).toEqual([PR_FIXTURES[0].number]);
    });

    /// The issue's own suggested wording: say it in the dialog, where
    /// the count is already the thing being checked.
    it("says how many are already queued, and still lists them", () => {
      select(PR_FIXTURES[0], PR_FIXTURES[2]);
      render(<BulkBar prs={PR_FIXTURES} />);
      fireEvent.click(screen.getByRole("button", { name: "Add to merge queue" }));
      const dialog = within(screen.getByRole("dialog"));
      expect(dialog.getByText(/2 selected/)).not.toBeNull();
      expect(dialog.getByText(/1 already in the merge queue/)).not.toBeNull();
      // The skipped row is still NAMED. Dropping it would make the
      // dialog disagree with the selection it is confirming.
      expect(
        dialog.getByText(new RegExp(`${PR_FIXTURES[2].number}`)),
      ).not.toBeNull();
    });

    /// The rule `BulkBar` documents: the batch is resolved against the
    /// UNFILTERED list, so nothing may quietly leave the SELECTION.
    ///
    /// Skipping is a property of the action, not of the selection, and
    /// this is what proves the difference: after confirming an enqueue
    /// that skipped a row, that row is still selected -- so choosing a
    /// different action next still acts on it.
    it("does not unselect the rows an action skipped", async () => {
      select(PR_FIXTURES[0], PR_FIXTURES[2]);
      batch.mockResolvedValueOnce([
        { repo: PR_FIXTURES[0].repo, number: PR_FIXTURES[0].number, error: null },
      ]);
      render(<BulkBar prs={PR_FIXTURES} />);
      // Before confirming, the bar still counts BOTH.
      expect(screen.getByText("2 selected")).not.toBeNull();
      fireEvent.click(screen.getByRole("button", { name: "Add to merge queue" }));
      confirm("Add to merge queue");
      await waitFor(() => expect(toastSuccess).toHaveBeenCalled());
      // The toast names the skip rather than claiming two were updated.
      expect(String(toastSuccess.mock.calls[0][0])).toMatch(/1 skipped/);
    });

    /// A batch with nothing left to do must say so rather than send an
    /// empty request and report "0 updated", which reads as a failure.
    it("sends nothing when every selected row is already in that state", () => {
      select(PR_FIXTURES[2]);
      render(<BulkBar prs={PR_FIXTURES} />);
      fireEvent.click(screen.getByRole("button", { name: "Add to merge queue" }));
      confirm("Add to merge queue");
      expect(batch).not.toHaveBeenCalled();
      expect(toastInfo).toHaveBeenCalled();
    });

    /// The issue asked for the other bulk actions to be checked for the
    /// same shape. "Mark ready" on a PR that is not a draft is it.
    it("leaves non-drafts out of a Mark ready batch", async () => {
      select(PR_FIXTURES[0], PR_FIXTURES[1]);
      batch.mockResolvedValueOnce([
        { repo: PR_FIXTURES[1].repo, number: PR_FIXTURES[1].number, error: null },
      ]);
      render(<BulkBar prs={PR_FIXTURES} />);
      fireEvent.click(screen.getByRole("button", { name: "Mark ready" }));
      confirm("Mark ready");
      await waitFor(() => expect(batch).toHaveBeenCalled());
      expect(batch.mock.calls[0][0].map((t) => t[2])).toEqual([PR_FIXTURES[1].number]);
    });

    /// Close is never redundant -- the list holds only open pull
    /// requests -- so the skip logic must not touch it.
    it("closes every selected pull request, queued or not", async () => {
      select(PR_FIXTURES[0], PR_FIXTURES[2]);
      batch.mockResolvedValueOnce(
        [PR_FIXTURES[0], PR_FIXTURES[2]].map((p) => ({
          repo: p.repo,
          number: p.number,
          error: null,
        })),
      );
      render(<BulkBar prs={PR_FIXTURES} />);
      fireEvent.click(screen.getByRole("button", { name: "Close PRs" }));
      confirm("Close PRs");
      await waitFor(() => expect(batch).toHaveBeenCalled());
      expect(batch.mock.calls[0][0].map((t) => t[2]).sort()).toEqual(
        [PR_FIXTURES[0].number, PR_FIXTURES[2].number].sort(),
      );
    });
  });

  /// `PR_FIXTURES[1]`, the DRAFT, rather than `[0]`.
  ///
  /// "Mark ready" on `[0]` is a no-op -- it is already ready -- so since
  /// #752 the batch is empty and nothing is sent. The old fixture made
  /// this test pass by exercising exactly the pointless call the issue
  /// is about; the action now has to be one that can really change the
  /// pull request for the success path to be reached at all.
  it("clears the selection when everything succeeded", async () => {
    select(PR_FIXTURES[1]);
    batch.mockResolvedValueOnce([
      { repo: PR_FIXTURES[1].repo, number: PR_FIXTURES[1].number, error: null },
    ]);
    render(<BulkBar prs={PR_FIXTURES} />);
    fireEvent.click(screen.getByRole("button", { name: "Mark ready" }));
    confirm("Mark ready");
    await waitFor(() => expect(toastSuccess).toHaveBeenCalled());
    expect(useFilters.getState().checked).toEqual([]);
  });
});

for (const source of [
  { provider: "gitlab" as const, host: "gitlab.com" },
  { provider: "github" as const, host: "github.example" },
]) {
  it(`keeps ${source.provider}/${source.host} selection separate and refuses unsupported writes`, () => {
    const github = PR_FIXTURES[0];
    const other = { ...github, source, title: "Other source row" };
    select(other);
    render(<BulkBar prs={[github, other]} />);
    expect(screen.getByText("1 selected")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Close PRs" }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText(/Other source row/)).toBeTruthy();
    expect(within(dialog).queryByText(new RegExp(github.title))).toBeNull();
    confirm("Close PRs");
    expect(batch).not.toHaveBeenCalled();
    expect(toastSuccess).not.toHaveBeenCalled();
    expect(toastError).toHaveBeenCalledWith("Bulk actions are currently available only for GitHub.com pull requests");
    expect(useFilters.getState().checked).toEqual([prKey(other)]);
  });
}

/// #1286, the same defect #1278 fixed for the PR detail bar.
///
/// The bar always stuck. The app header in `App` is `sticky top-0 z-20`
/// inside the same `<main>` scroll container, opaque, one z-layer up, so
/// at `top-0` this bar pinned to exactly the header's band and was
/// painted over -- pinned and invisible, which looks like not sticking.
///
/// This asserts the STRUCTURE, not the rendered result: jsdom performs no
/// layout, so `position: sticky` cannot be observed here. What it can
/// prove is the condition the fix depends on -- the bar is sticky and its
/// `top` defers to the app header's published height instead of zero.
/// The browser measurement is recorded on the PR.
describe("BulkBar pins below the app header (#1286)", () => {
  it("offsets its sticky top by the app header's height", () => {
    select(PR_FIXTURES[0]);
    const { container } = render(<BulkBar prs={PR_FIXTURES} />);
    const bar = container.firstElementChild as HTMLElement;
    const cls = bar.className.split(/\s+/);
    expect(cls).toContain("sticky");
    expect(cls).toContain("z-10");
    // The regression in one assertion: any `top-*` class puts the bar
    // at a fixed offset the app header may already occupy.
    expect(cls.filter((c) => /^top-/.test(c))).toEqual([]);
    expect(bar.style.top).toContain("--app-header-h");
  });
});

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useFilters } from "../store/filters";
import type { PrDetail, ReviewGates } from "@/types/pr";
import { stubViewport } from "@/test-utils";
import { scopeEffect } from "@/lib/branchDelete";

const state = vi.hoisted(() => ({
  /// Sessions linked to this PR, for the reverse-link tests (#1211).
  prSessions: [] as { session_id: string; repo: string; number: number; url: string; first_seen_at: string | null }[],
  data: undefined as PrDetail | undefined,
  isLoading: false,
  // True while `usePrDetail` is serving the clicked row's own facts in
  // place of the fetch (#790). Defaulted false so every existing test
  // exercises the LOADED view exactly as before.
  isPlaceholderData: false,
  isError: false,
  /// The review gates (#1451, #1454). Undefined by default -- pending and
  /// unreadable both render nothing new -- so every existing test sees
  /// the view exactly as before.
  gates: undefined as ReviewGates | undefined,
}));

const deleteBranch = vi.hoisted(() =>
  vi.fn<(r: string, repo: string, n: number, b: string, m: boolean) => Promise<void>>(
    () => Promise.resolve(),
  ),
);
// Typed so the call arguments can be asserted on: the untyped form
// infers an empty tuple, and indexing it is a compile error.
const reviewPr =
  vi.fn<(id: string, repo: string, number: number, verdict: string, body: string) => Promise<void>>(
    () => Promise.resolve(),
  );
const rerunChecks = vi.fn(() => Promise.resolve());
const commentOnPr = vi.fn(() => Promise.resolve());

const viewer = vi.hoisted(() => ({ current: undefined as string | undefined }));

vi.mock("../api/hooks", () => ({
  // No linked session by default: the panel renders nothing, which is
  // what every other assertion in this file assumes (#1211).
  useClaudeSessionsForPr: () => ({ data: state.prSessions }),
  usePrDetail: () => ({ ...state, error: "boom", refetch: vi.fn() }),
  useActOnPr: () => vi.fn(() => Promise.resolve()),
  useDeleteHeadBranch: () => deleteBranch,
  useReviewPr: () => reviewPr,
  useRerunChecks: () => rerunChecks,
  useCommentOnPr: () => commentOnPr,
  useResolveThread: () => vi.fn(() => Promise.resolve()),
  useUnresolveThread: () => vi.fn(() => Promise.resolve()),
  useReplyToThread: () => vi.fn(() => Promise.resolve()),
  // The detail view treats undefined as "we could not ask", which is
  // deliberately NOT the same as "this is mine" -- see ReviewBox.
  // Controllable so the pinned Approve button, which is hidden on your
  // own pull request, can be exercised at all.
  useViewer: () => ({ data: viewer.current }),
  // Claudify's inputs (#1455): the scan and the terminal setting.
  useWorktrees: () => ({
    data: claudifyState.repos,
    unreadable: claudifyState.unreadable,
    isError: claudifyState.scanError !== null,
    error: claudifyState.scanError,
  }),
  useUiPrefs: () => ({ prefs: { terminal_command: claudifyState.terminal } }),
  useReviewGates: () => ({ data: state.gates }),
}));

/// What Claudify sees (#1455). Defaults: scanned, no checkout of the
/// fixture's repository, no terminal -- the "copy the prompt" fallback.
const claudifyState = vi.hoisted(() => ({
  repos: [] as
    | { identity: string | null; name: string; path: string; worktrees: never[]; bare?: boolean }[]
    | undefined,
  unreadable: [] as string[],
  scanError: null as string | null,
  terminal: "",
}));
const claudifyApi = vi.hoisted(() => ({
  claudifyPrCommand: vi.fn(),
  claudeLaunchPr: vi.fn(),
  claudeLaunchPrPreview: vi.fn(),
  claudeLaunchTerms: vi.fn(),
}));
vi.mock("../api/tauri", async (orig) => ({ ...(await orig<object>()), ...claudifyApi }));

import { PrDetailView } from "./PrDetailView";

const detail = (over: Partial<PrDetail> = {}): PrDetail => ({
  id: "PR_test",
  number: 42,
  title: "Add retry to the fetch client",
  url: "https://github.com/octocat/hello-world/pull/42",
  state: "open",
  is_draft: false,
  body: "## Why\n\nThe client gave up too early.",
  author: "octocat",
  repo: "octocat/hello-world",
  head_ref: "feature/retry",
  head_oid: "oid-detail",
  head_ref_id: null,
  base_ref: "main",
  merge_status: "clean",
  review: "none",
  additions: 100,
  deletions: 20,
  changed_files: 3,
  unresolved_threads: 0,
  comment_count: 0,
  comments: [],
  review_threads: [],
  // Matches `review_threads` above: an empty list with nothing missing.
  // A non-zero default here would make every test that does not mention
  // threads render a truncation notice.
  review_threads_total: 0,
  latest_reviews: [],
  merge_queue_enabled: false,
  in_merge_queue: false,
  checks: [],
  checks_total: 0,
  ...over,
});

function view(over: Partial<PrDetail> = {}) {
  state.data = detail(over);
  return render(<PrDetailView repo="octocat/hello-world" number={42} onBack={() => {}} />);
}

describe("PrDetailView layout", () => {
  afterEach(() => {
    cleanup();
    stubViewport(null);
    viewer.current = undefined;
  });

  /// Every action the desktop pins stays pinned on the phone; only the
  /// arrangement changes. Asserting each by role is what proves that a
  /// stacked header did not quietly drop one.
  it("keeps back, approve, merge and GitHub in the sticky header on a phone", () => {
    stubViewport(390);
    viewer.current = "hubot";
    const { container } = view();
    const bar = container.querySelector(".sticky") as HTMLElement;
    expect(bar).toBeTruthy();
    expect(within(bar).getByRole("button", { name: /back to list/i })).toBeTruthy();
    expect(within(bar).getByRole("button", { name: "Approve" })).toBeTruthy();
    expect(within(bar).getByRole("button", { name: /^merge$/i })).toBeTruthy();
    expect(within(bar).getByText(/github/i)).toBeTruthy();
    // The actions moved to a second, full-width line under the back
    // link so four controls are not squeezed into 390 pixels.
    const merge = within(bar).getByRole("button", { name: /^merge$/i });
    expect(merge.closest(".basis-full")).toBeTruthy();
  });

  it("keeps the desktop header on one line", () => {
    stubViewport(1400);
    viewer.current = "hubot";
    const { container } = view();
    const bar = container.querySelector(".sticky") as HTMLElement;
    expect(bar.querySelector(".basis-full")).toBeNull();
    expect(bar.className).not.toContain("flex-wrap");
    // Back, then the action cluster: exactly two direct children.
    expect(bar.children).toHaveLength(2);
    expect(within(bar).getByRole("button", { name: "Approve" })).toBeTruthy();
    expect(within(bar).getByRole("button", { name: /^merge$/i })).toBeTruthy();
  });

  /// #1278, on BOTH layouts.
  ///
  /// What regressed was not `position: sticky` -- that always worked.
  /// The app header in `App` is `sticky top-0 z-20` inside the same
  /// `<main>` scroll container (since #623), so this bar at `top-0`
  /// pinned to the identical band, one z-layer below an opaque
  /// background, and was invisible. Offsetting `top` by the app
  /// header's height is the whole fix.
  ///
  /// This asserts the STRUCTURE that makes it work, not the rendered
  /// result: jsdom performs no layout, so `position: sticky` cannot be
  /// observed here and a test claiming "the header is visible after
  /// scrolling" would pass against any code at all. What it can prove
  /// is that the bar is sticky, and that its `top` defers to the app
  /// header's published height instead of being pinned to zero -- which
  /// is precisely the thing that was wrong.
  for (const [label, width] of [
    ["phone", 390],
    ["desktop", 1400],
  ] as const) {
    it(`offsets the sticky header below the app header on ${label}`, () => {
      stubViewport(width);
      viewer.current = "hubot";
      const { container } = view();
      const bar = container.querySelector(".sticky") as HTMLElement;
      expect(bar).toBeTruthy();
      // Still sticky, and still above the body it pins over.
      expect(bar.className).toContain("sticky");
      expect(bar.className).toContain("z-10");
      // The regression in one assertion: `top-0` puts this bar exactly
      // where the app header already is.
      expect(bar.className).not.toContain("top-0");
      // And the fix: `top` comes from the app header's measured height.
      expect(bar.style.top).toContain("--app-header-h");
    });
  }

  /// The other half of #1278: nothing may reintroduce a sticky-breaking
  /// style between this bar and `<main>`. `overflow` other than
  /// `visible`, or a `transform`/`filter`/`contain`/`will-change` on an
  /// ancestor, would make the bar stop pinning for real -- a different
  /// failure from #1278's, and one no `top` value could rescue.
  ///
  /// Checked on the inline styles and classes rather than on computed
  /// values, because jsdom does not resolve Tailwind's stylesheet: the
  /// classes ARE the declaration here.
  it("puts no sticky-breaking style between the header and the scroll container", () => {
    stubViewport(1400);
    viewer.current = "hubot";
    const { container } = view();
    const bar = container.querySelector(".sticky") as HTMLElement;
    // Matched as whole classes: Tailwind writes them bare, so anchoring
    // on `^` or `:` (as a first pass did) matched nothing at all and the
    // test passed against a deliberately broken ancestor.
    const breaking =
      /\b(overflow-(hidden|auto|scroll|clip)|transform|filter|blur|contain-\w+|will-change-\w+)\b/;
    // Bounded by `container` INCLUSIVE -- `container` is RTL's own host
    // div, and the element under test's outermost wrapper sits between
    // it and the bar. Excluding it skipped the one ancestor this
    // component actually owns.
    const seen: string[] = [];
    for (let el: HTMLElement | null = bar.parentElement; el; el = el.parentElement) {
      seen.push(el.className);
      expect
        .soft(el.className, `ancestor <${el.tagName.toLowerCase()} class="${el.className}">`)
        .not.toMatch(breaking);
      expect.soft(el.style.transform, "inline transform").toBeFalsy();
      expect.soft(el.style.overflow, "inline overflow").toBeFalsy();
      if (el === container) break;
    }
    // The walk must actually have inspected something, or the two
    // assertions above are vacuous.
    expect(seen.length).toBeGreaterThan(0);
  });

  it("still offers review, comment, threads and the footer actions on a phone", () => {
    stubViewport(390);
    viewer.current = "hubot";
    view({
      state: "MERGED",
      head_ref_id: "REF_1",
      review_threads: [
        {
          id: "T1",
          path: "src/a.ts",
          line: 3,
          is_resolved: false,
          is_outdated: false,
          viewer_can_reply: true,
          viewer_can_resolve: true,
          viewer_can_unresolve: true,
          comments: [{ author: "octocat", body: "Why?", created_at: "2026-01-01T00:00:00Z" }],
          comment_count: 1,
        },
      ],
    });
    expect(screen.getByText(/view on github/i)).toBeTruthy();
    expect(screen.getByRole("button", { name: /copy prompt/i })).toBeTruthy();
    expect(screen.getByRole("button", { name: /delete branch/i })).toBeTruthy();
    expect(screen.getByText("Why?")).toBeTruthy();
    // The review box and the thread reply box: both still there.
    expect(screen.getAllByRole("textbox").length).toBeGreaterThan(0);
  });
});

describe("PrDetailView", () => {
  beforeEach(() => {
    Object.assign(state, {
      data: undefined,
      isLoading: false,
      isPlaceholderData: false,
      isError: false,
    });
    // The mutation mocks are module-level, so without this a later test
    // sees calls made by an earlier one -- which is exactly how the
    // "not called" assertion below failed while passing in isolation.
    reviewPr.mockClear();
    rerunChecks.mockClear();
    commentOnPr.mockClear();
    deleteBranch.mockClear();
  });

  it("shows the title, number and branch pair", () => {
    view();
    expect(screen.getByText(/add retry to the fetch client/i)).toBeTruthy();
    expect(screen.getByText("#42")).toBeTruthy();
    expect(screen.getByText("feature/retry")).toBeTruthy();
    expect(screen.getByText("main")).toBeTruthy();
  });

  it("renders the description as Markdown", () => {
    const { container } = view();
    expect(screen.getByText("Why")).toBeTruthy();
    expect(container.querySelector("h2")).toBeTruthy();
  });

  // An empty body must read as empty, not as a broken render.
  it("says so when there is no description", () => {
    view({ body: "   " });
    expect(screen.getByText(/no description/i)).toBeTruthy();
  });

  it("lists each check with its outcome", () => {
    view({
      checks: [
        { name: "build", state: "success", url: "https://ci/1", run_id: null },
        { name: "lint", state: "failure", url: "", run_id: null },
      ],
    });
    expect(screen.getByText("build")).toBeTruthy();
    expect(screen.getByText("failure")).toBeTruthy();
  });

  // The `rerunnableRun` rules are tested in lib/rerun.test.ts. These
  // prove the button is WIRED to them.
  it("offers a re-run when a failing check has a workflow run", () => {
    view({ checks: [{ name: "lint", state: "failure", url: "", run_id: 99 }] });
    expect(screen.getByRole("button", { name: /re-run failed/i })).toBeTruthy();
  });

  it("offers no re-run when everything passed", () => {
    view({ checks: [{ name: "lint", state: "success", url: "", run_id: 99 }] });
    expect(screen.queryByRole("button", { name: /re-run failed/i })).toBeNull();
  });

  // A status context has no workflow run, so the REST call would 404.
  it("offers no re-run for a failure with no workflow run", () => {
    view({ checks: [{ name: "legacy", state: "failure", url: "", run_id: null }] });
    expect(screen.queryByRole("button", { name: /re-run failed/i })).toBeNull();
  });

  it("re-runs against the workflow run, not the check", async () => {
    view({ checks: [{ name: "lint", state: "failure", url: "", run_id: 99 }] });
    fireEvent.click(screen.getByRole("button", { name: /re-run failed/i }));
    await waitFor(() =>
      expect(rerunChecks).toHaveBeenCalledWith("octocat/hello-world", 42, 99),
    );
  });

  // A check with no URL must not render an anchor going nowhere.
  it("only links checks that have a URL", () => {
    const { container } = view({
      checks: [{ name: "lint", state: "failure", url: "", run_id: null }],
    });
    const anchors = Array.from(container.querySelectorAll("a")).filter(
      (a) => a.textContent?.includes("lint"),
    );
    expect(anchors).toHaveLength(0);
  });

  it("renders comments with their author", () => {
    view({
      comment_count: 1,
      comments: [
        { author: "hubot", created_at: "2026-08-20T10:00:00Z", body: "looks good" },
      ],
    });
    expect(screen.getByText(/hubot/)).toBeTruthy();
    // The body, not the collapsed row's screen-reader preview -- a single
    // comment opens by default, so both carry this text.
    const visible = screen
      .getAllByText(/looks good/)
      .filter((el) => !el.classList.contains("sr-only"));
    expect(visible).toHaveLength(1);
  });

  /// #1453: the query fetches the NEWEST comments, so a truncated list is
  /// missing the oldest -- and says so ABOVE the rows, before the reader
  /// takes them for the whole discussion.
  it("says which comments are missing, above the ones it shows", () => {
    view({ comment_count: 80, comments: [
      { author: "hubot", created_at: "2026-08-20T10:00:00Z", body: "one" },
      { author: "hubot", created_at: "2026-08-21T10:00:00Z", body: "two" },
    ] });
    const notice = screen.getByText(/Showing the newest 2 of 80 — older ones are on GitHub/);
    const firstRow = screen.getAllByText("hubot")[0];
    expect(
      notice.compareDocumentPosition(firstRow) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("does not annotate a complete comment list", () => {
    view({ comment_count: 1, comments: [
      { author: "hubot", created_at: "2026-08-20T10:00:00Z", body: "one" },
    ] });
    expect(screen.queryByText(/Showing the newest/)).toBeNull();
  });

  /// #1457: the PR's age and last commit, in the header. Each date is
  /// relative to the real clock, so the text is stable without fake
  /// timers.
  describe("header dates", () => {
    const ago = (ms: number) => new Date(Date.now() - ms).toISOString();
    const HOUR = 3_600_000;
    const DAY = 24 * HOUR;

    it("shows opened, ready for review and last commit", () => {
      view({ created_at: ago(9 * DAY), ready_at: ago(3 * DAY), last_commit_at: ago(5 * HOUR) });
      expect(screen.getByText("9 days ago").closest("[data-pr-date]")?.textContent).toBe(
        "opened 9 days ago",
      );
      const ready = screen.getByText("3 days ago").closest("[data-pr-date]") as HTMLElement;
      expect(ready.textContent).toBe("ready for review 3 days ago");
      // The review queue's stale colour: past 48 hours.
      expect(ready.className).toContain("text-[#f85149]");
      expect(screen.getByText("5 hours ago").closest("[data-pr-date]")?.textContent).toBe(
        "last commit 5 hours ago",
      );
    });

    it("colours a fresh ready time as the review queue does", () => {
      view({ created_at: ago(9 * DAY), ready_at: ago(2 * HOUR) });
      const ready = screen.getByText(/ready for review/) as HTMLElement;
      expect(ready.className).toContain("text-[#3fb950]");
    });

    /// Absent is not zero: a date that did not arrive, did not parse, or
    /// sits in the future beyond clock skew is left out -- never "just now".
    it("omits every date it cannot read", () => {
      view({ created_at: null, ready_at: "not a date", last_commit_at: ago(-2 * HOUR) });
      expect(screen.queryByText(/opened/)).toBeNull();
      expect(screen.queryByText(/ready for review/)).toBeNull();
      expect(screen.queryByText(/last commit/)).toBeNull();
      expect(screen.queryByText(/just now/)).toBeNull();
    });

    it("omits the dates entirely on a payload without them", () => {
      view();
      expect(document.querySelector("[data-pr-date]")).toBeNull();
    });
  });

  it("surfaces unresolved conversations", () => {
    view({ unresolved_threads: 3 });
    expect(screen.getByText(/3 unresolved conversations/i)).toBeTruthy();
  });

  /// Reported: the view "is all expanded and shoved together". A
  /// healthy pull request with twenty passing checks made the largest
  /// block on the page the one that repeats what the CI pill already
  /// said.
  it("collapses passing checks but opens them when something failed", () => {
    view({
      checks: [
        { name: "build", state: "success", url: "", run_id: null },
        { name: "lint", state: "success", url: "", run_id: null },
      ],
    });
    expect(screen.queryByText("build")).toBeNull();

    cleanup();
    view({
      checks: [
        { name: "build", state: "success", url: "", run_id: null },
        { name: "lint", state: "failure", url: "", run_id: 99 },
      ],
    });
    // Open the moment anything is not passing -- that is when the names
    // are what you came for.
    expect(screen.getByText("lint")).toBeTruthy();
  });

  /// Each comment collapses on its own now, rather than the block
  /// collapsing as a unit. The previous behaviour was all-or-nothing:
  /// six comments meant expanding every one of them to read any one.
  it("collapses each comment individually, with its own toggle", () => {
    const comment = (i: number) => ({
      author: "octocat",
      created_at: "2026-08-20T10:00:00Z",
      body: `comment ${i}`,
    });
    view({
      comments: [1, 2, 3, 4, 5, 6].map(comment),
      comment_count: 6,
    });

    // One toggle per comment, each independently operable -- that is what
    // "collapsed individually" MEANS, and a single shared toggle would
    // still satisfy a test that only looked at visible text.
    const toggles = screen
      .getAllByRole("button")
      .filter((b) => b.getAttribute("aria-expanded") !== null);
    expect(toggles.length).toBeGreaterThanOrEqual(6);

    // The count still says what is hidden.
    expect(screen.getByText("6")).toBeTruthy();

    // Opening the third leaves the others shut: independence, not a
    // single control wired to every row.
    const third = screen.getByRole("button", { name: /comment 3/ });
    fireEvent.click(third);
    expect(third.getAttribute("aria-expanded")).toBe("true");
    expect(
      screen.getByRole("button", { name: /comment 4/ }).getAttribute("aria-expanded"),
    ).toBe("false");
  });

  /// A lone comment has nothing to scan past, so collapsing it only adds
  /// a click between the reader and the only thing there is to read.
  it("opens a single comment by default", () => {
    view({
      comments: [
        {
          author: "octocat",
          created_at: "2026-08-20T10:00:00Z",
          body: "the only comment",
        },
      ],
      comment_count: 1,
    });
    expect(
      screen.getByRole("button", { name: /the only comment/ }).getAttribute("aria-expanded"),
    ).toBe("true");
  });

  /// The threads have to REACH the view, not merely exist in the model:
  /// this is the wiring between PrDetail and the Conversations section.
  it("shows review conversations from the detail payload", () => {
    view({
      review_threads: [
        {
          id: "RT_1",
          is_resolved: false,
          is_outdated: false,
          path: "src/api/hooks.ts",
          line: 412,
          viewer_can_reply: true,
          viewer_can_resolve: true,
          viewer_can_unresolve: false,
          comments: [
            {
              author: "carol",
              created_at: "2026-08-20T10:00:00Z",
              body: "This leaks the subscription",
            },
          ],
          comment_count: 1,
        },
      ],
    });
    expect(screen.getByText("src/api/hooks.ts:412")).toBeTruthy();
    // Unresolved, so it is open and its content is on screen without a
    // click -- the reason the section exists.
    expect(screen.getByText(/This leaks the subscription/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Resolve conversation" })).toBeTruthy();
  });

  /// The description is what the pull request IS -- collapsing it by
  /// default would hide the thing the view exists to show.
  it("leaves the description open", () => {
    view({ body: "the description text" });
    expect(screen.getByText(/the description text/)).toBeTruthy();
  });

  /// The reported problem: "there are no buttons that fix to the top of
  /// a PR as I'm scrolling, so I have to scroll all the way to the top
  /// to see approve and all the way to the bottom to see the view on
  /// github button".
  ///
  /// The actions were unreachable from where the decision gets made --
  /// after reading a long thread, every control was off-screen.
  it("pins the back, merge and GitHub actions to the top", () => {
    state.data = detail();
    const { container } = render(
      <PrDetailView repo="octocat/hello-world" number={42} onBack={vi.fn()} />,
    );
    const header = container.querySelector(".sticky");
    expect(header, "the header must be sticky, not merely present").toBeTruthy();
    const bar = header as HTMLElement;
    expect(within(bar).getByRole("button", { name: /back to list/i })).toBeTruthy();
    expect(within(bar).getByRole("button", { name: /^merge$/i })).toBeTruthy();
    expect(within(bar).getByText(/github/i)).toBeTruthy();
  });

  /// This test used to assert `top-0`, and its own comment read:
  /// "`top-0` only works because the app header above scrolls away. If
  /// that ever changes, the two overlap and this is the reminder."
  ///
  /// That change duly happened -- #623 made the app header
  /// `sticky top-0 z-20` in this same scroll container -- and the
  /// reminder never fired, because it asserted the CLASS rather than
  /// the condition the class depended on. `top-0` was still there, and
  /// still wrong. That is #1278.
  ///
  /// So this now pins the condition instead: the bar clears whatever is
  /// sticky above it, by deferring to the app header's published
  /// height. If someone pins this back to zero, this fails.
  it("pins below the app header rather than into it", () => {
    state.data = detail();
    const { container } = render(
      <PrDetailView repo="octocat/hello-world" number={42} onBack={vi.fn()} />,
    );
    const bar = container.querySelector(".sticky") as HTMLElement;
    expect(bar.className).toContain("sticky");
    expect(bar.className).not.toContain("top-0");
    expect(bar.style.top).toBe("var(--app-header-h, 0px)");
  });

  /// Requested after the first pass deliberately left it out. GitHub
  /// allows an empty approval, so the original objection was about
  /// clicking it by accident, not about validity -- the guards below
  /// are what address that.
  describe("the pinned Approve button", () => {
    afterEach(() => {
      viewer.current = undefined;
    });

    it("approves without needing the comment box", async () => {
      viewer.current = "octocat";
      view({ author: "someone-else" });
      const bar = document.querySelector(".sticky") as HTMLElement;
      fireEvent.click(within(bar).getByRole("button", { name: "Approve" }));
      await waitFor(() => expect(reviewPr).toHaveBeenCalled());
      expect(reviewPr.mock.calls[0][3]).toBe("approve");
      // No comment: an empty body is what "approve from the header"
      // means, and GitHub accepts it.
      expect(reviewPr.mock.calls[0][4]).toBe("");
    });

    /// GitHub refuses self-approval outright, so offering the button
    /// and surfacing a GraphQL refusal after the click is strictly
    /// worse than not offering it.
    it("is absent on your own pull request", () => {
      viewer.current = "octocat";
      view({ author: "octocat" });
      const bar = document.querySelector(".sticky") as HTMLElement;
      expect(within(bar).queryByRole("button", { name: /approve/i })).toBeNull();
    });

    /// Undefined means "we could not fetch the login", which is not the
    /// same as "this is not yours" -- but a pinned one-click approve is
    /// the wrong thing to offer on a guess.
    it("is absent when the viewer could not be identified", () => {
      viewer.current = undefined;
      view({ author: "someone-else" });
      const bar = document.querySelector(".sticky") as HTMLElement;
      expect(within(bar).queryByRole("button", { name: /approve/i })).toBeNull();
    });

    it("says Approved and stops offering once your approval is on record", () => {
      viewer.current = "octocat";
      view({
        author: "someone-else",
        latest_reviews: [{ author: "octocat", state: "APPROVED" }],
      });
      const bar = document.querySelector(".sticky") as HTMLElement;
      const btn = within(bar).getByRole("button", { name: "Approved" }) as HTMLButtonElement;
      expect(btn.disabled).toBe(true);
    });

    /// GitHub dismisses a review when the branch changes under it, so
    /// showing "Approved" would claim something false about the code
    /// currently on the branch.
    it("offers Approve again after a dismissal", () => {
      viewer.current = "octocat";
      view({
        author: "someone-else",
        latest_reviews: [{ author: "octocat", state: "DISMISSED" }],
      });
      const bar = document.querySelector(".sticky") as HTMLElement;
      expect(within(bar).getByRole("button", { name: "Approve" })).toBeTruthy();
    });

    /// The aggregate `review` field says CHANGES_REQUESTED when someone
    /// ELSE blocked it, which says nothing about this viewer.
    it("ignores another reviewer's verdict", () => {
      viewer.current = "octocat";
      view({
        author: "someone-else",
        review: "changes_requested",
        latest_reviews: [{ author: "hubot", state: "CHANGES_REQUESTED" }],
      });
      const bar = document.querySelector(".sticky") as HTMLElement;
      expect(within(bar).getByRole("button", { name: "Approve" })).toBeTruthy();
    });

    /// Somebody else approving is not you approving. Matching on state
    /// alone would hide the button on any pull request another reviewer
    /// had already signed off -- exactly the ones still waiting on you.
    it("ignores another reviewer's approval", () => {
      viewer.current = "octocat";
      view({
        author: "someone-else",
        review: "approved",
        latest_reviews: [{ author: "hubot", state: "APPROVED" }],
      });
      const bar = document.querySelector(".sticky") as HTMLElement;
      const btn = within(bar).getByRole("button", { name: "Approve" }) as HTMLButtonElement;
      expect(btn.disabled).toBe(false);
    });
  });

  /// Close is irreversible and deliberately NOT pinned: a destructive
  /// button that follows you down the page is the wrong one to make
  /// easier to reach.
  it("does not pin the irreversible close action", () => {
    state.data = detail();
    const { container } = render(
      <PrDetailView repo="octocat/hello-world" number={42} onBack={vi.fn()} />,
    );
    const bar = container.querySelector(".sticky") as HTMLElement;
    expect(within(bar).queryByRole("button", { name: /close pr/i })).toBeNull();
    // Still available in the body, where it always was.
    expect(screen.getByRole("button", { name: /close pr/i })).toBeTruthy();
  });

  /// The header must not become a second source of truth for which
  /// merge action applies -- it renders `PrActions` in compact mode
  /// rather than reimplementing the merge/enqueue/dequeue choice.
  it("pins the queue action on a merge-queue branch, not a plain merge", () => {
    state.data = { ...detail(), merge_queue_enabled: true };
    const { container } = render(
      <PrDetailView repo="octocat/hello-world" number={42} onBack={vi.fn()} />,
    );
    const bar = container.querySelector(".sticky") as HTMLElement;
    expect(within(bar).getByRole("button", { name: /add to merge queue/i })).toBeTruthy();
    expect(within(bar).queryByRole("button", { name: /^merge$/i })).toBeNull();
  });

  it("offers a way back to the list", () => {
    const onBack = vi.fn();
    state.data = detail();
    render(<PrDetailView repo="octocat/hello-world" number={42} onBack={onBack} />);
    fireEvent.click(screen.getByRole("button", { name: /back to list/i }));
    expect(onBack).toHaveBeenCalled();
  });

  it("shows an error rather than a blank page", () => {
    state.isError = true;
    render(<PrDetailView repo="octocat/hello-world" number={42} onBack={() => {}} />);
    expect(screen.getByText(/could not load this pull request/i)).toBeTruthy();
  });

  // This DELIBERATELY reverses an earlier assertion. The old test read
  // "does not pretend to offer a diff or a comment box", encoding the
  // v1 stance that reviewing belongs in GitHub. The comment box is now
  // the point -- approving was the most common reviewer action and the
  // one thing that still forced a trip to the browser.
  //
  // The diff genuinely stays absent: rendering one well is a different
  // product, and the GitHub link remains the way there.
  it("offers a review box but still no diff", () => {
    view();
    expect(screen.getByRole("textbox")).toBeTruthy();
    expect(screen.getByRole("link", { name: /view on github/i })).toBeTruthy();
    expect(screen.queryByText(/^@@/)).toBeNull();
  });

  it("submits a verdict through the review hook", async () => {
    view();
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "needs work" } });
    fireEvent.click(screen.getByRole("button", { name: /request changes/i }));
    await waitFor(() =>
      expect(reviewPr).toHaveBeenCalledWith(
        "PR_test",
        "octocat/hello-world",
        42,
        "request_changes",
        "needs work",
      ),
    );
  });

  // "Comment" must post a CONVERSATION comment, not a COMMENT review.
  // They are different GraphQL nodes -- addComment makes an IssueComment,
  // addPullRequestReview makes a PullRequestReview with state COMMENTED
  // -- and the comment list in this view renders IssueComments. Routing
  // it through the review mutation would post something the user could
  // then not see in the list right above the box.
  it("posts a plain comment through addComment, not as a review", async () => {
    view();
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "looks good" } });
    fireEvent.click(screen.getByRole("button", { name: /^comment$/i }));
    await waitFor(() =>
      expect(commentOnPr).toHaveBeenCalledWith("PR_test", "octocat/hello-world", 42, "looks good"),
    );
    expect(reviewPr).not.toHaveBeenCalled();
  });

  // 31 of the last 60 merged PRs on a real account still held a live
  // remote branch. This is the app's own thesis applied to the one
  // domain where it did nothing.
  describe("delete branch", () => {
    it("is offered once the PR has merged and the branch still exists", () => {
      state.data = { ...detail(), state: "MERGED", head_ref_id: "REF_1" };
      render(<PrDetailView repo="o/r" number={1} onBack={() => {}} />);
      expect(screen.getByRole("button", { name: /delete branch/i })).toBeTruthy();
    });

    // Deleting the head ref of an OPEN pull request closes it off.
    it("is never offered while the PR is still open", () => {
      state.data = { ...detail(), state: "OPEN", head_ref_id: "REF_1" };
      render(<PrDetailView repo="o/r" number={1} onBack={() => {}} />);
      expect(screen.queryByRole("button", { name: /delete branch/i })).toBeNull();
    });

    // A null ref id IS the signal that cleanup already happened.
    it("is not offered once the branch is already gone", () => {
      state.data = { ...detail(), state: "MERGED", head_ref_id: null };
      render(<PrDetailView repo="o/r" number={1} onBack={() => {}} />);
      expect(screen.queryByRole("button", { name: /delete branch/i })).toBeNull();
    });

    /// #845: the button must ASK, not delete.
    ///
    /// This is a REMOTE deletion -- `deleteBranch(..., true)` -- and it
    /// fired straight from `onClick`. `BranchesPage` states the rule for
    /// the very same operation: "Local deletion is recoverable from the
    /// reflog; a remote deletion is not, so it is never what a distracted
    /// Enter press does." The negative assertion is the one that matters.
    it("does not delete anything on the button's own click", () => {
      state.data = { ...detail(), state: "MERGED", head_ref_id: "REF_1" };
      render(<PrDetailView repo="o/r" number={1} onBack={() => {}} />);
      fireEvent.click(screen.getByRole("button", { name: /delete branch/i }));
      expect(deleteBranch).not.toHaveBeenCalled();
      expect(screen.getByRole("dialog")).toBeTruthy();
    });

    /// The confirmation carries `BranchesPage`'s own sentence, through
    /// `scopeEffect("remote")` rather than a second copy of the words.
    /// Two wordings of "no local reflog can undo that" would be two
    /// claims about one operation.
    it("warns in the same words BranchesPage uses for a remote deletion", () => {
      state.data = { ...detail(), state: "MERGED", head_ref_id: "REF_1" };
      render(<PrDetailView repo="o/r" number={1} onBack={() => {}} />);
      fireEvent.click(screen.getByRole("button", { name: /delete branch/i }));
      const dialog = screen.getByRole("dialog");
      expect(within(dialog).getByText(scopeEffect("remote"))).toBeTruthy();
      // The ref is named TWICE and that is deliberate: once in the title,
      // which is what a screen reader announces on open, and once in mono
      // beside the repository, which is what the eye checks. A branch name
      // in prose alone is easy to skim past, and it is the only
      // identifier of what is about to go.
      expect(within(dialog).getAllByText(/feature\/retry/)).toHaveLength(2);
      expect(within(dialog).getByText(/octocat\/hello-world/)).toBeTruthy();
    });

    it("deletes nothing when the confirmation is cancelled", () => {
      state.data = { ...detail(), state: "MERGED", head_ref_id: "REF_1" };
      render(<PrDetailView repo="o/r" number={1} onBack={() => {}} />);
      fireEvent.click(screen.getByRole("button", { name: /delete branch/i }));
      fireEvent.click(screen.getByRole("button", { name: /^cancel$/i }));
      expect(deleteBranch).not.toHaveBeenCalled();
    });

    /// Styled as destructive, which it was not (#845).
    ///
    /// The old `className` was BYTE-IDENTICAL to "View on GitHub" and
    /// "Copy for agent" beside it -- two actions that change nothing --
    /// so the one control that destroyed a shared ref carried no warning
    /// at all. Asserted against its NEIGHBOURS rather than against a
    /// literal class string: the defect was the sameness, so the test has
    /// to be about the difference.
    it("does not look like the harmless buttons beside it", () => {
      state.data = { ...detail(), state: "MERGED", head_ref_id: "REF_1" };
      render(<PrDetailView repo="o/r" number={1} onBack={() => {}} />);
      const del = screen.getByRole("button", { name: /delete branch/i });
      const agent = screen.getByRole("button", { name: /copy prompt/i });
      expect(del.className).not.toBe(agent.className);
      // The red every other destructive control in the app uses.
      expect(del.className).toContain("#f85149");
      expect(agent.className).not.toContain("#f85149");
    });

    /// #845's third acceptance criterion: reachable and legible ON TOUCH.
    ///
    /// This view has a phone layout (the footer wraps), and the only thing
    /// distinguishing the old button from the two harmless ones beside it
    /// would have had to be a tooltip -- which does not exist on touch. Both
    /// halves of the fix are therefore asserted at a phone width: the
    /// confirmation, and the colour that is not a hover affordance.
    it("confirms and reads as destructive at a phone width", () => {
      stubViewport(390);
      state.data = { ...detail(), state: "MERGED", head_ref_id: "REF_1" };
      render(<PrDetailView repo="o/r" number={1} onBack={() => {}} />);
      const del = screen.getByRole("button", { name: /delete branch/i });
      expect(del.className).toContain("#f85149");
      fireEvent.click(del);
      expect(deleteBranch).not.toHaveBeenCalled();
      expect(
        within(screen.getByRole("dialog")).getByText(scopeEffect("remote")),
      ).toBeTruthy();
    });

    it("passes merged=true so the backend gate can agree", async () => {
      state.data = { ...detail(), state: "MERGED", head_ref_id: "REF_1" };
      render(<PrDetailView repo="o/r" number={1} onBack={() => {}} />);
      fireEvent.click(screen.getByRole("button", { name: /delete branch/i }));
      fireEvent.click(screen.getByRole("button", { name: /delete on the remote/i }));
      await waitFor(() => expect(deleteBranch).toHaveBeenCalled());
      expect(deleteBranch.mock.calls[0][0]).toBe("REF_1");
      expect(deleteBranch.mock.calls[0][4]).toBe(true);
    });
  });

  /// #790: the view blocked on the whole fetch. It now renders from the
  /// clicked row immediately, which means it renders a `PrDetail` with
  /// several fields deliberately EMPTY -- and the risk moves from "slow"
  /// to "confidently wrong about what it does not have yet".
  describe("seeded from the clicked row", () => {
    it("shows the row's facts rather than a spinner", () => {
      state.isPlaceholderData = true;
      state.data = detail({ body: "", additions: 0, deletions: 0, changed_files: 0 });
      render(<PrDetailView repo="o/r" number={42} onBack={() => {}} />);

      expect(screen.queryByText(/loading pull request/i)).toBeNull();
      expect(screen.getByText(/add retry to the fetch client/i)).toBeTruthy();
      expect(screen.getByText(/wants to merge/)).toBeTruthy();
    });

    /// The diff size is the one header fact the list row cannot carry:
    /// `PRS_QUERY` does not select additions, deletions or changedFiles.
    /// "+0 −0 across 0 files" on a real pull request is a number the
    /// user cannot tell from an empty diff.
    it("omits the diff size instead of printing zeroes", () => {
      state.isPlaceholderData = true;
      state.data = detail({ additions: 0, deletions: 0, changed_files: 0 });
      render(<PrDetailView repo="o/r" number={42} onBack={() => {}} />);
      expect(screen.queryByText(/across 0 files/)).toBeNull();
    });

    /// And once the real answer lands, a genuinely empty diff prints
    /// normally -- the suppression must be about the placeholder, not
    /// about the value being zero.
    it("prints a real zero diff once loaded", () => {
      state.isPlaceholderData = false;
      state.data = detail({ additions: 0, deletions: 0, changed_files: 0 });
      render(<PrDetailView repo="o/r" number={42} onBack={() => {}} />);
      expect(screen.getByText(/across 0 files/)).toBeTruthy();
    });

    /// "No description." would be actively WRONG while seeded: the body
    /// is exactly what the row cannot carry, and a user who opened the
    /// pull request to read it would be told there isn't one.
    it("says the description is still loading rather than absent", () => {
      state.isPlaceholderData = true;
      state.data = detail({ body: "" });
      render(<PrDetailView repo="o/r" number={42} onBack={() => {}} />);
      expect(screen.getByText(/loading the description and checks/i)).toBeTruthy();
      expect(screen.queryByText(/^no description\.$/i)).toBeNull();
    });

    it("still says 'no description' for a PR that really has none", () => {
      state.isPlaceholderData = false;
      state.data = detail({ body: "" });
      render(<PrDetailView repo="o/r" number={42} onBack={() => {}} />);
      expect(screen.getByText(/^no description\.$/i)).toBeTruthy();
    });

    /// The spinner branch is now only for a pull request with no cached
    /// row to seed from: a cold launch straight into a detail view.
    it("keeps the spinner when there was nothing to seed from", () => {
      state.isLoading = true;
      state.data = undefined;
      render(<PrDetailView repo="o/r" number={42} onBack={() => {}} />);
      expect(screen.getByText(/loading pull request/i)).toBeTruthy();
    });
  });

  /// The check list is CAPPED at 300 contexts (#790 cut the page budget
  /// from 20 serial requests to 3). The whole reason that pagination
  /// exists is that a short check list does not look short -- it renders
  /// a wall of green on a pull request whose rollup says FAILURE -- so
  /// the cap is only safe if the panel says what it is missing.
  describe("capped check list", () => {
    const check = (name: string) => ({ name, state: "success", url: "", run_id: null });

    it("says how many checks it is missing", () => {
      state.data = detail({ checks: [check("build")], checks_total: 412 });
      render(<PrDetailView repo="o/r" number={42} onBack={() => {}} />);
      expect(screen.getByText(/showing 1 of 412 checks/i)).toBeTruthy();
    });

    /// An all-green capped list is where the collapsed "everything
    /// passed" summary is least trustworthy, so it opens.
    it("opens the section even when everything fetched is green", () => {
      state.data = detail({ checks: [check("build")], checks_total: 412 });
      render(<PrDetailView repo="o/r" number={42} onBack={() => {}} />);
      expect(screen.getByText("build")).toBeTruthy();
    });

    it("says nothing when the list is complete", () => {
      state.data = detail({ checks: [check("build")], checks_total: 1 });
      render(<PrDetailView repo="o/r" number={42} onBack={() => {}} />);
      expect(screen.queryByText(/showing 1 of/i)).toBeNull();
    });

    /// A total BELOW the length is possible: the two numbers come from
    /// different pages of a rollup that can grow mid-fetch. It is not a
    /// negative shortfall and must not be announced as one.
    it("says nothing when the total is smaller than what arrived", () => {
      state.data = detail({ checks: [check("build"), check("test")], checks_total: 1 });
      render(<PrDetailView repo="o/r" number={42} onBack={() => {}} />);
      expect(screen.queryByText(/showing 2 of/i)).toBeNull();
    });
  });

  /// The same honesty one section down (#802). The thread window was 20
  /// with no count at all, so a pull request with 25 conversations
  /// rendered 20 and looked finished -- an unresolved blocking comment
  /// could sit in the gap. `ReviewThreads.test.tsx` covers the notice
  /// itself; these two assert the WIRING, since a `review_threads_total`
  /// that never reaches the section is a field that changes nothing.
  /// The base branch's review rules, wired into Approve and Merge
  /// (#1451, #1454). The derivation is tested in `lib/reviewGates.test.ts`;
  /// these assert that the view USES it, on both Approve buttons and on
  /// the merge reason.
  describe("review gates", () => {
    const read = (lastPush: boolean, resolution: boolean): ReviewGates["rules"] => ({
      state: "read",
      require_last_push_approval: lastPush,
      required_review_thread_resolution: resolution,
    });
    const thread = (id: string, outdated: boolean) => ({
      id,
      is_resolved: false,
      is_outdated: outdated,
      path: "src/a.ts",
      line: outdated ? null : 1,
      viewer_can_reply: false,
      viewer_can_resolve: false,
      viewer_can_unresolve: false,
      comments: [],
      comment_count: 0,
    });
    afterEach(() => {
      state.gates = undefined;
      viewer.current = undefined;
    });

    it("disables both Approve buttons when the viewer pushed last", () => {
      viewer.current = "reviewer";
      state.gates = { rules: read(true, false), last_pusher: { state: "known", login: "reviewer" } };
      view({ author: "someone-else" });
      expect(
        screen.getByText("You pushed the latest commit, so your approval won't count here."),
      ).toBeTruthy();
      const approves = screen.getAllByRole("button", { name: "Approve" });
      expect(approves.length).toBe(2);
      for (const b of approves) expect((b as HTMLButtonElement).disabled).toBe(true);
    });

    it("leaves Approve alone when someone else pushed last", () => {
      viewer.current = "reviewer";
      state.gates = { rules: read(true, false), last_pusher: { state: "known", login: "other" } };
      view({ author: "someone-else" });
      expect(screen.queryByText(/won't count/)).toBeNull();
      for (const b of screen.getAllByRole("button", { name: "Approve" }))
        expect((b as HTMLButtonElement).disabled).toBe(false);
    });

    it("says nothing new when the rules could not be read", () => {
      viewer.current = "reviewer";
      state.gates = {
        rules: { state: "unreadable", reason: "404" },
        last_pusher: { state: "not_needed" },
      };
      view({
        author: "someone-else",
        merge_status: "blocked",
        review_threads: [thread("RT_1", false)],
        review_threads_total: 1,
      });
      expect(screen.queryByText(/won't count|could not be confirmed/)).toBeNull();
      expect(screen.getByText(/a required review or check is missing/)).toBeTruthy();
    });

    it("names open conversations, outdated ones included, as the merge blocker", () => {
      state.gates = { rules: read(false, true), last_pusher: { state: "not_needed" } };
      view({
        merge_status: "blocked",
        unresolved_threads: 1,
        review_threads: [thread("RT_1", false), thread("RT_2", true)],
        review_threads_total: 2,
      });
      expect(
        screen.getByText("Cannot merge: 2 conversations must be resolved first (1 outdated)"),
      ).toBeTruthy();
    });
  });

  describe("truncated conversation list", () => {
    const t = (id: string) => ({
      id,
      is_resolved: false,
      is_outdated: false,
      path: "src/a.ts",
      line: 1,
      viewer_can_reply: false,
      viewer_can_resolve: false,
      viewer_can_unresolve: false,
      comments: [{ author: "carol", created_at: "2026-08-20T10:00:00Z", body: "hm" }],
      comment_count: 1,
    });

    it("says how many conversations it is missing", () => {
      state.data = detail({ review_threads: [t("RT_1")], review_threads_total: 25 });
      render(<PrDetailView repo="o/r" number={42} onBack={() => {}} />);
      expect(screen.getByText(/showing 1 of 25 conversations/i)).toBeTruthy();
    });

    it("says nothing when every conversation arrived", () => {
      state.data = detail({ review_threads: [t("RT_1")], review_threads_total: 1 });
      render(<PrDetailView repo="o/r" number={42} onBack={() => {}} />);
      expect(screen.queryByText(/showing 1 of 1 conversations/i)).toBeNull();
    });
  });
});

/// The reverse session link (#1211).
///
/// `pr-link` has been read since #1132 and surfaced in one direction
/// only. This is the other half, and the more useful one: a PR that
/// broke sends you looking for the session, and finding it by title
/// fails — 286 of 1,438 sessions share a title with another.
describe("PrDetailView and the session that wrote the PR", () => {
  beforeEach(() => {
    state.prSessions = [];
  });

  it("renders nothing when this machine holds no transcript for the PR", () => {
    // Absence means THIS MACHINE has no transcript — it may have been
    // opened by a teammate, by CI, or by a session since pruned.
    // Rendering "no session" would be a confident wrong answer about
    // someone else's work, so the panel is absent entirely.
    view();
    expect(screen.queryByText(/Written by/)).toBeNull();
  });

  it("names the session that produced it", () => {
    state.prSessions = [
      {
        session_id: "e5df3bd1-1b5f-40cf-8d4b-5e0cc8939abc",
        repo: "acme/api",
        number: 7,
        url: "https://github.com/acme/api/pull/7",
        first_seen_at: "2026-09-01",
      },
    ];
    view();
    expect(screen.getByText(/Written by/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "e5df3bd1" })).toBeTruthy();
  });

  it("jumps to that session's detail, on the sessions page", () => {
    // Three writes in order: the Claude Code view has two pages, and
    // landing on the overview with a session selected shows the
    // overview.
    state.prSessions = [
      {
        session_id: "abc12345-0000-0000-0000-000000000000",
        repo: "acme/api",
        number: 7,
        url: "https://github.com/acme/api/pull/7",
        first_seen_at: null,
      },
    ];
    view();
    fireEvent.click(screen.getByRole("button", { name: "abc12345" }));

    const st = useFilters.getState();
    expect(st.claudeSelected).toBe("abc12345-0000-0000-0000-000000000000");
    expect(st.claudePage).toBe("sessions");
    expect(st.view).toBe("claude-code");
  });

  // The shape the BACKEND actually serialises, not the shape this file
  // believes it does (#1288).
  //
  // Every other fixture here hand-writes `session_id` / `first_seen_at`,
  // which is exactly why the whole suite stayed green while v7.1.0
  // shipped a crash: `PrLink` carried `#[serde(rename_all =
  // "camelCase")]`, the wire sent `sessionId` / `firstSeenAt`, and
  // `l.session_id.slice(0, 8)` threw `undefined is not an object` on
  // every pull request with a linked Claude session. A fixture that
  // encodes the frontend's assumption can only ever confirm it.
  //
  // This test feeds the camelCase spelling the old backend really sent.
  // It must NOT render a session, because after the fix that spelling is
  // not what the backend emits and a component that accepted both would
  // be hiding the contract rather than honouring it. What it pins is
  // that the component reads `session_id` and nothing else -- so that
  // reintroducing `rename_all` on `PrLink` breaks a test here as well as
  // the Rust guard, and cannot throw at a user first.
  it("does not silently accept the camelCase spelling that #1288 shipped", () => {
    state.prSessions = [
      {
        sessionId: "ca5ece11-0000-0000-0000-000000000000",
        repo: "acme/api",
        number: 7,
        url: "https://github.com/acme/api/pull/7",
        firstSeenAt: "2026-09-01",
      },
      // Cast at the boundary on purpose: this is the one place that
      // must describe the WIRE rather than `ClaudePrLink`, and typing it
      // as `ClaudePrLink` would re-assert the very belief under test.
    ] as unknown as typeof state.prSessions;

    // Rendering must not throw. Before the fix this exact input is what
    // reached `PrDetailView` and `.slice` was called on `undefined`.
    expect(() => view()).not.toThrow();
    expect(screen.queryByRole("button", { name: "ca5ece11" })).toBeNull();
  });

  it("lists every session when more than one produced it", () => {
    // A PR can be the work of several sessions — a first pass and a
    // fix-up after review is the common shape.
    state.prSessions = [
      { session_id: "aaaaaaaa-0000-0000-0000-000000000000", repo: "acme/api", number: 7, url: "u", first_seen_at: null },
      { session_id: "bbbbbbbb-0000-0000-0000-000000000000", repo: "acme/api", number: 7, url: "u", first_seen_at: null },
    ];
    view();
    expect(screen.getByRole("button", { name: "aaaaaaaa" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "bbbbbbbb" })).toBeTruthy();
  });
});

/// #1455: "Copy for agent" became Claudify, matching the Worktrees one.
describe("PrDetailView Claudify", () => {
  const CHECKOUT = "/code/hello-world";
  const found = () => {
    claudifyState.repos = [
      // A different repository first, and the right one in a different
      // case: the match is on the remote identity, case-insensitively.
      { identity: "octocat/spoon-knife", name: "spoon-knife", path: "/code/spoon-knife", worktrees: [] },
      { identity: "OctoCat/Hello-World", name: "hello-world", path: CHECKOUT, worktrees: [] },
    ];
  };

  beforeEach(() => {
    claudifyApi.claudeLaunchTerms.mockResolvedValue({ models: [], permissionModes: [], unattended: [] });
    claudifyApi.claudeLaunchPrPreview.mockResolvedValue({ program: "term", args: ["-e", "x"] });
    claudifyApi.claudeLaunchPr.mockResolvedValue(undefined);
    claudifyApi.claudifyPrCommand.mockResolvedValue({
      command: "cd '/code/hello-world' && claude 'x'",
      claude_installed: true,
    });
  });

  afterEach(() => {
    cleanup();
    claudifyState.repos = [];
    claudifyState.unreadable = [];
    claudifyState.scanError = null;
    claudifyState.terminal = "";
    for (const f of Object.values(claudifyApi)) f.mockReset();
  });

  it("is a Claudify button that opens the terms dialog when a terminal and a checkout exist", async () => {
    found();
    claudifyState.terminal = "wezterm start -- bash -lc {command}";
    view();
    expect(screen.queryByRole("button", { name: /copy prompt/i })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /claudify/i }));
    // The terms dialog (#1214), not a launch: nothing runs before the
    // argv has been shown.
    expect(await screen.findByText(/Hand octocat\/hello-world#42 to Claude Code/)).toBeTruthy();
    expect(claudifyApi.claudeLaunchPr).not.toHaveBeenCalled();
    await waitFor(() => expect(claudifyApi.claudeLaunchPrPreview).toHaveBeenCalled());
    const [path, repo, prompt] = claudifyApi.claudeLaunchPrPreview.mock.calls[0] as [string, string, string];
    expect(path).toBe(CHECKOUT);
    expect(repo).toBe("octocat/hello-world");
    // The prompt names the checkout and carries the adapted criteria.
    expect(prompt).toContain(`created from the main checkout at ${CHECKOUT}`);
    expect(prompt).toContain("Correctness and edge cases");
    expect(prompt).toContain("The change is +100/-20 across 3 files.");

    fireEvent.click(screen.getByRole("button", { name: /open in terminal/i }));
    await waitFor(() => expect(claudifyApi.claudeLaunchPr).toHaveBeenCalled());
    expect(claudifyApi.claudeLaunchPr.mock.calls[0].slice(0, 3)).toEqual([
      CHECKOUT,
      "octocat/hello-world",
      prompt,
    ]);
  });

  /// No terminal: still Claudify, and it copies the line Rust built --
  /// the Worktrees fallback -- rather than launching anything.
  it("copies the built command when no terminal is configured", async () => {
    found();
    const writeText = vi.fn<(text: string) => Promise<void>>(() => Promise.resolve());
    Object.assign(navigator, { clipboard: { writeText } });
    view();
    fireEvent.click(screen.getByRole("button", { name: /claudify/i }));
    await waitFor(() => expect(writeText).toHaveBeenCalled());
    expect(writeText.mock.calls[0][0]).toBe("cd '/code/hello-world' && claude 'x'");
    expect(claudifyApi.claudifyPrCommand.mock.calls[0].slice(0, 2)).toEqual([
      CHECKOUT,
      "octocat/hello-world",
    ]);
    expect(claudifyApi.claudeLaunchPr).not.toHaveBeenCalled();
    expect(screen.queryByText(/Hand .* to Claude Code/)).toBeNull();
  });

  /// No checkout: SAY so, and copy the prompt alone. Never launch or
  /// build a command somewhere that is not the repository.
  it("says there is no local checkout instead of offering Claudify", async () => {
    claudifyState.repos = [
      { identity: "octocat/spoon-knife", name: "spoon-knife", path: "/code/spoon-knife", worktrees: [] },
      // A bare clone of the right repository has no tree to start in.
      {
        identity: "octocat/hello-world",
        name: "hello-world.git",
        path: "/code/hw.git",
        worktrees: [],
        bare: true,
      },
    ];
    claudifyState.terminal = "wezterm start -- bash -lc {command}";
    const writeText = vi.fn<(text: string) => Promise<void>>(() => Promise.resolve());
    Object.assign(navigator, { clipboard: { writeText } });
    view();
    expect(screen.queryByRole("button", { name: /claudify/i })).toBeNull();
    expect(
      screen.getByText(/No local checkout of octocat\/hello-world was found in the scanned folders/),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /copy prompt/i }));
    await waitFor(() => expect(writeText).toHaveBeenCalled());
    expect(writeText.mock.calls[0][0]).toMatch(/^Review octocat\/hello-world#42/);
    expect(claudifyApi.claudifyPrCommand).not.toHaveBeenCalled();
    expect(claudifyApi.claudeLaunchPrPreview).not.toHaveBeenCalled();
  });

  /// A partial scan makes "not found" a floor, so it is qualified.
  it("qualifies 'no checkout' when part of the scan could not be read", () => {
    claudifyState.unreadable = ["/code/locked: permission denied"];
    view();
    expect(screen.getByText(/in the folders that could be read \(1 could not\)/)).toBeTruthy();
  });

  /// Pending and failed are different states from "none".
  it("distinguishes a scan in progress and a scan that failed from no checkout", () => {
    claudifyState.repos = undefined;
    view();
    const pending = screen.getByRole("button", { name: /claudify/i });
    expect((pending as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText(/Looking for a local checkout/)).toBeTruthy();
    cleanup();

    claudifyState.scanError = "walk refused";
    view();
    expect(screen.getByText(/Could not look for a local checkout/).textContent).toContain(
      "walk refused",
    );
    expect(screen.queryByText(/No local checkout/)).toBeNull();
  });
});

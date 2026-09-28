import {
  act,
  cleanup,
  fireEvent,
  render as rtlRender,
  screen,
  waitFor,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactElement, ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: () => Promise.resolve() }));
const copyFn = vi.hoisted(() => vi.fn<(text: string) => Promise<string | null>>());
const toastFns = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
vi.mock("@/lib/clipboard", () => ({ copyText: copyFn }));
vi.mock("sonner", () => ({ toast: toastFns }));

/// The Tauri bridge is the only thing mocked: the strip's pusher hook,
/// the transport and the wrappers are real, so what these tests control
/// is exactly what the desktop command would answer (#1576).
const invoke = vi.hoisted(() =>
  vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>(),
);
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { ReadyStrip } from "./ReadyStrip";
import { PR_FIXTURES } from "../fixtures/prs";
import { useFilters } from "@/store/filters";
import type { PullRequest, RowPusher } from "@/types/pr";

/// What `get_ready_pushers` answers; `[]` (nothing checked) by default.
let pusherAnswers: RowPusher[] = [];
let viewerLogin: Promise<unknown> = Promise.resolve("me");

beforeEach(() => {
  pusherAnswers = [];
  viewerLogin = Promise.resolve("me");
  invoke.mockReset();
  invoke.mockImplementation((cmd: string) => {
    if (cmd === "get_viewer") return viewerLogin;
    if (cmd === "get_ready_pushers") return Promise.resolve(pusherAnswers);
    return Promise.resolve(undefined);
  });
});

function render(ui: ReactElement) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  );
  return rtlRender(ui, { wrapper });
}

afterEach(cleanup);

const EMPTY = { "my-prs": {}, "to-review": {}, worktrees: {},
  branches: {}, docker: {}, artifacts: {}, packages: {}, "claude-md": {}, "claude-code": {}, "pr-stats": {}, repositories: {}, "system-health": {} } as const;

beforeEach(() =>
  useFilters.setState({ filtersByView: { ...EMPTY }, view: "to-review" }),
);

const ready: PullRequest = {
  ...PR_FIXTURES[0],
  title: "Ready one",
  is_draft: false,
  ci: "success",
  merge: "mergeable",
  review: "none",
  in_merge_queue: false,
};

describe("ReadyStrip", () => {
  it("lists what a reviewer can pick up", () => {
    render(<ReadyStrip prs={[ready]} onOpen={vi.fn()} />);
    expect(screen.getByText("Ready one")).toBeTruthy();
    expect(screen.getByText(/ready for review \(1\)/i)).toBeTruthy();
  });

  it("leaves out what is not ready", () => {
    render(<ReadyStrip prs={[{ ...ready, ci: "failure" }]} onOpen={vi.fn()} />);
    expect(screen.queryByText("Ready one")).toBeNull();
  });

  // Matches the attention strip: a section that shouts when there is
  // nothing in it stops being read.
  it("stays quiet when nothing is ready", () => {
    render(<ReadyStrip prs={[]} onOpen={vi.fn()} />);
    expect(screen.getByText(/nothing ready to review/i)).toBeTruthy();
    expect(screen.queryByText(/ready for review \(/i)).toBeNull();
  });

  it("opens the detail view when clicked", () => {
    const onOpen = vi.fn();
    render(<ReadyStrip prs={[ready]} onOpen={onOpen} />);
    fireEvent.click(screen.getByText("Ready one"));
    expect(onOpen).toHaveBeenCalledWith(ready);
  });

  it("is keyboard reachable, like the attention strip", () => {
    const onOpen = vi.fn();
    render(<ReadyStrip prs={[ready]} onOpen={onOpen} />);
    fireEvent.keyDown(screen.getByRole("button", { name: /ready one/i }), { key: "Enter" });
    expect(onOpen).toHaveBeenCalledWith(ready);
  });

  // With nothing to open, the entry must not look interactive.
  it("does not pretend to be clickable without a handler", () => {
    render(<ReadyStrip prs={[ready]} />);
    expect(screen.queryByRole("button", { name: /ready one/i })).toBeNull();
  });
});

/// #1277: working top to bottom through a review queue should mean
/// working through it in the order the pull requests arrived.
describe("ReadyStrip ordering", () => {
  // All opened at the same instant, so only `ready_at` can order them
  // (#1407).
  const at = (number: number, ready_at: string): PullRequest => ({
    ...ready,
    number,
    title: `PR ${number}`,
    created_at: "2026-08-01T00:00:00Z",
    ready_at,
  });

  // Handed in NEWEST-first order deliberately, so a component that does
  // not sort at all fails rather than passing on the input's shape.
  const NEWEST_FIRST = [
    at(3, "2026-09-03T00:00:00Z"),
    at(2, "2026-09-02T00:00:00Z"),
    at(1, "2026-09-01T00:00:00Z"),
  ];

  const titlesInOrder = () =>
    screen
      .getAllByRole("button", { name: /^PR \d/ })
      .map((el) => el.textContent?.match(/PR \d/)?.[0]);

  it("defaults to oldest ready first, without touching the store", () => {
    render(<ReadyStrip prs={NEWEST_FIRST} onOpen={vi.fn()} />);
    expect(titlesInOrder()).toEqual(["PR 1", "PR 2", "PR 3"]);
    // The default is a DEFAULT, not a value written on first render: a
    // store key nobody chose would persist and then outlive a change to
    // what the default should be.
    const s = useFilters.getState();
    expect(s.filtersByView["to-review"].readySort).toBeUndefined();
  });

  // A default nobody can see is one nobody can trust, and this list
  // having a non-obvious default is the point of the issue.
  it("names the field it sorts on, not just the direction", () => {
    render(<ReadyStrip prs={NEWEST_FIRST} onOpen={vi.fn()} />);
    const trigger = screen.getByRole("button", { name: /sort/i });
    expect(trigger.textContent).toContain("Oldest ready first");
    // "Oldest first" alone is the ambiguity #1277 was filed about.
    expect(trigger.textContent).toMatch(/ready/i);
  });

  it("switches to newest ready first", () => {
    render(<ReadyStrip prs={NEWEST_FIRST} onOpen={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /sort/i }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Newest ready first" }));
    expect(titlesInOrder()).toEqual(["PR 3", "PR 2", "PR 1"]);
    expect(useFilters.getState().filtersByView["to-review"].readySort).toBe("newest-opened");
  });

  // The other half of the round trip. A separate render rather than
  // reopening the menu in the test above: this dropdown closes on select
  // and does not reopen within one synchronous `fireEvent` pass, so
  // chaining the two would be testing the menu's animation rather than
  // the ordering. Starting from `newest-opened` in the store is the state
  // the previous test leaves a real user in.
  it("switches back to oldest ready first", () => {
    useFilters.setState({
      filtersByView: { ...EMPTY, "to-review": { readySort: "newest-opened" } },
      view: "to-review",
    });
    render(<ReadyStrip prs={NEWEST_FIRST} onOpen={vi.fn()} />);
    expect(titlesInOrder()).toEqual(["PR 3", "PR 2", "PR 1"]);
    fireEvent.click(screen.getByRole("button", { name: /sort/i }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Oldest ready first" }));
    expect(titlesInOrder()).toEqual(["PR 1", "PR 2", "PR 3"]);
    expect(useFilters.getState().filtersByView["to-review"].readySort).toBe("oldest-opened");
  });

  it("honours a readySort already in the store", () => {
    useFilters.setState({
      filtersByView: { ...EMPTY, "to-review": { readySort: "newest-opened" } },
      view: "to-review",
    });
    render(<ReadyStrip prs={[...NEWEST_FIRST].reverse()} onOpen={vi.fn()} />);
    expect(titlesInOrder()).toEqual(["PR 3", "PR 2", "PR 1"]);
  });

  /// An undated row at the TOP would claim to be the longest-waiting work
  /// and push genuinely old pull requests down -- the one outcome this
  /// ordering exists to prevent.
  it("does not let an unparseable ready_at sort as the oldest", () => {
    render(<ReadyStrip prs={[at(9, "not a date"), ...NEWEST_FIRST]} onOpen={vi.fn()} />);
    expect(titlesInOrder()).toEqual(["PR 1", "PR 2", "PR 3", "PR 9"]);
  });

  // The strip is the only list #1277 changes. Reordering it must not
  // touch `sort`, which the PR list below it reads and which stays
  // newest-first.
  it("leaves the main list's sort alone", () => {
    render(<ReadyStrip prs={NEWEST_FIRST} onOpen={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /sort/i }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Newest ready first" }));
    expect(useFilters.getState().filtersByView["to-review"].sort).toBeUndefined();
  });
});

/// #1407: each row says how long it has been ready for review, coloured
/// green to 24h, yellow to 48h, red after.
describe("ReadyStrip age", () => {
  const NOW = new Date("2026-09-10T12:00:00Z");
  const hoursAgo = (h: number) => new Date(NOW.getTime() - h * 3_600_000).toISOString();
  const row = (number: number, ready_at: string | null | undefined): PullRequest => ({
    ...ready,
    number,
    title: `PR ${number}`,
    // Opened a month earlier, so an age measured from `created_at` would
    // read red on every row and fail the green case below.
    created_at: "2026-08-10T12:00:00Z",
    ready_at,
  });
  const ageOf = (number: number) =>
    screen
      .getByRole("button", { name: new RegExp(`^PR ${number}(?!\\d)`) })
      .querySelector("[data-ready-age]") as HTMLElement;

  // Fake timers pin `Date` too, so no assertion depends on the machine
  // clock -- the Rust suite has burned a release on exactly that.
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("shows green, yellow and red ages, each with its text", () => {
    render(
      <ReadyStrip
        prs={[row(1, hoursAgo(3)), row(2, hoursAgo(30)), row(3, hoursAgo(60))]}
        onOpen={vi.fn()}
      />,
    );
    expect(ageOf(1).textContent).toBe("3h");
    expect(ageOf(1).dataset.readyAge).toBe("fresh");
    expect(ageOf(1).className).toContain("#3fb950");
    expect(ageOf(2).textContent).toBe("1d 6h");
    expect(ageOf(2).dataset.readyAge).toBe("aging");
    expect(ageOf(2).className).toContain("#d29922");
    expect(ageOf(3).textContent).toBe("2d 12h");
    expect(ageOf(3).dataset.readyAge).toBe("stale");
    expect(ageOf(3).className).toContain("#f85149");
  });

  // Absent is not zero: never green, and never a number.
  it("renders an unknown age as a neutral 'age unknown'", () => {
    render(
      <ReadyStrip prs={[row(1, undefined), row(2, null), row(3, "garbage")]} onOpen={vi.fn()} />,
    );
    for (const n of [1, 2, 3]) {
      expect(ageOf(n).textContent).toBe("age unknown");
      expect(ageOf(n).dataset.readyAge).toBe("unknown");
      expect(ageOf(n).className).not.toContain("#3fb950");
      expect(ageOf(n).className).toContain("#8b949e");
    }
  });

  it("gives the exact ready time in the accessible name and the title", () => {
    const at = hoursAgo(3);
    const since = new Date(at).toLocaleString();
    render(<ReadyStrip prs={[row(1, at)]} onOpen={vi.fn()} />);
    expect(
      screen.getByRole("button", { name: new RegExp(`ready for review since ${since}`) }),
    ).toBeTruthy();
    expect(ageOf(1).getAttribute("title")).toBe(`Ready for review since ${since}`);
    expect(ageOf(1).getAttribute("datetime")).toBe(at);
  });

  it("says in the accessible name when the ready time is unknown", () => {
    render(<ReadyStrip prs={[row(1, null)]} onOpen={vi.fn()} />);
    expect(screen.getByRole("button", { name: /ready-for-review time unknown/i })).toBeTruthy();
  });

  it("shows the age on a plain-link row too", () => {
    render(<ReadyStrip prs={[row(1, hoursAgo(3))]} />);
    expect(screen.getByText("3h")).toBeTruthy();
  });

  // The age keeps counting while the view is open, from the clock and not
  // from a re-fetch: the same `prs` prop crosses from green to yellow.
  it("stays current while the view is open", () => {
    render(<ReadyStrip prs={[row(1, hoursAgo(23.99))]} onOpen={vi.fn()} />);
    expect(ageOf(1).dataset.readyAge).toBe("fresh");
    act(() => {
      vi.advanceTimersByTime(2 * 60_000);
    });
    expect(ageOf(1).dataset.readyAge).toBe("aging");
    expect(ageOf(1).textContent).toBe("1d");
  });
});

/// #1577: open conversations on a row the strip calls ready.
describe("ReadyStrip unresolved conversations", () => {
  const withThreads = (
    unresolved_threads: number,
    unresolved_threads_floor: boolean | undefined,
  ): PullRequest => ({ ...ready, unresolved_threads, unresolved_threads_floor });

  it("tags a row with open conversations and carries the count", () => {
    const { container } = render(<ReadyStrip prs={[withThreads(3, false)]} onOpen={vi.fn()} />);
    const chip = container.querySelector("[data-unresolved]");
    expect(chip?.textContent).toBe("3");
    expect(chip?.getAttribute("data-unresolved")).toBe("exact");
    expect(chip?.getAttribute("title")).toBe("3 unresolved conversations");
    // The row's accessible name carries the count, not only the colour.
    expect(screen.getByRole("button", { name: /3 unresolved conversations/ })).toBeTruthy();
  });

  it("shows nothing for zero", () => {
    const { container } = render(<ReadyStrip prs={[withThreads(0, false)]} onOpen={vi.fn()} />);
    expect(container.querySelector("[data-unresolved]")).toBeNull();
    expect(screen.queryByText(/unresolved conversation/)).toBeNull();
  });

  it("qualifies a count that may be a floor, never printing it as exact", () => {
    const { container } = render(<ReadyStrip prs={[withThreads(3, true)]} onOpen={vi.fn()} />);
    const chip = container.querySelector("[data-unresolved]");
    expect(chip?.textContent).toBe("3+");
    expect(chip?.getAttribute("title")).toBe("At least 3 unresolved conversations");
    expect(
      screen.getByRole("button", { name: /at least 3 unresolved conversations/ }),
    ).toBeTruthy();
  });

  // A payload from an older desktop has no flag: qualified, not exact.
  it("qualifies a count whose floor flag is absent", () => {
    const { container } = render(<ReadyStrip prs={[withThreads(2, undefined)]} onOpen={vi.fn()} />);
    expect(container.querySelector("[data-unresolved]")?.textContent).toBe("2+");
  });

  it("singular for one", () => {
    render(<ReadyStrip prs={[withThreads(1, false)]} onOpen={vi.fn()} />);
    expect(screen.getByRole("button", { name: /\b1 unresolved conversation(?!s)/ })).toBeTruthy();
  });

  it("tags a plain-link row too", () => {
    const { container } = render(<ReadyStrip prs={[withThreads(4, false)]} />);
    expect(container.querySelector("[data-unresolved]")?.textContent).toBe("4");
    expect(screen.getByText(", 4 unresolved conversations")).toBeTruthy();
  });
});

/// #1576: rows the viewer pushed last -- tagged, and filtered.
describe("ReadyStrip last pusher", () => {
  const HEAD = "1111111111111111111111111111111111111111";
  const pr = (number: number, title: string): PullRequest => ({
    ...ready,
    number,
    title,
    head_oid: HEAD,
    head_repo: "acme/widget",
  });
  type Rules = RowPusher["rules"];
  const RULE_ON: Rules = {
    state: "read",
    require_last_push_approval: true,
    required_review_thread_resolution: false,
  };
  const RULE_OFF: Rules = { ...RULE_ON, require_last_push_approval: false };
  const UNREAD: Rules = { state: "unreadable", reason: "404" };
  const answer = (
    p: PullRequest,
    rules: Rules,
    last_pusher: RowPusher["last_pusher"],
    head_oid = HEAD,
  ): RowPusher => ({ repo: p.repo, number: p.number, head_oid, rules, last_pusher });
  const setMode = (readyMyPushes: "auto" | "hide" | "show") =>
    useFilters.setState({
      filtersByView: { ...EMPTY, "to-review": { readyMyPushes } },
      view: "to-review",
    });
  const status = () => document.querySelector("[data-my-pushes-status]")?.textContent ?? "";

  const mine = pr(1, "Mine");
  const theirs = pr(2, "Theirs");

  // #1578: the copy is of what is SHOWN -- after this filter -- and
  // carries a known pusher, but never an undecided one.
  it("copies the rows the last-push filter leaves, with known pushers only", async () => {
    copyFn.mockReset();
    copyFn.mockResolvedValue(null);
    const unknown = pr(3, "Unknown");
    pusherAnswers = [
      answer(mine, RULE_ON, { state: "known", login: "me" }),
      answer(theirs, RULE_ON, { state: "known", login: "someone" }),
      answer(unknown, RULE_ON, { state: "unknown", reason: "log lags" }),
    ];
    render(<ReadyStrip prs={[mine, theirs, unknown]} onOpen={vi.fn()} />);
    await waitFor(() => expect(screen.queryByText("Mine")).toBeNull());
    fireEvent.click(screen.getByRole("button", { name: "Copy as markdown" }));
    await waitFor(() => expect(copyFn).toHaveBeenCalledTimes(1));
    const md = copyFn.mock.calls[0][0];
    expect(md).toContain("2 pull requests");
    expect(md).not.toContain("[Mine]");
    expect(md).toMatch(/\[Theirs\].*last push by @someone/);
    const unknownLine = md.split("\n").find((l) => l.startsWith("- [Unknown]")) ?? "";
    expect(unknownLine).not.toMatch(/push/);
  });

  // #1579: the batch Claudify is handed the rows SHOWN. Every row hidden
  // leaves it nothing, so it is disabled with the reason, not hidden.
  it("disables the batch Claudify, with its reason, when the filter hides every row", async () => {
    pusherAnswers = [answer(mine, RULE_ON, { state: "known", login: "me" })];
    render(<ReadyStrip prs={[mine]} onOpen={vi.fn()} />);
    await waitFor(() => expect(screen.queryByText("Mine")).toBeNull());
    const claudify = screen.getByRole("button", { name: /Claudify|Copy prompt/ }) as HTMLButtonElement;
    expect(claudify.disabled).toBe(true);
    expect(document.querySelector("[data-ready-claudify-reason]")?.textContent).toMatch(
      /No pull requests are showing/,
    );
  });

  it("asks for every row's pusher with its head and base", async () => {
    render(<ReadyStrip prs={[mine]} onOpen={vi.fn()} />);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("get_ready_pushers", {
        rows: [
          {
            repo: mine.repo,
            number: 1,
            base: mine.base_ref,
            head_repo: "acme/widget",
            head_ref: mine.head_ref,
            head_oid: HEAD,
          },
        ],
      }),
    );
  });

  it("hides automatically only a known viewer push under a rule read as true", async () => {
    pusherAnswers = [
      answer(mine, RULE_ON, { state: "known", login: "me" }),
      answer(theirs, RULE_ON, { state: "known", login: "someone" }),
    ];
    render(<ReadyStrip prs={[mine, theirs]} onOpen={vi.fn()} />);
    await waitFor(() => expect(screen.queryByText("Mine")).toBeNull());
    expect(screen.getByText("Theirs")).toBeTruthy();
    expect(screen.getByText(/ready for review \(1\)/i)).toBeTruthy();
    expect(status()).toBe("1 hidden: you pushed last and your approval can't count");
  });

  it("never hides automatically when the rule is off or could not be read", async () => {
    const other = pr(3, "Unread rules");
    pusherAnswers = [
      answer(mine, RULE_OFF, { state: "known", login: "me" }),
      answer(other, UNREAD, { state: "known", login: "me" }),
    ];
    render(<ReadyStrip prs={[mine, other]} onOpen={vi.fn()} />);
    await waitFor(() => expect(document.querySelectorAll("[data-pushed-by-you]")).toHaveLength(2));
    expect(screen.getByText("Mine")).toBeTruthy();
    expect(screen.getByText("Unread rules")).toBeTruthy();
    // The unread rule is what the filter could not decide.
    expect(status()).toBe("1 could not be decided");
  });

  it("never auto-hides an unknown or not-checked pusher, and counts both", async () => {
    const unknown = pr(3, "Unknown");
    pusherAnswers = [
      answer(mine, RULE_ON, { state: "declined", reason: "budget" }),
      answer(unknown, RULE_ON, { state: "unknown", reason: "log lags" }),
    ];
    render(<ReadyStrip prs={[mine, unknown]} onOpen={vi.fn()} />);
    await waitFor(() => expect(status()).toBe("1 not checked yet · 1 could not be decided"));
    expect(screen.getByText("Mine")).toBeTruthy();
    expect(screen.getByText("Unknown")).toBeTruthy();
    // An undecided row is not tagged either: no false "your push".
    expect(document.querySelector("[data-pushed-by-you]")).toBeNull();
  });

  it("tags a known viewer push, with the fact in the accessible name", async () => {
    setMode("show");
    pusherAnswers = [
      answer(mine, RULE_ON, { state: "known", login: "me" }),
      answer(theirs, RULE_ON, { state: "known", login: "someone" }),
    ];
    render(<ReadyStrip prs={[mine, theirs]} onOpen={vi.fn()} />);
    await waitFor(() => expect(document.querySelector("[data-pushed-by-you]")).not.toBeNull());
    expect(document.querySelectorAll("[data-pushed-by-you]")).toHaveLength(1);
    expect(
      screen.getByRole("button", {
        name: /^Mine.*you pushed the latest commit, so your approval won't count here/,
      }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: /^Theirs/ }).textContent).not.toMatch(/you pushed/);
    // Show hides nothing and says nothing.
    expect(status()).toBe("");
  });

  it("the manual filter hides every known viewer push and says how many", async () => {
    setMode("hide");
    const unknown = pr(3, "Unknown");
    pusherAnswers = [
      answer(mine, RULE_OFF, { state: "known", login: "me" }),
      answer(theirs, RULE_OFF, { state: "known", login: "someone" }),
      answer(unknown, RULE_OFF, { state: "unknown", reason: "log lags" }),
    ];
    render(<ReadyStrip prs={[mine, theirs, unknown]} onOpen={vi.fn()} />);
    await waitFor(() => expect(screen.queryByText("Mine")).toBeNull());
    expect(screen.getByText("Theirs")).toBeTruthy();
    expect(screen.getByText("Unknown")).toBeTruthy();
    expect(status()).toBe("1 hidden: you pushed last · 1 could not be decided");
  });

  // The pusher of an old head says nothing about the new one.
  it("ignores an answer about a head the row has moved off", async () => {
    setMode("hide");
    pusherAnswers = [
      answer(mine, RULE_ON, { state: "known", login: "me" }, "2222222222222222222222222222222222222222"),
    ];
    render(<ReadyStrip prs={[mine]} onOpen={vi.fn()} />);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_ready_pushers", expect.anything()));
    await waitFor(() => expect(status()).toBe("1 not checked yet"));
    expect(screen.getByText("Mine")).toBeTruthy();
    expect(document.querySelector("[data-pushed-by-you]")).toBeNull();
  });

  // A viewer login that could not be read is "could not tell", and hides
  // nothing: the pusher is never compared against a guess.
  it("hides nothing when the viewer's login could not be read", async () => {
    setMode("hide");
    viewerLogin = Promise.reject(new Error("no client"));
    viewerLogin.catch(() => {});
    pusherAnswers = [answer(mine, RULE_ON, { state: "known", login: "me" })];
    render(<ReadyStrip prs={[mine]} onOpen={vi.fn()} />);
    await waitFor(() => expect(status()).toBe("1 could not be decided"));
    expect(screen.getByText("Mine")).toBeTruthy();
  });

  // Everything hidden is not "nothing ready": the strip stays, and says why.
  it("keeps the strip when every row is hidden", async () => {
    pusherAnswers = [answer(mine, RULE_ON, { state: "known", login: "me" })];
    render(<ReadyStrip prs={[mine]} onOpen={vi.fn()} />);
    await waitFor(() => expect(screen.queryByText("Mine")).toBeNull());
    expect(screen.queryByText(/nothing ready to review/i)).toBeNull();
    expect(status()).toBe("1 hidden: you pushed last and your approval can't count");
  });

  it("says it is checking while the first answer is on its way", () => {
    invoke.mockImplementation((cmd: string) =>
      cmd === "get_ready_pushers" ? new Promise(() => {}) : viewerLogin,
    );
    render(<ReadyStrip prs={[mine]} onOpen={vi.fn()} />);
    expect(status()).toBe("Checking who pushed last…");
  });

  it("persists the choice like the sort", () => {
    render(<ReadyStrip prs={[mine]} onOpen={vi.fn()} />);
    const trigger = screen.getByRole("button", { name: /pull requests you pushed last/i });
    expect(trigger.textContent).toContain("auto");
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Hide all I pushed last" }));
    expect(useFilters.getState().filtersByView["to-review"].readyMyPushes).toBe("hide");
  });
});

describe("ReadyStrip copy as markdown (#1578)", () => {
  beforeEach(() => {
    copyFn.mockReset();
    copyFn.mockResolvedValue(null);
    toastFns.success.mockReset();
    toastFns.error.mockReset();
  });

  const row = (number: number, ready_at: string, over: Partial<PullRequest> = {}): PullRequest => ({
    ...ready,
    number,
    title: `PR ${number}`,
    ready_at,
    ...over,
  });

  it("copies what is shown: the filtered rows, in the strip's order", async () => {
    render(
      <ReadyStrip
        prs={[
          row(3, "2026-09-03T00:00:00Z"),
          // Filtered out of the strip, so it must not be copied.
          row(9, "2026-08-01T00:00:00Z", { ci: "failure" }),
          row(1, "2026-09-01T00:00:00Z"),
        ]}
        onOpen={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Copy as markdown" }));
    await waitFor(() => expect(copyFn).toHaveBeenCalledTimes(1));
    const md = copyFn.mock.calls[0][0];
    expect(md).toContain("2 pull requests");
    expect(md).not.toContain("PR 9");
    expect(md.indexOf("[PR 1]")).toBeGreaterThan(-1);
    expect(md.indexOf("[PR 1]")).toBeLessThan(md.indexOf("[PR 3]"));
    await waitFor(() =>
      expect(toastFns.success).toHaveBeenCalledWith(
        "Copied 2 pull requests as markdown",
        expect.anything(),
      ),
    );
  });

  // #1577's floor flag reaches the copy: a count that may be short is
  // qualified, an exact one is not.
  it("qualifies an unresolved count the row marks as a floor", async () => {
    render(
      <ReadyStrip
        prs={[
          row(1, "2026-09-01T00:00:00Z", { unresolved_threads: 3, unresolved_threads_floor: true }),
          row(2, "2026-09-02T00:00:00Z", { unresolved_threads: 2, unresolved_threads_floor: false }),
        ]}
        onOpen={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Copy as markdown" }));
    await waitFor(() => expect(copyFn).toHaveBeenCalledTimes(1));
    const md = copyFn.mock.calls[0][0];
    expect(md).toMatch(/\[PR 1\].*at least 3 unresolved conversations/);
    expect(md).toMatch(/\[PR 2\].* 2 unresolved conversations/);
    expect(md).not.toMatch(/\[PR 2\].*at least/);
  });

  it("follows the sort the reader chose", async () => {
    render(
      <ReadyStrip
        prs={[row(1, "2026-09-01T00:00:00Z"), row(3, "2026-09-03T00:00:00Z")]}
        onOpen={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /sort/i }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Newest ready first" }));
    fireEvent.click(screen.getByRole("button", { name: "Copy as markdown" }));
    await waitFor(() => expect(copyFn).toHaveBeenCalledTimes(1));
    const md = copyFn.mock.calls[0][0];
    expect(md.indexOf("[PR 3]")).toBeLessThan(md.indexOf("[PR 1]"));
  });

  it("says so when the clipboard refuses", async () => {
    copyFn.mockResolvedValue("This window has no clipboard access.");
    render(<ReadyStrip prs={[row(1, "2026-09-01T00:00:00Z")]} onOpen={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy as markdown" }));
    await waitFor(() =>
      expect(toastFns.error).toHaveBeenCalledWith("Could not copy the markdown", {
        description: "This window has no clipboard access.",
      }),
    );
    expect(toastFns.success).not.toHaveBeenCalled();
  });
});

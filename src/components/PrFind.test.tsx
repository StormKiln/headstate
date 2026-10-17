import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { PrFind } from "./PrFind";
import { Markdown } from "./Markdown";
import { Section } from "./Section";
import { CommentRow } from "./CommentRow";
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
afterEach(cleanup);
it("finds sanitized format-spanning text through collapsed comments and nested disclosures, then restores them", async () => {
  const view = render(<PrFind partial><Section title="Comments" defaultOpen={false}><CommentRow author="bot" createdAt="2026-10-17" body={'<script>secretword</script><details><summary>Report</summary><details><summary>More</summary>needle <strong>phrase</strong></details></details>'} /></Section></PrFind>);
  screen.getByRole("button", { name: "Find in pull request" }).focus();
  fireEvent.click(screen.getByRole("button", { name: "Find in pull request" }));
  const input = screen.getByRole("searchbox", { name: "Find in pull request" });
  fireEvent.change(input, { target: { value: "needle phrase" } });
  await screen.findByText("1 of 1");
  expect([...view.container.querySelectorAll("details")].every(d => d.open)).toBe(true);
  expect(screen.getByText(/Some content is not loaded/)).toBeTruthy();
  fireEvent.change(input, { target: { value: "secretword" } });
  await screen.findByText("0 matches");
  fireEvent.keyDown(input, { key: "Escape" });
  expect(screen.queryByRole("searchbox")).toBeNull();
  expect(screen.getByRole("button", { name: "Comments" }).getAttribute("aria-expanded")).toBe("false");
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "Find in pull request" }));
});
it("captures keyboard Find, wraps navigation, and retains active hit on same-content refresh", async () => {
  const content = <Markdown>{"needle one\n\nneedle two"}</Markdown>;
  const view = render(<PrFind>{content}</PrFind>);
  fireEvent.keyDown(window, { key: "f", ctrlKey: true });
  const input = await screen.findByRole("searchbox");
  fireEvent.change(input, { target: { value: "needle" } });
  await screen.findByText("1 of 2");
  fireEvent.keyDown(input, { key: "Enter" });
  await screen.findByText("2 of 2");
  view.rerender(<PrFind><Markdown>{"needle one\n\nneedle two"}</Markdown></PrFind>);
  await waitFor(() => expect(screen.getByText("2 of 2")).toBeTruthy());
  fireEvent.click(screen.getByRole("button", { name: "Next match" }));
  expect(screen.getByText("1 of 2")).toBeTruthy();
});

it("does not recenter the active hit when unrelated loaded content changes", async () => {
  const scroll = vi.fn();
  const original = HTMLElement.prototype.scrollIntoView;
  HTMLElement.prototype.scrollIntoView = scroll;
  try {
    const view = render(<PrFind><Markdown>needle one needle two</Markdown><p>Loading status</p></PrFind>);
    fireEvent.click(screen.getByRole("button", { name: "Find in pull request" }));
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "needle" } });
    await screen.findByText("1 of 2"); expect(scroll).toHaveBeenCalledTimes(1);
    view.rerender(<PrFind><Markdown>needle one needle two</Markdown><p>Updated status</p></PrFind>);
    await waitFor(() => expect(screen.getByText("Updated status")).toBeTruthy());
    expect(scroll).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Next match" }));
    expect(scroll).toHaveBeenCalledTimes(2);
  } finally { HTMLElement.prototype.scrollIntoView = original; }
});

it("searches superseded loaded comments and preserves a reader-open nested disclosure", async () => {
  const { SupersededGroup } = await import("./SupersededComments");
  const view = render(<PrFind>
    <Markdown>{"<details><summary>Manual report</summary>Visible content</details>"}</Markdown>
    <SupersededGroup truncated={false} comments={[{ id: "old", author: "robot", author_is_bot: true, created_at: "2026-10-17", body: "Superseded evidence needle" }]} />
  </PrFind>);
  const details = view.container.querySelector("details")!;
  details.open = true;
  fireEvent.click(screen.getByRole("button", { name: "Find in pull request" }));
  fireEvent.change(screen.getByRole("searchbox"), { target: { value: "evidence needle" } });
  await screen.findByText("1 of 1");
  fireEvent.click(screen.getByRole("button", { name: "Close find" }));
  expect(details.open).toBe(true);
  expect(screen.queryByText("Superseded evidence needle")).toBeNull();
});

it("Escape outside the Find toolbar closes Find before global shortcuts and restores draft focus", async () => {
  render(<PrFind><textarea aria-label="Review draft" defaultValue="keep this draft" /><Markdown>needle</Markdown><button>Result action</button></PrFind>);
  const draft = screen.getByRole("textbox", { name: "Review draft" });
  draft.focus();
  fireEvent.keyDown(window, { key: "f", ctrlKey: true });
  fireEvent.change(await screen.findByRole("searchbox"), { target: { value: "needle" } });
  await screen.findByText("1 of 1");
  const globalKey = vi.fn();
  window.addEventListener("keydown", globalKey);
  try {
    const action = screen.getByRole("button", { name: "Result action" });
    action.focus();
    fireEvent.keyDown(action, { key: "Escape" });
    expect(screen.queryByRole("searchbox")).toBeNull();
    expect(globalKey).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(draft);
    expect((draft as HTMLTextAreaElement).value).toBe("keep this draft");
  } finally { window.removeEventListener("keydown", globalKey); }
});

it("leaves Escape to an open dialog above Find", async () => {
  render(<PrFind><Markdown>needle</Markdown><div role="dialog" data-open=""><button>Dialog action</button></div></PrFind>);
  fireEvent.click(screen.getByRole("button", { name: "Find in pull request" }));
  fireEvent.keyDown(screen.getByRole("button", { name: "Dialog action" }), { key: "Escape" });
  expect(screen.getByRole("searchbox")).toBeTruthy();
});

it("disables auto-open disclosure toggles and retains their manual state after Find", async () => {
  const { SupersededGroup } = await import("./SupersededComments");
  const { ReviewThreads } = await import("./ReviewThreads");
  const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
  const view = render(<QueryClientProvider client={new QueryClient()}><PrFind>
    <Section title="Collapsed section" defaultOpen={false}><CommentRow author="bot" createdAt="2026-10-17" body="needle" /></Section>
    <SupersededGroup truncated={false} comments={[{ id: "old", author: "robot", author_is_bot: true, created_at: "2026-10-17", body: "needle" }]} />
    <ReviewThreads repo="o/r" number={1} total={1} threads={[{ id: "thread", path: "a.ts", line: 1, is_resolved: true, is_outdated: false, viewer_can_reply: false, viewer_can_resolve: false, viewer_can_unresolve: false, comment_count: 1, comments: [{ author: "bot", author_is_bot: true, created_at: "2026-10-17", body: "needle" }] }]} />
  </PrFind></QueryClientProvider>);
  fireEvent.click(screen.getByRole("button", { name: "Find in pull request" }));
  fireEvent.change(screen.getByRole("searchbox"), { target: { value: "needle" } });
  await screen.findByText("1 of 3");
  for (const toggle of view.container.querySelectorAll<HTMLButtonElement>("button[aria-expanded]")) {
    expect(toggle.disabled).toBe(true);
    expect(toggle.getAttribute("aria-description")).toMatch(/Find/);
    fireEvent.click(toggle);
  }
  fireEvent.click(screen.getByRole("button", { name: "Close find" }));
  expect(screen.getByRole("button", { name: "Collapsed section" }).getAttribute("aria-expanded")).toBe("false");
  for (const toggle of view.container.querySelectorAll<HTMLButtonElement>("button[aria-expanded]")) {
    expect(toggle.disabled).toBe(false);
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
  }
});

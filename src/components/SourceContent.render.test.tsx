import { memo, useSyncExternalStore } from "react";
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { SourceRefreshState, type SourceStatus } from "../api/sourceRefresh";
import type { PullRequest } from "../types/pr";
const counts = vi.hoisted(() => ({ markdown: 0 }));
vi.mock("react-markdown", async original => {
  const { default: Real } = await original<typeof import("react-markdown")>();
  return { default: (props: React.ComponentProps<typeof Real>) => { counts.markdown++; return <Real {...props} />; } };
});
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
import { Markdown } from "./Markdown";
afterEach(cleanup);
it("updates source status while stable loaded prose renders once", () => {
  counts.markdown = 0;
  let renders = 0;
  const state = new SourceRefreshState();
  const rows = [{ number: 7, title: "Loaded **evidence**" }] as PullRequest[];
  const update = (revision: number, phase: SourceStatus["phase"]): SourceStatus => ({ source: { provider: "github", host: "github.com" }, list: "authored", session: "one", revision, receipt_revision: 1, phase, error: null, prs: rows });
  state.accept(update(1, "ready"));
  const Content = memo(function Content() {
    const snapshot = useSyncExternalStore(state.subscribe, state.snapshot);
    renders++;
    return <><span>{snapshot.phase}</span><Markdown>{snapshot.prs![0].title}</Markdown></>;
  });
  render(<Content />);
  act(() => state.accept(update(2, "fetching")));
  expect(screen.getByText("fetching")).toBeTruthy();
  act(() => state.accept(update(3, "ready")));
  act(() => state.accept(update(3, "ready")));
  expect(renders).toBe(3);
  expect(counts.markdown).toBe(1);
});

it("renders each of 150 loaded collapsed comments once per Find session, not per keystroke", async () => {
  const { PrFind } = await import("./PrFind");
  const { CommentRow } = await import("./CommentRow");
  const { fireEvent } = await import("@testing-library/react");
  counts.markdown = 0;
  render(<PrFind>{Array.from({ length: 150 }, (_, i) => <CommentRow key={i} author={`reader${i}`} createdAt="2026-10-17" body={`Needle ${i} **rendered** evidence`} />)}</PrFind>);
  expect(counts.markdown).toBe(0);
  fireEvent.click(screen.getByRole("button", { name: "Find in pull request" }));
  const box = screen.getByRole("searchbox");
  fireEvent.change(box, { target: { value: "needle" } });
  await screen.findByText("1 of 150"); expect(counts.markdown).toBe(150);
  fireEvent.change(box, { target: { value: "needle 14" } });
  await screen.findByText("1 of 11"); expect(counts.markdown).toBe(150);
  fireEvent.click(screen.getByRole("button", { name: "Next match" }));
  expect(counts.markdown).toBe(150);
});

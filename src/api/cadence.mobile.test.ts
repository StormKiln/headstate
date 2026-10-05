import { expect, it, vi } from "vitest";
import { renderHook } from "@testing-library/react";
vi.mock("@/lib/target", () => ({ IS_MOBILE_BUILD: true }));
const setNeeds = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock("./tauri", async (orig) => ({
  ...(await orig<Record<string, unknown>>()),
  setViewNeedsGithub: setNeeds,
}));
const { useViewCadence } = await import("./hooks");
it("phone navigation and disconnect leave desktop scheduling ownership intact", () => {
  const phone = renderHook(({ view }) => useViewCadence(view), { initialProps: { view: "worktrees" } });
  phone.rerender({ view: "to-review" });
  phone.rerender({ view: "my-prs" });
  phone.unmount();
  expect(setNeeds).not.toHaveBeenCalled();
});

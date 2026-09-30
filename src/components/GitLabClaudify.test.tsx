import { render, screen, fireEvent } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import type { GitLabDetail } from "../types/gitlabActions";
const controls = vi.hoisted(() => ({ lookup: vi.fn(), start: vi.fn() }));
vi.mock("./PrClaudify", () => ({
  useClaudifyCheckout: controls.lookup,
  startClaudify: controls.start,
  unavailableReason: () => null,
  PrClaudifyDialogs: () => null,
}));
import { GitLabClaudify } from "./GitLabClaudify";
it("uses the host and full namespace with the existing guarded launch flow", () => {
  controls.lookup.mockReturnValue({ checkout: { kind: "found", path: "/code/project" }, terminalConfigured: true });
  const detail = { core: { identity: { source: { provider: "gitlab", host: "gitlab.example" }, repo: "acme/team/project", number: 7 }, title: "A change", url: "https://gitlab.example/acme/team/project/-/merge_requests/7", head_ref: "topic", base_ref: "main", head_oid: "abc", body: "Description" } } as GitLabDetail;
  render(<GitLabClaudify detail={detail} />);
  expect(controls.lookup).toHaveBeenCalledWith(["gitlab.example/acme/team/project"]);
  fireEvent.click(screen.getByRole("button", { name: "Claudify" }));
  expect(controls.start).toHaveBeenCalledWith(expect.objectContaining({ repo: "gitlab.example/acme/team/project", subject: "gitlab.example/acme/team/project!7", prompt: expect.stringContaining(detail.core.url) }), expect.any(Function));
});

import { expect, it, vi } from "vitest";

const call = vi.hoisted(() => vi.fn().mockResolvedValue("gitlab.example"));
vi.mock("./transport", () => ({ call }));

import { getGitLabHost, setGitLabHost } from "./gitlabHost";

it("uses the command transport with the exact host argument", async () => {
  await getGitLabHost();
  await setGitLabHost("gitlab.example");
  expect(call.mock.calls).toEqual([
    ["get_gitlab_host"],
    ["set_gitlab_host", { host: "gitlab.example" }],
  ]);
});

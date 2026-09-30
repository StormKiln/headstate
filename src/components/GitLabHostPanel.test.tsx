import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider, QueryObserver } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { renderWithQuery } from "@/test-utils";

const getHost = vi.hoisted(() => vi.fn());
const setHost = vi.hoisted(() => vi.fn());
vi.mock("@/api/gitlabHost", () => ({ getGitLabHost: getHost, setGitLabHost: setHost }));

import { GitLabHostPanel } from "./GitLabHostPanel";

beforeEach(() => {
  getHost.mockReset().mockResolvedValue("gitlab.com");
  setHost.mockReset().mockResolvedValue("gitlab-fixture.k3s.dcd.cloud");
});

it("saves the explicit host and shows the desktop login command", async () => {
  renderWithQuery(<GitLabHostPanel />);
  const input = await screen.findByRole("textbox", { name: "GitLab host" });
  await waitFor(() => expect((input as HTMLInputElement).value).toBe("gitlab.com"));
  fireEvent.change(input, { target: { value: "gitlab-fixture.k3s.dcd.cloud" } });
  fireEvent.click(screen.getByRole("button", { name: "Save GitLab host" }));
  await waitFor(() => expect(setHost.mock.calls[0]?.[0]).toBe("gitlab-fixture.k3s.dcd.cloud"));
  await waitFor(() => expect(screen.getByText(/glab auth login --hostname gitlab-fixture.k3s.dcd.cloud/)).toBeTruthy());
});

it("shows a rejected save without claiming the host changed", async () => {
  setHost.mockRejectedValue("Enter a GitLab DNS hostname.");
  renderWithQuery(<GitLabHostPanel />);
  const input = await screen.findByRole("textbox", { name: "GitLab host" });
  await waitFor(() => expect((input as HTMLInputElement).value).toBe("gitlab.com"));
  fireEvent.change(input, { target: { value: "https://wrong.example" } });
  fireEvent.click(screen.getByRole("button", { name: "Save GitLab host" }));
  expect((await screen.findByRole("alert")).textContent).toContain("Enter a GitLab DNS hostname.");
  expect((input as HTMLInputElement).value).toBe("https://wrong.example");
});

it("resets an active GitLab observer and removes inactive receipts after changing hosts", async () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(["stats", "gitlab", "gitlab.com"], { old: true });
  client.setQueryData(["gitlab-detail", "gitlab.com/g/p/1"], { old: true });
  client.setQueryData(["gitlab-auth", "gitlab.com"], { old: true });
  client.setQueryData(["github", "octocat"], { keep: true });
  let fetches = 0;
  const observer = new QueryObserver(client, {
    queryKey: ["gitlab-auth", "gitlab.com"],
    queryFn: () => { fetches += 1; return Promise.resolve({ host: "gitlab-fixture.k3s.dcd.cloud" }); },
    staleTime: Infinity,
  });
  const unsubscribe = observer.subscribe(() => {});
  expect(observer.getCurrentResult().data).toEqual({ old: true });
  expect(fetches).toBe(0);
  render(<QueryClientProvider client={client}><GitLabHostPanel /></QueryClientProvider>);
  const input = await screen.findByRole("textbox", { name: "GitLab host" });
  await waitFor(() => expect((input as HTMLInputElement).value).toBe("gitlab.com"));
  fireEvent.change(input, { target: { value: "gitlab-fixture.k3s.dcd.cloud" } });
  fireEvent.click(screen.getByRole("button", { name: "Save GitLab host" }));
  await waitFor(() => expect(client.getQueryData(["gitlab-host"])).toBe("gitlab-fixture.k3s.dcd.cloud"));
  await waitFor(() => expect(observer.getCurrentResult().data).toEqual({ host: "gitlab-fixture.k3s.dcd.cloud" }));
  expect(fetches).toBe(1);
  expect(client.getQueryData(["stats", "gitlab", "gitlab.com"])).toBeUndefined();
  expect(client.getQueryData(["gitlab-detail", "gitlab.com/g/p/1"])).toBeUndefined();
  expect(client.getQueryData(["gitlab-auth", "gitlab.com"])).toEqual({ host: "gitlab-fixture.k3s.dcd.cloud" });
  expect(client.getQueryData(["github", "octocat"])).toEqual({ keep: true });
  unsubscribe();
});

import { createElement } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SourceQueue, SourceRepoSidebar, combinedRows, sourceRepoKey } from "./SourceQueue";
import type { PullRequest } from "../types/pr";
import type { MergeRequest } from "../types/gitlab";
import { activeRowCursor, resetRowCursorForTest } from "../lib/rowCursor";
import { useSourceSelection } from "../store/sourceSelection";
import { useFilters } from "../store/filters";
import { PR_FIXTURES } from "../fixtures/prs";

const gh = {
  source: { provider: "github", host: "github.com" },
  repo: "group/project", number: 7, title: "Same number", created_at: "2026-09-21T00:00:00Z",
} as PullRequest;
const gl = {
  source: { provider: "gitlab", host: "gitlab.com" },
  repo: "group/project", number: 7, title: "Same number", created_at: "2026-09-22T00:00:00Z",
  ci: null, review: null,
} as MergeRequest;
const selfManaged = {
  ...gl, source: { provider: "gitlab" as const, host: "gitlab.example" },
} as MergeRequest;

describe("source queue identity", () => {
  it("keeps overlapping project paths and numbers distinct across providers and hosts", () => {
    expect(new Set([sourceRepoKey(gh), sourceRepoKey(gl), sourceRepoKey(selfManaged)]).size).toBe(3);
    expect(combinedRows([gh], [gl], "both", null, "").map(({ value }) => value.source?.provider)).toEqual(["gitlab", "github"]);
    expect(combinedRows([gh], [gl], "both", sourceRepoKey(gl), "").map(({ value }) => value.source?.provider)).toEqual(["gitlab"]);
  });

  it("selects the requested provider and searches without borrowing the other source", () => {
    expect(combinedRows([gh], [gl], "github", null, "!7")).toEqual([{ kind: "github", value: gh }]);
    expect(combinedRows([gh], [gl], "gitlab", null, "#7")).toEqual([{ kind: "gitlab", value: gl }]);
    expect(combinedRows([gh], [gl], "both", null, "absent")).toEqual([]);
  });

  it("registers the rendered mixed rows for keyboard navigation", () => {
    resetRowCursorForTest();
    useSourceSelection.setState({ selection: "both", repoKey: null, query: "" });
    useFilters.setState({ cursor: null, selectedPr: null });
    const open = vi.fn();
    const view = render(createElement(QueryClientProvider, { client: new QueryClient() }, createElement(SourceQueue, {
      selection: "both", github: [{ ...PR_FIXTURES[0], ...gh }], gitlab: [gl], githubLoading: false,
      gitlabLoading: false, githubError: null, gitlabError: null,
      githubCoverage: "complete", gitlabCoverage: "complete",
      githubStaleSecs: null, gitlabStaleSecs: null, canWriteGitHub: true,
      onOpen: open, onRefreshGitHub: vi.fn(), onRefreshGitLab: vi.fn(),
    })));
    expect(activeRowCursor()?.rows()).toBe(2);
    activeRowCursor()?.open(0);
    expect(open).toHaveBeenCalledWith(expect.objectContaining({ source: { provider: "gitlab", host: "gitlab.com" }, number: 7 }));
    expect(activeRowCursor()?.toggle).toBeTypeOf("function");
    view.unmount();
    expect(activeRowCursor()).toBeNull();
  });
});

it("keeps existing Github filters and sort in the combined queue", () => {
  const a = { ...PR_FIXTURES[0], ...gh, ci: "failure" as const };
  const b = { ...a, number: 8, ci: "success" as const };
  expect(combinedRows([a, b], [], "both", null, "", { ci: "failure" }).map(row => row.value.number)).toEqual([7]);
  const older = { ...a, number: 9, created_at: "2020-01-01T00:00:00Z" };
  expect(combinedRows([a, older], [], "both", null, "", { sort: "oldest" }).map(row => row.value.number)).toEqual([9, 7]);
});

it("repository navigation clears the prior GitHub-only repository filter", () => {
  useFilters.getState().setFilter("repo", "previous/repository");
  useSourceSelection.setState({ selection: "both", repoKey: null, query: "" });
  const view = render(createElement(QueryClientProvider, { client: new QueryClient() }, createElement(SourceRepoSidebar, { github: [gh], gitlab: [gl], selection: "both" })));
  fireEvent.click(screen.getByRole("button", { name: /GitLab · gitlab.com · group\/project/ }));
  expect(useFilters.getState().filtersByView[useFilters.getState().view].repo).toBeUndefined();
  expect(useSourceSelection.getState().repoKey).toBe(sourceRepoKey(gl));
  view.unmount();
  useSourceSelection.setState({ selection: "github", repoKey: null, query: "" });
});

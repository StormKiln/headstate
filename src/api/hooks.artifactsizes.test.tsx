import { describe, expect, it, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import type { Artifact } from "@/types/pr";

const invoke = vi.hoisted(() =>
  vi.fn<(cmd: string, args?: unknown) => Promise<unknown>>((cmd, args) => {
    if (cmd === "size_artifacts") {
      const paths = (args as { paths: string[] }).paths;
      return Promise.resolve(paths.map((p) => [p, 1024, 60]));
    }
    return Promise.resolve([]);
  }),
);
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { useArtifactSizes } from "./hooks";

/// One artifact per repository, so each gets its own batch -- the hook
/// groups by `repo_path`.
const artifacts = (n: number): Artifact[] =>
  Array.from(
    { length: n },
    (_, i) =>
      ({
        repo_path: `/code/r${i}`,
        path: `/code/r${i}/target`,
        kind: "cargo",
      }) as unknown as Artifact,
  );

const wrapper = (qc: QueryClient) =>
  function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
  };

/// #956. `useArtifactSizes` counted only IN-FLIGHT batches, so a rejected
/// one made `pending` fall to zero with measurements still missing --
/// and `pending` was the only thing gating every honesty qualifier on
/// `ArtifactsPage`, including the byte figure on a Remove button.
///
/// `useAllWorktreeSizes` has carried the correct field, with this exact
/// reasoning in its doc comment, for longer than this hook has been
/// wrong: "a failed repository leaves `pending` and never comes back: a
/// caller that only watches `pending` sees the number fall to zero and
/// concludes everything was measured."
describe("useArtifactSizes failure counting", () => {
  beforeEach(() => invoke.mockClear());

  it("counts a rejected batch as failed rather than as answered", async () => {
    let call = 0;
    invoke.mockImplementation((cmd, args) => {
      if (cmd !== "size_artifacts") return Promise.resolve([]);
      call += 1;
      if (call === 1) return Promise.reject(new Error("permission denied"));
      const paths = (args as { paths: string[] }).paths;
      return Promise.resolve(paths.map((p) => [p, 2048, 30]));
    });
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { result } = renderHook(() => useArtifactSizes(artifacts(3), true), {
      wrapper: wrapper(qc),
    });

    await waitFor(() => expect(result.current.failed).toBe(1));
    // The PAIR is the point: `pending` has gone quiet and the work is
    // nonetheless incomplete. `pending` alone reads as "done".
    expect(result.current.pending).toBe(0);
    expect(result.current.total).toBe(3);
    // The batches that answered kept their sizes -- one failure must not
    // blank the list.
    expect(result.current.sizes.size).toBe(2);
  });

  it("reports no failures when every batch answers", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { result } = renderHook(() => useArtifactSizes(artifacts(3), true), {
      wrapper: wrapper(qc),
    });
    await waitFor(() => expect(result.current.sizes.size).toBe(3));
    expect(result.current.failed).toBe(0);
    expect(result.current.pending).toBe(0);
  });
});

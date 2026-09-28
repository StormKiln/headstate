/// The Worktrees page's browser harness (#1582).
///
/// NOT part of the app bundle: `harness/worktrees.html` loads it, and
/// only `vite.harness-worktrees.config.ts` builds that page. It mounts the
/// REAL `WorktreesPage` against one generated repository of `?n=` (default
/// 141) worktrees, with Tauri's IPC answered by `mockIPC`, and lets
/// `scripts/worktrees-browser-bench.mjs` stream `worktree-safety` verdicts
/// into it at a chosen rate -- the pass #1582 saw stall while the window
/// was in front.
///
/// What it records, on `window.__worktreesHarness`:
///
/// - every React commit under the page, from a `<Profiler>`: its
///   `actualDuration` is the render cost of one flush of the verdict
///   coalescer, which is the figure hypothesis (6) turns on. The harness
///   build aliases `react-dom/client` to the profiling build, which is
///   the only production build that calls `onRender`.
/// - each verdict's DELIVERY LAG: how late its emit ran against the
///   schedule. Tauri delivers an event by evaluating script on the
///   webview's main thread, so a verdict queues behind whatever that
///   thread is doing. Emits here are timer tasks on the same thread, so
///   a render that saturates it delays them the same way.
///
/// Fixtures are generated and generic: `acme/widget`, branches
/// `feature/widget-NNN`. Nothing is read from the machine.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { Profiler, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { WorktreesPage } from "../components/WorktreesPage";
import "../index.css";
import { useFilters } from "../store/filters";
import type { PullRequest, Safety, Upstream, Worktree, WorktreeRepo } from "../types/pr";

const ROOT = "/harness/code/acme";
const REPO = `${ROOT}/widget`;
const IDENTITY = "acme/widget";

/// One React commit under the page.
interface Commit {
  phase: string;
  /// Render time of this commit, ms.
  actual: number;
  /// When React committed it, on `performance.now()`'s clock.
  at: number;
}

/// One verdict the harness emitted.
interface Emitted {
  /// When it was due, and when its emit actually ran.
  due: number;
  ran: number;
}

interface HarnessProbe {
  commits: Commit[];
  emitted: Emitted[];
  /// Commands the harness had no answer for.
  refused: string[];
  /// Every command asked, in order, with a count.
  asked: Record<string, number>;
  /// How many worktrees the listing has.
  total: number;
  /// Stream `count` verdicts (from the first unclassified worktree) at
  /// `perSecond`, as timer tasks on the page's own thread. Resolves when
  /// the last emit has RUN, which is not when the page has shown it.
  stream(count: number, perSecond: number): Promise<void>;
  /// Settle `classify_worktrees` with every verdict, as the command does
  /// when its pass ends.
  settle(): void;
  /// Stream sizes too, as `worktree-size`, for the run where the size
  /// walk overlaps classification.
  streamSizes(count: number, perSecond: number): Promise<void>;
}

declare global {
  interface Window {
    __worktreesHarness?: HarnessProbe;
  }
}

const pad = (i: number) => String(i).padStart(3, "0");

function worktreeAt(i: number): Worktree {
  const main = i === 0;
  return {
    path: main ? REPO : `${ROOT}/widget-wt/feature-${pad(i)}`,
    branch: main ? "main" : `feature/widget-${pad(i)}`,
    head: (0x1000000 + i * 7919).toString(16).padStart(40, "0"),
    size_bytes: null,
    safety: { kind: "pending" },
    submodules: null,
    is_main: main,
    merged_at: null,
    upstream: null,
    last_commit: null,
    locked: null,
    prunable: null,
  };
}

/// A deterministic spread of verdicts, in roughly the proportions a
/// long-lived repository has: most unmerged, some merged one way or
/// another, a few dirty, unpushed or unknown.
function verdictFor(w: Worktree, i: number): Worktree {
  if (w.is_main) {
    return { ...w, safety: { kind: "main_checkout" } as Safety, upstream: { kind: "current" } };
  }
  const k = i % 10;
  const safety: Safety =
    k < 5
      ? { kind: "unmerged" }
      : k === 5
        ? { kind: "merged_as_pr", detail: 1000 + i }
        : k === 6
          ? { kind: "safe" }
          : k === 7
            ? ({ kind: "dirty", detail: ["src/lib/widget.ts"] } as unknown as Safety)
            : k === 8
              ? { kind: "unpushed", detail: (i % 4) + 1 }
              : { kind: "never_pushed" };
  const upstream: Upstream =
    k === 9 ? { kind: "untracked" } : k === 8 ? { kind: "ahead", n: (i % 4) + 1 } : { kind: "current" };
  const day = String((i % 28) + 1).padStart(2, "0");
  return {
    ...w,
    safety,
    upstream,
    last_commit: `2026-08-${day}T12:00:00Z`,
    merged_at: k === 6 ? `2026-09-${day}` : null,
  };
}

/// Open pull requests across several repositories, about a third of this
/// repository's branches with one: the join `prForWorktree` makes per row.
function pullRequests(n: number, worktrees: number): PullRequest[] {
  const repos = ["acme/widget", "acme/gadget", "acme/gizmo", "acme/sprocket"];
  return Array.from({ length: n }, (_, i) => {
    const repo = repos[i % repos.length];
    const branch = repo === IDENTITY ? `feature/widget-${pad((i * 3) % worktrees)}` : `topic/${pad(i)}`;
    return {
      id: `PR_${i}`,
      number: 1000 + i,
      title: `Change ${i}`,
      url: `https://github.com/${repo}/pull/${1000 + i}`,
      repo,
      author: "octocat",
      is_draft: false,
      head_ref: branch,
      head_oid: "0".repeat(40),
      head_ref_id: null,
      base_ref: "main",
      created_at: "2026-08-01T00:00:00Z",
      updated_at: "2026-09-01T00:00:00Z",
      ci: i % 7 === 0 ? "failure" : "success",
      merge: "mergeable",
      merge_status: "clean",
      review: "approved",
      in_merge_queue: false,
      labels: [],
      comment_count: 0,
      unresolved_threads: 0,
      requested_reviewers: [],
      assignees: [],
    } as unknown as PullRequest;
  });
}

async function main() {
  const params = new URLSearchParams(location.search);
  const n = Number(params.get("n") ?? 141);
  const prCount = Number(params.get("prs") ?? 300);
  const listed = Array.from({ length: n }, (_, i) => worktreeAt(i));
  const repo: WorktreeRepo = {
    identity: IDENTITY,
    name: "widget",
    path: REPO,
    worktrees: listed,
    stash_entries: 0,
    bare: false,
    fetched_at: new Date(Date.now() - 3_600_000).toISOString(),
    default_ref: "main",
  };
  const prs = pullRequests(prCount, n);
  const verdicts = listed.map((w, i) => verdictFor(w, i));
  let settleClassify: (v: Worktree[]) => void = () => {};
  let next = 0;
  let nextSize = 0;

  const probe: HarnessProbe = {
    commits: [],
    emitted: [],
    refused: [],
    asked: {},
    total: n,
    async stream(count, perSecond) {
      const gap = 1000 / perSecond;
      const start = performance.now();
      const upto = Math.min(verdicts.length, next + count);
      const batch = verdicts.slice(next, upto);
      next = upto;
      await new Promise<void>((done) => {
        let i = 0;
        const tick = () => {
          const due = start + i * gap;
          const ran = performance.now();
          probe.emitted.push({ due, ran });
          void emit("worktree-safety", batch[i]);
          i += 1;
          if (i >= batch.length) return done();
          setTimeout(tick, Math.max(0, start + i * gap - performance.now()));
        };
        tick();
      });
    },
    async streamSizes(count, perSecond) {
      const gap = 1000 / perSecond;
      const start = performance.now();
      const upto = Math.min(listed.length, nextSize + count);
      const batch = listed.slice(nextSize, upto);
      nextSize = upto;
      await new Promise<void>((done) => {
        let i = 0;
        const tick = () => {
          void emit("worktree-size", [batch[i].path, 1_000_000 * (i + 1)]);
          i += 1;
          if (i >= batch.length) return done();
          setTimeout(tick, Math.max(0, start + i * gap - performance.now()));
        };
        tick();
      });
    },
    settle() {
      settleClassify(verdicts);
    },
  };
  window.__worktreesHarness = probe;

  // Only what the page asks for is answered. Anything else is recorded
  // and answered with null, and the driver prints the list, so a gap is
  // visible rather than silently shaping the figure.
  mockIPC(
    (cmd) => {
      probe.asked[cmd] = (probe.asked[cmd] ?? 0) + 1;
      switch (cmd) {
        case "list_worktrees":
          return { repos: [repo], unreadable: [] };
        case "get_worktree_dirs":
          return [ROOT];
        case "classify_worktrees":
          // Pending until the driver settles it, exactly as the command
          // is while its pass runs.
          return new Promise<Worktree[]>((resolve) => {
            settleClassify = resolve;
          });
        case "size_worktrees":
          return new Promise(() => {});
        case "list_prs":
        case "get_prs":
        case "cached_prs":
          return prs;
        case "assessed_worktrees":
          return [];
        default:
          probe.refused.push(cmd);
          return null;
      }
    },
    { shouldMockEvents: true },
  );

  useFilters.setState((s) => ({
    ...s,
    view: "worktrees",
    filtersByView: { ...s.filtersByView, worktrees: { ...(s.filtersByView.worktrees ?? {}), repo: REPO } },
  }));

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  // The PR list, as the page's own poll would have it by the time the
  // Worktrees view is open.
  client.setQueryData(["prs"], prs);

  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <QueryClientProvider client={client}>
        <Profiler
          id="worktrees"
          onRender={(_id, phase, actual, _base, _start, commit) => {
            probe.commits.push({ phase, actual, at: commit });
          }}
        >
          <div className="flex h-screen flex-col bg-[#0d1117] p-3">
            <WorktreesPage />
          </div>
        </Profiler>
      </QueryClientProvider>
    </StrictMode>,
  );
  document.body.dataset.harness = "ready";
}

void main();

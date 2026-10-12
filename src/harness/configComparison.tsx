// Synthetic screenshot fixture; no access to real repositories or providers.
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { mockIPC } from "@tauri-apps/api/mocks";
import { ConfigComparisonPanel } from "../components/ConfigComparisonPanel";
import type { ClaudeConfigComparison } from "../api/tauri";
import "../index.css";

mockIPC((command, input) => {
  const args = input as Record<string, unknown> | undefined;
  if (command === "list_worktrees") return { repos: [
    { path: "/projects/atlas", name: "Atlas", worktrees: [] },
    { path: "/projects/beacon", name: "Beacon", worktrees: [] },
  ], unreadable: [] };
  if (command === "claude_compare_config") return {
    leftLabel: String(args?.leftRepo), rightLabel: args?.rightRepo ? String(args.rightRepo) : "User configuration",
    limitations: ["Hooks and definitions compare explicit scopes. Nested worktrees are excluded."],
    rows: [
      { id: "settings:model", name: "model", category: "settings", status: "different",
        left: { state: "present", value: '"opus"', detail: ".claude/settings.local.json" },
        right: { state: "present", value: '"sonnet"', detail: "~/.claude/settings.json" } },
      { id: "claudeMd:project/CLAUDE.md", name: "project/CLAUDE.md", category: "claudeMd", status: "onlyLeft",
        left: { state: "present", value: "# Atlas\nRun make test before submitting changes.", detail: "/projects/atlas/CLAUDE.md" },
        right: { state: "absent", value: null, detail: null } },
      { id: "settings:permissions", name: "permissions", category: "settings", status: "cannotCompare",
        left: { state: "unknown", value: null, detail: "Local settings could not be parsed." },
        right: { state: "present", value: '{"allow":["Read"]}', detail: "~/.claude/settings.json" } },
    ],
  } satisfies ClaudeConfigComparison;
  throw new Error(`Unsupported synthetic command: ${command}`);
});
createRoot(document.getElementById("root")!).render(
  <QueryClientProvider client={new QueryClient()}>
    <main className="mx-auto max-w-3xl p-5 text-[#e6edf3]">
      <h1 className="text-lg font-semibold">Claude Code</h1>
      <p className="text-xs text-[#8b949e]">Synthetic configuration comparison</p>
      <ConfigComparisonPanel enabled />
    </main>
  </QueryClientProvider>,
);

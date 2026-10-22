import type { ClaudeMcpServer } from "../api/tauri";

// Apart from `ClaudePluginsPage.tsx` so that file exports only components
// (#1965), which is what lets dev hot-reload swap it in place.

/// Whether a server applies in the selected repository.
///
/// The per-repository half of #1216. User-scope and plugin-scope
/// servers apply everywhere; a project-scope server applies only to the
/// project it was configured under. Mirrors
/// `claude::mcp::Inventory::in_force`, whose doc comment argues the
/// trailing-separator rule, why no symlink is resolved, and why the
/// rule deliberately exists on both sides. A change here is a change
/// there.
///
/// `null` when no repository is selected -- the question has no subject
/// then, and answering it anyway would mark rows as out of force for a
/// repository the user has not named.
export function mcpInForce(server: ClaudeMcpServer, repo: string | undefined): boolean | null {
  if (repo === undefined) return null;
  if (server.origin !== "project") return true;
  // BOTH separators, matching `claude::mcp::normalise`. Trimming only
  // `/` is correct on Unix and wrong on Windows, where the same project
  // would be reported twice with the second showing nothing.
  const trim = (p: string) => p.replace(/[/\\]+$/, "");
  return server.scopeDetail !== null && trim(server.scopeDetail) === trim(repo);
}

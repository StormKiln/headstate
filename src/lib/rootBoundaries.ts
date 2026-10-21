import type { DiscoveryDiagnostic } from "../types/pr";

/// The scan roots that are themselves repositories (#1948). Discovery does
/// not look inside a checkout (#1919), so when a root IS one, any count of
/// repositories under it is a floor and the surface printing it says
/// "at least".
export function rootBoundaries(diagnostics: readonly DiscoveryDiagnostic[] | undefined): string[] {
  return (diagnostics ?? []).filter((d) => d.kind === "repositoryBoundary").map((d) => d.root);
}

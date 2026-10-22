import type { ArtifactKind } from "@/types/pr";

/// The artifact groups the sidebar and page name. Apart from
/// `ArtifactSidebar.tsx` so that file exports only components (#1965),
/// which is what lets dev hot-reload swap it in place.

/// The venv "kind", which is not an `ArtifactKind` -- virtualenvs are a
/// tool cache rather than build output, and giving them a fake kind in
/// that enum would put them behind the manifest rules that decide what
/// counts as an artifact.
export const VENV_GROUP = "poetry-venvs";

/// A group id: either a build-artifact kind or the venv group.
export type ArtifactGroup = ArtifactKind | typeof VENV_GROUP;

export const GROUP_LABEL: Record<ArtifactGroup, string> = {
  cargo_target: "Rust targets",
  node_modules: "Node modules",
  terraform: "Terraform providers",
  dotnet_build: ".NET build output",
  build_output: "Build output",
  swift_build: "Swift builds",
  [VENV_GROUP]: "Poetry virtualenvs",
};

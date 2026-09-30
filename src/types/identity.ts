/// Mirrors src-tauri/src/identity.rs. A host is an authority, without a
/// scheme/path. Project paths include every namespace (including subgroups).
type Provider = "github" | "gitlab";
type Host = string;
type ProjectPath = string;
type PrNumber = number;

export interface Source {
  provider: Provider;
  host: Host;
}

export interface PrIdentity {
  /// Missing only on legacy GitHub desktop/phone payloads.
  source?: Source;
  repo: ProjectPath;
  /// GitHub PR number or GitLab project-local IID, never GitLab's global id.
  number: PrNumber;
}

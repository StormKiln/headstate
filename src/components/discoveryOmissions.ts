/// Grouping for the Artifacts page's "Discovery is incomplete" notice.
///
/// A scan can report hundreds of omitted locations, and one row each made
/// the page unusable. They are grouped twice -- by REASON, since the
/// reason decides what the reader can do, then by PROJECT (the first
/// folder below the configured scan folder holding the path), since that
/// is the unit a person recognises -- and the rows are capped until the
/// reader asks for all of them. Nothing is dropped: every group carries
/// its full count, and "Show all" renders every path.

export interface Omission {
  root: string;
  why: string;
}

export interface ProjectGroup {
  /// The project folder, or the path itself when no scan folder holds it.
  project: string;
  paths: string[];
}

export interface ReasonGroup {
  why: string;
  count: number;
  projects: ProjectGroup[];
}

const SEP = /[\\/]/;

function trimTrailing(dir: string): string {
  return dir.length > 1 ? dir.replace(/[\\/]+$/, "") : dir;
}

/// The scan folder plus the first path segment below it, or `null` when
/// `path` is not inside `dir`. Separator-agnostic: Windows paths arrive
/// with backslashes.
function projectUnder(path: string, dir: string): string | null {
  const base = trimTrailing(dir);
  if (!path.startsWith(base)) return null;
  const rest = path.slice(base.length);
  if (rest === "") return base;
  if (!SEP.test(rest[0])) return null;
  const segment = rest.slice(1).split(SEP)[0];
  return segment ? base + rest[0] + segment : base;
}

/// Which project a path belongs to. The LONGEST matching scan folder
/// wins, so a nested scan folder groups its own paths.
export function projectOf(path: string, scanDirs: readonly string[]): string {
  let best: { dir: string; project: string } | null = null;
  for (const dir of scanDirs) {
    const project = projectUnder(path, dir);
    if (project !== null && (best === null || dir.length > best.dir.length)) {
      best = { dir, project };
    }
  }
  return best?.project ?? path;
}

/// Grouped by reason (largest first), then by project (largest first).
export function groupOmissions(
  omissions: readonly Omission[],
  scanDirs: readonly string[],
): ReasonGroup[] {
  const byReason = new Map<string, Map<string, string[]>>();
  for (const o of omissions) {
    let projects = byReason.get(o.why);
    if (!projects) byReason.set(o.why, (projects = new Map()));
    const project = projectOf(o.root, scanDirs);
    const paths = projects.get(project);
    if (paths) paths.push(o.root);
    else projects.set(project, [o.root]);
  }
  const bySize = (a: { n: number; k: string }, b: { n: number; k: string }) =>
    b.n - a.n || a.k.localeCompare(b.k);
  return [...byReason.entries()]
    .map(([why, projects]) => ({
      why,
      count: [...projects.values()].reduce((n, p) => n + p.length, 0),
      projects: [...projects.entries()]
        .map(([project, paths]) => ({ project, paths }))
        .sort((a, b) => bySize({ n: a.paths.length, k: a.project }, { n: b.paths.length, k: b.project })),
    }))
    .sort((a, b) => bySize({ n: a.count, k: a.why }, { n: b.count, k: b.why }));
}

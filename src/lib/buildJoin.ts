import type { DockerBuild, DockerImage } from "@/types/pr";

/// The build that produced an image, matched on the commit.
///
/// `buildx history` records a VCS Revision, and images in this project
/// are TAGGED with that same commit — verified against real records:
///
///   image tags:      eb066adc  68c14d4d  a9441aec
///   build revisions: eb066adc  68c14d4d  a9441aec
///
/// So no new field was needed; the key was already on both sides. The
/// image's resolved `origin.commit` is preferred over its raw tags,
/// since that is the value the origin resolver already verified against
/// a local repository.
///
/// Returns null freely. A project that tags by version rather than by
/// commit will match nothing here, and that is an ordinary state rather
/// than an error — the row simply shows no build data.
export function buildForImage(
  img: DockerImage,
  builds: DockerBuild[],
): DockerBuild | null {
  const keys = new Set<string>();
  if (img.origin?.commit) keys.add(img.origin.commit);
  for (const t of img.tags) keys.add(t);

  // Newest first, so an image rebuilt from the same commit reports the
  // build that actually produced what is on disk now.
  const matches = builds.filter((b) => b.revision && matchesAny(b.revision, keys));
  if (matches.length === 0) return null;
  return matches.reduce((newest, b) => (b.started > newest.started ? b : newest));
}

/// Whether a full revision matches any key.
///
/// Tags are ABBREVIATED commits (`eb066adc`) while `revision` is the
/// full forty characters, so this is a prefix comparison in one
/// direction — never a bare substring, which would let an unrelated
/// short tag match anywhere inside a SHA.
function matchesAny(revision: string, keys: Set<string>): boolean {
  for (const k of keys) {
    // Guard against a very short tag matching by luck. Git's own
    // minimum abbreviation is 7, and anything shorter is not a commit.
    if (k.length >= 7 && revision.startsWith(k)) return true;
    if (k === revision) return true;
  }
  return false;
}


/// A duration a human reads at a glance.
///
/// Sub-second builds are common (a fully cached target finishes in
/// 0.4s), so seconds get a decimal below ten -- rendering those as "0s"
/// would hide the difference between cached and instant.
export function formatDuration(secs: number): string {
  if (secs < 10) return `${secs.toFixed(1)}s`;
  if (secs < 60) return `${Math.round(secs)}s`;
  const m = Math.floor(secs / 60);
  const s = Math.round(secs % 60);
  return `${m}m ${s}s`;
}

/// Cache ratio, coloured by what it implies.
///
/// This is the number that explains the duration beside it. Real data:
/// the same target at 48% cached took 56.9s, at 23% took 80.7s. A cold
/// build is not a problem in itself -- a cold build that used to be warm
/// is.
/// `null` when the ratio cannot be computed (#963).
///
/// Mirrors the Rust `Build::cache_percent`. Three cases: buildx did not
/// report the counts, buildx reported no steps at all (nothing to cache,
/// so there was no rate), or a real ratio. The first two used to answer
/// `0`, and 0% cached is the alarm -- a build that used to be warm going
/// cold -- so an absence raised it.
export function cachePercent(b: DockerBuild): number | null {
  if (b.total_steps === null || b.cached_steps === null) return null;
  if (b.total_steps === 0) return null;
  return Math.floor((b.cached_steps * 100) / b.total_steps);
}

export function cacheTone(pct: number): string {
  if (pct >= 60) return "text-[#3fb950]";
  if (pct >= 25) return "text-[#d29922]";
  return "text-[#8b949e]";
}

/// Cache health across the most recent builds.
///
/// The number the Builds page existed to show, reduced to the form that
/// is worth a glance: a cold build is not a problem in itself, but a
/// target that USED to be warm and is not any more means something
/// invalidated the cache -- a reordered Dockerfile layer, usually.
///
/// Deliberately over recent builds rather than all of them. Averaging
/// six months of history hides exactly the change worth noticing, and
/// the answer to "is my build cache working today" is not improved by
/// last quarter's numbers.
export function recentCacheHealth(
  builds: DockerBuild[],
  recent = 10,
): { percent: number; count: number } | null {
  // Only builds whose counts were actually reported. A build missing its
  // step counts contributes nothing rather than a fabricated zero to
  // both sides of the ratio (#963) -- which is what `total_steps > 0`
  // already achieved for the old `0` coercion, and now reads as what it
  // means.
  const usable = builds
    .filter(
      (b): b is DockerBuild & { total_steps: number; cached_steps: number } =>
        b.total_steps !== null && b.cached_steps !== null && b.total_steps > 0,
    )
    // Newest first. `started` is RFC 3339, so lexical order is
    // chronological.
    .sort((a, b) => b.started.localeCompare(a.started))
    .slice(0, recent);
  if (usable.length === 0) return null;

  // Weighted by STEPS, not a mean of percentages: a 2-step build at 0%
  // and a 40-step build at 90% is not 45% cached, and the unweighted
  // form would make one trivial target swing the whole number.
  const cached = usable.reduce((n, b) => n + b.cached_steps, 0);
  const total = usable.reduce((n, b) => n + b.total_steps, 0);
  return { percent: Math.floor((cached * 100) / total), count: usable.length };
}

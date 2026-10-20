/// The label every saved-scan paint carries (#1152, #1950).
///
/// One component, so Artifacts, Worktrees and Repositories say the same
/// thing in the same words: the rows below are a previous scan, this is
/// how old it is, and a live one is running. The age is not a caveat; it
/// is the whole claim -- an undated previous scan is a lie about disk
/// (`ArtifactsFromCache` states why).
export function SavedScanNotice({
  ageSecs,
  stale,
  compact = false,
}: {
  ageSecs: number;
  stale: boolean;
  compact?: boolean;
}) {
  return (
    <div
      className={`rounded-md border ${compact ? "px-2 py-1" : "px-3 py-2"} text-xs ${
        stale
          ? "border-[#d29922]/40 bg-[#d29922]/5 text-[#d29922]"
          : "border-[#30363d] bg-[#161b22] text-[#8b949e]"
      }`}
      role="status"
    >
      Showing the last scan, from {describeAge(ageSecs)}. Rescanning now
      {stale ? " — this result is old enough that it may have changed." : "."}
    </div>
  );
}

/// How long ago, in words a sentence can use.
function describeAge(secs: number): string {
  if (secs < 60) return "moments ago";
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins} minute${mins === 1 ? "" : "s"} ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours} hour${hours === 1 ? "" : "s"} ago`;
  const days = Math.floor(hours / 24);
  return `${days} day${days === 1 ? "" : "s"} ago`;
}

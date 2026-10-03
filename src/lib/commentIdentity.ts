import type { PrComment } from "@/types/pr";

// Keys are local to one PR/thread. IDs survive edits and bounded-page churn.
// Old snapshots have no IDs: a unique author/time survives body edits. In an
// ambiguous bucket, body + occurrence avoids conflating different comments.
// Edits or membership changes in such buckets can reset state; indistinguishable
// duplicates cannot have durable identity without a provider ID.
export function commentKeys(comments: readonly PrComment[]): string[] {
  const bases = comments.map(c => JSON.stringify(c.id ? ["id", c.id] : ["legacy", c.author, c.created_at]));
  const counts = new Map<string, number>();
  for (const base of bases) counts.set(base, (counts.get(base) ?? 0) + 1);
  const occurrences = new Map<string, number>();
  return comments.map((c, i) => {
    const base = bases[i];
    if (counts.get(base) === 1) return base;
    const collision = JSON.stringify([base, c.body]);
    const occurrence = occurrences.get(collision) ?? 0;
    occurrences.set(collision, occurrence + 1);
    return JSON.stringify([collision, occurrence]);
  });
}

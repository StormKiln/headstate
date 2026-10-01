import { safeUnlisten } from "@/api/unlisten";
import { useFilters } from "@/store/filters";
import { useSourceSelection } from "@/store/sourceSelection";
import type { PrIdentity } from "@/types/identity";

// Subscribe before draining: a click arriving before React mounts remains in
// Rust, and one arriving during listener registration cannot fall in a gap.
export async function connectNotificationNavigation(
  subscribe: (wake: () => void) => Promise<() => void>,
  take: () => Promise<PrIdentity | null>,
  signal?: AbortSignal,
): Promise<() => void> {
  let active = !signal?.aborted;
  signal?.addEventListener("abort", () => { active = false; }, { once: true });
  let drain = Promise.resolve();
  const wake = () => {
    drain = drain.then(async () => {
      if (!active) return;
      const target = await take();
      // Once taken, route even if React tears this effect down while IPC
      // resolves. The destination is app state, not a mounted component.
      if (!target) return;
      useSourceSelection.getState().setSelection(target.source?.provider ?? "github");
      useFilters.getState().setView("my-prs");
      useFilters.getState().selectPr(target);
    }).catch((error: unknown) => console.error("Could not open notification", error));
  };
  const unlisten = await subscribe(wake);
  if (!active) { safeUnlisten(unlisten); return () => {}; }
  wake();
  await drain;
  return () => { active = false; safeUnlisten(unlisten); };
}

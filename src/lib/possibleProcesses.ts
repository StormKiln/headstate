import type { Liveness, PossibleProcesses } from "@/types/pr";

/** Invalid detail data loses only its display evidence, never the Unknown verdict. */
export function possibleProcesses(value: unknown): PossibleProcesses | undefined {
  if (!value || typeof value !== "object" || !("total" in value)) return undefined;
  const { total } = value;
  if (typeof total !== "number" || !Number.isSafeInteger(total) || total <= 0) return undefined;
  const candidates = "candidates" in value ? value.candidates : undefined;
  const omitted = { total, candidates: [] };
  if (!Array.isArray(candidates) || candidates.length > 8 || candidates.length > total) return omitted;
  if (!candidates.every((c: unknown) => {
    if (!c || typeof c !== "object" || !("pid" in c) || !("cwd" in c) || !("cwd_truncated" in c)) return false;
    return typeof c.pid === "number" && Number.isInteger(c.pid) && c.pid > 0 && c.pid <= 0xffffffff &&
      typeof c.cwd_truncated === "boolean" &&
      (c.cwd === null ? !c.cwd_truncated : typeof c.cwd === "string" && c.cwd.length <= 1024 && new TextEncoder().encode(c.cwd).length <= 1024);
  })) return omitted;
  return { total, candidates };
}

export function hydratePossibleProcesses(reference: unknown, groups: unknown): PossibleProcesses | undefined {
  if (!reference || typeof reference !== "object" || !("total" in reference)) return undefined;
  const group = "group" in reference ? reference.group : null;
  const candidates = typeof group === "number" && Number.isInteger(group) && group >= 0 && group < 32 && Array.isArray(groups)
    ? groups[group] : undefined;
  return possibleProcesses({ total: reference.total, candidates });
}

export function possibleProcessLabel(liveness: Liveness): string | null {
  if (liveness.state !== "unknown") return null;
  const evidence = possibleProcesses(liveness.possible_processes);
  if (!evidence) return null;
  return evidence.total === 1 && evidence.candidates.length === 1
    ? `May be running · pid ${evidence.candidates[0].pid}`
    : `May be running · ${evidence.total} possible process${evidence.total === 1 ? "" : "es"}`;
}

import { Layers } from "lucide-react";
import type { PrStack } from "@/types/pr";
import { stackLabel, stackTitle } from "@/lib/stack";

export function ReadyStackChip({ stack, retained = false, observedAt }: { stack: PrStack | undefined; retained?: boolean; observedAt?: number }) {
  if (stack?.kind !== "stacked") return null;
  const position = stack.position_exact ? `${stack.position}` : `at least ${stack.position}`;
  const size = stack.size_exact ? `${stack.size}` : `at least ${stack.size}`;
  const observed = observedAt === undefined ? "" : `, observed ${new Date(observedAt).toLocaleString()}`;
  const explanation = retained ? `Last known stack${observed}; current membership is unconfirmed. ${stackTitle(stack)}` : stackTitle(stack);
  return (
    <span
      title={explanation}
      aria-description={retained ? explanation : undefined}
      aria-label={`${retained ? "Last known stack" : "Stack"} position ${position} of ${size}`}
      className="inline-flex shrink-0 items-center gap-1 rounded-full border border-[#a371f7]/40 px-1.5 py-0.5 text-xs tabular-nums text-[#a371f7]"
    >
      <Layers className="h-3 w-3 shrink-0" aria-hidden="true" />
      {retained && "last known · "}
      {stackLabel(stack)?.replace(/^stack /, "")}
    </span>
  );
}

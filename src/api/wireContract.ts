import { wireGraph } from "./wireContract.generated";
import { validateWire } from "./wireValidator";

const own = (registry: Record<string, number>, name: string) => Object.prototype.hasOwnProperty.call(registry, name);

export function assertRemoteReply(name: string, value: unknown): void {
  if (!own(wireGraph.commands, name)) {
    // Unknown names might themselves contain arbitrary values. Do not echo.
    throw new Error("This command has no remote response contract. Update both apps.");
  }
  const { field } = validateWire(wireGraph.nodes, wireGraph.commands[name], value);
  if (field) throw new Error(`The desktop returned an incompatible response for ${name} at ${field}. Update both apps.`);
}

export function isRemoteEvent(name: string): boolean {
  return own(wireGraph.events, name);
}

export function remoteEventError(name: string, value: unknown): string | null {
  if (!isRemoteEvent(name)) return null; // Companion-local events.
  const { field } = validateWire(wireGraph.nodes, wireGraph.events[name], value);
  return field ? `The desktop sent an incompatible ${name} event at ${field}. Update both apps.` : null;
}

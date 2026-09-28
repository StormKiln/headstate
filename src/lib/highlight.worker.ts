import { runJob } from "./highlightCore";
import type { GrammarName } from "./highlightLangs";

/// The highlighting worker (#1482). One job in, one tree or error out.
///
/// Its whole reason to exist is that it can be KILLED: see `highlight.ts`.
/// A job's code never leaves this worker except as the tree it asked
/// for -- no logging of it, here or on failure.

type Job = { id: number; code: string; grammar: GrammarName };

const scope = self as unknown as {
  onmessage: ((e: MessageEvent<Job>) => void) | null;
  postMessage(message: unknown): void;
};

scope.onmessage = (e) => {
  const { id, code, grammar } = e.data;
  runJob(code, grammar).then(
    (tree) => scope.postMessage({ id, tree }),
    () => scope.postMessage({ id, failed: true }),
  );
};

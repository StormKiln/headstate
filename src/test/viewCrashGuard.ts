/// Fails a test in which a `ViewErrorBoundary` caught a render error (#1965).
///
/// The boundary exists so one broken view cannot take the window down,
/// and in a test that is exactly the problem: the throw becomes a tidy
/// error panel, the assertion that follows looks at something else, and
/// the test passes. `App.test.tsx` passed for weeks while the Worktrees
/// view crashed on every render, because its `./api/hooks` mock had no
/// `useWorktreeSafety` and the boundary swallowed the result.
///
/// Detection reads the boundary's own log line, so it needs no import of
/// app code here -- a setup file that imported `ViewErrorBoundary` would
/// load `QueryError` and its tree BEFORE a test file's `vi.mock` calls
/// were registered, and those modules would escape the test's mocks.
/// `viewCrashGuard.test.tsx` renders the real boundary, so a reworded log
/// line fails that test instead of silently disabling this guard.
///
/// A test that crashes a view on purpose calls `expectViewCrash()`. A
/// test that silences `console.error` with its own spy has also opted out,
/// knowingly: the line never reaches the wrapper installed here.

/// The prefix `ViewErrorBoundary.componentDidCatch` logs with.
const BOUNDARY_LOG = "Unhandled render error in ";

let caught: string[] = [];
let expected = false;

/// Declare that this test crashes a view deliberately.
///
/// Returns the crashes seen so far in this test, so the caller can assert
/// on them rather than merely tolerating them.
export function expectViewCrash(): { readonly caught: readonly string[] } {
  expected = true;
  return {
    get caught() {
      return caught;
    },
  };
}

/// Wrap `console.error` for one test. Passes every call through unchanged:
/// this records, it never silences.
export function installViewCrashGuard(): () => void {
  caught = [];
  expected = false;
  const previous = console.error;
  const wrapper = (...args: unknown[]) => {
    if (typeof args[0] === "string" && args[0].startsWith(BOUNDARY_LOG)) {
      const error = args[1];
      const detail = error instanceof Error ? error.message : String(error);
      caught.push(`${args[0]} ${detail}`);
    }
    previous.apply(console, args);
  };
  console.error = wrapper;
  return () => {
    // Only undo our own wrapper; a test that replaced `console.error`
    // outright and left it that way is not ours to repair.
    if (console.error === wrapper) console.error = previous;
  };
}

/// The crashes this test did not declare, then reset for the next test.
export function takeUnexpectedViewCrashes(): string[] {
  const out = expected ? [] : caught;
  caught = [];
  expected = false;
  return out;
}

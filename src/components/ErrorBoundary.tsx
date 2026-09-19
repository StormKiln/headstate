import { Component, type ErrorInfo, type ReactNode } from "react";
import { QueryError } from "./QueryError";
import { ReportLink } from "./ReportLink";
import { dismissSplash } from "../splash";

/// Last-resort catch for a render-time throw.
///
/// Without this, a throw during render unmounts the ENTIRE React tree and
/// leaves an empty window: no message, no recovery, no clue what failed.
/// That is how #244 presented -- the cause was a one-line undefined read
/// in the filters store, but the symptom was indistinguishable from a
/// hang, and diagnosing it took the dev-server console. A user on a
/// release build has nothing to report beyond "it's black".
///
/// A class component because that is the only way to catch: there is no
/// hook equivalent of `getDerivedStateFromError`.
///
/// This catches RENDER errors only. It does not replace the `poll-error`
/// banner or `QueryError` -- async rejections never reach a boundary, and
/// both of those remain the right surface for a failed query.
interface Props {
  children: ReactNode;
  /// Clears persisted state. Injected rather than imported so the boundary
  /// stays free of store knowledge and the test can observe the call.
  onReset?: () => void;
}

interface State {
  error: Error | null;
  /// The component stack, for the report (#1148).
  ///
  /// Separate from `error` because the two arrive at different times:
  /// `getDerivedStateFromError` gets the error and NOT the stack, and
  /// `componentDidCatch` runs afterwards with the stack. `null` means
  /// either not caught yet or React did not supply one -- the report
  /// omits the section rather than printing an empty block.
  componentStack: string | null;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, componentStack: null };

  static getDerivedStateFromError(error: Error): Partial<State> {
    // NOT the stack: `getDerivedStateFromError` does not receive it.
    // `componentDidCatch` runs after and adds it, which is why the two
    // live in separate state fields rather than one object.
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    // The console is the only record of the component stack, which is what
    // actually locates the throw. Kept even though the UI shows the
    // message: the message alone did not identify the file in #244.
    console.error("Unhandled render error:", error, info.componentStack);

    // And kept in state, so "Report this" can attach it (#1148). The
    // stack is the single most valuable thing in a crash report and
    // until now it existed only in a console nobody has open on a
    // release build -- so the user's report said "it showed a red box"
    // and the maintainer had nothing to go on.
    this.setState({ componentStack: info.componentStack ?? null });

    // The splash is a fixed, inset-0, z-index-9999 overlay dismissed only
    // by AuthGate's settled-auth effect. A crash before that point renders
    // this message perfectly, and perfectly INVISIBLY, underneath it --
    // the exact v1.0.0 hang the splash failsafe exists for. Anything that
    // leaves the app on a non-App branch must uncover the window.
    dismissSplash();
  }

  private handleReset = (): void => {
    // Order matters: clear the bad state BEFORE reloading, or the reload
    // rehydrates the same crash. #244 came from persisted state and so
    // reproduced on every launch -- a plain reload would loop forever.
    this.props.onReset?.();
    window.location.reload();
  };

  render(): ReactNode {
    const { error } = this.state;
    if (!error) return this.props.children;

    return (
      <div className="flex h-screen items-center justify-center bg-[#0d1117] p-8 text-[#e6edf3]">
        <div className="w-full max-w-lg">
          <QueryError title="Something went wrong" message={error.message}>
            <div className="mt-4 flex items-center justify-center gap-3">
              <button
                type="button"
                onClick={this.handleReset}
                className="rounded border border-[#30363d] px-3 py-1.5 text-sm text-[#e6edf3] hover:bg-[#161b22]"
              >
                Reset and reload
              </button>
              {/* Beside the remedy, not instead of it. A crash is the
                  error most worth reporting and the one the user is
                  least able to describe -- their only route was to
                  recall a red box from memory (#1148).

                  No `view`: this boundary sits above `AuthGate` and
                  `QueryClientProvider`, so a throw here may be a boot
                  failure with no view to name. Naming one would be a
                  guess, and an omitted line is honest. */}
              <ReportLink
                error={error.message}
                componentStack={this.state.componentStack ?? undefined}
                className="text-sm underline hover:no-underline"
              />
            </div>
          </QueryError>
        </div>
      </div>
    );
  }
}

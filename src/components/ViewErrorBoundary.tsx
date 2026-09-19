import { Component, type ErrorInfo, type ReactNode } from "react";
import { QueryError } from "./QueryError";

/// A render throw in ONE view, contained to that view (#1146).
///
/// Every view sat under the single root boundary, so a throw anywhere --
/// a chart component, a lazy chunk that failed to load after an update
/// or while offline -- replaced the entire window with a full-screen
/// error and a "Reset and reload" that clears persisted state. A user
/// looking at Docker lost the pull-request list, the sidebar and their
/// filters because one component threw.
///
/// This is about BLAST RADIUS, not about what a boundary catches. The
/// root one stays exactly as it is, including its heavier remedy: a
/// throw in `QueryClientProvider`, `PairingGate` or `AuthGate` is a boot
/// failure and there is no chrome left to render into.
///
/// A class component because that is the only way to catch -- there is
/// no hook equivalent of `getDerivedStateFromError`. Render errors only;
/// async rejections never reach a boundary, and `QueryError` remains the
/// surface for a failed query.
interface Props {
  children: ReactNode;
  /// The view's own name, so the panel can say which pane is dead.
  ///
  /// "Something went wrong" over a still-working shell does not say
  /// that, and the shell is precisely what this boundary preserves.
  view: string;
}

interface State {
  error: Error | null;
}

/// Whether this looks like a lazy chunk that could not be fetched.
///
/// Four views are `React.lazy` chunks. `Suspense` does not catch a
/// rejected `import()`, so a failed fetch throws past it to a boundary
/// -- and the remedy is a relaunch after an update, not a state reset.
/// Saying "reset" there sends the user to do something that cannot help.
function isChunkLoadFailure(e: Error): boolean {
  const text = `${e.name} ${e.message}`;
  return (
    /ChunkLoadError/i.test(text) ||
    /Failed to fetch dynamically imported module/i.test(text) ||
    /error loading dynamically imported module/i.test(text) ||
    /Importing a module script failed/i.test(text)
  );
}

export class ViewErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    // The component stack is what actually locates the throw, and on a
    // release build the console is the only record of it -- the same
    // reasoning the root boundary states.
    console.error(`Unhandled render error in ${this.props.view}:`, error, info.componentStack);
  }

  /// Recover this view only.
  ///
  /// NOT a reload: the sidebar, the switcher and every other view are
  /// still working, and reloading would throw them away to recover one.
  ///
  /// Clearing `error` is a full remount on its own -- the children are
  /// unmounted while the panel is up, so they come back with fresh
  /// state and re-run their `useState` initialisers. An `attempt` key
  /// on the subtree was here to force that and was removed: it could
  /// not be made to fail a test, because there is no path where the
  /// children survive the panel.
  private retry = (): void => {
    this.setState({ error: null });
  };

  render(): ReactNode {
    const { error } = this.state;
    if (!error) return this.props.children;

    const chunk = isChunkLoadFailure(error);
    return (
      <div className="p-4">
        <QueryError
          // NAMES the view. Over a still-working shell, "something went
          // wrong" does not say which pane is dead.
          title={
            chunk
              ? `${this.props.view} could not load`
              : `Something went wrong in ${this.props.view}`
          }
          message={
            chunk
              ? "This part of the app could not be downloaded. If Headstate updated while it " +
                "was open, relaunching will pick up the new version."
              : error.message
          }
          onRetry={this.retry}
        >
          <p className="mt-2 text-xs text-[#8b949e]">
            {/* Says what SURVIVED, which is the whole point of the
                smaller blast radius: the user can carry on elsewhere
                rather than reaching for the reload. */}
            Everything else is still working — the other views are unaffected.
          </p>
        </QueryError>
      </div>
    );
  }
}

import { QueryClient, QueryClientProvider, focusManager } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { Toaster } from "sonner";
import { AuthGate } from "./components/AuthGate";
import { PairingGate } from "./components/PairingGate";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { PairingRequestModal } from "./components/PairingRequestModal";
import { PERSIST_KEY } from "./store/filters";
import { PhoneMeasurementStartup } from "./api/phoneMeasurements";
import { IS_MOBILE_BUILD } from "./lib/target";
import { IS_DESKTOP_BUILD } from "./lib/target";
import { initSplash } from "./splash";
import "./index.css";
// Sonner 2.x ships its layout in a SEPARATE stylesheet and its runtime
// never references it. Without this import the toast still mounts and
// still holds the right text, but with no `position: fixed` it lands in
// normal document flow at the bottom of the page: an unstyled black
// block, off-screen, that grows the document and makes a scrollbar
// appear -- which is what the "layout shifts when I click Claudify"
// reports were actually describing.
import "sonner/dist/styles.css";

// Starts the splash's minimum-visible window and arms its failsafe. An
// explicit call rather than module-load side effects, so tests can drive
// the timing with fake timers instead of real sleeps.
initSplash();

const queryClient = new QueryClient();

// iOS suspends the webview, and a resume fires `visibilitychange` but
// does NOT reliably fire `window.focus` -- which is what TanStack's
// default `refetchOnWindowFocus` waits for. So on the one platform where
// suspension is guaranteed, the focus-based refetch that keeps the
// desktop current was unreliable: the `prs` list recovered (the
// companion replays a snapshot frame on resubscribe) but everything
// pulled rather than pushed did not. Leave a PR detail view, background
// the app for an hour, come back: hour-old checks and review threads,
// with nothing to say so.
//
// Driving `focusManager` rather than calling `invalidateQueries` keeps
// each query's own `staleTime` in charge -- a blanket invalidate would
// refetch things that are deliberately cached forever. Registered for
// both builds: on the desktop `visibilitychange` fires when the window
// is minimised or hidden, which is a moment worth refetching on there
// too, and the desktop's existing focus behaviour is unchanged.
if (typeof document !== "undefined") {
  focusManager.setEventListener((handleFocus) => {
    const onChange = () => handleFocus(document.visibilityState === "visible");
    document.addEventListener("visibilitychange", onChange, false);
    window.addEventListener("focus", onChange, false);
    return () => {
      document.removeEventListener("visibilitychange", onChange);
      window.removeEventListener("focus", onChange);
    };
  });
}

createRoot(document.getElementById("root") as HTMLElement).render(
  <StrictMode>
    {/* Outside QueryClientProvider and AuthGate on purpose: a throw in
        either of those must still land on a readable screen, and this is
        the only thing left to render it. */}
    <ErrorBoundary onReset={() => localStorage.removeItem(PERSIST_KEY)}>
      <QueryClientProvider client={queryClient}>
        {IS_MOBILE_BUILD ? <PhoneMeasurementStartup /> : null}
        {/* Pairing is the OUTER gate, and only on the phone. The
            companion forwards every desktop command over `remote_call`,
            which rejects while unpaired -- so `AuthGate`'s check failed
            on a fresh install and showed the desktop's "install the
            GitHub CLI" screen to a device that has no shell. On the
            desktop build `PairingGate` is a pass-through. */}
        <PairingGate>
          <AuthGate>
            <App />
          </AuthGate>
        </PairingGate>
        {/* Dark to match the app, and bottom-right so it never covers the
            list the user is acting on. */}
        <Toaster theme="dark" position="bottom-right" richColors />
        {/* Beside the toaster, not inside App: a phone can scan the
            pairing code whether or not GitHub is signed in, and the
            request must reach a person either way.

            Desktop only. This is the desktop's APPROVAL dialog -- it
            polls `pairing_request` and answers with
            `respond_to_pairing`, both `Class::Local`, so on the phone it
            asks a phone to decide who may pair with it. Deciding that is
            the desktop user's job at the desktop, which is exactly why
            those commands are Local. */}
        {IS_DESKTOP_BUILD ? <PairingRequestModal /> : null}
      </QueryClientProvider>
    </ErrorBoundary>
  </StrictMode>,
);

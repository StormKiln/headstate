import { useQuery } from "@tanstack/react-query";
import { useEffect, type ReactNode } from "react";
import { clearPollError, usePollError, useStoreError } from "../api/hooks";
import { ReportLink } from "./ReportLink";
import { getAuthState } from "../api/tauri";
import { useConnectionState } from "@/api/connection";
import { IS_MOBILE_BUILD } from "@/lib/target";
import { dismissSplash } from "../splash";
import { isNotAsked, notAskedMessage } from "@/lib/notAsked";

/// Gates the whole app on `get_auth_state`. Rust computes auth once at
/// startup from the `gh` CLI token, so this is a one-shot check, not a
/// poll -- `staleTime: Infinity` avoids a pointless refetch on window
/// focus for a value that cannot change without an app restart.
///
/// `isLoading` and "authenticated but not yet ok" are deliberately
/// distinct from the failure screen below: returning `null` while loading
/// avoids flashing the "install gh" message for authenticated users before
/// the first render settles.
///
/// On the PHONE there is a third case, and it is the ordinary one: the
/// desktop is not reachable. `get_auth_state` is a `Class::Read` command
/// forwarded over `remote_call`, and the companion serves only
/// `get_cached` from its stored snapshot -- every other read rejects with
/// "<desktop> is unreachable". So a phone away from its desktop, which is
/// a phone on cellular, which is most of the time, failed this check on
/// launch. See `offline` below for what that produced and why the answer
/// is to let the app through rather than to gate on it.
export function AuthGate({ children }: { children: ReactNode }) {
  const { data, isLoading } = useQuery({
    queryKey: ["auth"],
    queryFn: getAuthState,
    staleTime: Infinity,
    // Off-network the retries are the black page (#684). TanStack's
    // default is three with exponential backoff -- about seven seconds
    // of `isLoading`, during which the branch below renders `null`
    // while `PairingGate` has already taken the splash down at its 3s
    // floor. The result is a `#0d1117` window with nothing in it, which
    // is indistinguishable from a crash and is the FIRST thing a user
    // sees when they open the app away from their desk.
    //
    // Retrying is also pointless here: `connection_state` already
    // knows the desktop is away, and the auth query refetches on its
    // own when the app returns to the foreground (`focusManager` in
    // `main.tsx`) and when the connection comes back. Three doomed
    // round-trips only buy a longer blank.
    //
    // Kept for the DESKTOP, where a rejection is a genuinely transient
    // IPC failure worth a second attempt and there is no connection
    // state to consult.
    //
    // SPREAD, not `retry: IS_MOBILE_BUILD ? false : undefined`. That
    // reads the same and is not: an explicit `undefined` is still a
    // present key, and TanStack takes a present key over the client's
    // `defaultOptions.queries.retry`. Written that way it silently
    // turned retries back ON for every desktop test that had switched
    // them off, which is a thing this file's own test caught only
    // because it asserts on the desktop path too.
    ...(IS_MOBILE_BUILD ? { retry: false } : {}),
  });
  const pollError = usePollError();
  const storeError = useStoreError();
  // `local` on the desktop build by construction, so `offline` below is
  // always false there and every branch after it renders exactly what it
  // rendered before.
  const connection = useConnectionState();
  // The states in which the desktop cannot answer for its own GitHub
  // auth. Named positively rather than as `!== "connected"` so that
  // `unknown` is a deliberate omission: `PairingGate` sits ABOVE this
  // component and holds the splash on `unknown`, so this never renders
  // in that state, and treating it as offline here would be a second,
  // silently disagreeing copy of that rule.
  //
  // `connecting` belongs with `unreachable`: the answer is not in yet,
  // and there is a cached list to show while it arrives.
  //
  // On `IS_MOBILE_BUILD`, not `useIsMobile()`: whether this app can
  // reach a desktop at all is a capability of the build, and a desktop
  // window dragged under 768px still has `gh` and must still be gated
  // (#598).
  const offline =
    IS_MOBILE_BUILD && (connection.kind === "unreachable" || connection.kind === "connecting");

  // Dismissal belongs HERE, not in `App`, and keys off the auth check
  // having SETTLED rather than succeeded.
  //
  // `App` only mounts when auth is ok, so dismissing there left an
  // unauthenticated machine showing the splash forever -- with the "needs
  // the GitHub CLI" screen rendered correctly underneath a fixed,
  // inset-0, z-index-9999 overlay that hides it. Anything that leaves the
  // app on a non-App branch must still uncover the window; the only state
  // that should hold the splash is "we do not know yet".
  //
  // `offline` joins `!isLoading` for the same reason `PairingGate`
  // dismisses on every terminal state: an offline phone is about to
  // render a real screen -- the cached list -- and a fixed inset-0
  // z-index-9999 overlay left over it would hide that screen. Without
  // this the mobile fix would have swapped a blank window for a blank
  // window with the app behind it.
  useEffect(() => {
    if (!isLoading || offline) dismissSplash();
  }, [isLoading, offline]);

  // Offline FIRST, ahead of the loading branch. Waiting out an auth
  // check that cannot be answered is the black page: `isLoading` stays
  // true across the retries, this returned `null`, and `PairingGate`
  // had already lifted the splash -- so the launch screen for a phone
  // away from its desk was an empty `#0d1117` window (#684).
  //
  // Letting the children through is not a guess that the desktop is
  // signed in. It is that the desktop's auth is unknowable from here
  // and NOT what the user needs told: `ConnectionBanner` already names
  // the desktop and when it was last seen, `StaleRibbon` already marks
  // the cached rows as a saved copy, and `useWritesPaused` already
  // disables the actions that would need the desktop. Those three are
  // the honest, proportionate report, and they are already built. The
  // rule from #602 -- cached data is MARKED, not hidden -- is the same
  // rule one layer up: unreachable must not mean blank.
  //
  // `get_cached` is the one read the companion serves from its stored
  // snapshot, so there is genuinely something to show. Where there is
  // not, `PrList` renders its own empty state, which is honest too.
  if (offline) return <>{children}</>;

  if (isLoading) return null;
  if (data?.ok) {
    return (
      <>
        {/* Its own banner, on its own channel. A store failure describes
            a condition a later successful poll did not fix, so it must
            not be cleared by one -- which is what sharing `poll-error`
            did, microseconds after it appeared. */}
        {storeError.message !== null && (
          <div
            role="alert"
            className="flex items-start gap-2 border-b border-[#d29922]/30 bg-[#d29922]/10 px-4 py-2 text-sm text-[#d29922]"
          >
            <span className="flex-1">
              {storeError.message} Your pull requests are still live; only the local
              cache is affected.
            </span>
            <button
              type="button"
              onClick={storeError.dismiss}
              aria-label="Dismiss"
              className="shrink-0 rounded px-1 hover:bg-[#d29922]/20"
            >
              ×
            </button>
          </div>
        )}
        {pollError !== null && (
          <div
            // #1124: a poll the app DECLINED to issue is not a failed
            // refresh. The banner said "Background refresh failed" and
            // printed the rejection verbatim -- both wrong, and the
            // marker would have reached the screen. Amber and `status`
            // rather than red and `alert`, matching the store-error
            // banner above: nothing went wrong, something is not set up.
            role={isNotAsked(pollError) ? "status" : "alert"}
            className={
              isNotAsked(pollError)
                ? "flex items-start gap-2 border-b border-[#d29922]/30 bg-[#d29922]/10 px-4 py-2 text-sm text-[#d29922]"
                : "flex items-start gap-2 border-b border-[#f85149]/30 bg-[#f85149]/10 px-4 py-2 text-sm text-[#f85149]"
            }
          >
            <span className="flex-1">
            {isNotAsked(pollError)
              ? `Not refreshing in the background: ${notAskedMessage(pollError)}`
              : `Background refresh failed: ${pollError}`}
            {/* The errors that most need reporting are exactly the ones
                a user cannot diagnose, and the banner offered nothing.
                Opens a PREFILLED form rather than posting: the user is
                the only one who can confirm nothing sensitive survived
                scrubbing. */}
            <ReportLink error={pollError} />
            {/* The token is read once at startup and held for the process
                lifetime, so a revoked or expired one 401s forever with the
                list silently going stale. A relaunch is the actual fix;
                saying so beats an opaque message the user cannot act on.
                Refreshing the token in-process is tracked separately. */}
            {/401|unauthorized|bad credentials/i.test(pollError) ? (
              <span className="ml-1">
                Your GitHub token may have expired — run <code>gh auth login</code> and
                restart Headstate.
              </span>
            ) : null}
            </span>
            {/* Dismissable: a rate limit the user has read is not
                information worth pinning for an hour, and its own text
                says polling resumes automatically. */}
            <button
              type="button"
              onClick={clearPollError}
              aria-label="Dismiss"
              className="shrink-0 rounded px-1 hover:bg-[#f85149]/20"
            >
              ×
            </button>
          </div>
        )}
        {children}
      </>
    );
  }

  // On the phone the remediation below is impossible advice: there is
  // no Homebrew, no shell, and by design no GitHub token -- the DESKTOP
  // holds it. A failed auth check here means the paired desktop is not
  // signed in, which is a thing to fix at the desktop.
  //
  // `PairingGate` above already sends an unpaired phone to the pairing
  // screen, so reaching this branch on mobile means paired-but-the-
  // desktop-cannot-authenticate. Guarded on the build target rather than
  // `useIsMobile()`: a narrow desktop window still has `gh`, and telling
  // its user to fix it elsewhere would be the same bug mirrored.
  if (IS_MOBILE_BUILD) {
    // The desktop must have ANSWERED for the screen below to be true.
    //
    // This guard is the accusation half of #684. Without it a rejected
    // query landed here as well -- `data` at its undefined default,
    // `data?.ok` falsy -- and the phone covered the whole app with
    // "your desktop is not signed in to GitHub" on the strength of
    // never having managed to ask it. Off-network that was the reported
    // full-screen error; the `offline` branch above now takes that case
    // first, and this catches the remainder.
    //
    // What remains is a desktop the connection says is THERE whose auth
    // call did not come back: a transient forwarding failure, not a
    // verdict on anybody's GitHub login. So the phone degrades the same
    // way it does offline -- the app over its cached snapshot, with the
    // shell's own banners carrying the failure.
    if (data === undefined) return <>{children}</>;
    return (
      <div className="flex min-h-dvh items-center justify-center bg-[#0d1117] px-6 text-[#e6edf3]">
        <div className="max-w-md space-y-4">
          <h1 className="text-xl font-semibold">Your desktop is not signed in to GitHub</h1>
          <p className="text-sm text-[#8b949e]">
            Headstate on your computer could not reach GitHub, so there is nothing for
            this phone to show yet.
          </p>
          {data?.message !== undefined ? (
            <p className="text-sm text-[#8b949e]">{data.message}</p>
          ) : null}
          <p className="text-sm text-[#8b949e]">
            Open Headstate on that computer and follow the instructions it shows, then
            come back — this screen clears on its own once it can sign in.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-screen items-center justify-center bg-[#0d1117] text-[#e6edf3]">
      <div className="max-w-md space-y-4">
        <h1 className="text-xl font-semibold">Headstate needs the GitHub CLI</h1>
        {/* What the app IS, which this screen never said. It explained
            only how to install `gh`, and the one statement of scope
            lived in an empty-list branch most users never see -- so
            anyone WITH pull requests skipped straight past the single
            most important fact about the data they were about to be
            shown. Above the error because it is the reason to fix it. */}
        <p className="text-sm text-[#e6edf3]">
          Headstate watches the pull requests you opened and the ones waiting on
          your review, and tells you when one breaks.
        </p>
        <p className="text-sm text-[#8b949e]">{data?.message}</p>
        <pre className="rounded bg-[#161b22] p-3 text-sm">
          brew install gh{"\n"}gh auth login
        </pre>
        <p className="text-sm text-[#8b949e]">
          Headstate reads your token from <code>gh</code> and keeps it in memory only.
        </p>
      </div>
    </div>
  );
}

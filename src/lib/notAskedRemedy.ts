import { commandError } from "./errorKind";
import { NOT_AUTHENTICATED, SOURCE_NOT_ENABLED } from "./notAsked";

/// What can clear a request Headstate DECLINED to send (#2004).
///
/// - `retry`: the refusal was the moment -- the rate budget, admission,
///   a cooldown or a deadline. Asking again later can work, so "Try
///   again" is offered.
/// - `sign-in`: there is no credential to ask with. A retry issues the
///   same refused request, so it is NOT offered; signing in is.
/// - `none`: the source is not set up to be fetched. Nothing on the panel
///   can change that, so no action is offered.
///
/// #1050's rule, read per reason: never offer a retry that cannot work.
/// Before this, the App's not-asked panels offered "Try again" whatever
/// the reason, and `NotAskedNotice` withheld it whatever the reason; each
/// was wrong for some of the reasons.
export type NotAskedRemedy = "retry" | "sign-in" | "none";

/// The remedy for a not-asked reason. `message` is the reason with the
/// wire marker already stripped -- `commandError`'s `message`.
///
/// Matches the reasons that CANNOT clear on retry, a short list of
/// constants mirrored from Rust; everything else Headstate declines is
/// admission (`github/admission.rs`'s `refused(...)`), which can. A reason
/// added on the Rust side without a remedy here is therefore offered a
/// retry, not stranded without one.
///
/// `includes`, not `startsWith`, for `SOURCE_NOT_ENABLED`: its Rust
/// literal is written `"headstate:not-asked: fetching…"`, so the stripped
/// message keeps a leading colon.
export function notAskedRemedy(message: string): NotAskedRemedy {
  if (message.startsWith(NOT_AUTHENTICATED)) return "sign-in";
  if (message.includes(SOURCE_NOT_ENABLED)) return "none";
  return "retry";
}

/// The sign-in remedy, worded for the build it is read on. On the phone
/// the credential lives on the paired desktop, so that is where to sign in.
export function signInRemedy(mobile: boolean): string {
  return mobile
    ? "On the desktop, sign in with `gh auth login`, then relaunch Headstate there."
    : "Sign in with `gh auth login`, then relaunch Headstate.";
}

/// One source's line in the multi-source banner (#2005): its prose, and
/// -- when Headstate declined the request -- what can clear it.
///
/// `raw` may still carry a wire marker (the authored list and `pollError`
/// hand theirs over unstripped), or may already be stripped with the
/// verdict passed as `notAsked` (`useReviewing` does both). Either way the
/// marker never reaches the screen, and a declined request is never
/// reported as "Could not refresh": that is "they did not answer" for
/// "we did not ask" (#1050).
export function sourceBanner(
  raw: string | undefined,
  notAsked: boolean,
): { message: string; notAsked: NotAskedRemedy | null } {
  const err = raw === undefined ? undefined : commandError(raw);
  if (notAsked || err?.kind === "not-asked") {
    const reason = err?.message ?? "";
    return { message: reason ? `not checked: ${reason}` : "not checked", notAsked: notAskedRemedy(reason) };
  }
  return { message: err?.message || "Could not refresh GitHub", notAsked: null };
}

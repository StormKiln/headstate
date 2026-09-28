import { useState } from "react";
import { Dialog, DialogContent, DialogTitle } from "./ui/dialog";
import { installUpdate } from "../api/updater";
import { ExternalLink } from "./ExternalLink";
import { LATEST_RELEASE_URL } from "../lib/repo";

/// Announces a new release once per version.
///
/// The status bar has always carried an update hint, and it is easy to
/// miss -- a small line at the bottom edge of a window that is often
/// not the one you are looking at. A user running a build with a fixed
/// crash in it should not have to notice a footnote.
///
/// Dismissal is per VERSION, not per launch: nagging about a release
/// someone has already declined is how a notice trains people to
/// dismiss it unread. A user who wants none of it can turn the whole
/// thing off in Settings.
export function UpdateDialog({
  version,
  open,
  onDismiss,
}: {
  version: string;
  open: boolean;
  onDismiss: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [downloaded, setDownloaded] = useState(0);
  const [total, setTotal] = useState<number | null>(null);

  return (
    // Not dismissible while installing: closing mid-download would leave
    // the install running with nothing reporting it.
    <Dialog open={open} onOpenChange={(o) => !o && !busy && onDismiss()}>
      <DialogContent className="max-w-lg">
        <DialogTitle>Headstate {version} is available</DialogTitle>
        <p className="text-sm text-[#8b949e]">
          You are running an older build. Releases carry fixes for problems you
          may be seeing. Installing replaces the app and restarts it.
        </p>
        {error ? (
          // The plugin's OWN message. Its refusals are specific -- a
          // signature mismatch, no bundle for this platform -- and a
          // generic "update failed" would throw away the one part that
          // tells the user what happened.
          <p role="alert" className="mt-3 text-xs text-[#f85149]">
            {error}
          </p>
        ) : null}
        {/* A LIVE REGION, always mounted, which this had none of (#852).

            The error path beside it has `role="alert"` and this had
            nothing -- so the operation that REPLACES AND RESTARTS THE APP
            reported its progress to sighted users only. A bundle is ~20 MB
            and long enough on a slow connection that a dialog with no
            feedback reads as hung; a reader who cannot see it had no
            feedback at all, not even the line saying the install had
            started.

            Always mounted rather than wrapped in `busy`, which is the rule
            `StatusBar` states: "A live region has to exist before the text
            appears or the first announcement is missed -- the one that
            matters most, since it is the one saying work started." Here
            that announcement is the whole point -- the user has just
            pressed Install on something that will close the app under
            them, and "Downloading…" is the confirmation that it took.

            `polite` rather than `assertive`: a percentage must not
            interrupt, and the same choice is made in the status bar for
            the same reason. The error beside it keeps `role="alert"`,
            which is assertive, and that asymmetry is correct -- a refused
            update is news, a progressing one is not. */}
        <p aria-live="polite" className={busy ? "mt-3 text-xs text-[#8b949e]" : "text-xs"}>
          {busy
            ? total
              ? `Downloading — ${Math.round((downloaded / total) * 100)}%`
              : "Downloading…"
            : ""}
        </p>
        {/* `whitespace-nowrap` on the row, not on each button: adding
            a third action to a dialog sized for two squeezed every
            label onto two lines. The wider dialog gives them room and
            this stops any future label wrapping inside its own box. */}
        <div className="mt-4 flex flex-wrap items-center justify-end gap-2 whitespace-nowrap">
          <button
            type="button"
            onClick={onDismiss}
            disabled={busy}
            className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#161b22] disabled:opacity-50"
          >
            Not now
          </button>
          {/* Kept alongside Install: a user who wants to read the notes
              before replacing their app should not have to choose
              between that and updating. */}
          <ExternalLink
            href={LATEST_RELEASE_URL}
            className="rounded border border-[#30363d] px-3 py-1.5 text-sm hover:bg-[#161b22]"
          >
            Release notes
          </ExternalLink>
          <button
            type="button"
            disabled={busy}
            onClick={() => {
              setBusy(true);
              setError(null);
              installUpdate((d, t) => {
                setDownloaded(d);
                setTotal(t);
              }).catch((e: unknown) => {
                setBusy(false);
                setError(e instanceof Error ? e.message : String(e));
              });
            }}
            className="rounded bg-[#238636] px-3 py-1.5 text-sm font-medium text-white hover:bg-[#1a7f37] disabled:opacity-50"
          >
            {busy ? "Installing…" : "Install"}
          </button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

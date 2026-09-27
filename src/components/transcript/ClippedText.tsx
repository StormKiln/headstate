import { useState, type ReactNode } from "react";
import type { TranscriptClip } from "../../types/transcript";
import { palette } from "./palette";
import { isClipped } from "./summary";
import type { BlockAddress, LoadFullText } from "./types";

type Load =
  | { state: "idle" }
  | { state: "loading" }
  | { state: "loaded"; text: string; clip: TranscriptClip | null }
  | { state: "failed"; why: string };

/// A block's text, and -- when the read clipped it -- a sentence saying
/// how much is shown and a button that fetches the rest (#1483).
///
/// The fetch is the injected `onLoadFullText`; without one, the clip is
/// still stated and no button is offered, because a button that cannot
/// do anything is the retry-that-cannot-succeed #1050 is about. The full
/// fetch has a bound of its own, and when that bites too its clip is
/// stated the same way.
export function ClippedText({
  text,
  clip,
  address,
  onLoadFullText,
  children,
}: {
  text: string;
  clip: TranscriptClip | null;
  address: BlockAddress;
  onLoadFullText?: LoadFullText;
  /// Renders the text: monospace output, prose, a thinking block.
  children: (text: string) => ReactNode;
}) {
  const [load, setLoad] = useState<Load>({ state: "idle" });
  const shown = load.state === "loaded" ? load.text : text;
  const shownClip = load.state === "loaded" ? load.clip : clip;

  const fetchAll = () => {
    if (!onLoadFullText) return;
    setLoad({ state: "loading" });
    onLoadFullText(address).then(
      (r) => setLoad({ state: "loaded", text: r.text, clip: r.clip }),
      (e: unknown) => setLoad({ state: "failed", why: e instanceof Error ? e.message : String(e) }),
    );
  };

  return (
    <>
      {children(shown)}
      {isClipped(shownClip) && shownClip ? (
        <p className="mt-0.5 text-[11px]" style={{ color: palette.muted }}>
          Showing the first {shownClip.shown_chars.toLocaleString()} of{" "}
          {shownClip.total_chars.toLocaleString()} characters.
          {onLoadFullText && load.state !== "loaded" ? (
            <>
              {" "}
              <button
                type="button"
                className="underline focus-visible:outline focus-visible:outline-2"
                style={{ color: palette.link }}
                disabled={load.state === "loading"}
                onClick={fetchAll}
              >
                {load.state === "loading"
                  ? "Loading…"
                  : `Show all ${shownClip.total_chars.toLocaleString()} characters`}
              </button>
            </>
          ) : null}
        </p>
      ) : null}
      {load.state === "failed" ? (
        <p role="alert" className="mt-0.5 text-[11px]" style={{ color: palette.error }}>
          The full text could not be loaded: {load.why}
        </p>
      ) : null}
    </>
  );
}

import { useEffect, useState } from "react";

/// The reader's iOS text size (Dynamic Type), as a scale for the phone's
/// transcript (#1481).
///
/// # How it is read
///
/// WebKit resolves the `-apple-system-body` font keyword to the body
/// size the reader chose in Settings > Display & Brightness > Text Size
/// (and Accessibility's larger sizes). 17 px is that size at the
/// default setting, so the scale is the resolved size over 17. Anywhere
/// the keyword is not supported -- every desktop engine but Safari's,
/// and jsdom -- the scale is 1.
///
/// # How it is applied
///
/// As CSS `zoom` on each message's own content (and on a sheet's), not
/// on the scroller: the scroller's measurements stay in its own
/// unzoomed pixels, and `zoom` -- unlike a transform -- takes part in
/// layout, so a larger size WRAPS to the width it has rather than
/// spilling past it. It also reaches the shared tool components, which
/// size their text in pixels for the desktop's density.
///
/// # When it is re-read
///
/// The setting is changed in another app. Coming back fires
/// `visibilitychange`, and that is when the size is re-read.

/// The body size at the default text-size setting.
const DEFAULT_BODY_PX = 17;

/// Bounds on the scale. The top is past Accessibility's largest body
/// size (53 px, about 3.1x); the bottom is under the smallest (14 px).
const MIN_SCALE = 0.75;
const MAX_SCALE = 3.25;

function supportsSystemBody(): boolean {
  return (
    typeof CSS !== "undefined" &&
    typeof CSS.supports === "function" &&
    CSS.supports("font", "-apple-system-body")
  );
}

/// The scale now: 1 where Dynamic Type cannot be read.
export function readTextScale(): number {
  if (typeof document === "undefined" || !supportsSystemBody()) return 1;
  const probe = document.createElement("span");
  probe.style.font = "-apple-system-body";
  probe.style.position = "absolute";
  probe.style.visibility = "hidden";
  document.body.appendChild(probe);
  const px = parseFloat(getComputedStyle(probe).fontSize);
  probe.remove();
  if (!Number.isFinite(px) || px <= 0) return 1;
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, px / DEFAULT_BODY_PX));
}

/// The scale, re-read whenever the app comes back to the foreground.
export function useTextScale(): number {
  const [scale, setScale] = useState(readTextScale);
  useEffect(() => {
    const reread = () => {
      if (document.visibilityState === "visible") setScale(readTextScale());
    };
    document.addEventListener("visibilitychange", reread);
    return () => document.removeEventListener("visibilitychange", reread);
  }, []);
  return scale;
}

/// The style that applies a scale: nothing at all at 1, so the default
/// size adds no property to every row.
export function scaleStyle(scale: number): { zoom: number } | undefined {
  return scale === 1 ? undefined : { zoom: scale };
}

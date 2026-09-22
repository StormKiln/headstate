import { describe, expect, it } from "vitest";
import { cn } from "@/lib/utils";

/// The side margin every dialog relies on, and the merge behaviour that
/// used to silently delete it.
///
/// `DialogContent`'s base classes carried the margin as
/// `max-w-[calc(100%-2rem)]`. `cn` is `twMerge`, which treats `max-w-*`
/// as ONE conflict key -- so a caller passing `max-w-lg` did not narrow
/// the dialog within the cap, it removed the cap. All 18 call sites pass
/// one, so every dialog in the app lost it, and at 390px each rendered
/// exactly 390px wide: edge to edge, corners clipped by the screen, and
/// no backdrop strip left at the sides to tap.
///
/// This asserts the property rather than the string: the fix is that the
/// margin lives on a key the callers do not set, and a future refactor
/// that moves it back onto `max-w-*` should fail here rather than in
/// someone's hands.

/// The width classes `DialogContent` actually ships, kept in step with
/// `dialog.tsx` by the last test below.
const BASE = "w-[calc(100%-2rem)] sm:max-w-sm";

/// Every override passed by a real call site.
const CALLER_WIDTHS = ["max-w-lg", "max-w-2xl", "max-w-3xl sm:max-w-3xl"];

describe("the dialog's side margin", () => {
  it("survives every width a caller passes", () => {
    for (const caller of CALLER_WIDTHS) {
      expect(cn(BASE, caller)).toContain("w-[calc(100%-2rem)]");
    }
  });

  it("would NOT have survived on max-w, which is the bug", () => {
    // The shape of the old base. Kept as a test so the reason for the
    // `w-*` spelling is recorded as behaviour, not just a comment.
    const old = "w-full max-w-[calc(100%-2rem)] sm:max-w-sm";
    expect(cn(old, "max-w-lg")).not.toContain("max-w-[calc(100%-2rem)]");
  });

  it("still lets a caller set the dialog's maximum width", () => {
    // The margin must not have been fixed by pinning the width: a
    // desktop dialog that asked for `max-w-3xl` must still get it.
    expect(cn(BASE, "max-w-3xl")).toContain("max-w-3xl");
  });

  it("matches the classes dialog.tsx actually ships", async () => {
    // Guards the fixture above: a `BASE` that had drifted from the
    // component would make every assertion here meaningless.
    const source = await import("./dialog.tsx?raw");
    for (const cls of BASE.split(" ")) {
      expect(source.default).toContain(cls);
    }
  });
});

/// The safe-area budget survives the `max-h` a caller might pass, and
/// the sheet's padding survives `p-0`.
///
/// Both are tailwind-merge questions, and both were nearly wrong (#648):
/// a sheet's inset padding written as plain `pt-*` WOULD be deleted by
/// the `p-0` that `App.tsx` passes to the navigation sheet. It survives
/// only because it is variant-prefixed (`data-[side=left]:pt-*`), which
/// merges under a different key. That is subtle enough to deserve a
/// test rather than a comment.
describe("safe-area insets survive the classes callers pass", () => {
  it("keeps a variant-prefixed inset padding through p-0", () => {
    const out = cn("data-[side=left]:pt-[env(safe-area-inset-top)]", "p-0");
    expect(out).toContain("data-[side=left]:pt-[env(safe-area-inset-top)]");
    expect(out).toContain("p-0");
  });

  it("shows why a PLAIN padding would not have survived", () => {
    // The mistake this guards against: same intent, wrong key.
    expect(cn("pt-[env(safe-area-inset-top)]", "p-0")).toBe("p-0");
  });

  it("keeps the dialog's inset-aware height unless a caller sets max-h", () => {
    const H = "max-h-[calc(100dvh-2rem-env(safe-area-inset-top)-env(safe-area-inset-bottom))]";
    expect(cn(H, "w-full")).toContain("env(safe-area-inset-top)");
    // A caller that sets its own max-h still wins, which is intended --
    // but it then owns the inset budget too.
    expect(cn(H, "max-h-96")).toBe("max-h-96");
  });
});

/// The OTHER half of the same tailwind-merge trap, found in a browser
/// while fixing #1303.
///
/// The base carries `sm:max-w-sm`, and `twMerge` keys `max-w-*` and
/// `sm:max-w-*` SEPARATELY -- so a caller passing a bare `max-w-2xl`
/// does not replace the base's cap, it sits beside it, and above the
/// `sm` breakpoint the media-query rule wins. Measured in Chrome at
/// 1280px: `sm:max-w-sm max-w-2xl` computes to 384px, not 672px.
///
/// Which means every dialog in the app that passes only the bare form is
/// 384px wide on the desktop, whatever width it asked for. That is a
/// pre-existing bug wider than #1303 and is NOT fixed here -- it is
/// filed as #1306. This records the mechanism, and pins the spelling the
/// one dialog that needs its width to apply actually uses.
describe("a caller's width only applies if it is spelt for the breakpoint", () => {
  it("keeps the base's sm cap beside a bare max-w, which is the trap", () => {
    const out = cn(BASE, "max-w-2xl");
    // BOTH survive, and at >=640px the sm one is the one that renders.
    expect(out).toContain("sm:max-w-sm");
    expect(out).toContain("max-w-2xl");
  });

  it("replaces the base's sm cap when the caller prefixes it too", () => {
    const out = cn(BASE, "max-w-2xl sm:max-w-2xl");
    // This is the spelling that actually widens the dialog.
    expect(out).not.toContain("sm:max-w-sm");
    expect(out).toContain("sm:max-w-2xl");
    // The unprefixed half stays, for viewports below `sm`.
    expect(out).toContain("max-w-2xl");
  });

  it("is the spelling the Claudify launch dialog ships", async () => {
    // #1303's dialog is the one that renders a built argv, so it is the
    // one whose width had to actually take effect. Pinned against the
    // source so a revert to the bare form fails here.
    const source = await import("../WorktreesPage.tsx?raw");
    expect(source.default).toContain('className="max-w-2xl sm:max-w-2xl"');
  });
});

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
const BASE = "w-[calc(100%-2rem)] max-w-sm";

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
    //
    // Matched as WHOLE CLASS NAMES, not as substrings. `toContain` was
    // the original spelling and it is too weak for exactly the class
    // this file is now about: `max-w-sm` is a substring of
    // `sm:max-w-sm`, so a base that regressed to the prefixed form
    // still satisfied a `toContain("max-w-sm")` and the drift guard
    // passed while the fixture described a component that no longer
    // existed. Verified by reverting `dialog.tsx` and watching this
    // test stay green.
    const source = await import("./dialog.tsx?raw");
    const shipped = new Set(
      (source.default.match(/"fixed top-1\/2[^"]*"/)?.[0] ?? "")
        .replace(/"/g, "")
        .split(/\s+/),
    );
    for (const cls of BASE.split(" ")) {
      expect(shipped).toContain(cls);
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

/// The OTHER half of the same tailwind-merge trap (#1306), and the
/// reason the base's cap is spelt `max-w-sm` rather than `sm:max-w-sm`.
///
/// The base USED to carry `sm:max-w-sm`. `twMerge` keys `max-w-*` and
/// `sm:max-w-*` SEPARATELY, so a caller's bare `max-w-2xl` did not
/// replace that cap -- it sat beside it, and above the `sm` breakpoint
/// the media-query rule won on specificity. Every one of the ~25 call
/// sites passed the bare form, so every dialog in the app rendered at
/// 384px whatever width it asked for. Measured in Chrome at a 1280px
/// viewport, through the real component: `max-w-lg`, `max-w-2xl` and
/// `max-w-md` all computed to 384px.
///
/// Two call sites had already worked around it by spelling the width
/// twice (`max-w-3xl sm:max-w-3xl`), which is the evidence that a
/// workaround does not scale: the trap stayed armed for everyone who
/// had not yet been bitten, and the next author had no way to know.
///
/// The fix is to keep the default cap on the SAME conflict key the
/// callers use. Then a caller's plain `max-w-2xl` simply replaces it,
/// and there is no breakpoint-keyed cap left to silently beat a third
/// caller. The `sm:` prefix bought nothing anyway: below 640px the
/// `w-[calc(100%-2rem)]` margin is already narrower than 24rem on any
/// phone, so the cap never applied there (measured: 358px at a 390px
/// viewport, with the cap inactive).
describe("a caller's width applies without having to be spelt twice", () => {
  /// The property the fix exists to provide, asserted for every width a
  /// real call site passes. This is the test that fails if the base's
  /// cap ever moves back onto a `sm:`-prefixed key.
  it("lets a bare max-w from a caller replace the base's cap", () => {
    for (const width of ["max-w-md", "max-w-lg", "max-w-2xl", "max-w-3xl"]) {
      const out = cn(BASE, width);
      expect(out).toContain(width);
      // The cap it replaced is GONE, not sitting beside it waiting to
      // win at >=640px. `toContain` on the whole string would be
      // satisfied by `max-w-sm` inside `sm:max-w-sm`, so match a word.
      expect(out).not.toMatch(/(^|\s|:)max-w-sm(\s|$)/);
    }
  });

  /// The side margin is the other thing on the same element, and the
  /// earlier bug in this file was the margin being deleted by these
  /// exact class names. Both properties have to hold at once.
  it("keeps the side margin while the caller's width applies", () => {
    const out = cn(BASE, "max-w-2xl");
    expect(out).toContain("w-[calc(100%-2rem)]");
    expect(out).toContain("max-w-2xl");
  });

  /// `UpdateWizard` is the one dialog that sizes itself with `w-*` and
  /// passes NO `max-w-*`, so nothing at its call site overrode the
  /// default cap and it was held at 384px while asking for 46rem.
  /// #1306 reported it as unaffected for that reason. It needs
  /// `max-w-none` explicitly, and that is easy to drop in a later edit.
  it("still caps a caller that sets only a width, unless it opts out", () => {
    const sized = "max-h-[80vh] w-[min(46rem,92vw)] overflow-y-auto";
    expect(cn(BASE, sized)).toMatch(/(^|\s)max-w-sm(\s|$)/);
    expect(cn(BASE, `${sized} max-w-none`)).toContain("max-w-none");
  });

  /// And the opt-out is actually THERE, pinned against the source.
  ///
  /// The assertion above is about `cn`, so it would pass just as
  /// happily if `UpdateWizard` never opted out at all -- which is the
  /// state #1306 found it in, and which sabotaging the call site
  /// confirmed the `cn` test alone does not catch. Measured in Chrome
  /// at 1280px: 384px without `max-w-none`, 736px with it.
  it("opts UpdateWizard out, because it sets no max-w of its own", async () => {
    const source = await import("../UpdateWizard.tsx?raw");
    const tag = source.default.match(/<DialogContent[\s\S]*?>/)?.[0] ?? "";
    expect(tag).toContain("w-[min(46rem,92vw)]");
    expect(tag).toContain("max-w-none");
  });
});

/// The guard: no call site may carry a breakpoint-prefixed `max-w-*`.
///
/// The three tests above are assertions about `cn`, which is a thin
/// `twMerge` wrapper -- they pin the MECHANISM, and the first of them
/// does fail if `dialog.tsx`'s base regresses (via the `BASE` fixture
/// and the drift test that pins it to the source). But none of them can
/// see a NEW call site, and a new call site is how this bug reached ~25
/// dialogs in the first place.
///
/// So this scans every component's source the way `emptyStateGuard`
/// does, for the same stated reason: the property is about every call
/// site rather than about one, and a list of imports is exactly the
/// enumeration that lets the next one be forgotten.
///
/// What it cannot see, stated rather than glossed:
/// - A width built at runtime (a variable, a template literal). Every
///   current call site passes a literal or a ternary of literals; a
///   computed one would be invisible here.
/// - Whether the width CHOSEN is the right one for the content. That is
///   a judgement, and the browser is where it was made.
const componentSources = import.meta.glob("../**/*.tsx", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

describe("no call site re-arms the breakpoint trap", () => {
  /// A `sm:`/`md:`/`lg:` prefixed `max-w-*` anywhere near a
  /// `DialogContent` is the broken spelling: it means the author is
  /// either working around a cap that no longer exists, or has
  /// reintroduced one. Either way the next reader learns the wrong rule.
  it("passes no breakpoint-prefixed max-w to a DialogContent", () => {
    const offenders: string[] = [];
    for (const [path, source] of Object.entries(componentSources)) {
      if (path.includes(".test.")) continue;
      if (!source.includes("<DialogContent")) continue;
      // Every `<DialogContent ...>` opening tag, attributes included.
      for (const tag of source.match(/<DialogContent[\s\S]*?>/g) ?? []) {
        for (const hit of tag.match(/\b[a-z]+:max-w-[\w[\]().,%-]+/g) ?? []) {
          offenders.push(`${path}: ${hit}`);
        }
      }
    }
    // `DialogContent`'s cap is unprefixed, so a prefixed width from a
    // caller does not replace it -- it wins above the breakpoint and
    // loses below, which is never what anyone means.
    expect(offenders).toEqual([]);
  });

  /// Guards the guard. A scan that silently stopped finding call sites
  /// would report zero offenders forever, which is the failure mode of
  /// every source scan and the reason `emptyStateGuard` self-checks too.
  it("is actually looking at the app's dialogs", () => {
    const withDialogs = Object.entries(componentSources).filter(
      ([path, source]) =>
        !path.includes(".test.") && source.includes("<DialogContent"),
    );
    expect(withDialogs.length).toBeGreaterThanOrEqual(12);
    const paths = withDialogs.map(([p]) => p).join("\n");
    for (const known of ["WorktreesPage", "SettingsDialog", "UpdateWizard"]) {
      expect(paths).toContain(known);
    }
  });

  /// The base itself, read from the source rather than from the `BASE`
  /// fixture, so a regression in `dialog.tsx` fails here even if someone
  /// updates the fixture to match it.
  it("keeps the default cap off a breakpoint key in dialog.tsx", async () => {
    const source = await import("./dialog.tsx?raw");
    const tag = source.default.match(/data-slot="dialog-content"[\s\S]*?\bclassName=\{cn\([\s\S]*?\n\s*\)/)?.[0];
    expect(tag).toBeTruthy();
    // The class list lives on the one long string literal in that call.
    const classes = tag!.match(/"fixed top-1\/2[^"]*"/)?.[0] ?? "";
    expect(classes).toMatch(/(^|\s)max-w-sm(\s|")/);
    expect(classes).not.toContain("sm:max-w-");
  });
});

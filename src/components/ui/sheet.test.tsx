import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { HelpButton } from "@/components/HelpButton";
import { Sheet, SheetContent, SheetTitle } from "./sheet";

afterEach(cleanup);

/// #1314: a caller's size REPLACES the sheet's default instead of
/// sitting beside it and losing.
///
/// `SheetContent`'s base used to carry its size keyed by side --
/// `data-[side=right]:w-3/4`, `data-[side=right]:sm:max-w-sm`,
/// `data-[side=bottom]:h-auto`. `cn` is `twMerge`, which keys those
/// SEPARATELY from a caller's bare `w-full`/`max-w-md`/`h-[90dvh]`, so
/// both survived the merge and the attribute-selector one won in the
/// browser on specificity. Measured in Chrome at 1280px before the fix:
/// `HelpButton` asked for 448px and rendered 384px.
///
/// These read the className of the REAL rendered popup -- the actual
/// `cn` output of `SheetContent` for a given caller -- not a fixture of
/// the base string, so they fail if `sheet.tsx` regresses. jsdom does
/// no layout, so none of this proves a width RENDERS; that was measured
/// in a browser and is recorded on the PR.

/// Class names, split, so `max-w-sm` is never matched inside
/// `sm:max-w-sm` or `data-[side=right]:sm:max-w-sm`.
const classesOf = (el: Element) => new Set((el.getAttribute("class") ?? "").split(/\s+/));

/// Any size class still keyed by a variant -- the shape of the trap.
/// A caller's bare class cannot replace one of these in `twMerge`.
const VARIANT_SIZE = /(^|\s)[^\s]*:(max-w|w|h)-[^\s]+/g;

function popup(side: "left" | "right" | "top" | "bottom" | undefined, className: string) {
  render(
    <Sheet open>
      <SheetContent side={side} className={className}>
        <SheetTitle>t</SheetTitle>
      </SheetContent>
    </Sheet>,
  );
  const el = document.querySelector('[data-slot="sheet-content"]');
  expect(el).toBeTruthy();
  return el!;
}

describe("a caller's sheet size applies (#1314)", () => {
  it("lets HelpButton's width replace the right sheet's default cap", async () => {
    // Through the real call site, so a HelpButton that drifted back to a
    // breakpoint-keyed width fails here too, not only the scan below.
    render(<HelpButton topic="needs-attention" />);
    fireEvent.click(screen.getByRole("button"));
    await waitFor(() => expect(document.querySelector('[data-slot="sheet-content"]')).toBeTruthy());
    const el = document.querySelector('[data-slot="sheet-content"]')!;
    const cls = classesOf(el);
    expect(cls).toContain("max-w-md");
    expect(cls).toContain("w-full");
    // The default cap and width it replaced are GONE, not waiting to win.
    expect(cls).not.toContain("max-w-sm");
    expect(cls).not.toContain("w-3/4");
    expect(el.getAttribute("class")!.match(VARIANT_SIZE)).toBeNull();
  });

  it("keeps the default size for a caller that sets none", () => {
    for (const side of ["left", "right"] as const) {
      const cls = classesOf(popup(side, ""));
      expect(cls).toContain("w-3/4");
      expect(cls).toContain("max-w-sm");
      expect(cls).toContain("h-full");
      cleanup();
    }
    for (const side of ["top", "bottom"] as const) {
      const cls = classesOf(popup(side, ""));
      expect(cls).toContain("h-auto");
      // A top/bottom sheet spans the width: no side sheet's cap on it.
      expect(cls).not.toContain("max-w-sm");
      expect(cls).not.toContain("w-3/4");
      cleanup();
    }
  });

  it("lets a left sheet's w-72 replace the default width", () => {
    // `App`'s navigation sheet. Measured before the fix: 292.5px (3/4 of
    // a 390px viewport) instead of the 288px it asked for.
    const el = popup("left", "w-72 p-0");
    const cls = classesOf(el);
    expect(cls).toContain("w-72");
    expect(cls).not.toContain("w-3/4");
    expect(el.getAttribute("class")!.match(VARIANT_SIZE)).toBeNull();
  });

  it("lets a bottom sheet's height replace h-auto", () => {
    // The phone subagent sheet (`PhoneTranscript`). Measured before the
    // fix: its content height (93px for a one-line body) instead of 90dvh.
    const el = popup("bottom", "flex h-[90dvh] flex-col");
    const cls = classesOf(el);
    expect(cls).toContain("h-[90dvh]");
    expect(cls).not.toContain("h-auto");
    // Nor a side-keyed `h-auto` beside it, which would win in the browser.
    expect(el.getAttribute("class")!.match(VARIANT_SIZE)).toBeNull();
  });

  it("keeps the safe-area padding through a caller's p-0 (#648)", () => {
    // The ONE place a side-keyed class is deliberate: it must survive.
    const cls = classesOf(popup("left", "p-0"));
    expect(cls).toContain("p-0");
    expect(cls).toContain("data-[side=left]:pt-[env(safe-area-inset-top)]");
  });
});

/// Pinned against the source as well, so a size re-keyed by side in
/// `sheet.tsx` fails even for a side the render tests above do not use.
describe("sheet.tsx keeps no size on a side-keyed variant", () => {
  it("has no data-[side=*] w/h/max-w class", async () => {
    const source = (await import("./sheet.tsx?raw")).default;
    // Only the class STRINGS, not the doc comment that quotes the old
    // spelling to explain it.
    const code = source
      .split("\n")
      .filter((l) => !l.trim().startsWith("//"))
      .join("\n");
    expect(code).toContain('data-slot="sheet-content"');
    expect(code.match(/data-\[side=\w+\]:(?:[a-z]+:)*(?:max-w|w|h)-[^\s"]+/g)).toBeNull();
  });
});

/// The guard for the NEXT call site, in the shape `dialog.test.ts` uses
/// for `DialogContent` (#1306): the base's cap is bare `max-w-sm`, so a
/// breakpoint-prefixed width from a caller is a separate twMerge key
/// that does not replace it.
///
/// What it cannot see, stated rather than glossed: a width built at
/// runtime (a variable, a template literal). Every current call site
/// passes a literal.
const componentSources = import.meta.glob("../../**/*.tsx", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

describe("no SheetContent call site re-arms the breakpoint trap", () => {
  it("passes no variant-prefixed size to a SheetContent", () => {
    const offenders: string[] = [];
    for (const [path, source] of Object.entries(componentSources)) {
      if (path.includes(".test.") || path.endsWith("ui/sheet.tsx")) continue;
      for (const tag of source.match(/<SheetContent[\s\S]*?>/g) ?? []) {
        for (const hit of tag.match(/[^\s"]*:(?:max-w|w|h)-[^\s"]+/g) ?? []) {
          // `[&>nav]:w-full` targets a CHILD, not the sheet: not a size
          // of this element, so not the trap.
          if (hit.startsWith("[&")) continue;
          offenders.push(`${path}: ${hit}`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });

  /// Guards the guard: a scan that stopped finding call sites would
  /// report zero offenders forever.
  it("is actually looking at the app's sheets", () => {
    const paths = Object.entries(componentSources)
      .filter(([p, s]) => !p.includes(".test.") && !p.endsWith("ui/sheet.tsx") && s.includes("<SheetContent"))
      .map(([p]) => p);
    expect(paths.length).toBeGreaterThanOrEqual(5);
    for (const known of ["HelpButton", "FilterBar", "App.tsx", "PhoneTranscript"]) {
      expect(paths.join("\n")).toContain(known);
    }
  });
});

import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { useRef } from "react";
import { APP_HEADER_HEIGHT_VAR, useStickyHeaderOffset } from "./useStickyHeaderOffset";

/// jsdom reports every box as 0x0, so the height has to be stubbed to
/// assert that a REAL one is read and written through.
function Harness({ height }: { height: number | null }) {
  const scroll = useRef<HTMLElement>(null);
  const header = useRef<HTMLElement>(null);
  useStickyHeaderOffset(scroll, header);
  return (
    <main ref={scroll} data-testid="scroll">
      <header
        ref={(el) => {
          if (el && height !== null) {
            el.getBoundingClientRect = () => ({ height }) as DOMRect;
          }
          header.current = el;
        }}
      >
        app header
      </header>
    </main>
  );
}

describe("useStickyHeaderOffset", () => {
  afterEach(cleanup);

  /// The point of #1278: the offset the other stickies read has to be
  /// the app header's ACTUAL height, or they pin into it or below it.
  it("publishes the header's measured height onto the scroll container", () => {
    const { getByTestId } = render(<Harness height={43.5} />);
    const main = getByTestId("scroll");
    expect(main.style.getPropertyValue(APP_HEADER_HEIGHT_VAR)).toBe("43.5px");
  });

  /// The fractional part is load-bearing: rounding 43.5 down to 43
  /// leaves half a pixel of the pinned bar showing above the header,
  /// which is why this reads `getBoundingClientRect` and not
  /// `offsetHeight`.
  it("keeps a fractional height rather than rounding it", () => {
    const { getByTestId } = render(<Harness height={43.5} />);
    expect(getByTestId("scroll").style.getPropertyValue(APP_HEADER_HEIGHT_VAR)).not.toBe("43px");
  });

  /// The phone header is taller than the desktop's, which is the reason
  /// the value is measured instead of being a constant.
  it("publishes whatever height the header actually has", () => {
    const { getByTestId } = render(<Harness height={60} />);
    expect(getByTestId("scroll").style.getPropertyValue(APP_HEADER_HEIGHT_VAR)).toBe("60px");
  });
});

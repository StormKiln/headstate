import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { MaskedText } from "./MaskedText";

describe("MaskedText", () => {
  it("renders unmasked text as itself", () => {
    const { container } = render(<MaskedText text="plain words" />);
    expect(container.textContent).toBe("plain words");
    expect(screen.queryByText("hidden")).toBeNull();
  });

  it("draws a hidden pill where the desktop masked a span, and never the marker", () => {
    const { container } = render(<MaskedText text="API_KEY=⟦hidden:secret⟧ done" />);
    const pill = screen.getByTitle("Hidden on this phone: a secret value");
    expect(pill.textContent).toBe("hidden a secret value");
    // What a screen reader reads: the kind, as text in the pill.
    expect(container.textContent).toBe("API_KEY=hidden a secret value done");
    expect(container.textContent).not.toContain("⟦");
  });

  /// #1489: an `aria-label` on a plain span is prohibited by ARIA, so the
  /// kind is text -- seen by the reader only in the title.
  it("speaks the kind as text, not as a label on a span", () => {
    render(<MaskedText text="⟦hidden:secret⟧" />);
    const pill = screen.getByTitle("Hidden on this phone: a secret value");
    expect(pill.hasAttribute("aria-label")).toBe(false);
    expect(pill.querySelector(".sr-only")?.textContent).toBe(" a secret value");
  });
});

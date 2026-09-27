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
    const pill = screen.getByText("hidden");
    expect(pill.getAttribute("title")).toBe("Hidden on this phone: a secret value");
    expect(container.textContent).toBe("API_KEY=hidden done");
    expect(container.textContent).not.toContain("⟦");
  });
});

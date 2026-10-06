import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { PositionScrubber } from "./PositionScrubber";
afterEach(cleanup);
it("commits once at pointer release, keyboard release, or assistive change", () => {
  const seek = vi.fn(); render(<PositionScrubber seek={seek} />);
  const input = screen.getByRole("slider", { name: "Transcript position" });
  input.focus();
  fireEvent.pointerDown(input);
  fireEvent.change(input, { target: { value: "40" } });
  fireEvent.change(input, { target: { value: "45" } });
  expect(seek).not.toHaveBeenCalled();
  fireEvent.pointerUp(input); fireEvent.blur(input);
  expect(seek.mock.calls).toEqual([[45]]);
  fireEvent.keyDown(input, { key: "ArrowRight" });
  fireEvent.change(input, { target: { value: "46" } });
  expect(seek).toHaveBeenCalledTimes(1);
  fireEvent.keyUp(input, { key: "ArrowRight" });
  fireEvent.change(input, { target: { value: "70" } });
  expect(seek.mock.calls).toEqual([[45], [46], [70]]);
  expect(input.getAttribute("aria-valuetext")).toBe("About 70% through transcript");
});
it("cancelled pointer gesture does not navigate", () => {
  const seek = vi.fn(); render(<PositionScrubber seek={seek} />);
  const input = screen.getByRole("slider"); fireEvent.pointerDown(input);
  fireEvent.change(input, { target: { value: "30" } }); fireEvent.pointerCancel(input);
  fireEvent.blur(input); expect(seek).not.toHaveBeenCalled();
});

import { act, cleanup, render } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import { ShowControls } from "./navigation";
import { SHOW_ALL } from "./filters";

afterEach(cleanup);

it("does not reset unchanged filter checkboxes on an idle host update", async () => {
  const show = SHOW_ALL;
  const { container, rerender } = render(<ShowControls show={show} hidden={0} />);
  const mutations: MutationRecord[] = [];
  const observer = new MutationObserver((records) => mutations.push(...records));
  observer.observe(container, { attributes: true, subtree: true });
  await act(async () => rerender(<ShowControls show={show} hidden={0} />));
  observer.disconnect();
  expect(mutations).toHaveLength(0);
});

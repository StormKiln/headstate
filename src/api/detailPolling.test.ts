import { expect, it } from "vitest";
import { DetailPollBackoff } from "./detailPolling";
it("backs off distinct unknown/error receipts and caps the rate", () => {
  const polling = new DetailPollBackoff();
  expect(polling.delay("head", true, 1)).toBe(3000);
  expect(polling.delay("head", true, 1)).toBe(3000);
  expect(polling.delay("head", true, 2)).toBe(6000);
  expect(polling.delay("head", true, 3)).toBe(12000);
  for (let n=4; n<=20; n++) polling.delay("head", true, n);
  expect(polling.delay("head", true, 21)).toBe(60000);
});
it("resets on a new head or a known result", () => {
  const polling = new DetailPollBackoff();
  polling.delay("old", true, 1); polling.delay("old", true, 2);
  expect(polling.delay("new", true, 3)).toBe(3000);
  expect(polling.delay("new", false, 4)).toBe(false);
  expect(polling.delay("new", true, 5)).toBe(3000);
});

import { afterEach, expect, it } from "vitest";
import { markNewestText } from "./elementTiming";
afterEach(() => { document.body.innerHTML = ""; });
it("times actual text in the newest row rather than its wrapper or copy button", () => {
  document.body.innerHTML = '<div data-newest-message="true"><button>Copy</button><div><span aria-hidden="true">icon</span><p>Newest content</p></div></div>';
  markNewestText(document.body);
  const target = document.querySelector('[elementtiming="newest"]');
  expect(target?.tagName).toBe("P");
  expect(target?.firstChild?.nodeType).toBe(Node.TEXT_NODE);
  expect(target?.textContent).toBe("Newest content");
});

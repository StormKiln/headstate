/// Chromium's Element Timing requires directly contained text. Tag the
/// newest row's first content text before paint, skipping UI affordances.
export function markNewestText(root: ParentNode): void {
  const row = root.querySelector('[data-newest-message="true"]');
  if (!row || row.querySelector('[elementtiming="newest"]')) return;
  const walker = document.createTreeWalker(row, NodeFilter.SHOW_TEXT);
  let node: Node | null;
  while ((node = walker.nextNode())) {
    if (!node.textContent?.trim()) continue;
    const element = node.parentElement;
    if (!element || element.closest("button, [aria-hidden=true]")) continue;
    element.setAttribute("elementtiming", "newest");
    break;
  }
}

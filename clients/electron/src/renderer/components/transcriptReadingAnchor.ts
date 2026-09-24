/** Find the first visible character in a row that crosses the viewport top. */
export function readingTextAnchor(row: Element, viewportTop: number): Range | undefined {
  const walker = document.createTreeWalker(row, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const length = node.textContent?.length ?? 0;
    if (!length || !node.textContent?.trim()) continue;
    const range = document.createRange();
    range.selectNodeContents(node);
    const bounds = range.getBoundingClientRect();
    if (!bounds.height || bounds.bottom <= viewportTop || bounds.top > viewportTop) continue;
    // Text can span hundreds of lines. Locate the visible line without
    // measuring every character in a long assistant response.
    let low = 0;
    let high = length;
    while (low < high) {
      const middle = (low + high) >> 1;
      range.setStart(node, middle);
      range.setEnd(node, middle + 1);
      if (range.getBoundingClientRect().bottom > viewportTop) high = middle;
      else low = middle + 1;
    }
    if (low === length) continue;
    range.setStart(node, low);
    range.setEnd(node, low + 1);
    return range;
  }
  return undefined;
}

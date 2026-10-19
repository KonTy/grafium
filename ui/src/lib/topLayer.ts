/**
 * The browser's top layer.
 *
 * A modal `<dialog>` and an open popover are drawn above everything else on
 * the page whatever its z-index, and a modal dialog also makes everything
 * outside it inert. Something that must be seen and clicked while one is open
 * has to live inside it.
 */

function matches(element: Element, selector: string): boolean | null {
  try {
    return element.matches(selector);
  } catch {
    // A webview too old to know the selector.
    return null;
  }
}

function isTopLayerElement(element: Element): boolean {
  if (element.localName === "dialog") {
    // Without `:modal`, an open dialog is treated as modal: wrongly putting a menu inside a plain one is harmless.
    return matches(element, ":modal") ?? element.hasAttribute("open");
  }
  return element.hasAttribute("popover") && (matches(element, ":popover-open") ?? false);
}

/**
 * The innermost open modal dialog or open popover that is `start` or holds it,
 * or null when it sits in the ordinary page.
 */
export function enclosingTopLayer(start: Element | null | undefined): HTMLElement | null {
  for (let node = start ?? null; node; node = node.parentElement) {
    if (isTopLayerElement(node)) return node as HTMLElement;
  }
  return null;
}

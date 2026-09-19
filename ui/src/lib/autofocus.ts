/**
 * Focus an element as soon as it is mounted.
 *
 * Preferred over the bare `autofocus` attribute: the attribute is only
 * honoured reliably during initial document load, so it is a poor fit for
 * elements that appear later (dialogs, command palettes), and it moves focus
 * with no accompanying intent for assistive technology. Doing it here keeps
 * the focus move explicit, scoped to the element's own lifetime, and undone
 * automatically when the element goes away.
 */
export function autofocus(node: HTMLElement) {
  const previous = document.activeElement as HTMLElement | null;
  node.focus();
  return {
    destroy() {
      if (!previous || !previous.isConnected) return;
      // Teardown order is not guaranteed: the node may still be focused, focus
      // may have moved to a child (arrow keys inside a menu), or the browser
      // may already have dropped it to <body> because the focused element was
      // detached. All three mean "this element still owned the focus", and
      // only then should it be handed back. Checking `activeElement === node`
      // alone silently skipped the common case and left focus on <body>.
      const active = node.ownerDocument.activeElement;
      const ownedFocus =
        active === node ||
        node.contains(active) ||
        active === null ||
        active === node.ownerDocument.body;
      if (ownedFocus) previous.focus();
    },
  };
}

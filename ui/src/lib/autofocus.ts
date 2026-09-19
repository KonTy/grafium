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
      if (previous && previous.isConnected && document.activeElement === node) {
        previous.focus();
      }
    },
  };
}

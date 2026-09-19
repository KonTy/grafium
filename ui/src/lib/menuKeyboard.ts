/**
 * Keyboard behaviour for the app's popup menus (context menus, the image size
 * menu).
 *
 * These menus are rendered as `role="menu"` containers that take focus when
 * they open. Without this they were pointer-only: the markup announced a menu
 * but Escape/arrow keys did nothing once focus was inside it.
 */

const ITEM_SELECTOR = '[role="menuitem"]:not([disabled]):not([aria-disabled="true"])';

/** Focusable items of a menu, in DOM order. */
export function menuItems(menu: HTMLElement): HTMLElement[] {
  return Array.from(menu.querySelectorAll<HTMLElement>(ITEM_SELECTOR));
}

function nextIndex(key: string, current: number, count: number): number {
  switch (key) {
    case "Home":
      return 0;
    case "End":
      return count - 1;
    case "ArrowDown":
      return current < 0 ? 0 : (current + 1) % count;
    default:
      return current <= 0 ? count - 1 : current - 1;
  }
}

/**
 * Handle a keydown on a menu container. Escape closes it; Arrow/Home/End move
 * focus between items and wrap around. Anything else is left alone so typing
 * still reaches the app once the menu closes.
 */
export function handleMenuKeydown(event: KeyboardEvent, close: () => void): void {
  const menu = event.currentTarget as HTMLElement | null;
  if (!menu) return;

  if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    close();
    return;
  }

  if (event.key !== "ArrowDown" && event.key !== "ArrowUp" && event.key !== "Home" && event.key !== "End") {
    return;
  }

  const items = menuItems(menu);
  if (items.length === 0) return;
  event.preventDefault();
  event.stopPropagation();
  const current = items.indexOf(menu.ownerDocument.activeElement as HTMLElement);
  items[nextIndex(event.key, current, items.length)]?.focus();
}

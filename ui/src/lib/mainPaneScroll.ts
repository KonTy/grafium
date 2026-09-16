export const MAIN_PANE_SCROLL_INTENT = "grafium-main-pane-scroll";

const PAGED_VIEWS = new Set(["page", "journal", "all-pages", "statistics"]);
const KEYBOARD_OVERLAYS = [
  "[role='dialog']", "[role='alertdialog']", "[role='menu']", "dialog[open]",
  ".dialog-backdrop", ".page-dialog-backdrop", ".folder-browser-backdrop",
  ".date-picker-backdrop", ".menu-backdrop", ".app-context-menu",
  ".cm-tooltip-autocomplete",
].join(", ");

export function hasKeyboardOverlay(document: Document): boolean {
  return Array.from(document.querySelectorAll<HTMLElement>(KEYBOARD_OVERLAYS))
    .some((element) => element.getClientRects().length > 0
      && getComputedStyle(element).visibility !== "hidden");
}

export function handleMainPanePageKey(
  event: KeyboardEvent,
  main: HTMLElement | null,
  view: string,
): boolean {
  if (!main || !PAGED_VIEWS.has(view)
    || (event.key !== "PageDown" && event.key !== "PageUp")
    || event.defaultPrevented || event.isComposing
    || event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return false;
  if (hasKeyboardOverlay(main.ownerDocument)) return false;
  if (event.target instanceof Element && event.target.closest("select, [role='listbox']")) return false;

  const scroller = main.querySelector<HTMLElement>("[data-main-scroll-pane]") ?? main;
  if (scroller.clientHeight <= 0) return false;
  event.preventDefault();
  event.stopImmediatePropagation();
  // Journal hydration must not restore an anchor captured before this scroll.
  scroller.dispatchEvent(new Event(MAIN_PANE_SCROLL_INTENT));
  scroller.scrollBy({
    top: Math.max(120, scroller.clientHeight * 0.9) * (event.key === "PageDown" ? 1 : -1),
    behavior: "instant",
  });
  return true;
}

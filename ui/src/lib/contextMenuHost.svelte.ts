/**
 * The app's one context menu.
 *
 * Components do not draw menus of their own. They register a provider on an
 * element (`use:contextMenu`), and a single capture-phase listener on the
 * window handles every right-click: it always suppresses the webview's native
 * menu, asks the providers on the elements around the pointer for sections,
 * adds the clipboard actions, and shows them as one menu in
 * `ContextMenuHost.svelte` (drawn by `ContextMenuView.svelte`). Buttons and
 * keyboard shortcuts that open a menu call `openContextMenu` directly.
 */
import { EditorView } from "@codemirror/view";
import type { MenuPosition } from "./contextMenu";
import {
  dedupeItems,
  mergeSections,
  type ContextMenuAction,
  type ContextMenuContext,
  type ContextMenuItem,
  type ContextMenuProvider,
  type ContextMenuSection,
} from "./contextMenuSections";
import { buildEditSection } from "./editActions";
import { retainEditorForDialog } from "./editorDialogFocus";
import { focusElement } from "./focusElement";
import { describeError, showToast } from "./toast.svelte";
import { enclosingTopLayer } from "./topLayer";

/** The menu element's class; the listener leaves right-clicks inside it alone. */
export const CONTEXT_MENU_CLASS = "ctx-menu";

/**
 * Marks chrome that never has a menu, such as a toolbar whose buttons use
 * long-press for something else. The webview's own menu is still suppressed.
 */
export const NO_CONTEXT_MENU_ATTRIBUTE = "data-no-context-menu";

export interface ContextMenuRequest {
  /** Where the menu opens, in viewport pixels. */
  anchor: MenuPosition;
  /** The edge to flip around when the menu does not fit; a button's top edge, say. */
  flipAnchor?: MenuPosition;
  sections: readonly ContextMenuSection[];
  /** Names the menu for assistive technology. */
  label?: string;
  /** Takes focus back when the menu closes. Defaults to whatever had focus when it opened. */
  returnFocus?: HTMLElement | null;
  /** What the menu is about. An editor containing it stays open while the menu has focus. */
  target?: Element | null;
}

export interface OpenContextMenu {
  readonly token: number;
  readonly anchor: MenuPosition;
  readonly flipAnchor: MenuPosition;
  readonly label: string;
  readonly sections: readonly ContextMenuSection[];
  readonly returnFocus: HTMLElement | null;
  /**
   * The open modal dialog or popover the menu is about, which the menu must be
   * drawn inside to be seen and used. Null for the ordinary page.
   */
  readonly layer: HTMLElement | null;
}

let current = $state.raw<OpenContextMenu | null>(null);
let nextToken = 0;
let releaseEditors: Array<() => void> = [];

/** The menu being shown, if any. Reactive. */
export function getOpenContextMenu(): OpenContextMenu | null {
  return current;
}

interface Registration {
  provider: ContextMenuProvider;
}

const registry = new WeakMap<Element, Set<Registration>>();

/** Adds a provider for right-clicks on `element` or anything inside it. */
export function registerContextMenu(element: Element, provider: ContextMenuProvider): () => void {
  const registration: Registration = { provider };
  let registrations = registry.get(element);
  if (!registrations) {
    registrations = new Set();
    registry.set(element, registrations);
  }
  registrations.add(registration);
  return () => {
    const set = registry.get(element);
    if (!set) return;
    set.delete(registration);
    if (set.size === 0) registry.delete(element);
  };
}

/** `use:contextMenu={provider}` */
export function contextMenu(node: Element, provider: ContextMenuProvider | null | undefined) {
  let unregister = provider ? registerContextMenu(node, provider) : null;
  return {
    update(next: ContextMenuProvider | null | undefined) {
      unregister?.();
      unregister = next ? registerContextMenu(node, next) : null;
    },
    destroy() {
      unregister?.();
    },
  };
}

/** Sections from every provider between `target` and the root, innermost first. */
function collectSections(event: MouseEvent, target: Element, point: MenuPosition): ContextMenuSection[] {
  const context: ContextMenuContext = { event, target, x: point.x, y: point.y };
  const sections: ContextMenuSection[] = [];
  for (let node: Element | null = target; node; node = node.parentElement) {
    const registrations = registry.get(node);
    if (!registrations) continue;
    for (const { provider } of [...registrations]) {
      try {
        const result = provider(context);
        if (!result) continue;
        if (Array.isArray(result)) sections.push(...result);
        else sections.push(result as ContextMenuSection);
      } catch (error) {
        console.error("A context menu provider failed:", error);
      }
    }
  }
  return sections;
}

function applyResolved(token: number, sectionId: string, items: readonly ContextMenuItem[]): void {
  const open = current;
  if (!open || open.token !== token) return;

  const taken = new Set<string>();
  for (const section of open.sections) {
    if (section.id !== sectionId) for (const item of section.items) taken.add(item.id);
  }
  const sections = open.sections
    .map((section) =>
      section.id === sectionId
        ? { ...section, items: dedupeItems(items, taken), resolve: undefined }
        : section,
    )
    .filter((section) => section.items.length > 0 || section.resolve);

  if (sections.length === 0) {
    closeContextMenu();
    return;
  }
  current = { ...open, sections };
}

function resolveSections(token: number, sections: readonly ContextMenuSection[]): void {
  for (const section of sections) {
    const { resolve } = section;
    if (!resolve) continue;
    void Promise.resolve()
      // Not worth fetching for a menu that is already gone.
      .then(() => (current?.token === token ? resolve() : []))
      .then(
        (items) => applyResolved(token, section.id, items),
        (error) => {
          console.error(`The "${section.id}" context menu section could not be loaded:`, error);
          const failed: ContextMenuItem[] = section.errorLabel
            ? [{ kind: "status", id: `${section.id}.error`, label: section.errorLabel }]
            : [];
          applyResolved(token, section.id, failed);
        },
      );
  }
}

function releaseEditorRetention(): void {
  const releases = releaseEditors;
  releaseEditors = [];
  for (const release of releases) release();
}

/**
 * Opens the menu, replacing any open one. Returns false, and shows nothing,
 * when the sections come to no items.
 */
export function openContextMenu(request: ContextMenuRequest): boolean {
  closeContextMenu();
  const sections = mergeSections(request.sections);
  if (sections.length === 0) return false;

  const active = document.activeElement;
  const returnFocus =
    request.returnFocus !== undefined
      ? request.returnFocus
      : active instanceof HTMLElement && active !== document.body
        ? active
        : null;

  // The menu takes focus, which would make an editor tear itself down as if
  // the user had clicked away. This must be in place before that happens.
  const editors = new Set<HTMLElement>();
  for (const element of [request.target, returnFocus]) {
    const editor = element?.closest<HTMLElement>(".cm-editor");
    if (editor) editors.add(editor);
  }
  releaseEditors = [...editors].map(retainEditorForDialog);

  const token = ++nextToken;
  current = {
    token,
    anchor: request.anchor,
    flipAnchor: request.flipAnchor ?? request.anchor,
    label: request.label ?? "Context menu",
    sections,
    returnFocus,
    layer: enclosingTopLayer(request.target) ?? enclosingTopLayer(returnFocus),
  };
  resolveSections(token, sections);
  return true;
}

/** The gap between a button and the menu it opens. */
const BUTTON_MENU_GAP = 4;

/**
 * Opens the menu from a button: under it, or above it when there is no room
 * below, lining up with its right edge when there is none to the right. Focus
 * goes back to the button when the menu closes.
 */
export function openContextMenuBelow(
  button: HTMLElement,
  request: Pick<ContextMenuRequest, "sections" | "label">,
): boolean {
  const rect = button.getBoundingClientRect();
  return openContextMenu({
    ...request,
    anchor: { x: rect.left, y: rect.bottom + BUTTON_MENU_GAP },
    flipAnchor: { x: rect.right, y: rect.top - BUTTON_MENU_GAP },
    returnFocus: button,
    target: button,
  });
}

/** Closes the menu and, unless told not to, puts focus back where it was. */
export function closeContextMenu({ restoreFocus = true }: { restoreFocus?: boolean } = {}): void {
  const open = current;
  if (!open) return;
  current = null;
  releaseEditorRetention();
  if (restoreFocus) focusElement(open.returnFocus);
}

function reportActionError(error: unknown): void {
  showToast(describeError(error));
}

/**
 * Runs a menu item. The menu closes and focus returns first, and the action
 * runs synchronously inside the click: copying, pasting and file pickers need
 * the user gesture, and the field they act on needs its focus back.
 */
export function activateContextMenuItem(item: ContextMenuAction): void {
  if (item.disabled) return;
  closeContextMenu({ restoreFocus: true });
  try {
    const result: unknown = item.onSelect();
    if (result instanceof Promise) result.catch(reportActionError);
  } catch (error) {
    reportActionError(error);
  }
}

function eventTarget(event: Event): Element | null {
  const { target } = event;
  if (target instanceof Element) return target;
  return target instanceof Node ? target.parentElement : null;
}

/** Where the caret or selection ends, for a menu opened from the keyboard. */
function caretAnchor(target: Element): MenuPosition | null {
  try {
    const editor = target.closest<HTMLElement>(".cm-editor");
    const view = editor ? EditorView.findFromDOM(editor) : null;
    const caret = view?.coordsAtPos(view.state.selection.main.head);
    if (caret) return { x: caret.left, y: caret.bottom };

    const selection = window.getSelection();
    if (selection && selection.rangeCount > 0) {
      const range = selection.getRangeAt(0);
      const rect = range.getBoundingClientRect();
      if ((rect.width > 0 || rect.height > 0) && target.contains(range.commonAncestorContainer)) {
        return { x: rect.left, y: rect.bottom };
      }
    }
  } catch {
    // Opening beside the element is good enough when the caret cannot be found.
  }
  return null;
}

/** Keyboard-opened menus report no pointer, so anchor to the caret or the element. */
function anchorFor(event: MouseEvent, target: Element): MenuPosition {
  if (event.clientX !== 0 || event.clientY !== 0) return { x: event.clientX, y: event.clientY };
  const caret = caretAnchor(target);
  if (caret) return caret;
  const rect = target.getBoundingClientRect();
  return { x: rect.left, y: rect.bottom };
}

function handleContextMenu(event: MouseEvent): void {
  // Always: whatever is not claimed below must not fall through to the webview's own menu.
  event.preventDefault();
  const target = eventTarget(event);
  if (!target || target.closest(`.${CONTEXT_MENU_CLASS}`)) return;
  if (target.closest(`[${NO_CONTEXT_MENU_ATTRIBUTE}]`)) {
    closeContextMenu();
    return;
  }

  const anchor = anchorFor(event, target);
  // Close first, with focus restored, so the new menu remembers the right element.
  closeContextMenu();
  const sections = collectSections(event, target, anchor);
  const edit = buildEditSection(target, anchor);
  if (edit) sections.push(edit);
  openContextMenu({ anchor, sections, target });
}

/** Starts handling every right-click in the window. Returns a function that stops. */
export function installContextMenu(target: Window = window): () => void {
  target.addEventListener("contextmenu", handleContextMenu, true);
  return () => {
    target.removeEventListener("contextmenu", handleContextMenu, true);
    closeContextMenu({ restoreFocus: false });
  };
}

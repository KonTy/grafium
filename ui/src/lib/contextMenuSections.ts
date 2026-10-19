/**
 * What a context menu is made of.
 *
 * The app has one context menu. Whatever sits under the pointer contributes
 * sections to it through providers (see `contextMenuHost.svelte.ts`), and the
 * host merges them into a single list, so a right-click on a misspelled word
 * inside a selection shows spelling, link and clipboard actions together
 * instead of whichever component happened to answer first.
 */

export interface ContextMenuAction {
  kind?: "action";
  /** Unique across the whole menu; a repeated id is dropped, the first one wins. */
  id: string;
  label: string;
  /** Runs synchronously inside the click, so clipboard and file dialogs keep their user gesture. */
  onSelect: () => void | Promise<void>;
  disabled?: boolean;
  danger?: boolean;
  /** Secondary text on the right, such as a shortcut or a size. */
  hint?: string;
  title?: string;
  /** Set for the primary choices of a section, such as spelling suggestions. */
  emphasis?: boolean;
}

/** A line of text that is not a choice, such as "No suggestions". */
export interface ContextMenuStatus {
  kind: "status";
  id: string;
  label: string;
}

export type ContextMenuItem = ContextMenuAction | ContextMenuStatus;

export interface ContextMenuSection {
  id: string;
  /** A visible heading. */
  title?: string;
  /** Names the group for assistive technology when there is no visible heading. */
  ariaLabel?: string;
  items: ContextMenuItem[];
  /**
   * Fetches the section's real items after the menu is already showing; until
   * then `items` is what is shown, typically a status line. A failure is
   * logged, and the section then shows `errorLabel`, or is dropped without one.
   */
  resolve?: () => Promise<ContextMenuItem[]>;
  /** The status line shown in place of a section whose `resolve` failed. */
  errorLabel?: string;
  /** Lower comes first. Defaults to `DEFAULT_SECTION_ORDER`. */
  order?: number;
}

/** What a provider is told about the right-click. */
export interface ContextMenuContext {
  event: MouseEvent;
  /** The element under the pointer; for a text node, its parent. */
  target: Element;
  /** Viewport pixels. */
  x: number;
  y: number;
}

export type ContextMenuProvider = (
  context: ContextMenuContext,
) => readonly ContextMenuSection[] | ContextMenuSection | null | undefined | void;

/** Where sections go unless they say otherwise. */
export const DEFAULT_SECTION_ORDER = 50;
/** Spelling comes first: it is about the word under the pointer. */
export const SPELLING_SECTION_ORDER = 10;
/** Clipboard actions come last, as in a native menu. */
export const EDIT_SECTION_ORDER = 90;

export function isStatus(item: ContextMenuItem): item is ContextMenuStatus {
  return item.kind === "status";
}

/**
 * Combines the sections that providers returned, innermost provider first.
 *
 * - Items with an id that was already seen are dropped, so the element closest
 *   to the pointer wins when two providers offer the same action.
 * - Sections that share an id are joined into one.
 * - Sections with nothing to show and nothing to fetch are dropped.
 * - The rest are ordered by `order`, keeping the given order among equals.
 */
export function mergeSections(input: readonly ContextMenuSection[]): ContextMenuSection[] {
  const byId = new Map<string, ContextMenuSection>();
  const seenItems = new Set<string>();

  for (const section of input) {
    const items = section.items.filter((item) => {
      if (seenItems.has(item.id)) return false;
      seenItems.add(item.id);
      return true;
    });
    const existing = byId.get(section.id);
    if (existing) {
      existing.items.push(...items);
      existing.resolve ??= section.resolve;
      existing.errorLabel ??= section.errorLabel;
      continue;
    }
    byId.set(section.id, { ...section, items });
  }

  return [...byId.values()]
    .filter((section) => section.items.length > 0 || section.resolve)
    .sort((a, b) => (a.order ?? DEFAULT_SECTION_ORDER) - (b.order ?? DEFAULT_SECTION_ORDER));
}

/**
 * Items fetched later, minus any whose id the rest of the menu already uses.
 * `taken` is the ids of every other section.
 */
export function dedupeItems(
  items: readonly ContextMenuItem[],
  taken: ReadonlySet<string>,
): ContextMenuItem[] {
  const seen = new Set(taken);
  return items.filter((item) => {
    if (seen.has(item.id)) return false;
    seen.add(item.id);
    return true;
  });
}

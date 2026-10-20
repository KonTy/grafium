/**
 * The right-click menu of a page's blocks: what can be done with the selected
 * text or blocks, and with the page itself. It is built here, as data, so the
 * wording and the order are tested without mounting the page. The same sections
 * are offered whether the block under the pointer is being edited or only shown;
 * what changes is what is added around them, such as spelling suggestions.
 */
import type { ContextMenuAction, ContextMenuSection } from "./contextMenuSections";

type Run = () => void | Promise<void>;

export interface BlockMenuOptions {
  /** How many blocks the menu acts on; 0 when the pointer is not on a block. */
  blockCount: number;
  /** At least one of those blocks is a task. */
  hasTasks: boolean;
  /** At least one of those blocks is a plain bullet. */
  hasBullets: boolean;
  /** Present when the pointer is on selected text that can become a page link. */
  makeLink?: Run;
  turnTasksIntoBullets: Run;
  turnBulletsIntoTodos: Run;
  copyBlocks: Run;
  /** Absent when no block is selected, as while one is being edited. */
  clearSelection?: Run;
  /** Absent when this page cannot be renamed or deleted. */
  renamePage?: Run;
  deletePage?: Run;
}

export type PageMenuOptions = Pick<BlockMenuOptions, "renamePage" | "deletePage">;

/** What the page itself offers, for a pointer that is on no block. */
export function buildPageMenuSections(options: PageMenuOptions): ContextMenuSection[] {
  const page: ContextMenuAction[] = [];
  if (options.renamePage) {
    page.push({ id: "page.rename", label: "Rename page", onSelect: options.renamePage });
  }
  if (options.deletePage) {
    page.push({ id: "page.delete", label: "Delete page", danger: true, onSelect: options.deletePage });
  }
  return page.length > 0 ? [{ id: "page", ariaLabel: "Page", items: page }] : [];
}

export function buildBlockMenuSections(options: BlockMenuOptions): ContextMenuSection[] {
  const { blockCount } = options;
  const several = blockCount > 1;

  const selection: ContextMenuAction[] = [];
  if (options.makeLink) {
    selection.push({ id: "link.make", label: "Make link", onSelect: options.makeLink });
  }
  if (blockCount > 0 && options.hasTasks) {
    selection.push({
      id: "blocks.taskToBullet",
      label: several ? "Turn tasks into bullets" : "Turn task into bullet",
      onSelect: options.turnTasksIntoBullets,
    });
  }
  if (blockCount > 0 && options.hasBullets) {
    selection.push({
      id: "blocks.bulletToTodo",
      label: several ? "Turn bullets into TODOs" : "Turn bullet into TODO",
      onSelect: options.turnBulletsIntoTodos,
    });
  }
  if (blockCount > 0) {
    // Named for the blocks: with text selected, the plain "Copy" below is the text.
    selection.push({
      id: "blocks.copy",
      label: several ? "Copy blocks" : "Copy block",
      onSelect: options.copyBlocks,
    });
    if (options.clearSelection) {
      selection.push({ id: "blocks.clear", label: "Clear selection", onSelect: options.clearSelection });
    }
  }

  const sections: ContextMenuSection[] = [];
  if (selection.length > 0) sections.push({ id: "selection", ariaLabel: "Selection", items: selection });
  sections.push(...buildPageMenuSections(options));
  return sections;
}

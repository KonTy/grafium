/**
 * The menu of a page, a folder of pages or an imported book in All Pages.
 * A right-click, the row's "⋯" button and the keyboard all open this one menu.
 */
import type { ContextMenuAction, ContextMenuSection } from "./contextMenuSections";

export interface PageActionTarget {
  /** The page the row stands for; a folder that is no page itself has none. */
  pageId: string | null;
  /** The full title, namespace included. */
  title: string;
  /** The imported book it belongs to, if any. */
  bookTitle: string | null;
  folder: boolean;
}

export interface PageActionHandlers {
  rename: (target: PageActionTarget) => void | Promise<void>;
  openInFileBrowser: (target: PageActionTarget) => void | Promise<void>;
  remove: (target: PageActionTarget) => void | Promise<void>;
}

/** An imported book goes as its whole folder; a namespace as a folder of pages. */
export function deleteLabel(target: PageActionTarget): string {
  if (target.bookTitle) return "Delete whole book folder";
  return target.folder ? "Delete folder" : "Delete page";
}

/** Names the menu when a button opens it. */
export function pageActionLabel(target: PageActionTarget): string {
  return `Actions for ${target.title}`;
}

export function buildPageActionSections(
  target: PageActionTarget,
  handlers: PageActionHandlers,
): ContextMenuSection[] {
  const items: ContextMenuAction[] = [];
  // A book is renamed where it was imported, not from this list.
  if (!target.bookTitle) {
    items.push({
      id: "page.rename",
      label: target.folder ? "Rename folder" : "Rename",
      onSelect: () => handlers.rename(target),
    });
  }
  if (target.pageId || target.bookTitle || target.folder) {
    items.push({
      id: "page.openInFileBrowser",
      label: "Open in file browser",
      onSelect: () => handlers.openInFileBrowser(target),
    });
  }

  return [
    { id: "page", title: target.bookTitle ?? target.title, items },
    {
      id: "danger",
      ariaLabel: "Delete",
      items: [
        {
          id: "page.delete",
          label: deleteLabel(target),
          danger: true,
          onSelect: () => handlers.remove(target),
        },
      ],
    },
  ];
}

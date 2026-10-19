/**
 * The right-click menu of a page in the sidebar's Favorites and Recent lists.
 */
import type { ContextMenuItem, ContextMenuSection } from "./contextMenuSections";

/** What the graph says about a page; it decides what the collection item offers. */
export type PageCollectionState = "page" | "collection" | "original" | "unavailable";

export interface PageMenuOptions {
  isFavorite: boolean;
  toggleFavorite: () => void | Promise<void>;
  /** Reads the page's kind from the graph. A rejection becomes an "unavailable" line. */
  loadCollectionState: () => Promise<PageCollectionState>;
  /** Makes the page a book collection, or a regular page again when `collection` is false. */
  setCollection: (collection: boolean) => void | Promise<void>;
}

const COLLECTION_STATUS_ID = "page.collection.status";

function collectionStatus(label: string): ContextMenuItem {
  return { kind: "status", id: COLLECTION_STATUS_ID, label };
}

function collectionItem(
  state: PageCollectionState,
  setCollection: PageMenuOptions["setCollection"],
): ContextMenuItem {
  switch (state) {
    case "original":
      return collectionStatus("Original book (read-only)");
    case "unavailable":
      return collectionStatus("Collections Unavailable");
    case "collection":
      return {
        id: "page.collection",
        label: "Convert to Regular Page",
        onSelect: () => setCollection(false),
      };
    case "page":
      return {
        id: "page.collection",
        label: "Mark as Book Collection",
        onSelect: () => setCollection(true),
      };
  }
}

/**
 * Favorites is known at once. Whether the page is a book collection takes a
 * read from the graph, so that item arrives after the menu is already showing.
 */
export function buildPageMenuSections(options: PageMenuOptions): ContextMenuSection[] {
  const favorite: ContextMenuItem = {
    id: "page.favorite",
    label: options.isFavorite ? "Remove from Favorites" : "Add to Favorites",
    onSelect: options.toggleFavorite,
  };

  return [
    {
      id: "page",
      ariaLabel: "Page",
      items: [favorite, collectionStatus("Loading Collection Status…")],
      resolve: async () => {
        try {
          return [favorite, collectionItem(await options.loadCollectionState(), options.setCollection)];
        } catch {
          return [favorite, collectionStatus("Collection Status Unavailable")];
        }
      },
    },
  ];
}

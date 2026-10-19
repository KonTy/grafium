import { describe, expect, it, vi } from "vitest";
import { isStatus, type ContextMenuAction } from "./contextMenuSections";
import { buildPageMenuSections, type PageCollectionState, type PageMenuOptions } from "./sidebarMenu";

function options(extra: Partial<PageMenuOptions> = {}): PageMenuOptions {
  return {
    isFavorite: false,
    toggleFavorite: vi.fn(),
    loadCollectionState: async () => "page",
    setCollection: vi.fn(),
    ...extra,
  };
}

async function resolved(extra: Partial<PageMenuOptions> = {}) {
  const [section] = buildPageMenuSections(options(extra));
  return (await section.resolve!()) ?? [];
}

const labels = (items: readonly { label: string }[]) => items.map((item) => item.label);

describe("sidebar page menu", () => {
  it("is one section named for assistive technology", () => {
    const sections = buildPageMenuSections(options());
    expect(sections).toHaveLength(1);
    expect(sections[0].id).toBe("page");
    expect(sections[0].ariaLabel).toBe("Page");
  });

  it("offers favoriting at once, while the collection state is still being read", () => {
    const [section] = buildPageMenuSections(options());
    expect(labels(section.items)).toEqual(["Add to Favorites", "Loading Collection Status…"]);
    expect(isStatus(section.items[1])).toBe(true);
    expect(section.resolve).toBeTypeOf("function");
  });

  it("offers removing a page that is already a favorite", async () => {
    const [section] = buildPageMenuSections(options({ isFavorite: true }));
    expect(section.items[0].label).toBe("Remove from Favorites");
    expect(labels(await resolved({ isFavorite: true }))[0]).toBe("Remove from Favorites");
  });

  it("runs the favorite action it was given, before and after the state loads", async () => {
    const toggleFavorite = vi.fn();
    const [section] = buildPageMenuSections(options({ toggleFavorite }));
    (section.items[0] as ContextMenuAction).onSelect();
    expect(toggleFavorite).toHaveBeenCalledTimes(1);

    const items = await section.resolve!();
    (items[0] as ContextMenuAction).onSelect();
    expect(toggleFavorite).toHaveBeenCalledTimes(2);
  });

  it.each<[PageCollectionState, string, boolean]>([
    ["page", "Mark as Book Collection", true],
    ["collection", "Convert to Regular Page", true],
    ["original", "Original book (read-only)", false],
    ["unavailable", "Collections Unavailable", false],
  ])("shows %s as %j, which is a choice only when %s", async (state, label, isChoice) => {
    const items = await resolved({ loadCollectionState: async () => state });
    expect(labels(items)).toEqual(["Add to Favorites", label]);
    expect(isStatus(items[1])).toBe(!isChoice);
  });

  it("marks a page as a collection, and turns a collection back", async () => {
    const setCollection = vi.fn();
    const mark = await resolved({ loadCollectionState: async () => "page", setCollection });
    (mark[1] as ContextMenuAction).onSelect();
    expect(setCollection).toHaveBeenLastCalledWith(true);

    const convert = await resolved({ loadCollectionState: async () => "collection", setCollection });
    (convert[1] as ContextMenuAction).onSelect();
    expect(setCollection).toHaveBeenLastCalledWith(false);
  });

  it("hands a failing collection change back to the menu so it can be reported", async () => {
    const failure = new Error("disk full");
    const items = await resolved({
      loadCollectionState: async () => "page",
      setCollection: () => Promise.reject(failure),
    });
    await expect((items[1] as ContextMenuAction).onSelect()).rejects.toBe(failure);
  });

  it("keeps favoriting when the collection state cannot be read", async () => {
    const items = await resolved({
      isFavorite: true,
      loadCollectionState: () => Promise.reject(new Error("offline")),
    });
    expect(labels(items)).toEqual(["Remove from Favorites", "Collection Status Unavailable"]);
    expect(isStatus(items[1])).toBe(true);
  });

  it("uses ids that are stable and unique, so merging with other sections cannot drop them", async () => {
    const ids = (await resolved()).map((item) => item.id);
    expect(ids).toEqual(["page.favorite", "page.collection"]);
    expect(new Set(ids).size).toBe(ids.length);
  });
});

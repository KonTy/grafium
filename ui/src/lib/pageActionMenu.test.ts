import { describe, expect, it, vi } from "vitest";
import { mergeSections, type ContextMenuAction } from "./contextMenuSections";
import {
  buildPageActionSections,
  deleteLabel,
  pageActionLabel,
  type PageActionHandlers,
  type PageActionTarget,
} from "./pageActionMenu";

const page: PageActionTarget = { pageId: "p1", title: "zebra", bookTitle: null, folder: false };
const folder: PageActionTarget = { pageId: null, title: "tech", bookTitle: null, folder: true };
const book: PageActionTarget = {
  pageId: "p9",
  title: "Books/Dune/Chapter 1",
  bookTitle: "Books/Dune",
  folder: false,
};

function handlers(): PageActionHandlers {
  return { rename: vi.fn(), openInFileBrowser: vi.fn(), remove: vi.fn() };
}

const labels = (target: PageActionTarget) =>
  buildPageActionSections(target, handlers()).map((section) => section.items.map((item) => item.label));

function item(target: PageActionTarget, id: string, given = handlers()): ContextMenuAction {
  const found = buildPageActionSections(target, given)
    .flatMap((section) => section.items)
    .find((candidate) => candidate.id === id);
  if (!found || found.kind === "status") throw new Error(`no action ${id}`);
  return found;
}

describe("page list menu", () => {
  it("names a page, with Rename and the file browser first and the delete apart", () => {
    const sections = buildPageActionSections(page, handlers());
    expect(sections.map((section) => section.id)).toEqual(["page", "danger"]);
    expect(sections[0].title).toBe("zebra");
    expect(labels(page)).toEqual([["Rename", "Open in file browser"], ["Delete page"]]);
  });

  it("calls a namespace a folder throughout", () => {
    expect(labels(folder)).toEqual([["Rename folder", "Open in file browser"], ["Delete folder"]]);
  });

  it("heads a book's pages with the book, leaves renaming to where it was imported, and deletes its whole folder", () => {
    const sections = buildPageActionSections(book, handlers());
    expect(sections[0].title).toBe("Books/Dune");
    expect(labels(book)).toEqual([["Open in file browser"], ["Delete whole book folder"]]);
  });

  it("shows the file browser only for something that is stored: a page, a folder or a book", () => {
    const nothing: PageActionTarget = { pageId: null, title: "draft", bookTitle: null, folder: false };
    expect(labels(nothing)).toEqual([["Rename"], ["Delete page"]]);
  });

  it("marks only the delete as dangerous", () => {
    const actions = buildPageActionSections(folder, handlers()).flatMap((section) => section.items);
    expect(actions.filter((entry) => (entry as ContextMenuAction).danger).map((entry) => entry.label)).toEqual([
      "Delete folder",
    ]);
  });

  it("hands each action the target the menu was built for", () => {
    const given = handlers();
    item(folder, "page.rename", given).onSelect();
    item(folder, "page.openInFileBrowser", given).onSelect();
    item(folder, "page.delete", given).onSelect();
    expect(given.rename).toHaveBeenCalledWith(folder);
    expect(given.openInFileBrowser).toHaveBeenCalledWith(folder);
    expect(given.remove).toHaveBeenCalledWith(folder);
  });

  it("survives merging into the shared menu unchanged", () => {
    const merged = mergeSections(buildPageActionSections(page, handlers()));
    expect(merged.map((section) => section.id)).toEqual(["page", "danger"]);
    expect(merged[0].items.map((entry) => entry.label)).toEqual(["Rename", "Open in file browser"]);
  });

  it("labels the delete and the menu from the target", () => {
    expect(deleteLabel(page)).toBe("Delete page");
    expect(deleteLabel(folder)).toBe("Delete folder");
    expect(deleteLabel(book)).toBe("Delete whole book folder");
    expect(pageActionLabel(book)).toBe("Actions for Books/Dune/Chapter 1");
  });
});

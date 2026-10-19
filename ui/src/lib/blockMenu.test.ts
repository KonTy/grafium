import { describe, expect, it, vi } from "vitest";
import { buildBlockMenuSections, type BlockMenuOptions } from "./blockMenu";
import { isStatus, type ContextMenuAction } from "./contextMenuSections";

function options(overrides: Partial<BlockMenuOptions> = {}): BlockMenuOptions {
  return {
    blockCount: 1,
    hasTasks: false,
    hasBullets: true,
    turnTasksIntoBullets: vi.fn(),
    turnBulletsIntoTodos: vi.fn(),
    copyBlocks: vi.fn(),
    clearSelection: vi.fn(),
    renamePage: vi.fn(),
    deletePage: vi.fn(),
    ...overrides,
  };
}

function labels(overrides: Partial<BlockMenuOptions> = {}) {
  return buildBlockMenuSections(options(overrides)).map((section) =>
    section.items.map((item) => item.label),
  );
}

function action(sections: ReturnType<typeof buildBlockMenuSections>, id: string): ContextMenuAction {
  const found = sections.flatMap((section) => section.items).find((item) => item.id === id);
  if (!found || isStatus(found)) throw new Error(`no action ${id}`);
  return found;
}

describe("the block menu", () => {
  it("offers the block actions, then the page actions, for a bullet", () => {
    expect(labels()).toEqual([
      ["Turn bullet into TODO", "Copy block", "Clear selection"],
      ["Rename page", "Delete page"],
    ]);
  });

  it("offers the task conversion for a task", () => {
    expect(labels({ hasTasks: true, hasBullets: false })).toEqual([
      ["Turn task into bullet", "Copy block", "Clear selection"],
      ["Rename page", "Delete page"],
    ]);
  });

  it("offers both conversions for a mix, in the plural for several blocks", () => {
    expect(labels({ blockCount: 3, hasTasks: true, hasBullets: true })[0]).toEqual([
      "Turn tasks into bullets",
      "Turn bullets into TODOs",
      "Copy blocks",
      "Clear selection",
    ]);
  });

  it("puts Make link first when text is selected", () => {
    expect(labels({ makeLink: vi.fn() })[0]).toEqual([
      "Make link",
      "Turn bullet into TODO",
      "Copy block",
      "Clear selection",
    ]);
  });

  it("leaves out the block actions when the pointer is not on a block", () => {
    expect(labels({ blockCount: 0, hasBullets: false })).toEqual([["Rename page", "Delete page"]]);
  });

  it("keeps Make link even without a block", () => {
    expect(labels({ blockCount: 0, hasBullets: false, makeLink: vi.fn() })).toEqual([
      ["Make link"],
      ["Rename page", "Delete page"],
    ]);
  });

  it("leaves out the page actions for a page that cannot be renamed or deleted", () => {
    expect(labels({ renamePage: undefined, deletePage: undefined })).toEqual([
      ["Turn bullet into TODO", "Copy block", "Clear selection"],
    ]);
  });

  it("has nothing to offer when there is nothing to act on", () => {
    expect(
      buildBlockMenuSections(
        options({ blockCount: 0, hasBullets: false, renamePage: undefined, deletePage: undefined }),
      ),
    ).toEqual([]);
  });

  it("names the groups for assistive technology", () => {
    expect(buildBlockMenuSections(options()).map((s) => [s.id, s.ariaLabel])).toEqual([
      ["selection", "Selection"],
      ["page", "Page"],
    ]);
  });

  it("marks only Delete page as dangerous", () => {
    const sections = buildBlockMenuSections(options({ makeLink: vi.fn() }));
    const danger = sections
      .flatMap((section) => section.items)
      .filter((item) => !isStatus(item) && item.danger)
      .map((item) => item.label);
    expect(danger).toEqual(["Delete page"]);
  });

  it("gives every action its own id, sharing Make link's with the editor menu", () => {
    const sections = buildBlockMenuSections(
      options({ blockCount: 2, hasTasks: true, hasBullets: true, makeLink: vi.fn() }),
    );
    const ids = sections.flatMap((section) => section.items.map((item) => item.id));
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids).toContain("link.make");
  });

  it("runs the callback that belongs to each item", () => {
    const o = options({ blockCount: 2, hasTasks: true, hasBullets: true, makeLink: vi.fn() });
    const sections = buildBlockMenuSections(o);
    const byId: Record<string, () => void | Promise<void>> = {
      "link.make": o.makeLink!,
      "blocks.taskToBullet": o.turnTasksIntoBullets,
      "blocks.bulletToTodo": o.turnBulletsIntoTodos,
      "blocks.copy": o.copyBlocks,
      "blocks.clear": o.clearSelection,
      "page.rename": o.renamePage!,
      "page.delete": o.deletePage!,
    };
    for (const [id, expected] of Object.entries(byId)) {
      void action(sections, id).onSelect();
      expect(expected, id).toHaveBeenCalledTimes(1);
    }
  });
});

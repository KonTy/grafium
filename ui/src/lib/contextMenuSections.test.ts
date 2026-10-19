import { describe, expect, it } from "vitest";
import {
  DEFAULT_SECTION_ORDER,
  EDIT_SECTION_ORDER,
  SPELLING_SECTION_ORDER,
  dedupeItems,
  isStatus,
  mergeSections,
  type ContextMenuAction,
  type ContextMenuItem,
  type ContextMenuSection,
} from "./contextMenuSections";

function action(id: string, label = id): ContextMenuAction {
  return { id, label, onSelect: () => {} };
}

function section(id: string, items: ContextMenuItem[], extra: Partial<ContextMenuSection> = {}): ContextMenuSection {
  return { id, items, ...extra };
}

const ids = (sections: readonly ContextMenuSection[]) => sections.map((s) => s.id);
const itemIds = (s: ContextMenuSection) => s.items.map((item) => item.id);

describe("isStatus", () => {
  it("tells status lines from actions", () => {
    expect(isStatus({ kind: "status", id: "s", label: "No suggestions" })).toBe(true);
    expect(isStatus(action("a"))).toBe(false);
    expect(isStatus({ kind: "action", id: "b", label: "B", onSelect: () => {} })).toBe(false);
  });
});

describe("mergeSections", () => {
  it("orders sections by `order`, spelling first and edit last", () => {
    const merged = mergeSections([
      section("edit", [action("edit.copy")], { order: EDIT_SECTION_ORDER }),
      section("page", [action("page.rename")]),
      section("spelling", [action("spell.fix")], { order: SPELLING_SECTION_ORDER }),
    ]);
    expect(ids(merged)).toEqual(["spelling", "page", "edit"]);
  });

  it("keeps the given order among sections of equal order", () => {
    const merged = mergeSections([
      section("b", [action("b1")]),
      section("a", [action("a1")]),
      section("c", [action("c1")], { order: DEFAULT_SECTION_ORDER }),
    ]);
    expect(ids(merged)).toEqual(["b", "a", "c"]);
  });

  it("drops an item whose id was already offered, so the first provider wins", () => {
    const first = { ...action("make-link"), label: "Make link (editor)" };
    const second = { ...action("make-link"), label: "Make link (page)" };
    const merged = mergeSections([section("selection", [first]), section("page", [second, action("page.rename")])]);
    expect(merged.flatMap((s) => s.items.map((i) => i.label))).toEqual(["Make link (editor)", "page.rename"]);
  });

  it("joins sections that share an id", () => {
    const merged = mergeSections([
      section("selection", [action("one")]),
      section("other", [action("x")]),
      section("selection", [action("two")]),
    ]);
    expect(ids(merged)).toEqual(["selection", "other"]);
    expect(itemIds(merged[0])).toEqual(["one", "two"]);
  });

  it("keeps the first resolver of joined sections", async () => {
    const first = async () => [action("late")];
    const second = async () => [action("ignored")];
    const [merged] = mergeSections([
      section("s", [], { resolve: first }),
      section("s", [action("a")], { resolve: second }),
    ]);
    expect(merged.resolve).toBe(first);
    expect(itemIds(merged)).toEqual(["a"]);
  });

  it("keeps the first failure notice of joined sections", () => {
    const [merged] = mergeSections([
      section("s", [action("a")]),
      section("s", [action("b")], { errorLabel: "Could not load" }),
      section("s", [action("c")], { errorLabel: "Ignored" }),
    ]);
    expect(merged.errorLabel).toBe("Could not load");
  });

  it("drops sections with nothing to show and nothing to fetch", () => {
    const merged = mergeSections([
      section("empty", []),
      section("fetched", [], { resolve: async () => [action("later")] }),
      section("shown", [action("now")]),
    ]);
    expect(ids(merged)).toEqual(["fetched", "shown"]);
  });

  it("drops a section whose every item was already offered", () => {
    const merged = mergeSections([section("a", [action("same")]), section("b", [action("same")])]);
    expect(ids(merged)).toEqual(["a"]);
  });

  it("does not modify its input", () => {
    const input = [section("a", [action("1")]), section("a", [action("2")])];
    mergeSections(input);
    expect(itemIds(input[0])).toEqual(["1"]);
    expect(input).toHaveLength(2);
  });

  it("returns nothing for nothing", () => {
    expect(mergeSections([])).toEqual([]);
  });
});

describe("dedupeItems", () => {
  it("drops items the rest of the menu already has", () => {
    const result = dedupeItems([action("a"), action("b"), action("c")], new Set(["b"]));
    expect(result.map((i) => i.id)).toEqual(["a", "c"]);
  });

  it("drops repeats within the fetched items", () => {
    const result = dedupeItems([action("a"), action("a")], new Set());
    expect(result.map((i) => i.id)).toEqual(["a"]);
  });
});

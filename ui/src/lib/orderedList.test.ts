import { describe, expect, it } from "vitest";
import { isEmptyOrderedListItem, movedOrderedListChanges as movedChanges, orderedListMarker, parseOrderedListMarker } from "./orderedList";
import type { Block } from "./api";
import { buildBlockRenderState } from "./pageContentVirtualization";

describe("ordered list markers", () => {
  it.each(["2.", "2)", "2. \t", "2.\u00a0", "2.1.", "2.1) ", "  2.1.\t\u200b", "2.\u200b", "2. \ufeff"])(
    "recognizes visually empty %s without continuing numbers", (line) => {
      expect(isEmptyOrderedListItem(line)).toBe(true);
    },
  );
  it.each(["2. Content", "2.1) Text", "2.1", "1.5 metres", "    2. Code", "```\n2. \n```"])(
    "preserves written content %s", (line) => expect(isEmptyOrderedListItem(line)).toBe(false),
  );
  it.each([
    ["1. First", "1. ", "2. "],
    ["3) Third", "3) ", "4) "],
    ["  7.\tIndented", "  7.\t", "  8.\t"],
    ["0. Zero", "0. ", "1. "],
    ["2.", "2.", "3. "],
    ["2) ", "2) ", "3) "],
    ["2.1. Child", "2.1. ", "2.2. "],
    ["2.9.99) Child", "2.9.99) ", "2.9.100) "],
  ])("recognizes %s without reading other lines", (line, prefix, nextPrefix) => {
    expect(orderedListMarker(line)).toEqual({ prefix, nextPrefix });
  });

  it.each(["1.text", "1.5 metres", "3)word", "    1. Code", "\t1. Code",
    "`1. Code`", "\\1. Literal", "- 1. Nested source", "Text 1. Number", "1000000000. Too long"])(
    "does not treat %s as a list", (line) => {
      expect(orderedListMarker(line)).toBeNull();
    },
  );

  it("does not generate a marker outside Markdown's nine-digit limit", () => {
    expect(orderedListMarker("999999999. Last")?.nextPrefix).toBeNull();
    expect(orderedListMarker("2.999999999) Last")?.nextPrefix).toBeNull();
  });

  it("bounds segment lengths and depth without accepting decimal prose", () => {
    expect(parseOrderedListMarker(`${Array(12).fill("9").join(".")}. Step`)).not.toBeNull();
    expect(parseOrderedListMarker(`${Array(13).fill("9").join(".")}. Step`)).toBeNull();
    expect(parseOrderedListMarker("2.1000000000. Step")).toBeNull();
  });
});

describe("moved numbered branches", () => {
  const movedOrderedListChanges = (blocks: Block[], blockId: string, oldParentId: string | null, direction: "in" | "out") =>
    movedChanges(buildBlockRenderState(blocks, new Set()), blockId, oldParentId, direction);
  const block = (id: string, parent_id: string | null, order_index: number, content: string): Block => ({
    id, parent_id, order_index, content, page_id: "p", block_type: "text", properties: {}, created_at: "0", updated_at: "0",
  });
  it("changes only the moved branch, follows existing child numbers, and preserves literal descendants", () => {
    const blocks = [
      block("one", null, 0, "1. First"), block("two", null, 1, "2. Second"),
      block("existing", "two", 0, "2.9. Existing"), block("three", "two", 1, "3.  Text\nKeep\n"),
      block("child", "three", 0, "1) Child"), block("plain", "three", 1, "Plain"),
      block("code", "child", 0, "```\n1. Literal\n```"), block("other", null, 8, "9. Unrelated"),
    ];
    expect(movedOrderedListChanges(blocks, "three", null, "in")).toEqual([
      { blockId: "three", beforeContent: "3.  Text\nKeep\n", afterContent: "2.10.  Text\nKeep\n" },
      { blockId: "child", beforeContent: "1) Child", afterContent: "2.10.1) Child" },
    ]);
    expect(blocks[3].content).toBe("3.  Text\nKeep\n");
  });
  it("outdents after the former parent without relabeling other siblings", () => {
    const blocks = [block("parent", null, 0, "2) Parent"), block("moving", null, 1, "2.4) Text"),
      block("later", null, 2, "8) Later"), block("child", "moving", 0, "2.4.1. Child")];
    expect(movedOrderedListChanges(blocks, "moving", "parent", "out")).toEqual([
      { blockId: "moving", beforeContent: "2.4) Text", afterContent: "3) Text" },
      { blockId: "child", beforeContent: "2.4.1. Child", afterContent: "3.1. Child" },
    ]);
  });
  it("leaves ordinary outline moves alone and does not exceed prefix limits", () => {
    expect(movedOrderedListChanges([block("plain", null, 0, "Plain"), block("child", "plain", 0, "3. Text")],
      "child", null, "in")).toEqual([]);
    expect(movedOrderedListChanges([block("parent", null, 0, "1. Parent"), block("plain", "parent", 0, "Plain")],
      "plain", null, "in")).toEqual([]);
    const parent = `${Array(12).fill("1").join(".")}. Parent`;
    expect(movedOrderedListChanges([block("parent", null, 0, parent), block("child", "parent", 0, "2. Text")],
      "child", null, "in")).toEqual([]);
  });
});

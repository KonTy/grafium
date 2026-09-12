import { describe, expect, it } from "vitest";
import type { Block } from "./api";
import { buildBacklinkSourceIndex, buildBacklinkTree, isEmptyOutlineBlock } from "./backlinkTree";

function block(partial: Pick<Block, "id" | "content"> & Partial<Block>): Block {
  return {
    page_id: "journal",
    parent_id: null,
    order_index: 0,
    block_type: "text",
    properties: {},
    created_at: "0",
    updated_at: "0",
    ...partial,
  };
}

describe("backlink trees", () => {
  it("treats blank and dash-only bullets as empty wrappers", () => {
    expect(isEmptyOutlineBlock("")).toBe(true);
    expect(isEmptyOutlineBlock("-")).toBe(true);
    expect(isEmptyOutlineBlock("[[Cars/Offroad]]")).toBe(false);
  });

  it("includes descendants of empty siblings that follow a page link", () => {
    const blocks = [
      block({ id: "cars", content: "[[Cars/Offroad]] #lift", order_index: 0 }),
      block({ id: "ads", content: "### Ads", parent_id: "cars", order_index: 0 }),
      block({ id: "empty", content: "-", order_index: 1 }),
      block({ id: "van", content: "### Van rebuild video", parent_id: "empty", order_index: 0 }),
      block({ id: "health", content: "[[Health]]", order_index: 2 }),
    ];
    const tree = buildBacklinkTree("cars", buildBacklinkSourceIndex(blocks));
    expect(tree.map((node) => node.block.content)).toEqual([
      "[[Cars/Offroad]] #lift",
      "### Ads",
      "### Van rebuild video",
    ]);
    expect(tree.map((node) => node.depth)).toEqual([0, 1, 1]);
  });

  it("does not steal the next real heading", () => {
    const blocks = [
      block({ id: "cars", content: "[[Cars/Offroad]]", order_index: 0 }),
      block({ id: "health", content: "[[Health]]", order_index: 1 }),
      block({ id: "video", content: "longevity video", parent_id: "health", order_index: 0 }),
    ];
    const tree = buildBacklinkTree("cars", buildBacklinkSourceIndex(blocks));
    expect(tree.map((node) => node.block.id)).toEqual(["cars"]);
  });

  it("includes tab-indented children until the next same-level block", () => {
    const blocks = [
      block({ id: "cars", content: "[[Cars/Offroad]]", order_index: 0 }),
      block({ id: "ads", content: "Ads", parent_id: "cars", order_index: 0 }),
      block({ id: "url", content: "carfax", parent_id: "ads", order_index: 0 }),
      block({ id: "health", content: "[[Health]]", order_index: 1 }),
    ];
    const tree = buildBacklinkTree("cars", buildBacklinkSourceIndex(blocks));
    expect(tree.map((node) => node.block.id)).toEqual(["cars", "ads", "url"]);
    expect(tree.map((node) => node.depth)).toEqual([0, 1, 2]);
  });

  it("stops a nested page link at its same-level sibling", () => {
    const blocks = [
      block({ id: "day", content: "Journal", order_index: 0 }),
      block({ id: "cars", content: "[[Cars/Offroad]]", parent_id: "day", order_index: 0 }),
      block({ id: "ads", content: "Ads", parent_id: "cars", order_index: 0 }),
      block({ id: "health", content: "[[Health]]", parent_id: "day", order_index: 1 }),
    ];
    const tree = buildBacklinkTree("cars", buildBacklinkSourceIndex(blocks));
    expect(tree.map((node) => node.block.id)).toEqual(["cars", "ads"]);
  });
});

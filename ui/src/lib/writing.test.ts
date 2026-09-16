import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Block } from "./api";
import { getGraphInfo, getPage, listBlocks } from "./api";
import { registerEditorFlush } from "./editorPersistence";
import { captureWritingTarget, isWritingBlock, writingChanges } from "./writing";

vi.mock("./api", () => ({
  getGraphInfo: vi.fn(), getPage: vi.fn(), listBlocks: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const block = (id: string, content = "Original prose"): Block => ({
  id, page_id: "day-2", parent_id: null, order_index: 0, content,
  block_type: "Text", properties: {}, created_at: "0", updated_at: "0",
});

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(getGraphInfo).mockResolvedValue({ name: "Synthetic", path: "/tmp/synthetic-graph" });
  vi.mocked(getPage).mockResolvedValue({
    id: "day-2", title: "2026-09-14", is_journal: true, properties: {},
    file_path: "journals/2026-09-14.md", created_at: "0", updated_at: "0",
  });
  vi.mocked(listBlocks).mockResolvedValue([block("one"), block("child"), { ...block("query"), block_type: "Query" }]);
});

describe("writing scope and snapshots", () => {
  it("uses only the focused journal day rather than the first visible day", async () => {
    const target = await captureWritingTarget({
      pageId: "day-1", anchor: { pageId: "day-2", blockId: "one" }, preferFocusedPageForPageScope: true,
    }, "page");
    expect(listBlocks).toHaveBeenCalledWith("day-2");
    expect(target.blocks.map(({ id }) => id)).toEqual(["one", "child"]);
    expect(target.snapshot).toHaveLength(3);
    expect(target.skippedBlocks).toBe(1);
  });
  it("block scope never includes children", async () => {
    const target = await captureWritingTarget({
      pageId: "day-2", anchor: { pageId: "day-2", blockId: "one" }, preferFocusedPageForPageScope: false,
    }, "block");
    expect(target.blocks.map(({ id }) => id)).toEqual(["one"]);
    expect(target.snapshot).toHaveLength(3);
  });
  it("does not reuse a stale anchor from another page outside the journal", async () => {
    await expect(captureWritingTarget({
      pageId: "day-1", anchor: { pageId: "day-2", blockId: "one" }, preferFocusedPageForPageScope: false,
    }, "block")).rejects.toThrow("Click a block");
    expect(listBlocks).not.toHaveBeenCalled();
  });
  it("flushes the editor before capturing persisted content", async () => {
    const flush = vi.fn(async () => {
      vi.mocked(listBlocks).mockResolvedValue([block("one", "Just typed")]);
    });
    const unregister = registerEditorFlush("day-2", flush);
    try {
      const target = await captureWritingTarget({
        pageId: "day-2", anchor: null, preferFocusedPageForPageScope: false,
      }, "page");
      expect(target.blocks[0].content).toBe("Just typed");
      expect(flush).toHaveBeenCalledOnce();
    } finally {
      unregister();
    }
  });
  it("surfaces save failures rather than assessing old text", async () => {
    const unregister = registerEditorFlush("day-2", async () => { throw new Error("Save failed"); });
    try {
      await expect(captureWritingTarget({
        pageId: "day-2", anchor: null, preferFocusedPageForPageScope: false,
      }, "page")).rejects.toThrow("Save failed");
      expect(listBlocks).not.toHaveBeenCalled();
    } finally {
      unregister();
    }
  });
  it("rejects graph switches during capture", async () => {
    vi.mocked(getGraphInfo).mockResolvedValueOnce({ name: "First", path: "/tmp/first" })
      .mockResolvedValueOnce({ name: "Other", path: "/tmp/other" });
    await expect(captureWritingTarget({
      pageId: "day-2", anchor: null, preferFocusedPageForPageScope: false,
    }, "page")).rejects.toThrow("graph changed");
  });
  it("excludes code, queries, media and empty blocks", () => {
    expect(isWritingBlock(block("code", "```js\ncode();\n```"))).toBe(false);
    expect(isWritingBlock(block("query", "{{query SELECT 1}}"))).toBe(false);
    expect(isWritingBlock({ ...block("audio"), block_type: "Audio" })).toBe(false);
    expect(isWritingBlock(block("empty", " "))).toBe(false);
  });
  it("includes scientific text without changing its content during capture", async () => {
    const scientific = block("one", "At 37 C, the concentration was 2.5 mg/L (p < 0.01) [4].");
    vi.mocked(listBlocks).mockResolvedValue([scientific]);
    const target = await captureWritingTarget({
      pageId: "day-2", anchor: null, preferFocusedPageForPageScope: false,
    }, "page");
    expect(target.blocks).toEqual([{ id: scientific.id, content: scientific.content }]);
  });
  it("describes an unsupported scope as having no eligible text", async () => {
    vi.mocked(listBlocks).mockResolvedValue([block("code", "```js\ncode();\n```")]);
    await expect(captureWritingTarget({
      pageId: "day-2", anchor: null, preferFocusedPageForPageScope: false,
    }, "page")).rejects.toThrow("No eligible text to analyze.");
  });
});

describe("complete rewrite validation", () => {
  const original = [{ id: "one", content: "Before" }, { id: "two", content: "Unchanged" }];
  it("records only changes, preserving IDs and exact original contents", () => {
    expect(writingChanges(original, [{ id: "one", content: "After" }, original[1]])).toEqual([
      { blockId: "one", beforeContent: "Before", afterContent: "After" },
    ]);
    expect(writingChanges(original, original)).toEqual([]);
  });
  it("rejects incomplete, duplicate, reordered and empty replacements", () => {
    for (const result of [original.slice(0, 1), [original[0], original[0]], [...original].reverse(),
      [original[0], { id: "two", content: "" }]]) {
      expect(() => writingChanges(original, result)).toThrow();
    }
  });
});

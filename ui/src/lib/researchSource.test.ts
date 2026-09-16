import { beforeEach, describe, expect, it, vi } from "vitest";
import { getGraphInfo, getPage, listBlocks, type Block } from "./api";
import { registerEditorFlush } from "./editorPersistence";
import { assertResearchSourceGraph, captureResearchSource } from "./researchSource";

vi.mock("./api", () => ({ getGraphInfo: vi.fn(), getPage: vi.fn(), listBlocks: vi.fn() }));

const block: Block = {
  id: "paragraph", page_id: "source", parent_id: null, order_index: 0,
  content: "A synthetic source.", block_type: "Text", properties: {},
  created_at: "0", updated_at: "0",
};

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(getGraphInfo).mockResolvedValue({ name: "Fixture", path: "/synthetic/research" });
  vi.mocked(getPage).mockResolvedValue({
    id: "source", title: "Source title", is_journal: false, properties: {},
    file_path: "pages/source.md", created_at: "0", updated_at: "0",
  });
  vi.mocked(listBlocks).mockResolvedValue([block]);
});

describe("research source snapshots", () => {
  it("flushes drafts before reading the source and pins its graph/page/anchor", async () => {
    const unregister = registerEditorFlush("source", async () => {
      vi.mocked(listBlocks).mockResolvedValue([{ ...block, content: "Saved draft." }]);
    });
    try {
      const source = await captureResearchSource("source", "paragraph");
      expect(source).toMatchObject({
        pageId: "source", pageTitle: "Source title", graphPath: "/synthetic/research", afterBlockId: "paragraph",
      });
      expect(source.snapshot[0].content).toBe("Saved draft.");
    } finally { unregister(); }
  });

  it("does not place a result after a block from another page", async () => {
    expect((await captureResearchSource("source", "foreign")).afterBlockId).toBeNull();
  });

  it("does not continue with stale text when saving fails", async () => {
    const unregister = registerEditorFlush("source", async () => { throw new Error("Save failed"); });
    try {
      await expect(captureResearchSource("source", null)).rejects.toThrow("Save failed");
      expect(listBlocks).not.toHaveBeenCalled();
    } finally { unregister(); }
  });

  it("rejects graph switches while capturing or before insertion", async () => {
    const source = await captureResearchSource("source", null);
    vi.mocked(getGraphInfo).mockResolvedValue({ name: "Other", path: "/synthetic/other" });
    await expect(assertResearchSourceGraph(source)).rejects.toThrow("different graph");
    vi.mocked(getGraphInfo).mockResolvedValueOnce({ name: "First", path: "/synthetic/first" });
    await expect(captureResearchSource("source", null)).rejects.toThrow("graph changed");
  });

  it("requires an explicit source, including for journals", async () => {
    await expect(captureResearchSource("", null)).rejects.toThrow("select a journal day");
    expect(getGraphInfo).not.toHaveBeenCalled();
  });
});

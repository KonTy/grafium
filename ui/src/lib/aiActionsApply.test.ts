import { describe, expect, it, vi } from "vitest";
import { applyEditPlan, summarizeApplyResult, type ApplyDeps } from "./aiActionsApply";
import type { EditAction } from "./aiActions";
import type { Block, Page } from "./api";
import type { UndoAction } from "./undoStack";

function page(title: string, id = `id-${title}`): Page {
  return { id, title, is_journal: /^\d{4}-\d{2}-\d{2}$/.test(title), created_at: "0", updated_at: "0", properties: {} } as Page;
}

function block(id: string, content: string, orderIndex: number, parentId: string | null = null): Block {
  return { id, page_id: "p", parent_id: parentId, order_index: orderIndex, content, block_type: "text", properties: {} } as Block;
}

function action(overrides: Partial<EditAction> = {}): EditAction {
  return { type: "append_to_journal", title: "Notes", content: "", source: "text", tags: [], date: "2026-09-17", ...overrides };
}

/** Fake backend: `existingPages` decides what a title lookup finds. */
function deps(existingPages: string[] = [], existingBlocks: Block[] = []) {
  const undos: UndoAction[] = [];
  const created: { pageId: string; blocks: any[] }[] = [];
  const pagesCreated: string[] = [];
  const writes: any[] = [];
  const impl: ApplyDeps = {
    getPage: vi.fn(async ({ title }) => {
      if (title && existingPages.includes(title)) return page(title);
      throw new Error("PageNotFound");
    }),
    createPage: vi.fn(async (title: string) => {
      pagesCreated.push(title);
      existingPages.push(title);
      return page(title);
    }),
    listBlocks: vi.fn(async () => existingBlocks),
    createBlocks: vi.fn(async (pageId: string, blocks: any[]) => {
      created.push({ pageId, blocks });
      return blocks.map((item, index) => block(`new-${index}`, item.content, item.orderIndex));
    }),
    applyWritingChanges: vi.fn(async (...args: any[]) => {
      writes.push(args);
    }),
    pushUndo: vi.fn((undoAction: UndoAction) => {
      undos.push(undoAction);
    }),
  };
  return { impl, undos, created, pagesCreated, writes };
}

describe("applying an edit plan", () => {
  it("creates today's journal page when it does not exist yet and marks it as a journal", async () => {
    const fake = deps([]);
    await applyEditPlan({ actions: [action({ content: "Body" })] }, { deps: fake.impl });
    expect(fake.pagesCreated).toEqual(["2026-09-17"]);
    expect(fake.impl.createPage).toHaveBeenCalledWith("2026-09-17", true);
  });

  it("reuses an existing journal page rather than creating a duplicate", async () => {
    const fake = deps(["2026-09-17"]);
    const result = await applyEditPlan({ actions: [action({ content: "Body" })] }, { deps: fake.impl });
    expect(fake.pagesCreated).toEqual([]);
    expect(result.applied[0].pageCreated).toBe(false);
  });

  it("appends after existing notes instead of overwriting the top of the page", async () => {
    const existing = [block("a", "old", 0), block("b", "older", 4)];
    const fake = deps(["2026-09-17"], existing);
    await applyEditPlan({ actions: [action({ content: "Body" })] }, { deps: fake.impl });
    expect(fake.created[0].blocks[0].orderIndex).toBe(5);
  });

  it("ignores child blocks when computing the append position", async () => {
    const existing = [block("a", "root", 0), block("child", "nested", 9, "a")];
    const fake = deps(["2026-09-17"], existing);
    await applyEditPlan({ actions: [action({ content: "Body" })] }, { deps: fake.impl });
    expect(fake.created[0].blocks[0].orderIndex).toBe(1);
  });

  it("files the entry under its tags and nests the body beneath it", async () => {
    const fake = deps(["2026-09-17"]);
    await applyEditPlan(
      { actions: [action({ title: "Supplements", tags: ["health/supplements"], content: "- one\n- two" })] },
      { deps: fake.impl },
    );
    const [parent, ...children] = fake.created[0].blocks;
    expect(parent.content).toBe("Supplements [[health/supplements]]");
    expect(children.map((c: any) => c.content)).toEqual(["one", "two"]);
    expect(children.every((c: any) => c.parentIndex === 0)).toBe(true);
  });

  it("pushes an undo entry for every page it wrote to", async () => {
    const fake = deps(["2026-09-17", "Supplements"]);
    const result = await applyEditPlan(
      {
        actions: [
          action({ content: "Body" }),
          action({ type: "append_to_page", page: "Supplements", content: "Body" }),
        ],
      },
      { deps: fake.impl },
    );
    expect(fake.undos).toHaveLength(2);
    expect(fake.undos[0].type).toBe("insert_blocks");
    expect(result.undoSteps).toBe(2);
  });

  it("creates a named page and queues a link scan when asked to find links", async () => {
    const fake = deps([]);
    const result = await applyEditPlan(
      { actions: [action({ type: "create_page", page: "Supplements", content: "Body", findLinks: true })] },
      { deps: fake.impl },
    );
    expect(fake.impl.createPage).toHaveBeenCalledWith("Supplements", false);
    expect(result.findLinks).toEqual([{ pageId: "id-Supplements", title: "Supplements" }]);
  });

  it("tags a page by appending real wiki links so it shows up in backlinks", async () => {
    const fake = deps(["Supplements"]);
    await applyEditPlan(
      { actions: [action({ type: "add_tags", page: "Supplements", tags: ["health", "longevity"] })] },
      { deps: fake.impl },
    );
    expect(fake.created[0].blocks[0].content).toBe("[[health]] [[longevity]]");
  });

  it("routes a block replacement through the snapshot-checked writing path", async () => {
    const fake = deps([]);
    const snapshot = [block("b1", "original", 0)];
    await applyEditPlan(
      { actions: [action({ type: "replace_block", content: "replacement" })] },
      {
        deps: fake.impl,
        blockTarget: { graphPath: "/graph", pageId: "p1", blockId: "b1", content: "original", snapshot },
      },
    );
    expect(fake.writes[0]).toEqual([
      "/graph",
      "p1",
      [{ blockId: "b1", beforeContent: "original", afterContent: "replacement" }],
      snapshot,
    ]);
    expect(fake.undos[0].type).toBe("rewrite_writing");
  });

  it("refuses to replace a block when the conversation is not attached to one", async () => {
    const fake = deps([]);
    const result = await applyEditPlan(
      { actions: [action({ type: "replace_block", content: "replacement" })] },
      { deps: fake.impl },
    );
    expect(result.errors[0]).toContain("not attached to a block");
    expect(fake.writes).toHaveLength(0);
  });

  it("keeps the work that succeeded when a later action fails", async () => {
    const fake = deps(["2026-09-17"]);
    const result = await applyEditPlan(
      {
        actions: [
          action({ content: "Body" }),
          action({ type: "replace_block", content: "replacement" }),
        ],
      },
      { deps: fake.impl },
    );
    expect(result.applied).toHaveLength(1);
    expect(result.errors).toHaveLength(1);
    expect(result.undoSteps).toBe(1);
  });
});

describe("summarising what was applied", () => {
  it("distinguishes a created page from an appended one", async () => {
    const fake = deps([]);
    const result = await applyEditPlan(
      { actions: [action({ type: "create_page", page: "Supplements", content: "Body" })] },
      { deps: fake.impl },
    );
    expect(summarizeApplyResult(result)).toBe("Created Supplements. Ctrl+Z undoes this.");
  });

  it("says how many undo steps a multi-page plan takes", async () => {
    const fake = deps(["2026-09-17", "Supplements"]);
    const result = await applyEditPlan(
      {
        actions: [
          action({ content: "Body" }),
          action({ type: "append_to_page", page: "Supplements", content: "Body" }),
        ],
      },
      { deps: fake.impl },
    );
    expect(summarizeApplyResult(result)).toContain("Ctrl+Z undoes this (2 steps).");
  });

  it("reports failures rather than claiming success", async () => {
    const fake = deps([]);
    const result = await applyEditPlan(
      { actions: [action({ type: "replace_block", content: "x" })] },
      { deps: fake.impl },
    );
    expect(summarizeApplyResult(result)).toContain("Failed:");
  });
});

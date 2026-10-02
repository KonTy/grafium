import { beforeEach, describe, expect, it, vi } from "vitest";
const mocks = vi.hoisted(() => ({ invoke: vi.fn(), tasks: vi.fn(), topics: vi.fn(), rewrite: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("./assistantTaskGrouping", () => ({ groupAssistantTasks: mocks.tasks }));
vi.mock("./assistantNoteWorkflows", () => ({ buildRelatedTopicDraft: mocks.topics, buildCleanedNoteDraft: mocks.rewrite }));
import { applyAssistantWorkflow, detectAssistantWorkflow, prepareAssistantWorkflow, workflowPreviewLine, type WorkflowSnapshot } from "./assistantWorkflows";

const id = "12345678-1234-4234-8234-123456789abc";
const pageId = "12345678-1234-4234-8234-123456789abd";
const snapshot: WorkflowSnapshot = { token: "token", graphPath: "/fixture", kind: "tasks",
  sourcePageId: pageId, sourcePageTitle: "Rough notes", coverage: "1 open task.",
  pages: [{ id: pageId, title: "Rough notes", blocks: [{ id, content: "My original draft", parentId: null, orderIndex: 0 }] }],
  tasks: [{ id, pageId, pageTitle: "Rough notes", content: "TODO arrange a visit", state: "TODO" }] };
const options = () => ({ signal: new AbortController().signal, onProgress: vi.fn() });
beforeEach(() => {
  vi.resetAllMocks();
  for (const mock of [mocks.tasks, mocks.topics, mocks.rewrite]) mock.mockResolvedValue({ title: "Review", content: `## Group\n- ((${id}))`, summary: "All supplied tasks considered." });
  mocks.invoke.mockResolvedValue(snapshot);
});

describe("ASK workflow routing", () => {
  it.each([
    ["Can you go through all TODOs and group them by similarity?", "tasks"],
    ["please find possible duplicate tasks", "tasks"],
    ["Scan all notes and find similar topics to this page and create a reference page.", "topics"],
    ["Clean up these rough notes and put a new draft below mine.", "rewrite"],
    ["Can you rewrite this page?", "rewrite"],
    ["How can I group my tasks?", null],
    ["What notes cover similar topics?", null],
    ["Tell me how to clean up a page", null],
    ["What did I say yesterday?", null],
  ])("routes %s to %s", (text, kind) => expect(detectAssistantWorkflow(text)).toBe(kind));

  it("prepares all-task analysis without invoking a write or implicit chat retrieval", async () => {
    const proposal = await prepareAssistantWorkflow("tasks", "/fixture", { kind: "none" }, null, "Group tasks", options());
    expect(mocks.invoke).toHaveBeenCalledOnce();
    expect(mocks.invoke).toHaveBeenCalledWith("assistant_workflow_snapshot", { graphPath: "/fixture", kind: "tasks" });
    expect(mocks.tasks).toHaveBeenCalledWith(snapshot.tasks, "Group tasks", expect.objectContaining({ signal: expect.any(AbortSignal) }));
    expect(proposal.snapshot.token).toBe("token");
    expect(proposal.content).toContain(`((${id}))`);
    expect(proposal.summary).toContain("1 open task");
  });

  it("uses no-RAG completion and propagates cancellation without accepting its late result", async () => {
    const controller = new AbortController();
    let finish!: (value: string) => void;
    mocks.tasks.mockImplementation(async (_tasks, _request, settings) => {
      await settings.complete("bounded task data");
      return { title: "Late", content: "must not appear", summary: "" };
    });
    mocks.invoke.mockImplementation(async command => {
      if (command === "assistant_workflow_snapshot") return snapshot;
      if (command === "assistant_workflow_complete") return new Promise<string>(resolve => { finish = resolve; });
    });
    const pending = prepareAssistantWorkflow("tasks", "/fixture", { kind: "graph" }, null, "Group tasks",
      { signal: controller.signal, onProgress: vi.fn() });
    await vi.waitFor(() => expect(finish).toBeDefined());
    controller.abort(); finish("ignored");
    await expect(pending).rejects.toThrow("cancelled");
    expect(mocks.invoke).toHaveBeenCalledWith("assistant_workflow_cancel", { requestId: expect.any(String) });
    expect(mocks.invoke.mock.calls.some(([command]) => command === "assistant_workflow_apply" || command === "ai_ask")).toBe(false);
  });

  it("keeps selected text and captured block IDs rather than rewriting unrelated blocks", async () => {
    mocks.invoke.mockResolvedValue({ ...snapshot, kind: "rewrite" });
    await prepareAssistantWorkflow("rewrite", "/fixture", { kind: "selection", pageId, selection: { blockIds: [id], text: "original draft" } },
      pageId, "Clean this selection", options());
    expect(mocks.invoke).toHaveBeenCalledWith("assistant_workflow_snapshot", { graphPath: "/fixture", kind: "rewrite", pageId, blockIds: [id] });
    expect(mocks.rewrite).toHaveBeenCalledWith(snapshot.pages[0], "Clean this selection", expect.any(Object), "original draft");
  });

  it("refuses ambiguous graph-wide rewrites and changed-graph snapshots", async () => {
    await expect(prepareAssistantWorkflow("rewrite", "/fixture", { kind: "graph" }, pageId, "Clean notes", options())).rejects.toThrow("Choose Selection");
    expect(mocks.invoke).not.toHaveBeenCalled();
    mocks.invoke.mockResolvedValue({ ...snapshot, graphPath: "/different" });
    await expect(prepareAssistantWorkflow("tasks", "/fixture", { kind: "graph" }, null, "Group tasks", options())).rejects.toThrow("graph or action");
    expect(mocks.tasks).not.toHaveBeenCalled();
  });
  it.each(["block", "section"] as const)("includes descendants for %s context", async kind => {
    mocks.invoke.mockResolvedValue({ ...snapshot, kind: "rewrite" });
    await prepareAssistantWorkflow("rewrite", "/fixture", { kind, pageId, blockId: id }, pageId, "Clean this draft", options());
    expect(mocks.invoke).toHaveBeenCalledWith("assistant_workflow_snapshot", {
      graphPath: "/fixture", kind: "rewrite", pageId, blockIds: [id], includeDescendants: true,
    });
  });

  it("applies only reviewed text with the graph-bound snapshot token", async () => {
    const proposal = await prepareAssistantWorkflow("tasks", "/fixture", { kind: "graph" }, null, "Group tasks", options());
    proposal.title = "My review";
    proposal.content = `## Planning\n\n- ((${id}))`;
    mocks.invoke.mockClear();
    await applyAssistantWorkflow(proposal);
    expect(mocks.invoke).toHaveBeenCalledOnce();
    expect(mocks.invoke).toHaveBeenCalledWith("assistant_workflow_apply", {
      graphPath: "/fixture", token: "token", title: "My review",
      reviewedMarkdown: `## Planning\n\n- ((${id}))`,
      blocks: [{ content: "AI review" }],
    });
  });

  it("makes references readable without interpreting source text as HTML", () => {
    expect(workflowPreviewLine(`- ((${id}))`, snapshot)).toEqual([
      { text: "- " }, { text: "TODO arrange a visit", source: { id: pageId, title: "Rough notes", blockId: id } },
    ]);
    expect(workflowPreviewLine("<img src=https://not-contacted.test>", snapshot)).toEqual([{ text: "<img src=https://not-contacted.test>" }]);
  });
  it("passes reviewed Markdown intact for native hierarchy preservation and task quoting", async () => {
    await applyAssistantWorkflow({ snapshot: { ...snapshot, kind: "rewrite" }, request: "Clean my notes",
      title: "Suggested rewrite", content: "TODO call the clinic\n\n- [ ] Book a visit\n  - Ask for directions", summary: "" });
    expect(mocks.invoke).toHaveBeenCalledWith("assistant_workflow_apply", expect.objectContaining({
      reviewedMarkdown: "TODO call the clinic\n\n- [ ] Book a visit\n  - Ask for directions",
      blocks: [{ content: "Suggested rewrite" }],
    }));
  });
});

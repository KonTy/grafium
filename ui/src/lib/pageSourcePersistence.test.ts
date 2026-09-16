import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { loadPageSourceSession } from "./pageSourcePersistence";
import { updatePageSource } from "./api";
import editor from "../components/UnifiedPageEditor.svelte?raw";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

function deferred<T = void>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

let graphPath: string;
let disk: string;
let writes: { pageId: string; content: string; expectedSource: string; graphPath: string }[];
let writeBarrier: ReturnType<typeof deferred> | undefined;

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  graphPath = "/graph-original";
  disk = "- Loaded source.\r\n";
  writes = [];
  writeBarrier = undefined;
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "get_graph_info") return { path: graphPath };
    if (command === "get_page_source") return disk;
    if (command !== "update_page_source") throw new Error(`Unexpected IPC: ${command}`);
    const request = args as typeof writes[number];
    writes.push(request);
    if (writeBarrier) await writeBarrier.promise;
    if (request.graphPath !== graphPath) throw new Error("Graph changed");
    if (request.expectedSource !== disk) throw new Error("Source changed on disk");
    disk = request.content;
  });
});

describe("guarded continuous source persistence", () => {
  it("requires and forwards the loaded source and graph in the existing IPC", async () => {
    await updatePageSource("page", "edited", { expectedSource: disk, graphPath });
    expect(invoke).toHaveBeenCalledWith("update_page_source", {
      pageId: "page", content: "edited", expectedSource: "- Loaded source.\r\n", graphPath,
    });
  });

  it("verifies the graph before and after loading, retaining exact raw bytes as the first save base", async () => {
    const session = await loadPageSourceSession("page");
    expect(vi.mocked(invoke).mock.calls.map(([command]) => command))
      .toEqual(["get_graph_info", "get_page_source", "get_graph_info"]);
    session.content = "- Edited text.\n";
    await session.save();
    expect(writes[0]).toEqual({
      pageId: "page", content: "- Edited text.\n", expectedSource: "- Loaded source.\r\n", graphPath,
    });
    expect(session.expectedSource).toBe("- Edited text.\n");
    expect(session.dirty).toBe(false);
  });

  it("does not mistake CodeMirror newline normalization for an unsaved edit", async () => {
    const session = await loadPageSourceSession("page");
    session.content = "- Loaded source.\n";
    expect(session.dirty).toBe(false);
    expect(session.expectedSource).toBe("- Loaded source.\r\n");
  });

  it("serializes queued snapshots against the last successful write and retains newer typing", async () => {
    const session = await loadPageSourceSession("page");
    writeBarrier = deferred();
    session.content = "- First edit.\n";
    const first = session.save();
    await vi.waitFor(() => expect(writes).toHaveLength(1));
    session.content = "- Second edit.\n";
    const second = session.save();
    session.content = "- Typed while both saves are queued.\n";
    expect(writes).toHaveLength(1);
    expect(session.expectedSource).toBe("- Loaded source.\r\n");
    writeBarrier.resolve(undefined);
    await Promise.all([first, second]);
    expect(writes.map(({ expectedSource, content }) => ({ expectedSource, content }))).toEqual([
      { expectedSource: "- Loaded source.\r\n", content: "- First edit.\n" },
      { expectedSource: "- First edit.\n", content: "- Second edit.\n" },
    ]);
    expect(session.content).toBe("- Typed while both saves are queued.\n");
    expect(session.expectedSource).toBe("- Second edit.\n");
    expect(session.dirty).toBe(true);
    expect(session.pending).toBe(0);
  });

  it("rejects stale saves without publishing over a newly added footnote or adopting its revision", async () => {
    const session = await loadPageSourceSession("page");
    const loaded = disk;
    disk += "\n<!-- newly saved footnote -->\n";
    const external = disk;
    session.content = "- My unsaved source edit.\n";
    await expect(session.save()).rejects.toThrow("Source changed on disk");
    expect(disk).toBe(external);
    expect(session.expectedSource).toBe(loaded);
    expect(session.content).toBe("- My unsaved source edit.\n");
    expect(session.dirty).toBe(true);
    await expect(session.save()).rejects.toThrow("Source changed on disk");
    expect(writes.every((request) => request.expectedSource === loaded)).toBe(true);
  });

  it("keeps the old base after an unsuccessful write so an explicit queued retry can succeed", async () => {
    const session = await loadPageSourceSession("page");
    writeBarrier = deferred();
    session.content = "- First attempt.\n";
    const first = session.save();
    const rejected = expect(first).rejects.toThrow("Disk unavailable");
    await vi.waitFor(() => expect(writes).toHaveLength(1));
    session.content = "- Preserved newer draft.\n";
    const retry = session.save();
    const barrier = writeBarrier;
    writeBarrier = undefined;
    barrier.reject(new Error("Disk unavailable"));
    await rejected;
    await retry;
    expect(writes[1].expectedSource).toBe("- Loaded source.\r\n");
    expect(session.content).toBe("- Preserved newer draft.\n");
    expect(session.expectedSource).toBe(session.content);
    expect(session.dirty).toBe(false);
  });

  it("prevents an old-root save before IPC and also retains the buffer when the native graph guard rejects", async () => {
    const session = await loadPageSourceSession("page");
    session.content = "- Original graph draft.\n";
    graphPath = "/graph-other";
    await expect(session.save()).rejects.toThrow("graph changed");
    expect(writes).toEqual([]);
    graphPath = session.graphPath;
    writeBarrier = deferred();
    const pending = session.save();
    const rejected = expect(pending).rejects.toThrow("Graph changed");
    await vi.waitFor(() => expect(writes).toHaveLength(1));
    graphPath = "/graph-other";
    writeBarrier.resolve(undefined);
    await rejected;
    expect(session.expectedSource).toBe("- Loaded source.\r\n");
    expect(session.content).toBe("- Original graph draft.\n");
    expect(session.dirty).toBe(true);
  });

  it("refuses a load crossing graph boundaries or reloading an existing editor into a different graph", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "get_graph_info") return { path: graphPath };
      if (command === "get_page_source") { graphPath = "/graph-other"; return disk; }
      throw new Error("Unexpected write");
    });
    await expect(loadPageSourceSession("page")).rejects.toThrow("graph changed while loading");
    vi.mocked(invoke).mockClear();
    await expect(loadPageSourceSession("page", "/graph-original")).rejects.toThrow("graph changed");
    expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).toEqual(["get_graph_info"]);
  });

  it("cannot load or save without a concrete graph identity", async () => {
    graphPath = "";
    await expect(loadPageSourceSession("page")).rejects.toThrow("graph changed");
    expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).toEqual(["get_graph_info"]);
    expect(writes).toEqual([]);
  });

  it("uses a refreshed source as the next base and invalidates queued writes from the replaced editor", async () => {
    const previous = await loadPageSourceSession("page");
    previous.content = "- Old draft.\n";
    const pending = previous.save();
    previous.invalidate();
    await expect(pending).rejects.toThrow("source editor changed");
    expect(writes).toEqual([]);
    disk += "\n[^grafium-note-1]: Native note.\n";
    const refreshed = await loadPageSourceSession("page", graphPath);
    const withNote = disk;
    refreshed.content = withNote.replace("Loaded", "Edited");
    await refreshed.save();
    expect(writes[0].expectedSource).toBe(withNote);
    expect(disk).toContain("[^grafium-note-1]: Native note.");
  });

  it("routes selection flushes through the same queue and refuses automatic dirty-buffer replacement", () => {
    expect(editor).not.toContain("updatePageSource(");
    expect(editor).toContain("if (!await saveSource()) throw");
    expect(editor).toContain("if (saving || (dirty && !discardDraft))");
    expect(editor).toContain("previousView?.state.doc !== previousDoc");
    expect(editor).toContain("sourceReady = untrack(() => loadSource(pageId))");
    expect(editor).toContain("sourceReady = loadSource(page.id, false, detail)");
    expect(editor).toContain("dirty = session.dirty");
  });
});

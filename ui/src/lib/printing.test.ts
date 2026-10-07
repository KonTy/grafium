import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { Block } from "./api";
import {
  buildRequestedDocument,
  focusedBlockId,
  loadPrintSource,
  mountPrintDocument,
  printHeadings,
  requestedBlocks,
  selectedBlockIds,
  printingAvailable,
  sendToPrinter,
  systemDialogSavesPdf,
  unmountPrintDocument,
  type PrintSource,
} from "./printing";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const store = vi.hoisted(() => ({
  getPage: vi.fn(),
  listBlocks: vi.fn(),
  flushAllPageEditors: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("./api", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  getPage: store.getPage,
  listBlocks: store.listBlocks,
}));
vi.mock("./editorPersistence", () => ({ flushAllPageEditors: store.flushAllPageEditors }));

function mk(id: string, content: string, parent_id: string | null = null, order_index = 0): Block {
  return {
    id,
    page_id: "p",
    parent_id,
    order_index,
    content,
    block_type: "text",
    properties: {},
    created_at: "0",
    updated_at: "0",
  };
}

function source(overrides: Partial<PrintSource> = {}): PrintSource {
  return {
    pageId: "p",
    pageTitle: "My notes",
    blocks: [mk("h", "# Chapter One"), mk("a", "Body text.", null, 1)],
    assetBaseDir: "",
    chapters: [{ id: "h", level: 1, text: "Chapter One" }],
    suggestedChapterId: "h",
    selectedIds: [],
    ...overrides,
  };
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("reading the user's selection", () => {
  it("takes the ids of the selected blocks from the page", () => {
    document.body.innerHTML = `
      <div class="block-item selected" data-block-id="one"></div>
      <div class="block-item" data-block-id="two"></div>
      <div class="block-item selected" data-block-id="three"></div>`;
    expect(selectedBlockIds()).toEqual(["one", "three"]);
  });

  it("finds nothing when the user has selected nothing", () => {
    document.body.innerHTML = '<div class="block-item" data-block-id="one"></div>';
    expect(selectedBlockIds()).toEqual([]);
  });

  it("finds the block being edited, to guess the chapter", () => {
    document.body.innerHTML = `
      <div class="block-item" data-block-id="one"></div>
      <div class="block-item editing" data-block-id="two"></div>`;
    expect(focusedBlockId()).toBe("two");
  });

  it("has no focused block when nothing is being edited", () => {
    document.body.innerHTML = '<div class="block-item" data-block-id="one"></div>';
    expect(focusedBlockId()).toBeNull();
  });
});

describe("printHeadings", () => {
  it("titles a whole-page print with the page title", () => {
    const { title, subtitle } = printHeadings(source(), { scope: "page", chapterId: null });
    expect(title).toBe("My notes");
    expect(subtitle).toMatch(/^Printed /);
  });

  it("titles a chapter print with the chapter, keeping the page for context", () => {
    const { title, subtitle } = printHeadings(source(), { scope: "chapter", chapterId: "h" });
    expect(title).toBe("Chapter One");
    expect(subtitle).toContain("My notes");
  });

  it("falls back to the page title when the chapter has gone", () => {
    const { title } = printHeadings(source(), { scope: "chapter", chapterId: "missing" });
    expect(title).toBe("My notes");
  });

  it("says a selection print is a selection", () => {
    const { subtitle } = printHeadings(source(), { scope: "selection", chapterId: null });
    expect(subtitle).toContain("Selected blocks");
  });
});

describe("buildRequestedDocument", () => {
  it("builds the whole page", () => {
    const html = buildRequestedDocument(source(), {
      pageId: "p",
      scope: "page",
      chapterId: null,
      colour: "colour",
    });
    expect(html).toContain("Chapter One");
    expect(html).toContain("Body text.");
  });

  it("builds only the chosen chapter", () => {
    const blocks = [
      mk("h", "# One"),
      mk("a", "First body.", null, 1),
      mk("h2", "# Two", null, 2),
      mk("b", "Second body.", null, 3),
    ];
    const chapters = [
      { id: "h", level: 1, text: "One" },
      { id: "h2", level: 1, text: "Two" },
    ];
    const html = buildRequestedDocument(source({ blocks, chapters }), {
      pageId: "p",
      scope: "chapter",
      chapterId: "h2",
      colour: "colour",
    });
    expect(html).toContain("Second body.");
    expect(html).not.toContain("First body.");
  });

  it("builds only the selected blocks", () => {
    const html = buildRequestedDocument(source({ selectedIds: ["a"] }), {
      pageId: "p",
      scope: "selection",
      chapterId: null,
      colour: "colour",
    });
    expect(html).toContain("Body text.");
    expect(html).not.toContain("Chapter One</h1>");
  });
});

describe("mounting the print document", () => {
  it("puts the document beside the app, not inside it, so the app can be hidden", () => {
    document.body.innerHTML = '<div id="app"></div>';
    mountPrintDocument("<p>Hello</p>", "colour");
    const root = document.getElementById("print-root");
    expect(root?.parentElement).toBe(document.body);
    expect(root?.innerHTML).toBe("<p>Hello</p>");
  });

  it("marks a black and white job so the stylesheet can drop the colour", () => {
    mountPrintDocument("<p>Hello</p>", "mono");
    expect(document.getElementById("print-root")?.className).toBe("print-mono");
  });

  it("clears the colour choice again for a later colour job", () => {
    mountPrintDocument("<p>Hello</p>", "mono");
    mountPrintDocument("<p>Hello</p>", "colour");
    expect(document.getElementById("print-root")?.className).toBe("");
  });

  it("leaves nothing behind after the job, so the next print starts clean", () => {
    mountPrintDocument("<p>Hello</p>", "mono");
    unmountPrintDocument();
    const root = document.getElementById("print-root");
    expect(root?.innerHTML).toBe("");
    expect(root?.className).toBe("");
  });

  it("reuses one container rather than stacking copies of the document", () => {
    mountPrintDocument("<p>One</p>", "colour");
    mountPrintDocument("<p>Two</p>", "colour");
    expect(document.querySelectorAll("#print-root")).toHaveLength(1);
  });
});

describe("sending the job", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("asks the backend for the system print dialog", async () => {
    vi.mocked(invoke).mockResolvedValue("printed");
    await expect(sendToPrinter("printer")).resolves.toBe("printed");
    expect(invoke).toHaveBeenCalledWith("print_document", { target: { kind: "dialog" } });
  });

  it("passes the chosen file through for a PDF", async () => {
    vi.mocked(invoke).mockResolvedValue("printed");
    await sendToPrinter("pdf", "/tmp/notes.pdf");
    expect(invoke).toHaveBeenCalledWith("print_document", {
      target: { kind: "pdf", path: "/tmp/notes.pdf" },
    });
  });

  it("reports the user cancelling at the printer, so the dialog stays open", async () => {
    vi.mocked(invoke).mockResolvedValue("cancelled");
    await expect(sendToPrinter("printer")).resolves.toBe("cancelled");
  });

  it("refuses a second job while one is still printing the shared document", async () => {
    let release = (_: string) => {};
    vi.mocked(invoke).mockReturnValue(
      new Promise<string>((resolve) => {
        release = resolve;
      }),
    );
    const first = sendToPrinter("printer");
    await expect(sendToPrinter("printer")).rejects.toThrow("already running");
    release("printed");
    await expect(first).resolves.toBe("printed");
  });

  it("frees the next job after a failure rather than wedging printing", async () => {
    vi.mocked(invoke).mockRejectedValueOnce(new Error("no printers"));
    await expect(sendToPrinter("printer")).rejects.toThrow("no printers");
    vi.mocked(invoke).mockResolvedValue("printed");
    await expect(sendToPrinter("printer")).resolves.toBe("printed");
  });
});

describe("requestedBlocks", () => {
  it("knows a chapter that no longer exists has nothing to print", () => {
    expect(
      requestedBlocks(source(), { pageId: "p", scope: "chapter", chapterId: "gone", colour: "colour" }),
    ).toEqual([]);
  });

  it("knows an empty selection has nothing to print", () => {
    expect(
      requestedBlocks(source(), { pageId: "p", scope: "selection", chapterId: null, colour: "colour" }),
    ).toEqual([]);
  });

  it("knows the whole page has something to print", () => {
    expect(
      requestedBlocks(source(), { pageId: "p", scope: "page", chapterId: null, colour: "colour" }),
    ).toHaveLength(2);
  });
});

describe("loadPrintSource", () => {
  beforeEach(() => {
    store.getPage.mockReset();
    store.listBlocks.mockReset();
    store.flushAllPageEditors.mockClear();
    store.getPage.mockResolvedValue({ id: "p", title: "Field notes", file_path: "pages/trip/day one.md" });
    store.listBlocks.mockResolvedValue([mk("h", "# Morning"), mk("a", "Rain.", "h")]);
  });

  it("asks for the page by id, not by title", async () => {
    await loadPrintSource("p");
    expect(store.getPage).toHaveBeenCalledWith({ id: "p" });
  });

  it("saves pending edits first, so you print what you can see", async () => {
    await loadPrintSource("p");
    expect(store.flushAllPageEditors).toHaveBeenCalled();
  });

  it("offers the page's headings as chapters", async () => {
    const loaded = await loadPrintSource("p");
    expect(loaded.chapters).toEqual([{ id: "h", level: 1, text: "Morning" }]);
    expect(loaded.pageTitle).toBe("Field notes");
  });

  it("resolves images against the page's own folder", async () => {
    const loaded = await loadPrintSource("p");
    expect(loaded.assetBaseDir).toBe("pages/trip");
  });

  it("names an untitled page rather than printing a blank heading", async () => {
    store.getPage.mockResolvedValue({ id: "p", title: "   ", file_path: null });
    expect((await loadPrintSource("p")).pageTitle).toBe("Untitled");
  });

  it("starts the chapter picker at the chapter being edited", async () => {
    document.body.innerHTML = '<div class="block-item editing" data-block-id="a"></div>';
    expect((await loadPrintSource("p")).suggestedChapterId).toBe("h");
  });
});

type Bridged = typeof window & {
  GrafiumPrintBridge?: { print: (job: string) => void; isAvailable: () => boolean };
  __GRAFIUM_PRINT_RESOLVE?: (started: boolean, message: string) => void;
};

describe("printing on Android", () => {
  const w = window as Bridged;

  afterEach(() => {
    delete w.GrafiumPrintBridge;
    delete w.__GRAFIUM_PRINT_RESOLVE;
  });

  function installBridge(onPrint: (job: string) => void, available = true) {
    w.GrafiumPrintBridge = { print: onPrint, isAvailable: () => available };
  }

  it("uses the platform print service instead of the unimplemented window.print()", async () => {
    vi.mocked(invoke).mockReset();
    const jobs: string[] = [];
    installBridge((job) => {
      jobs.push(job);
      w.__GRAFIUM_PRINT_RESOLVE?.(true, "");
    });
    await expect(sendToPrinter("printer", undefined, "Field notes")).resolves.toBe("printed");
    expect(jobs).toEqual(["Field notes"]);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("sends a PDF request to the same dialog, which offers Save as PDF itself", async () => {
    installBridge(() => w.__GRAFIUM_PRINT_RESOLVE?.(true, ""));
    await expect(sendToPrinter("pdf", undefined, "Notes")).resolves.toBe("printed");
  });

  it("surfaces a refusal from the platform rather than claiming it printed", async () => {
    installBridge(() => w.__GRAFIUM_PRINT_RESOLVE?.(false, "Printing is unavailable on this device"));
    await expect(sendToPrinter("printer")).rejects.toThrow("unavailable on this device");
  });

  it("frees the next job after the platform refuses", async () => {
    installBridge(() => w.__GRAFIUM_PRINT_RESOLVE?.(false, "nope"));
    await expect(sendToPrinter("printer")).rejects.toThrow();
    installBridge(() => w.__GRAFIUM_PRINT_RESOLVE?.(true, ""));
    await expect(sendToPrinter("printer")).resolves.toBe("printed");
  });

  it("stops listening once a job is answered, so a later job is not resolved twice", async () => {
    installBridge(() => w.__GRAFIUM_PRINT_RESOLVE?.(true, ""));
    await sendToPrinter("printer");
    expect(w.__GRAFIUM_PRINT_RESOLVE).toBeUndefined();
  });

  it("hides the separate PDF button only where the system dialog already saves one", () => {
    expect(systemDialogSavesPdf()).toBe(false);
    installBridge(() => {});
    expect(systemDialogSavesPdf()).toBe(true);
  });

  it("reports a device with no print service as unable to print", () => {
    expect(printingAvailable()).toBe(true);
    installBridge(() => {}, false);
    expect(printingAvailable()).toBe(false);
  });
});

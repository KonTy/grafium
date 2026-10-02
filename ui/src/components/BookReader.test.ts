import { afterEach, describe, expect, it, vi } from "vitest";
import { mount, unmount, flushSync } from "svelte";
import { get } from "svelte/store";
const ipc = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn(), changed: null as null | ((event: unknown) => void) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: ipc.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: ipc.listen }));
import BookReader from "./BookReader.svelte";
import { bookSelection, BOOK_RENDERER_VERSION, type BookInfo } from "../lib/books";
import { readingSelection } from "../lib/readingSelection";
import type { Page } from "../lib/api";

let mounted: ReturnType<typeof mount> | undefined;
afterEach(async () => {
  if (mounted) await unmount(mounted);
  mounted = undefined; document.body.replaceChildren(); bookSelection.set(null); vi.unstubAllGlobals();
});

describe("book reader host", () => {
  it("forwards trusted F1 to books help and invalidates changed source snapshots without publishing block selections", async () => {
    const book: BookInfo = { id: "book", pageId: "page", title: "Fixture", format: "epub", filePath: "books/original.epub",
      sourceSha256: "first", readingLocation: null, indexingWarning: null };
    let changed = false;
    let indexingWarning: string | null = "Text is not indexed yet.";
    ipc.invoke.mockImplementation(async command => {
      if (command === "book_open") return { ...book, indexingWarning, sourceSha256: changed ? "replaced" : "first" };
      if (command === "book_read_bytes") return new ArrayBuffer(4);
      if (command === "book_notes_list") return [];
      throw new Error(`Unexpected ${command}`);
    });
    ipc.listen.mockImplementation(async (_event, callback) => { ipc.changed = callback; return () => {}; });
    vi.stubGlobal("fetch", vi.fn(async () => ({ ok: true, text: async () => "/* trusted runtime fixture */" })));
    mounted = mount(BookReader, { target: document.body, props: {
      page: { id: "page", title: "Fixture", properties: {} } as Page, graphPath: "/graph",
    } });
    await vi.waitFor(() => expect(document.querySelector("iframe")).not.toBeNull());
    const frame = document.querySelector("iframe")!;
    const token = decodeURIComponent(frame.src).match(/const token="([^"]+)"/)![1];
    const previousMarkdownSelection = get(readingSelection);
    const send = (data: Record<string, unknown>) => {
      window.dispatchEvent(new MessageEvent("message", { source: frame.contentWindow, origin: "null",
        data: { channel: "grafium-book", token, ...data } }));
      flushSync();
    };
    send({ type: "ready", toc: [], annotations: true, notice: "Local book" });
    const help = vi.fn((event: KeyboardEvent) => {
      if (event.key === "F1") expect((event.target as Element).closest("[data-help-context]")?.getAttribute("data-help-context")).toBe("books");
    });
    window.addEventListener("keydown", help);
    send({ type: "help" });
    await vi.waitFor(() => expect(help).toHaveBeenCalledOnce());
    window.removeEventListener("keydown", help);
    send({ type: "selection", quote: "Passage", location: { kind: "epub", cfi: "epubcfi(/6/2)", rendererVersion: BOOK_RENDERER_VERSION } });
    expect(get(bookSelection)?.sourceSha256).toBe("first");
    expect(get(readingSelection)).toBe(previousMarkdownSelection);
    expect(document.body.textContent).toContain("Text is not indexed yet.");
    indexingWarning = null;
    ipc.changed?.({ payload: { graphPath: "/graph" } });
    await vi.waitFor(() => expect(document.body.textContent).not.toContain("Text is not indexed yet."));
    expect(get(bookSelection)?.sourceSha256).toBe("first");
    expect(document.querySelector<HTMLButtonElement>('[aria-label="Reading controls"] button')?.disabled).toBe(false);
    changed = true;
    ipc.changed?.({ payload: { graphPath: "/graph" } });
    await vi.waitFor(() => expect(document.body.textContent).toContain("Stale read-only snapshot"));
    expect(get(bookSelection)).toBeNull();
    send({ type: "selection", quote: "Stale quote", location: { kind: "epub", cfi: "epubcfi(/6/2)", rendererVersion: BOOK_RENDERER_VERSION } });
    expect(get(bookSelection)).toBeNull();
    expect(document.querySelector<HTMLButtonElement>('[aria-label="Reading controls"] button')?.disabled).toBe(true);
  });
});

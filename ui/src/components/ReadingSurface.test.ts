import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
const api = vi.hoisted(() => ({ invoke: vi.fn(), enter: vi.fn(), exit: vi.fn(), isActive: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: api.invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("../lib/readerFullscreen", () => ({ enterReaderFullscreen: api.enter }));
import PrivateReaderBook from "./PrivateReaderBook.svelte";
import { privateLibrary, type ReaderBook } from "../lib/privateReader";

const book: ReaderBook = {
  id: "reading", title: "A very long filename that should not take reading space.epub",
  kind: "epub", available: true, tracks: [], position: null,
  bookmarks: [{ id: "mark", bookId: "reading", createdAt: 1, note: "Private note",
    position: { offsetMs: 0, locator: { kind: "epub", cfi: "epubcfi(/6/2)", rendererVersion: "fixture" } } }],
};
let component: ReturnType<typeof mount> | undefined;
beforeEach(() => {
  vi.resetAllMocks();
  api.invoke.mockImplementation(async command => command === "reader_read_epub" ? new ArrayBuffer(8) : { libraryPath: "/fixture", books: [book] });
  api.exit.mockResolvedValue(undefined); api.isActive.mockResolvedValue(true);
  api.enter.mockResolvedValue({ exit: api.exit, isActive: api.isActive });
  privateLibrary.set({ libraryPath: "/fixture", books: [book] });
  vi.stubGlobal("fetch", vi.fn(async () => ({ ok: true, text: async () => "/* fixture */" })));
});
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined; document.body.replaceChildren(); vi.unstubAllGlobals();
});
const button = (label: string) => [...document.querySelectorAll("button")].find(button => button.textContent?.trim() === label)!;
const handle = () => document.querySelector<HTMLButtonElement>(".controls-handle")!;
async function openBook() {
  component = mount(PrivateReaderBook, { target: document.body, props: { bookId: book.id, onBack: vi.fn() } });
  await vi.waitFor(() => {
    expect(document.querySelector('[role="alert"]')?.textContent).toBeUndefined();
    expect(document.querySelector("iframe")).not.toBeNull();
  });
  const frame = document.querySelector("iframe")!;
  const token = decodeURIComponent(frame.src).match(/const token="([^"]+)"/)![1];
  return { frame, send(type: string, origin = "null", data: Record<string, unknown> = {}) {
    window.dispatchEvent(new MessageEvent("message", { source: frame.contentWindow, origin,
      data: { channel: "grafium-book", token, type, ...data } }));
    flushSync();
  } };
}
describe("distraction-free book reading", () => {
  it("routes host page keys only from the reader, not editing or another pane", async () => {
    const { frame, send } = await openBook();
    send("ready", "null", { toc: [], annotations: true, notice: "" });
    const post = vi.spyOn(frame.contentWindow!, "postMessage");
    const event = new KeyboardEvent("keydown", { key: "PageDown", bubbles: true, cancelable: true });
    handle().dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(post).toHaveBeenCalledWith(expect.objectContaining({ type: "turn", direction: "next" }), "*");
    post.mockClear();
    document.querySelector("select")!.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    const outside = document.createElement("button"); document.body.append(outside);
    outside.dispatchEvent(new KeyboardEvent("keydown", { key: "PageDown", bubbles: true }));
    expect(post).not.toHaveBeenCalled();
    expect(document.querySelector("iframe")).toBe(frame);
  });
  it("starts with only the book and one handle; toggles without reloading or recording activity", async () => {
    const { frame, send } = await openBook();
    const controls = document.querySelector<HTMLElement>(".reading-controls")!;
    expect(controls.hidden).toBe(true);
    expect(document.querySelector("h1")).toBeNull();
    expect(document.body.textContent).not.toContain(book.title);
    expect(button("← Library")).toBeUndefined();
    send("toggle-controls", "https://attacker.test");
    expect(controls.hidden).toBe(true);
    send("toggle-controls");
    expect(controls.hidden).toBe(false);
    expect(controls.textContent).toContain("Bookmarks");
    expect(controls.textContent).toContain("Private note");
    handle().click(); flushSync();
    expect(controls.hidden).toBe(true);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "F8", cancelable: true })); flushSync();
    expect(controls.hidden).toBe(false);
    expect(document.querySelector("iframe")).toBe(frame);
    expect(api.invoke.mock.calls.filter(([command]) => command === "reader_read_epub")).toHaveLength(1);
    expect(api.invoke.mock.calls.some(([command]) => command === "reader_record_activity")).toBe(false);
  });
  it("enters fullscreen, hides chrome, and restores on Escape without remounting", async () => {
    const { frame, send } = await openBook();
    handle().click(); flushSync(); button("Fullscreen").click();
    await vi.waitFor(() => expect(document.querySelector(".reading-surface.expanded")).not.toBeNull());
    expect(document.querySelector<HTMLElement>(".reading-controls")!.hidden).toBe(true);
    expect(api.enter).toHaveBeenCalledOnce();
    send("exit-fullscreen");
    await vi.waitFor(() => expect(api.exit).toHaveBeenCalledOnce());
    await vi.waitFor(() => expect(document.querySelector(".reading-surface.expanded")).toBeNull());
    expect(document.querySelector("iframe")).toBe(frame);
  });
  it("keeps fullscreen errors visible with the controls closed", async () => {
    api.enter.mockRejectedValue(new Error("Permission denied"));
    const { send } = await openBook();
    send("toggle-fullscreen");
    await vi.waitFor(() => expect(document.querySelector('[role="alert"]')?.textContent).toContain("Permission denied"));
    expect(document.querySelector(".reading-surface.expanded")).toBeNull();
  });
  it("reveals newly saved bookmarks with a count without reloading the book", async () => {
    const { frame } = await openBook();
    const scroll = vi.fn();
    const originalScroll = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = scroll;
    try {
      const added = { ...book.bookmarks[0], id: "new-mark", note: "New passage" };
      privateLibrary.set({ libraryPath: "/fixture", books: [{ ...book, bookmarks: [...book.bookmarks, added] }] });
      await vi.waitFor(() => {
        expect(document.querySelector<HTMLElement>(".reading-controls")!.hidden).toBe(false);
        expect(document.querySelector<HTMLDetailsElement>(".reading-controls details")!.open).toBe(true);
        expect(button("Bookmarks (2)").getAttribute("aria-expanded")).toBe("true");
        expect(document.querySelector('[aria-label="Go to bookmark: New passage"]')).not.toBeNull();
        expect(scroll).toHaveBeenCalled();
      });
      expect(document.querySelector("iframe")).toBe(frame);
      button("Bookmarks (2)").click(); flushSync();
      expect(document.querySelector<HTMLDetailsElement>(".reading-controls details")!.open).toBe(false);
    } finally { Element.prototype.scrollIntoView = originalScroll; }
  });
  it("restores a fullscreen entry that finishes after the reader is closed", async () => {
    let finish!: (session: { exit: typeof api.exit; isActive: typeof api.isActive }) => void;
    api.enter.mockReturnValue(new Promise(resolve => { finish = resolve; }));
    const { send } = await openBook();
    send("toggle-fullscreen");
    await unmount(component!); component = undefined;
    finish({ exit: api.exit, isActive: api.isActive });
    await vi.waitFor(() => expect(api.exit).toHaveBeenCalledOnce());
  });
});

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import { get } from "svelte/store";
const invoke = vi.hoisted(() => vi.fn());
const openDialog = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openDialog }));
import PrivateBookReader from "./PrivateBookReader.svelte";
import PrivateReaderLibrary from "./PrivateReaderLibrary.svelte";
import PrivateReaderBook from "./PrivateReaderBook.svelte";
import { privateBookJump, privateLibrary, privateVoiceLanguageSuggestion, type ReaderBook } from "../lib/privateReader";
import { bookSelection } from "../lib/books";
import { BOOK_RENDERER_VERSION } from "../lib/bookLocations";
import { startAndroidPrivateNarration } from "../lib/privateReaderSegments";
import { stopPrivatePlayback } from "../lib/privateReaderPlayback";
import { startPrivateReadAloud } from "../lib/privateReaderVoice";
import { sha256 } from "@noble/hashes/sha256";
import { readerFlow, readerTextSize } from "../lib/readerPreferences";
import { bionicReaderEnabled } from "../lib/bionicReader";

const book: ReaderBook = { id: "private", title: "Device-only EPUB", available: true, kind: "epub", tracks: [], position: null, bookmarks: [] };
let component: ReturnType<typeof mount> | undefined;
beforeEach(() => {
  invoke.mockReset();
  openDialog.mockReset();
  readerFlow.set("paginated"); readerTextSize.set(100); bionicReaderEnabled.set(false);
  vi.stubGlobal("localStorage", { getItem: vi.fn(() => null), setItem: vi.fn() });
  privateLibrary.set({ libraryPath: "/outside-graph", books: [book] });
  invoke.mockImplementation(async (command, args) => command.endsWith("read_epub") ? new ArrayBuffer(8)
    : command.endsWith("record_activity") ? {
      ...get(privateLibrary), books: get(privateLibrary).books.map(book => book.id === args.bookId
        ? { ...book, lastUsedAt: Date.now(), ...(args.progress ? { progress: args.progress } : {}) } : book),
    }
    : command.endsWith("snapshot") || command.endsWith("rescan") ? { libraryPath: "/outside-graph", books: [book] } : undefined);
  vi.stubGlobal("fetch", vi.fn(async () => ({ ok: true, text: async () => "/* bundled runtime */" })));
});
afterEach(async () => {
  await stopPrivatePlayback();
  if (component) await unmount(component);
  component = undefined; document.body.replaceChildren(); vi.unstubAllGlobals(); vi.restoreAllMocks();
  delete window.PrivateReaderBridge;
});
describe("private reader components", () => {
  it.each(["timeout", "close"])("rejects unconfirmed bookmark captures on %s without a saved bookmark", async failure => {
    const visual = mount(PrivateBookReader, { target: document.body, props: { bookId: book.id } });
    component = visual;
    await vi.waitFor(() => expect(document.querySelector("iframe")).not.toBeNull());
    const frame = document.querySelector("iframe")!;
    const token = decodeURIComponent(frame.src).match(/const token="([^"]+)"/)![1];
    window.dispatchEvent(new MessageEvent("message", { source: frame.contentWindow, origin: "null",
      data: { channel: "grafium-book", token, type: "ready", toc: [], annotations: true, notice: "" } }));
    flushSync();
    vi.useFakeTimers();
    try {
      const pending = visual.captureBookmark();
      const rejected = expect(pending).rejects.toThrow(failure === "timeout" ? "Nothing was saved" : "closed");
      if (failure === "timeout") await vi.advanceTimersByTimeAsync(10001);
      else { await unmount(visual); component = undefined; }
      await rejected;
      expect(invoke.mock.calls.some(([command]) => command === "reader_add_bookmark")).toBe(false);
    } finally { vi.useRealTimers(); }
  });
  it.each(["button", "shortcut", "frame shortcut"])("captures an exact bookmark through %s without replacing narration progress", async action => {
    const locator = { kind: "epub" as const, cfi: "epubcfi(/6/2!/4/2/1,:4,:12)", rendererVersion: BOOK_RENDERER_VERSION };
    const saved = { locator: { ...locator, cfi: "epubcfi(/6/2!/4/8)" }, offsetMs: 2370, voiceId: "saved-voice" };
    privateLibrary.set({ libraryPath: "/outside-graph", books: [{ ...book, position: saved }] });
    const original = invoke.getMockImplementation()!;
    invoke.mockImplementation(async (...args) => args[0] === "reader_snapshot" ? get(privateLibrary) : original(...args));
    component = mount(PrivateReaderBook, { target: document.body, props: { bookId: book.id, onBack: vi.fn() } });
    await vi.waitFor(() => expect(document.querySelector("iframe")).not.toBeNull());
    const frame = document.querySelector("iframe")!;
    const token = decodeURIComponent(frame.src).match(/const token="([^"]+)"/)![1];
    const send = (data: Record<string, unknown>) => {
      window.dispatchEvent(new MessageEvent("message", { source: frame.contentWindow, origin: "null",
        data: { channel: "grafium-book", token, ...data } }));
      flushSync();
    };
    send({ type: "ready", toc: [], annotations: true, notice: "" });
    const post = vi.spyOn(frame.contentWindow!, "postMessage").mockImplementation(message => {
      if (message.type === "capture-bookmark") queueMicrotask(() => send({
        type: "bookmark-captured", requestId: message.requestId, location: locator, quote: "Two words from the exact selected passage.",
      }));
    });
    if (action === "shortcut") {
      const event = new CustomEvent("grafium-bookmark", { cancelable: true });
      window.dispatchEvent(event); expect(event.defaultPrevented).toBe(true);
    } else if (action === "frame shortcut") send({ type: "bookmark" });
    else [...document.querySelectorAll("button")].find(button => button.textContent === "Bookmark")!.click();
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("reader_add_bookmark", {
      bookId: book.id, position: { offsetMs: 0, locator }, note: "Two words",
    }));
    expect(post).toHaveBeenCalledWith(expect.objectContaining({ type: "capture-bookmark" }), "*");
    expect(get(privateLibrary).books[0].position).toEqual(saved);
    expect(invoke.mock.calls.some(([command]) => command === "reader_save_position")).toBe(false);
  });
  it("reports manual reading interactions, not mounting or passive layout, and uses renderer progress", async () => {
    const onActivity = vi.fn(); const onProgress = vi.fn();
    component = mount(PrivateBookReader, { target: document.body, props: { bookId: book.id, onActivity, onProgress } });
    await vi.waitFor(() => expect(document.querySelector("iframe")).not.toBeNull());
    const frame = document.querySelector("iframe")!;
    const token = decodeURIComponent(frame.src).match(/const token="([^"]+)"/)![1];
    const locator = { kind: "epub", cfi: "epubcfi(/6/2!/4/8)", rendererVersion: BOOK_RENDERER_VERSION };
    const send = (data: Record<string, unknown>, origin = "null") => {
      window.dispatchEvent(new MessageEvent("message", { source: frame.contentWindow, origin,
        data: { channel: "grafium-book", token, ...data } })); flushSync();
    };
    send({ type: "ready", toc: [], annotations: true, notice: "" });
    send({ type: "location", location: locator, label: "Passive initial place", fraction: .25 });
    expect(onActivity).not.toHaveBeenCalled(); expect(onProgress).not.toHaveBeenCalled();
    document.querySelector<HTMLSelectElement>("label select")!.dispatchEvent(new Event("change", { bubbles: true }));
    expect(onActivity).toHaveBeenCalledOnce();
    send({ type: "selection", location: locator, quote: "Spoofed selection" }, "https://attacker.test");
    expect(onActivity).toHaveBeenCalledOnce();
    send({ type: "selection", location: locator, quote: "Read this passage" });
    expect(onActivity).toHaveBeenCalledTimes(2);
    expect(invoke).not.toHaveBeenCalledWith("reader_record_activity", expect.anything());
    document.querySelector<HTMLButtonElement>('[aria-label="Next page"]')!.click();
    send({ type: "location", location: locator, label: "Halfway", fraction: .5 });
    expect(onActivity).toHaveBeenCalledTimes(3);
    expect(onProgress).toHaveBeenLastCalledWith({ position: .5, total: 1, anchor: locator.cfi, label: "Halfway" });
    window.dispatchEvent(new Event("pagehide"));
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("reader_record_activity", {
      bookId: book.id, progress: { position: .5, total: 1, anchor: locator.cfi, label: "Halfway" },
    }));
  });
  it("targets an initial EPUB bookmark once after the isolated frame becomes ready", async () => {
    const locator = { kind: "epub" as const, cfi: "epubcfi(/6/2!/4/8)", rendererVersion: BOOK_RENDERER_VERSION };
    const target = { ...book, bookmarks: [{ id: "target", bookId: book.id, note: "", createdAt: 1, position: { offsetMs: 0, locator } }] };
    privateLibrary.set({ libraryPath: "/outside-graph", books: [target] });
    const onActivity = vi.fn(); const onProgress = vi.fn();
    component = mount(PrivateReaderBook, { target: document.body, props: {
      bookId: book.id, initialBookmarkId: "target", onBack: vi.fn(), onActivity, onProgress,
    } });
    await vi.waitFor(() => expect(document.querySelector("iframe")).not.toBeNull());
    const frame = document.querySelector("iframe")!;
    const token = decodeURIComponent(frame.src).match(/const token="([^"]+)"/)![1];
    const post = vi.spyOn(frame.contentWindow!, "postMessage");
    const send = (data: Record<string, unknown>) => {
      window.dispatchEvent(new MessageEvent("message", { source: frame.contentWindow, origin: "null", data: { channel: "grafium-book", token, ...data } }));
      flushSync();
    };
    send({ type: "ready", toc: [], annotations: true, notice: "" });
    expect(post).toHaveBeenCalledWith(expect.objectContaining({ type: "goto", location: locator }), "*");
    privateLibrary.set({ libraryPath: "/outside-graph", books: [{ ...target, favorite: true }] }); flushSync();
    expect(post.mock.calls.filter(([message]) => message.type === "goto")).toHaveLength(1);
    send({ type: "location", location: locator, label: "Bookmarked passage" });
    expect(onActivity).toHaveBeenCalledOnce();
    expect(onProgress).toHaveBeenCalledWith(expect.objectContaining({ anchor: locator.cfi, label: "Bookmarked passage" }));
  });
  it("preserves restored narration timing and voice through passive initial and reflow relocations", async () => {
    const locator = { kind: "epub" as const, cfi: "epubcfi(/6/2!/4/2/1,:320,:640)", rendererVersion: BOOK_RENDERER_VERSION };
    const position = { locator, offsetMs: 2370, voiceId: "saved-voice" };
    privateLibrary.set({ libraryPath: "/outside-graph", books: [{ ...book, position }] });
    const audios: { currentTime: number }[] = [];
    vi.stubGlobal("Audio", class extends EventTarget {
      currentTime = 0; duration = 20; preload = ""; src = "";
      constructor() { super(); audios.push(this); }
      async play() {}
      pause() {}
      load() { if (this.src) queueMicrotask(() => this.dispatchEvent(new Event("loadedmetadata"))); }
      removeAttribute() { this.src = ""; }
    });
    URL.createObjectURL = vi.fn(() => "blob:private-voice");
    URL.revokeObjectURL = vi.fn();
    const native = invoke.getMockImplementation()!;
    invoke.mockImplementation(async (...args) => {
      if (args[0] === "private_voice_status") return { available: true, selection: { voice_id: "saved-voice", language: "en" } };
      if (args[0] === "private_voice_synthesize") return { file_name: "clip.wav" };
      if (args[0] === "private_voice_audio") {
        const bytes = new Uint8Array(44);
        bytes.set(new TextEncoder().encode("RIFF")); bytes.set(new TextEncoder().encode("WAVE"), 8);
        return bytes.buffer;
      }
      return native(...args);
    });
    component = mount(PrivateBookReader, { target: document.body, props: { bookId: book.id } });
    await vi.waitFor(() => expect(document.querySelector("iframe")).not.toBeNull());
    const frame = document.querySelector("iframe")!;
    const token = decodeURIComponent(frame.src).match(/const token="([^"]+)"/)![1];
    const send = (data: Record<string, unknown>) => {
      window.dispatchEvent(new MessageEvent("message", { source: frame.contentWindow, origin: "null",
        data: { channel: "grafium-book", token, ...data } }));
      flushSync();
    };
    vi.spyOn(frame.contentWindow!, "postMessage").mockImplementation(message => {
      if (message.type === "read-aloud-segments") queueMicrotask(() => send({
        type: "read-aloud-segments", requestId: message.requestId, section: 0, sectionCount: 1,
        nextOffset: null, segments: [{ text: "Restored exact narration chunk", locator }],
      }));
    });
    send({ type: "ready", toc: [], annotations: true, notice: "" });
    send({ type: "location", location: { ...locator, cfi: "epubcfi(/6/2!/4/2)" }, label: "Initial visible page" });
    document.querySelector<HTMLSelectElement>("label select")!.dispatchEvent(new Event("change", { bubbles: true }));
    send({ type: "location", location: { ...locator, cfi: "epubcfi(/6/2!/4/4)" }, label: "Reflowed visible page" });
    readerFlow.set("scrolled"); bionicReaderEnabled.set(true); flushSync();
    send({ type: "location", location: { ...locator, cfi: "epubcfi(/6/2!/4/6)" }, label: "Scrolling Bionic layout" });
    expect(document.querySelector("iframe")).toBe(frame);
    await new Promise(resolve => setTimeout(resolve, 650));
    expect(invoke.mock.calls.some(([command]) => command === "reader_save_position")).toBe(false);
    expect(get(privateLibrary).books[0].position).toEqual(position);
    await startPrivateReadAloud(book.id);
    expect(audios[0].currentTime).toBe(2.37);
    expect(invoke).toHaveBeenCalledWith("reader_save_position", { bookId: book.id, position });
  });
  it.each(["Next", "Previous", "toc", "bookmark", "frame navigation"])("allows explicit %s to replace narration with visual progress", async action => {
    const locator = { kind: "epub" as const, cfi: "epubcfi(/6/2!/4/2)", rendererVersion: BOOK_RENDERER_VERSION };
    privateLibrary.set({ libraryPath: "/outside-graph", books: [{ ...book,
      position: { locator: { ...locator, cfi: "epubcfi(/6/2!/4/2/1,:320,:640)" }, offsetMs: 2370, voiceId: "saved-voice" } }] });
    component = mount(PrivateBookReader, { target: document.body, props: { bookId: book.id } });
    await vi.waitFor(() => expect(document.querySelector("iframe")).not.toBeNull());
    const frame = document.querySelector("iframe")!;
    const token = decodeURIComponent(frame.src).match(/const token="([^"]+)"/)![1];
    const send = (data: Record<string, unknown>) => {
      window.dispatchEvent(new MessageEvent("message", { source: frame.contentWindow, origin: "null",
        data: { channel: "grafium-book", token, ...data } }));
      flushSync();
    };
    send({ type: "ready", toc: [{ label: "Chapter", target: "one.xhtml", depth: 0 }], annotations: true, notice: "" });
    if (action === "toc") {
      const select = document.querySelector<HTMLSelectElement>('[aria-label="Private book contents"]')!;
      select.value = "0"; select.dispatchEvent(new Event("change", { bubbles: true }));
    } else if (action === "bookmark") {
      privateBookJump.set({ bookId: book.id, locator }); flushSync();
    } else if (action === "frame navigation") send({ type: "navigation" });
    else document.querySelector<HTMLButtonElement>(`[aria-label="${action} page"]`)!.click();
    send({ type: "location", location: locator, label: "Explicit destination" });
    window.dispatchEvent(new Event("pagehide"));
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("reader_save_position", {
      bookId: book.id, position: { offsetMs: 0, locator },
    }));
  });
  it("mounts an opaque secure frame and only persists trusted EPUB locators through native private APIs", async () => {
    component = mount(PrivateBookReader, { target: document.body, props: { bookId: book.id } });
    await vi.waitFor(() => expect(document.querySelector("iframe")).not.toBeNull());
    const frame = document.querySelector("iframe")!;
    expect(frame.src.startsWith("data:text/html")).toBe(true);
    expect(frame.getAttribute("sandbox")).toBe("allow-scripts allow-same-origin");
    expect(frame.hasAttribute("srcdoc")).toBe(false);
    const token = decodeURIComponent(frame.src).match(/const token="([^"]+)"/)![1];
    const locator = { kind: "epub", cfi: "epubcfi(/6/2!/4/2)", rendererVersion: BOOK_RENDERER_VERSION };
    const send = (data: Record<string, unknown>, origin = "null") => {
      window.dispatchEvent(new MessageEvent("message", { source: frame.contentWindow, origin, data: { channel: "grafium-book", token, ...data } }));
      flushSync();
    };
    send({ type: "location", location: locator, label: "Injected" }, "https://attacker.test");
    window.dispatchEvent(new Event("pagehide"));
    expect(invoke).not.toHaveBeenCalledWith("reader_save_position", expect.anything());
    send({ type: "ready", toc: [], annotations: true, notice: "", language: "fr-CA" });
    expect(get(privateVoiceLanguageSuggestion)).toEqual({ bookId: book.id, title: book.title, language: "fr-CA" });
    expect(invoke.mock.calls.some(([command]) => command.startsWith("private_voice_"))).toBe(false);
    const help = vi.fn((event: KeyboardEvent) => {
      expect(event.key).toBe("F1");
      expect((event.target as Element).closest("[data-help-context]")?.getAttribute("data-help-context")).toBe("reader");
    });
    window.addEventListener("keydown", help);
    send({ type: "help" });
    await vi.waitFor(() => expect(help).toHaveBeenCalledOnce());
    window.removeEventListener("keydown", help);
    send({ type: "location", location: locator, label: "Chapter 1" });
    send({ type: "selection", location: locator, quote: "Private passage" });
    window.dispatchEvent(new Event("pagehide"));
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("reader_save_position", { bookId: book.id, position: { offsetMs: 0, locator } }));
    expect(invoke).not.toHaveBeenCalledWith("reader_record_activity", expect.anything());
    expect(get(bookSelection)).toBeNull();
    expect(invoke.mock.calls.every(([command]) => command.startsWith("reader_"))).toBe(true);
  });
  it("keeps private discovery separate and exposes the graph-note privacy boundary", async () => {
    const onOpen = vi.fn();
    component = mount(PrivateReaderLibrary, { target: document.body, props: { onOpen, onSettings: vi.fn() } });
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("reader_rescan", undefined));
    expect(document.body.textContent).toContain("no graph pages, AI indexing, or graph sync");
    expect(document.body.textContent).not.toContain("[[Book title]]");
    expect(document.querySelector("[data-settings-help-text]")?.textContent).toContain("Journal note");
    [...document.querySelectorAll("button")].find(button => button.textContent === book.title)!.click();
    expect(onOpen).toHaveBeenCalledWith(book.id);
  });
  it("keeps restored unavailable history visible before a new library location is selected", async () => {
    invoke.mockResolvedValue({ libraryPath: null, books: [{ ...book, available: false }] });
    component = mount(PrivateReaderLibrary, { target: document.body, props: { onOpen: vi.fn(), onSettings: vi.fn() } });
    await vi.waitFor(() => expect(document.body.textContent).toContain("Add a folder, drive, SD card, or mounted share"));
    expect(document.body.textContent).toContain(book.title);
    expect(document.body.textContent).toContain("History / relink");
  });
  it("retains unavailable books and bookmark notes without reading source bytes", async () => {
    privateLibrary.set({ libraryPath: "/gone", books: [{ ...book, available: false, bookmarks: [{
      id: "mark", bookId: book.id, createdAt: "2026-10-01T00:00:00Z", note: "Remember privately",
      position: { offsetMs: 0, locator: { kind: "epub", cfi: "epubcfi(/6/2)", rendererVersion: BOOK_RENDERER_VERSION } },
    }] }] });
    component = mount(PrivateReaderBook, { target: document.body, props: { bookId: book.id, onBack: vi.fn() } });
    flushSync();
    expect(document.body.textContent).toContain("Progress and bookmarks have been retained");
    expect(document.body.textContent).toContain("Remember privately");
    expect(document.querySelector("iframe")).toBeNull();
    expect(invoke).not.toHaveBeenCalled();
  });
  it("binds the actual mounted reader's bytes and canonical segments to Android's committed queue", async () => {
    component = mount(PrivateBookReader, { target: document.body, props: { bookId: book.id } });
    await vi.waitFor(() => expect(document.querySelector("iframe")).not.toBeNull());
    const frame = document.querySelector("iframe")!;
    const token = decodeURIComponent(frame.src).match(/const token="([^"]+)"/)![1];
    const locator = { kind: "epub", cfi: "epubcfi(/6/2!/4/2)", rendererVersion: BOOK_RENDERER_VERSION };
    const send = (data: Record<string, unknown>) => window.dispatchEvent(new MessageEvent("message", {
      source: frame.contentWindow, origin: "null", data: { channel: "grafium-book", token, ...data },
    }));
    send({ type: "ready", toc: [], annotations: true, notice: "" }); flushSync();
    vi.spyOn(frame.contentWindow!, "postMessage").mockImplementation(message => {
      if (message.type === "read-aloud-segments") queueMicrotask(() => send({
        type: "read-aloud-segments", requestId: message.requestId, section: 0, sectionCount: 1,
        nextOffset: null, segments: [{ text: "Private canonical passage", locator }],
      }));
    });
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Android");
    const sourceHash = [...sha256(new Uint8Array(8))].map(value => value.toString(16).padStart(2, "0")).join("");
    const nativeCalls: { command: string; args: Record<string, unknown> }[] = [];
    window.PrivateReaderBridge = { request(raw) {
      const request = JSON.parse(raw);
      nativeCalls.push(request);
      const result = request.command === "voiceStatus" ? { available: true, selection: {} }
        : request.command === "narrationBegin" ? { uploadId: "canonical", sourceHash }
        : request.command === "narrationStart" ? { bookId: book.id, mode: "tts", offsetMs: 0, playing: true, buffering: false, locator: JSON.stringify(locator) }
        : request.command === "stop" ? { bookId: null, playing: false, buffering: false } : {};
      queueMicrotask(() => window.dispatchEvent(new CustomEvent("private-reader-response", { detail: { id: request.id, ok: true, result } })));
    } };
    await startAndroidPrivateNarration(book.id, true);
    expect(nativeCalls.find(call => call.command === "narrationAppend")?.args).toEqual({
      uploadId: "canonical", segments: [{ ordinal: 0, text: "Private canonical passage", locator }],
    });
    expect(nativeCalls.at(-1)).toMatchObject({ command: "narrationStart", args: { bookId: book.id, fromBeginning: true } });
    await stopPrivatePlayback();
  });
  it("relinks even a single-chapter audiobook as an explicitly selected top-level folder", async () => {
    privateLibrary.set({ libraryPath: "/outside-graph", books: [{ ...book, kind: "audio", available: false,
      tracks: [{ id: "one", title: "Only chapter", relativePath: "Old folder/1.mp3" }] }] });
    component = mount(PrivateReaderBook, { target: document.body, props: { bookId: book.id, onBack: vi.fn() } });
    flushSync();
    const button = (text: string) => [...document.querySelectorAll("button")].find(element => element.textContent?.trim() === text)!;
    button("Relink source…").click(); flushSync();
    await vi.waitFor(() => expect(button("Choose audiobook folder…").disabled).toBe(false));
    openDialog.mockResolvedValue("/outside-graph/New folder");
    button("Choose audiobook folder…").click();
    await vi.waitFor(() => expect(button("Confirm relink")).toBeTruthy());
    expect(openDialog).toHaveBeenCalledWith(expect.objectContaining({ directory: true }));
    expect(invoke).not.toHaveBeenCalledWith("reader_relink", expect.anything());
    await vi.waitFor(() => expect(button("Confirm relink").disabled).toBe(false));
    button("Confirm relink").click();
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("reader_relink", {
      bookId: book.id, relativePath: "New folder", confirmReplacement: true, location: "/outside-graph",
    }));
  });
});

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import PrivateReaderBook from "./PrivateReaderBook.svelte";
import { privateLibrary, privateLibraryError, type ReaderBook } from "../lib/privateReader";
import { libraryMediaRequest } from "../lib/library";
import { privatePlayback } from "../lib/privateReaderPlayback";
import { get } from "svelte/store";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-shell", () => ({ open: vi.fn() }));

let component: ReturnType<typeof mount> | undefined;
const videoBook: ReaderBook = {
  id: "video", title: "Repair video", kind: "video", available: true,
  tracks: [{ id: "track", title: "Track", relativePath: "video.webm" }],
  position: { trackId: "track", offsetMs: 123000 }, bookmarks: [],
};
const audioBook: ReaderBook = {
  id: "audio", title: "Repair audio", kind: "audio", available: true,
  tracks: [{ id: "track", title: "Track", relativePath: "audio.mp3" }],
  position: { trackId: "track", offsetMs: 123000 }, bookmarks: [],
};

beforeEach(() => {
  mocks.invoke.mockReset();
  privateLibraryError.set(""); libraryMediaRequest.set(null);
  privatePlayback.set({ bookId: null, title: "", mode: "audio", status: "stopped", position: null, error: "" });
  privateLibrary.set({ libraryPath: "/library", books: [videoBook, audioBook] });
  mocks.invoke.mockImplementation(async (command) => {
    if (command === "reader_media_url") return "http://127.0.0.1:3456/private/audio";
    if (command === "reader_snapshot" || command === "reader_rescan") return get(privateLibrary);
  });
  vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue();
  vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
  vi.spyOn(HTMLMediaElement.prototype, "load").mockImplementation(() => {});
});
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined; document.body.replaceChildren(); vi.restoreAllMocks();
});

describe("PrivateReaderBook Library search targets", () => {
  it("does not cue 0:00 or overwrite the saved media position for title-only hits", async () => {
    component = mount(PrivateReaderBook, { target: document.body, props: {
      bookId: "video", onBack: vi.fn(), initialOpenTarget: { bookId: "video", nonce: 1, startMs: null, trackId: null },
    } });
    flushSync();
    await vi.waitFor(() => expect(document.body.textContent).toContain("Use its saved place"));
    expect(get(libraryMediaRequest)).toBeNull();
  });

  it("keeps the Press Play note after opening a media timestamp", async () => {
    component = mount(PrivateReaderBook, { target: document.body, props: {
      bookId: "video", onBack: vi.fn(), initialOpenTarget: { bookId: "video", nonce: 2, startMs: 65000, trackId: "track" },
    } });
    await vi.waitFor(() => expect(document.body.textContent).toContain("Press Play when ready"));
    expect(get(libraryMediaRequest)?.autoplay).not.toBe(true);
  });

  it("ignores stale targets for another book", async () => {
    component = mount(PrivateReaderBook, { target: document.body, props: {
      bookId: "video", onBack: vi.fn(), initialOpenTarget: { bookId: "audio", nonce: 3, startMs: 65000, trackId: "track" },
    } });
    flushSync();
    expect(document.body.textContent).not.toContain("Press Play when ready");
    expect(get(libraryMediaRequest)).toBeNull();
  });
});

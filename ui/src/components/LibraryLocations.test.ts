import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import { get } from "svelte/store";
const mocks = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn(), open: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.open }));
vi.mock("@tauri-apps/plugin-shell", () => ({ open: vi.fn() }));
import PrivateReaderLibrary from "./PrivateReaderLibrary.svelte";
import PrivateReaderBook from "./PrivateReaderBook.svelte";
import PrivateReaderSettings from "./PrivateReaderSettings.svelte";
import { privateLibrary, privateLibraryError, type ReaderBook, type ReaderSnapshot } from "../lib/privateReader";
import { toasts } from "../lib/toast.svelte";

const DRIVE = "/run/media/blin/6TWDBACKUP/books";
const CARD = "/run/media/blin/SDCARD";
const book = (extra: Partial<ReaderBook> = {}): ReaderBook => ({
  id: "dune", title: "Dune", kind: "epub", available: false, disconnected: true, location: DRIVE,
  tracks: [], position: null, bookmarks: [{ id: "mark", bookId: "dune", createdAt: 1, note: "Kept note",
    position: { offsetMs: 0, locator: { kind: "epub", cfi: "epubcfi(/6/2)", rendererVersion: "foliate-js-ab2c6b8" } } }],
  ...extra,
});
const local = book({ id: "local", title: "On the card", available: true, disconnected: false, location: CARD, bookmarks: [] });
const disconnectedSnapshot = (): ReaderSnapshot => ({
  libraryPath: DRIVE,
  locations: [
    { path: DRIVE, connected: false, reason: "Not connected", items: 1 },
    { path: CARD, connected: true, items: 1 },
  ],
  books: [book(), local],
});
let rescans: ReaderSnapshot[] = [];
let component: ReturnType<typeof mount> | undefined;
const button = (name: string) => [...document.querySelectorAll("button")].find(element => element.textContent?.trim() === name);
const text = () => document.body.textContent ?? "";

beforeEach(() => {
  mocks.invoke.mockReset(); mocks.listen.mockReset(); mocks.open.mockReset();
  mocks.listen.mockResolvedValue(vi.fn());
  privateLibraryError.set(""); toasts.splice(0, toasts.length);
  privateLibrary.set(disconnectedSnapshot());
  rescans = [];
  mocks.invoke.mockImplementation(async (command: string, args: Record<string, unknown>) => {
    if (command === "library_index_status") return {
      enabled: true, transcribeMedia: true, running: false, jobId: null,
      items: { total: 2, indexed: 1, pending: 0, failed: 0, titleOnly: 0, waiting: 1 }, chunks: 3,
      semantic: "ready", semanticReason: null, transcription: "ready", transcriptionReason: null, lastIndexedAt: null, errors: [],
    };
    if (command === "reader_rescan") return rescans.shift() ?? get(privateLibrary);
    if (command === "reader_snapshot") return get(privateLibrary);
    if (command === "reader_add_location") return {
      ...get(privateLibrary), locations: [...get(privateLibrary).locations!, { path: args.path, connected: true, items: 0 }],
    };
    if (command === "reader_move_location") return {
      ...get(privateLibrary), locations: get(privateLibrary).locations!.map(location =>
        location.path === args.from ? { ...location, path: args.to as string, connected: true } : location),
    };
    if (command === "reader_remove_location") return {
      ...get(privateLibrary), locations: get(privateLibrary).locations!.filter(location => location.path !== args.path),
      books: get(privateLibrary).books.filter(item => item.location !== args.path),
    };
  });
});
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined; document.body.replaceChildren(); vi.restoreAllMocks();
});

describe("disconnected Library locations", () => {
  it("keeps a disconnected drive's items listed and tells how to open them", async () => {
    const onOpen = vi.fn();
    component = mount(PrivateReaderLibrary, { target: document.body, props: { onOpen, onSettings: vi.fn() } });
    await vi.waitFor(() => expect(text()).toContain("books on 6TWDBACKUP isn't connected. Its 1 item is kept here until you reconnect it."));
    await vi.waitFor(() => expect(text()).toContain("1 waiting for a disconnected location"));
    const row = [...document.querySelectorAll("li")].find(item => item.textContent?.includes("Dune"))!;
    expect(row.textContent).toContain("Disconnected · 6TWDBACKUP");
    expect(row.textContent).not.toContain("History / relink");

    // Still unplugged: clicking checks again, then explains instead of opening.
    [...row.querySelectorAll("button")].find(element => element.textContent === "Read")!.click();
    await vi.waitFor(() => expect(toasts.at(-1)?.message).toBe(
      "“Dune” is on 6TWDBACKUP, which isn't connected. Plug in the drive or SD card, or connect to the file server, then try again."));
    expect(mocks.invoke).toHaveBeenCalledWith("reader_rescan", undefined);
    expect(onOpen).not.toHaveBeenCalled();

    // Plugged in meanwhile: the same click opens it.
    rescans.push({ ...disconnectedSnapshot(), locations: [{ path: DRIVE, connected: true, items: 1 }, { path: CARD, connected: true, items: 1 }],
      books: [book({ available: true, disconnected: false }), local] });
    [...document.querySelectorAll("li")].find(item => item.textContent?.includes("Dune"))!
      .querySelectorAll("button").forEach(element => { if (element.textContent === "Read") element.click(); });
    await vi.waitFor(() => expect(onOpen).toHaveBeenCalledWith("dune"));
    await vi.waitFor(() => expect(text()).not.toContain("isn't connected"));
  });

  it("shows a disconnected item's history with a check-again action instead of relinking", async () => {
    component = mount(PrivateReaderBook, { target: document.body, props: { bookId: "dune", onBack: vi.fn() } });
    flushSync();
    expect(text()).toContain("“Dune” is on 6TWDBACKUP, which isn't connected.");
    expect(text()).toContain("Progress and bookmarks are kept.");
    expect(text()).toContain("Kept note");
    expect(text()).not.toContain("Source unavailable.");
    expect(document.querySelector("iframe")).toBeNull();
    rescans.push({ ...disconnectedSnapshot(), books: [book({ available: false, disconnected: true }), local] });
    button("Check again")!.click();
    await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("reader_rescan", undefined));
    await vi.waitFor(() => expect(button("Check again")?.disabled).toBe(false));
  });
});

describe("Library locations settings", () => {
  it("lists locations with their state and adds, moves, and removes them", async () => {
    component = mount(PrivateReaderSettings, { target: document.body });
    await vi.waitFor(() => expect(document.querySelectorAll(".locations li")).toHaveLength(2));
    const rows = () => [...document.querySelectorAll(".locations li")];
    expect(rows()[0].textContent).toContain("books on 6TWDBACKUP");
    expect(rows()[0].textContent).toContain("Disconnected");
    expect(rows()[0].textContent).toContain(`${DRIVE} · 1 item · Not connected`);
    expect(rows()[1].textContent).toContain("SDCARD");
    expect(rows()[1].textContent).toContain("Connected");
    expect(document.querySelector('button[aria-label="Help: Library locations"]')).not.toBeNull();
    await vi.waitFor(() => expect(text()).toContain("1 waiting for a disconnected location"));

    mocks.open.mockResolvedValueOnce("/mnt/nas/audio");
    button("Add location…")!.click();
    await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("reader_add_location", { path: "/mnt/nas/audio" }));
    await vi.waitFor(() => expect(rows()).toHaveLength(3));
    expect(text()).toContain("Added audio on nas.");

    mocks.open.mockResolvedValueOnce("/run/media/blin/6TWDBACKUP1/books");
    [...rows()[0].querySelectorAll("button")].find(element => element.textContent === "Change folder…")!.click();
    await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("reader_move_location",
      { from: DRIVE, to: "/run/media/blin/6TWDBACKUP1/books" }));
    expect(mocks.open).toHaveBeenLastCalledWith(expect.objectContaining({ directory: true, defaultPath: DRIVE }));

    const card = () => rows().find(row => row.textContent?.includes("SDCARD"))!;
    [...card().querySelectorAll("button")].find(element => element.textContent === "Remove…")!.click();
    await vi.waitFor(() => expect(document.activeElement?.textContent).toBe("Cancel"));
    expect(card().textContent).toContain("Remove “SDCARD” from Library? Its 1 item leaves Library with its reading progress, bookmarks, and search index. The files in the folder are not touched.");
    expect(mocks.invoke).not.toHaveBeenCalledWith("reader_remove_location", expect.anything());
    [...card().querySelectorAll("button")].find(element => element.textContent === "Remove location")!.click();
    await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("reader_remove_location", { path: CARD }));
    await vi.waitFor(() => expect(rows().some(row => row.textContent?.includes("SDCARD"))).toBe(false));
    expect(text()).toContain("Removed SDCARD from Library. The files in it were not touched.");
  });
});

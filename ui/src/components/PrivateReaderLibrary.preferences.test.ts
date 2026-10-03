import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import { get } from "svelte/store";
const mocks = vi.hoisted(() => ({ invoke: vi.fn(), showToast: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-shell", () => ({ open: vi.fn() }));
vi.mock("../lib/toast.svelte", () => ({ showToast: mocks.showToast }));
import PrivateReaderLibrary from "./PrivateReaderLibrary.svelte";
import { privateLibrary, privateLibraryError } from "../lib/privateReader";

const TYPE_KEY = "grafium.library.mediaType";
const saved = new Map<string, string>();
let component: ReturnType<typeof mount> | undefined;
const filter = () => document.querySelector<HTMLSelectElement>(".filters select")!;
const titles = () => [...document.querySelectorAll(".book-title")].map(element => element.textContent);
function render() {
  component = mount(PrivateReaderLibrary, {
    target: document.body, props: { onOpen: vi.fn(), onSettings: vi.fn() },
  });
  flushSync();
}
function select(value: string) {
  filter().value = value;
  filter().dispatchEvent(new Event("change", { bubbles: true }));
  flushSync();
}
beforeEach(() => {
  saved.clear();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => saved.get(key) ?? null,
    setItem: (key: string, value: string) => saved.set(key, value),
  });
  mocks.invoke.mockReset();
  mocks.showToast.mockReset();
  privateLibraryError.set("");
  privateLibrary.set({ libraryPath: "/synthetic/library", books: [
    { id: "epub", title: "Reading", kind: "epub", available: true, tracks: [], bookmarks: [], position: null },
    { id: "audio", title: "Listening", kind: "audio", favorite: true, available: true, tracks: [], bookmarks: [], position: null },
  ] });
  mocks.invoke.mockImplementation(async command => {
    if (["reader_snapshot", "reader_rescan", "library_index_status"].includes(command)) return get(privateLibrary);
  });
});
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("device-local Library type filter", () => {
  it("restores Audio after leaving and remounting, but does not persist search or Favorites", async () => {
    render();
    expect(filter().value).toBe("all");
    select("audio");
    expect(localStorage.getItem(TYPE_KEY)).toBe("audio");
    expect(titles()).toEqual(["Listening"]);
    const search = document.querySelector<HTMLInputElement>('input[type="search"]')!;
    search.value = "Listen";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    document.querySelector<HTMLButtonElement>(".filters button")!.click();
    flushSync();
    await unmount(component!);
    component = undefined;
    render();
    expect(filter().value).toBe("audio");
    expect(titles()).toEqual(["Listening"]);
    expect(document.querySelector<HTMLInputElement>('input[type="search"]')!.value).toBe("");
    expect(document.querySelector(".filters button")?.getAttribute("aria-pressed")).toBe("false");
    expect(mocks.invoke.mock.calls.every(([command]) => ["reader_snapshot", "reader_rescan", "library_index_status"].includes(command))).toBe(true);
    select("all");
    expect(localStorage.getItem(TYPE_KEY)).toBe("all");
    expect(titles()).toHaveLength(2);
  });

  it.each(["all", "epub", "audio", "video", "youtube"])("restores valid persisted %s on a fresh mount without rewriting storage", value => {
    localStorage.setItem(TYPE_KEY, value);
    const save = vi.spyOn(localStorage, "setItem");
    render();
    expect(filter().value).toBe(value);
    expect(save).not.toHaveBeenCalled();
  });

  it.each(["", "Audio", "\"audio\"", "bogus", "{\"type\":\"audio\"}"])("falls back safely for corrupt storage %j without changing it", value => {
    localStorage.setItem(TYPE_KEY, value);
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const save = vi.spyOn(localStorage, "setItem");
    render();
    expect(filter().value).toBe("all");
    expect(titles()).toHaveLength(2);
    expect(save).not.toHaveBeenCalled();
    expect(warn).toHaveBeenCalledWith("Invalid saved Library type filter; showing all types.");
  });

  it("reports failed reads and still renders the unfiltered shelf", () => {
    const cause = new Error("storage blocked");
    vi.spyOn(localStorage, "getItem").mockImplementation(() => { throw cause; });
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    render();
    expect(filter().value).toBe("all");
    expect(titles()).toHaveLength(2);
    expect(warn).toHaveBeenCalledWith("Could not load the Library type filter; showing all types.", cause);
  });

  it("reports failed writes while applying the selected type to the current shelf", () => {
    render();
    vi.spyOn(localStorage, "setItem").mockImplementation(() => { throw new Error("storage full"); });
    select("audio");
    expect(filter().value).toBe("audio");
    expect(titles()).toEqual(["Listening"]);
    expect(mocks.showToast).toHaveBeenCalledWith(expect.stringContaining("Could not remember the Library type filter"), "error");
  });

  it("does not save an unsupported synthetic selection", () => {
    render();
    const save = vi.spyOn(localStorage, "setItem");
    select("invalid");
    expect(save).not.toHaveBeenCalled();
    expect(titles()).toHaveLength(2);
  });
});

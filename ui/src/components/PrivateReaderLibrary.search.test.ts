import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import { get } from "svelte/store";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-shell", () => ({ open: vi.fn() }));
import PrivateReaderLibrary from "./PrivateReaderLibrary.svelte";
import { privateLibrary, privateLibraryError } from "../lib/privateReader";
import { keymap_manager } from "../lib/keymap";

let component: ReturnType<typeof mount> | undefined;
beforeEach(() => {
  vi.stubGlobal("localStorage", { getItem: () => null });
  invoke.mockReset();
  privateLibraryError.set("");
  privateLibrary.set({ libraryPath: "/synthetic/library", books: [
    { id: "first", title: "Distant stars", kind: "epub", available: true, tracks: [], bookmarks: [], position: null },
    { id: "second", title: "Ocean tides", kind: "epub", available: true, tracks: [], bookmarks: [], position: null },
  ] });
  invoke.mockImplementation(async command => {
    if (["reader_snapshot", "reader_rescan", "library_index_status"].includes(command)) return get(privateLibrary);
  });
});
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
  keymap_manager.register([]);
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("Library local search", () => {
  it.each(["Linux", "MacIntel"])("focuses and selects the query with a truthful %s shortcut hint", async platform => {
    vi.spyOn(navigator, "platform", "get").mockReturnValue(platform);
    keymap_manager.register([{ id: "search-local", binding: "mod+f", navOnly: false, action: () => {} }]);
    const library = mount(PrivateReaderLibrary, {
      target: document.body, props: { onOpen: vi.fn(), onSettings: vi.fn() },
    });
    component = library;
    flushSync();
    const input = document.querySelector<HTMLInputElement>('input[type="search"]')!;
    expect(input.closest("label")?.textContent).toContain("Search Library");
    expect(input.title).toBe(`Search Library (${platform === "MacIntel" ? "Cmd" : "Ctrl"}-F)`);
    expect(input.getAttribute("aria-keyshortcuts")).toBe(`${platform === "MacIntel" ? "Meta" : "Control"}+f`);
    input.value = "stars";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    expect([...document.querySelectorAll(".book-title")].map(element => element.textContent)).toEqual(["Distant stars"]);
    expect(library.focusSearch()).toBe(true);
    expect(document.activeElement).toBe(input);
    expect([input.selectionStart, input.selectionEnd]).toEqual([0, 5]);
    expect(invoke.mock.calls.every(([command]) => ["reader_snapshot", "reader_rescan", "library_index_status"].includes(command))).toBe(true);
  });

  it("does not claim to focus search when there is no configured shelf", () => {
    privateLibrary.set({ libraryPath: null, books: [] });
    const library = mount(PrivateReaderLibrary, {
      target: document.body, props: { onOpen: vi.fn(), onSettings: vi.fn() },
    });
    component = library;
    flushSync();
    expect(library.focusSearch()).toBe(false);
    expect(document.querySelector('input[type="search"]')).toBeNull();
  });
});

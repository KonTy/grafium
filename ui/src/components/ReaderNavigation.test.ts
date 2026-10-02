import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import { get } from "svelte/store";
import ReaderNavigation from "./ReaderNavigation.svelte";
import { readerFlow, loadReaderFlowPreference } from "../lib/readerPreferences";
import { bionicReaderEnabled } from "../lib/bionicReader";

let component: ReturnType<typeof mount>;
const storage = { getItem: vi.fn(), setItem: vi.fn() };
beforeEach(() => {
  vi.stubGlobal("localStorage", storage);
  storage.getItem.mockReset(); storage.setItem.mockReset();
  readerFlow.set("paginated"); bionicReaderEnabled.set(false);
});
afterEach(async () => { await unmount(component); document.body.replaceChildren(); vi.unstubAllGlobals(); });
const button = (name: string) => document.querySelector<HTMLButtonElement>(`[aria-label="${name}"]`)!;

describe("reader navigation controls", () => {
  it("uses labelled icons, remembers layout, and shares the global Bionic preference", () => {
    const navigate = vi.fn();
    component = mount(ReaderNavigation, { target: document.body, props: { ready: true, reflowable: true, onNavigate: navigate } });
    flushSync();
    expect(button("Previous page").querySelector("svg")).not.toBeNull();
    button("Next page").click(); expect(navigate).toHaveBeenCalledWith("next");
    const select = document.querySelector("select")!;
    select.value = "scrolled"; select.dispatchEvent(new Event("change", { bubbles: true })); flushSync();
    expect(get(readerFlow)).toBe("scrolled");
    expect(storage.setItem).toHaveBeenCalledWith("grafium.reader.flow", "scrolled");
    button("Bionic reading").click(); flushSync();
    expect(get(bionicReaderEnabled)).toBe(true);
    expect(button("Bionic reading").getAttribute("aria-pressed")).toBe("true");
    bionicReaderEnabled.set(false); flushSync();
    expect(button("Bionic reading").getAttribute("aria-pressed")).toBe("false");
    storage.getItem.mockReturnValue("scrolled"); readerFlow.set("paginated"); loadReaderFlowPreference();
    expect(get(readerFlow)).toBe("scrolled");
  });
  it("does not advertise reflow for fixed pages and labels RTL arrows correctly", () => {
    component = mount(ReaderNavigation, { target: document.body,
      props: { ready: false, reflowable: false, direction: "rtl", onNavigate: vi.fn() } });
    flushSync();
    expect(button("Next page").disabled).toBe(true);
    expect(button("Next page").getAttribute("aria-keyshortcuts")).toBe("ArrowLeft PageDown");
    expect(document.querySelector("select")).toBeNull();
    expect(button("Bionic reading").disabled).toBe(true);
    expect(button("Bionic reading").textContent).toBe("Bionic");
    expect(button("Bionic reading").title).toContain("after the book opens");
  });
  it("explains why Bionic is disabled rather than hiding it for fixed-layout books", () => {
    component = mount(ReaderNavigation, { target: document.body,
      props: { ready: true, reflowable: false, onNavigate: vi.fn() } });
    flushSync();
    expect(button("Bionic reading").disabled).toBe(true);
    expect(button("Bionic reading").title).toContain("fixed-layout");
  });
});

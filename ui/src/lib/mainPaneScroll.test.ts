import { afterEach, describe, expect, it, vi } from "vitest";
import { handleMainPanePageKey, MAIN_PANE_SCROLL_INTENT } from "./mainPaneScroll";

afterEach(() => document.body.replaceChildren());

function pane(nested = false) {
  const main = document.createElement("main");
  document.body.append(main);
  const scroller = nested ? document.createElement("div") : main;
  if (nested) {
    scroller.dataset.mainScrollPane = "";
    main.append(scroller);
  }
  Object.defineProperty(scroller, "clientHeight", { value: 800 });
  const scrollBy = vi.fn();
  Object.defineProperty(scroller, "scrollBy", { value: scrollBy });
  return { main, scroller, scrollBy };
}

const key = (init: KeyboardEventInit = {}) =>
  new KeyboardEvent("keydown", { key: "PageDown", bubbles: true, cancelable: true, ...init });

describe("main pane paging", () => {
  it.each(["page", "all-pages", "journal", "statistics"])("pages the correct scroller in %s without moving focus", (view) => {
    const { main, scroller, scrollBy } = pane(view === "journal" || view === "statistics");
    const input = document.createElement("textarea");
    main.append(input);
    input.value = "Text and selection must not change";
    input.focus();
    input.setSelectionRange(3, 8);
    const events: string[] = [];
    scroller.addEventListener(MAIN_PANE_SCROLL_INTENT, () => events.push("intent"));
    scrollBy.mockImplementation(() => events.push("scroll"));
    const down = key();
    expect(handleMainPanePageKey(down, main, view)).toBe(true);
    expect(down.defaultPrevented).toBe(true);
    expect(scrollBy).toHaveBeenCalledWith({ top: 720, behavior: "instant" });
    expect(events).toEqual(["intent", "scroll"]);
    expect(document.activeElement).toBe(input);
    expect([input.selectionStart, input.selectionEnd]).toEqual([3, 8]);
    expect(handleMainPanePageKey(key({ key: "PageUp" }), main, view)).toBe(true);
    expect(scrollBy).toHaveBeenLastCalledWith({ top: -720, behavior: "instant" });
  });

  it.each(["shiftKey", "ctrlKey", "metaKey", "altKey", "isComposing"])("preserves %s editor/control behavior", (modifier) => {
    const { main, scrollBy } = pane();
    const event = key({ [modifier]: true });
    expect(handleMainPanePageKey(event, main, "page")).toBe(false);
    expect(event.defaultPrevented).toBe(false);
    expect(scrollBy).not.toHaveBeenCalled();
  });

  it("ignores graphs, unrelated keys, consumed events and missing panes", () => {
    const { main, scrollBy } = pane();
    expect(handleMainPanePageKey(key(), main, "graph")).toBe(false);
    expect(handleMainPanePageKey(key({ key: "Home" }), main, "page")).toBe(false);
    expect(handleMainPanePageKey(key(), null, "page")).toBe(false);
    const prevented = key();
    prevented.preventDefault();
    expect(handleMainPanePageKey(prevented, main, "page")).toBe(false);
    expect(scrollBy).not.toHaveBeenCalled();
  });

  it("lets visible dialogs and completion menus keep their paging keys", () => {
    const { main, scrollBy } = pane();
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    Object.defineProperty(dialog, "getClientRects", { value: () => [new DOMRect(0, 0, 200, 200)] });
    document.body.append(dialog);
    expect(handleMainPanePageKey(key(), main, "journal")).toBe(false);
    expect(scrollBy).not.toHaveBeenCalled();
    dialog.style.visibility = "hidden";
    expect(handleMainPanePageKey(key(), main, "journal")).toBe(true);
  });

  it("keeps native select navigation intact", () => {
    const { main, scrollBy } = pane();
    const select = document.createElement("select");
    main.append(select);
    const event = key();
    select.dispatchEvent(event);
    expect(handleMainPanePageKey(event, main, "statistics")).toBe(false);
    expect(scrollBy).not.toHaveBeenCalled();
  });
});

import { afterEach, describe, expect, it } from "vitest";
import { readerNavigationKey, readerOwnsNavigation } from "./readerNavigation";

afterEach(() => { document.body.replaceChildren(); document.getSelection()?.removeAllRanges(); });
const key = (value: string, options: KeyboardEventInit = {}) => new KeyboardEvent("keydown", { key: value, cancelable: true, ...options });

describe("book navigation shortcuts", () => {
  it.each([["ArrowLeft", "left"], ["ArrowRight", "right"], ["PageUp", "prev"], ["PageDown", "next"]])("maps %s", (value, direction) => {
    expect(readerNavigationKey(key(value), document)).toBe(direction);
  });
  it.each(["ctrlKey", "metaKey", "altKey", "shiftKey", "isComposing"])("preserves %s", modifier => {
    expect(readerNavigationKey(key("PageDown", { [modifier]: true }), document)).toBeNull();
  });
  it.each(["input", "textarea", "select", "div[contenteditable]", "div[role=textbox]", "div[role=slider]"])("does not steal %s keys", selector => {
    const el = document.createElement(selector.split("[")[0]);
    const attribute = selector.match(/\[(\w+)(?:=(\w+))?\]/);
    if (attribute) el.setAttribute(attribute[1], attribute[2] ?? "true");
    document.body.append(el);
    const event = key("ArrowRight");
    el.dispatchEvent(event);
    expect(readerNavigationKey(event, document)).toBeNull();
  });
  it("protects selection, consumed events, and other panes", () => {
    const root = document.createElement("section");
    const outside = document.createElement("aside");
    root.textContent = "Book prose"; document.body.append(root, outside);
    const event = key("PageDown");
    outside.dispatchEvent(event);
    expect(readerOwnsNavigation(event, root)).toBe(false);
    root.dispatchEvent(event);
    expect(readerOwnsNavigation(event, root)).toBe(true);
    const range = document.createRange(); range.selectNodeContents(root);
    document.getSelection()?.addRange(range);
    expect(readerNavigationKey(event, document)).toBeNull();
    document.getSelection()?.removeAllRanges();
    event.preventDefault();
    expect(readerNavigationKey(event, document)).toBeNull();
  });
});

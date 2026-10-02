import { createRawSnippet, flushSync, mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ReaderMenu from "./ReaderMenu.svelte";
let component: ReturnType<typeof mount>;
const descriptors = ["showModal", "close"].map(name => Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, name));
beforeEach(() => {
  Object.defineProperties(HTMLDialogElement.prototype, {
    showModal: { configurable: true, value(this: HTMLDialogElement) { this.open = true; } },
    close: { configurable: true, value(this: HTMLDialogElement) { this.open = false; this.dispatchEvent(new Event("close")); } },
  });
  component = mount(ReaderMenu, { target: document.body, props: { label: "More reading actions", heading: "Reading actions",
    closeOnAction: true, children: createRawSnippet(() => ({ render: () => "<button>Favorite</button>" })) } });
  flushSync();
});
afterEach(async () => {
  await unmount(component); document.body.replaceChildren(); vi.restoreAllMocks();
  ["showModal", "close"].forEach((name, index) => {
    const descriptor = descriptors[index];
    if (descriptor) Object.defineProperty(HTMLDialogElement.prototype, name, descriptor);
    else Reflect.deleteProperty(HTMLDialogElement.prototype, name);
  });
});
describe("compact reader menus", () => {
  it("renders no hidden action buttons until opened, focuses actions and restores its trigger", async () => {
    const trigger = document.querySelector("button")!;
    expect(document.body.textContent).not.toContain("Favorite");
    trigger.click(); await vi.waitFor(() => expect(document.querySelector("dialog")?.open).toBe(true));
    const action = document.querySelector<HTMLButtonElement>(".menu-content button")!;
    expect(document.querySelector("dialog")?.open).toBe(true); expect(document.activeElement).toBe(action);
    action.click(); await tick();
    expect(document.querySelector("dialog")).toBeNull(); expect(document.activeElement).toBe(trigger);
  });
  it("closes on Escape or external F1 dismissal without hiding the parent reading bar", async () => {
    const trigger = document.querySelector("button")!;
    trigger.click(); await vi.waitFor(() => expect(document.querySelector("dialog")?.open).toBe(true));
    document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await tick(); expect(document.querySelector("dialog")).toBeNull();
    trigger.click(); await vi.waitFor(() => expect(document.querySelector("dialog")?.open).toBe(true));
    document.querySelector("dialog")!.close(); await tick();
    expect(trigger.getAttribute("aria-expanded")).toBe("false");
    expect(document.activeElement).toBe(trigger);
  });
});

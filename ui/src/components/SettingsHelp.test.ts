import { createRawSnippet, flushSync, mount, tick, unmount } from "svelte";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import SettingsHelp from "./SettingsHelp.svelte";
import { applySettingsSearch } from "../lib/settingsSearch";
import { closeSettingsHelpForContextualHelp, helpPageTitle } from "../lib/help";

let host: HTMLDivElement;
let component: ReturnType<typeof mount>;
const methods = ["showModal", "close"] as const;
const descriptors = methods.map((method) => Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, method));

beforeAll(() => {
  Object.defineProperties(HTMLDialogElement.prototype, {
    showModal: { configurable: true, writable: true, value(this: HTMLDialogElement) { this.open = true; } },
    close: { configurable: true, writable: true, value(this: HTMLDialogElement) { this.open = false; } },
  });
});
afterAll(() => {
  methods.forEach((method, index) => {
    if (descriptors[index]) Object.defineProperty(HTMLDialogElement.prototype, method, descriptors[index]!);
    else Reflect.deleteProperty(HTMLDialogElement.prototype, method);
  });
});

beforeEach(() => {
  host = document.createElement("div");
  host.innerHTML = '<details class="settings-section" data-help-context="settings" open><summary><span class="section-title">Settings</span></summary><div class="target"></div></details>';
  document.body.append(host);
  vi.spyOn(HTMLDialogElement.prototype, "showModal").mockImplementation(function (this: HTMLDialogElement) { this.open = true; });
  vi.spyOn(HTMLDialogElement.prototype, "close").mockImplementation(function (this: HTMLDialogElement) { this.open = false; });
  component = mount(SettingsHelp, {
    target: host.querySelector(".target")!,
    props: {
      title: "Attachment recovery",
      children: createRawSnippet(() => ({ render: () => "<p>Recovery keeps ZIP archives for Undo.</p>" })),
    },
  });
  flushSync();
});

afterEach(async () => {
  await unmount(component);
  host.remove();
  vi.restoreAllMocks();
});

function trigger() {
  return host.querySelector<HTMLButtonElement>('button[aria-haspopup="dialog"]')!;
}

async function open() {
  trigger().focus();
  trigger().click();
  await tick();
  flushSync();
}

describe("on-demand Settings help", () => {
  it("hides explanations initially, then opens a labelled modal and restores focus on Close", async () => {
    expect(host.querySelector("dialog")).toBeNull();
    expect(host.querySelector("[data-settings-help-text]")?.hasAttribute("hidden")).toBe(true);
    expect(trigger().textContent).toBe("?");
    expect(trigger().getAttribute("aria-label")).toBe("Help: Attachment recovery");
    await open();
    const dialog = host.querySelector("dialog")!;
    expect(dialog.open).toBe(true);
    expect(dialog.textContent).toContain("Recovery keeps ZIP archives for Undo.");
    expect(dialog.querySelector("h2")?.id).toBe(dialog.getAttribute("aria-labelledby"));
    expect(trigger().getAttribute("aria-expanded")).toBe("true");
    expect(document.activeElement).toBe(dialog.querySelector("button"));
    dialog.querySelector("button")!.click();
    await tick();
    expect(host.querySelector("dialog")).toBeNull();
    expect(document.activeElement).toBe(trigger());
    expect(trigger().getAttribute("aria-expanded")).toBe("false");
  });

  it("handles Escape and native cancel without closing the underlying Settings view", async () => {
    await open();
    const escaped = vi.fn();
    host.addEventListener("keydown", escaped);
    document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await tick();
    expect(escaped).not.toHaveBeenCalled();
    expect(host.querySelector("dialog")).toBeNull();
    expect(host.querySelector("details")?.open).toBe(true);
    await open();
    const event = new Event("cancel", { cancelable: true });
    host.querySelector("dialog")!.dispatchEvent(event);
    await tick();
    expect(event.defaultPrevented).toBe(true);
    expect(host.querySelector("dialog")).toBeNull();
    expect(document.activeElement).toBe(trigger());
  });

  it("keeps Tab focus within help", async () => {
    await open();
    const close = host.querySelector<HTMLButtonElement>("dialog button")!;
    const rect = new DOMRect(0, 0, 20, 20);
    vi.spyOn(close, "getClientRects").mockReturnValue(Object.assign([rect], {
      item: (index: number) => index === 0 ? rect : null,
    }));
    for (const shiftKey of [false, true]) {
      const tab = new KeyboardEvent("keydown", { key: "Tab", shiftKey, bubbles: true, cancelable: true });
      close.dispatchEvent(tab);
      expect(tab.defaultPrevented).toBe(true);
      expect(document.activeElement).toBe(close);
    }
  });

  it("searches hidden help without expanding prose and preserves search through open/close", async () => {
    expect(applySettingsSearch(host, "ZIP archives").sections).toBe(1);
    expect(trigger().closest<HTMLElement>(".settings-help")?.hidden).toBe(false);
    expect(host.querySelector("[data-settings-help-text]")?.hasAttribute("hidden")).toBe(true);
    expect(host.querySelector("dialog")).toBeNull();
    await open();
    expect(applySettingsSearch(host, "ZIP archives").sections).toBe(1);
    host.querySelector<HTMLButtonElement>("dialog button")!.click();
    await tick();
    applySettingsSearch(host, "");
    expect(host.querySelector("[data-settings-help-text]")?.hasAttribute("hidden")).toBe(true);
    expect(trigger().hidden).toBe(false);
  });

  it("dismisses the native modal before F1 opens the existing section help", async () => {
    await open();
    const focused = document.activeElement!;
    expect(focused.closest("[data-help-context]")?.getAttribute("data-help-context")).toBe("settings");
    expect(helpPageTitle("settings")).toBe("Help - Settings");
    closeSettingsHelpForContextualHelp(focused);
    await tick();
    expect(host.querySelector("dialog")).toBeNull();
    expect(document.activeElement).toBe(trigger());
  });

  it("reports a native dialog failure instead of leaving the help control stuck", async () => {
    vi.mocked(HTMLDialogElement.prototype.showModal).mockImplementationOnce(() => { throw new Error("unavailable"); });
    await open();
    await vi.waitFor(() => expect(host.querySelector('[role="alert"]')?.textContent).toContain("Could not open help: Error: unavailable"));
    expect(trigger().getAttribute("aria-expanded")).toBe("false");
    await open();
    expect(host.querySelector("dialog")?.open).toBe(true);
    expect(host.querySelector('[role="alert"]')).toBeNull();
  });
});

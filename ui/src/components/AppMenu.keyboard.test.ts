import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import AppMenu from "./AppMenu.svelte";
import GraphMenu from "./GraphMenu.svelte";
import { keymap_manager } from "../lib/keymap";
import { bionicReaderEnabled } from "../lib/bionicReader";
import { get } from "svelte/store";

/**
 * The seeded help promises `↑`/`↓` move through the menus opened from a button.
 * `menuKeyboard.test.ts` proves the handler itself; these prove the *wiring* —
 * that each dropdown is a real `role="menu"` that takes the keyboard when it
 * opens, exposes its buttons as `role="menuitem"`, and hands focus back to its
 * trigger on the way out. Without the focus move the arrow keys never reach the
 * container at all, so asserting on markup alone would not catch a regression.
 */

vi.mock("../lib/api", () => ({
  getAppVersion: () => Promise.resolve("0.0.0"),
  getGraphInfo: () => Promise.resolve(null),
  listGraphs: () => Promise.resolve([]),
  openGraph: () => Promise.resolve(null),
  validateGraph: () => Promise.resolve({ valid: true }),
  createGraph: () => Promise.resolve(null),
  reindexGraph: () => Promise.resolve(null),
  getRecentGraphs: () => Promise.resolve([]),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: () => Promise.resolve(null) }));

let host: HTMLElement;
let component: Record<string, unknown> | null = null;

beforeEach(() => {
  host = document.createElement("div");
  document.body.appendChild(host);
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  host.remove();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  keymap_manager.register([]);
});

it("shows platform-correct zoom and registered settings hints without claiming About has a shortcut", () => {
  vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
  keymap_manager.register([
    { id: "toggle-settings", binding: "t s", action: () => {} },
    { id: "toggle-settings", binding: "alt+s", action: () => {}, navOnly: false },
  ]);
  component = mount(AppMenu, { target: host });
  flushSync();
  const menu = openMenu(host.querySelector<HTMLElement>(".menu-trigger")!);
  const zoom = menu.querySelector('[aria-label="Zoom in"]');
  // Both default keys: Cmd-= and, on layouts where it needs Shift, Cmd-+.
  expect(zoom?.getAttribute("title")).toBe("Zoom in (Cmd-= | Cmd-+)");
  expect(zoom?.getAttribute("aria-keyshortcuts")).toBe("Meta+= Meta+Plus");
  const settings = menu.querySelector('[title="Settings (t s | Alt-S)"]');
  expect(settings?.getAttribute("aria-keyshortcuts")).toBe("Alt+s");
  const about = [...menu.querySelectorAll("button")].find(button => button.textContent?.includes("About"));
  expect(about?.getAttribute("aria-keyshortcuts")).toBeNull();
});

it("keeps Bionic discoverable in the app menu and shares its global preference", () => {
  bionicReaderEnabled.set(false);
  const setItem = vi.fn();
  vi.stubGlobal("localStorage", { setItem });
  component = mount(AppMenu, { target: host });
  flushSync();
  const menu = openMenu(host.querySelector<HTMLElement>(".menu-trigger")!);
  const toggle = [...menu.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')]
    .find(button => button.textContent?.includes("Bionic reading: Off"))!;
  expect(toggle.getAttribute("aria-keyshortcuts")).toContain("Alt+b");
  toggle.click(); flushSync();
  expect(get(bionicReaderEnabled)).toBe(true);
  expect(setItem).toHaveBeenCalledWith("grafium.reader.bionic", "1");
  expect(host.querySelector('[role="menu"]')).toBeNull();
  bionicReaderEnabled.set(false);
});

function openMenu(trigger: HTMLElement): HTMLElement {
  trigger.focus();
  trigger.click();
  flushSync();
  const menu = host.querySelector<HTMLElement>('[role="menu"]');
  if (!menu) throw new Error("menu did not open");
  return menu;
}

function press(menu: HTMLElement, key: string): void {
  menu.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
  flushSync();
}

describe.each([
  { name: "AppMenu", Component: AppMenu, triggerSelector: ".menu-trigger" },
  { name: "GraphMenu", Component: GraphMenu, triggerSelector: ".graph-selector" },
])("$name dropdown keyboard navigation", ({ Component, triggerSelector }) => {
  function render(): HTMLElement {
    component = mount(Component as never, { target: host, props: {} }) as Record<string, unknown>;
    flushSync();
    const trigger = host.querySelector<HTMLElement>(triggerSelector);
    if (!trigger) throw new Error(`no trigger matching ${triggerSelector}`);
    return trigger;
  }

  it("moves focus into the menu when it opens", () => {
    const trigger = render();
    const menu = openMenu(trigger);
    expect(document.activeElement).toBe(menu);
  });

  it("exposes its actions as menu items", () => {
    const trigger = render();
    const menu = openMenu(trigger);
    expect(menu.querySelectorAll('[role="menuitem"]').length).toBeGreaterThan(0);
  });

  it("walks the items with ArrowDown and wraps back to the first", () => {
    const trigger = render();
    const menu = openMenu(trigger);
    const items = Array.from(menu.querySelectorAll<HTMLElement>('[role="menuitem"]'));

    press(menu, "ArrowDown");
    expect(document.activeElement).toBe(items[0]);

    for (let i = 1; i < items.length; i++) {
      press(menu, "ArrowDown");
      expect(document.activeElement).toBe(items[i]);
    }

    press(menu, "ArrowDown");
    expect(document.activeElement).toBe(items[0]);
  });

  it("jumps to the last item with ArrowUp from the container", () => {
    const trigger = render();
    const menu = openMenu(trigger);
    const items = Array.from(menu.querySelectorAll<HTMLElement>('[role="menuitem"]'));

    press(menu, "ArrowUp");
    expect(document.activeElement).toBe(items[items.length - 1]);
  });

  it("closes on Escape and returns focus to the trigger", () => {
    const trigger = render();
    const menu = openMenu(trigger);

    press(menu, "Escape");

    expect(host.querySelector('[role="menu"]')).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it("leaves ordinary typing alone so it still reaches the app", () => {
    const trigger = render();
    const menu = openMenu(trigger);

    const event = new KeyboardEvent("keydown", { key: "a", bubbles: true, cancelable: true });
    menu.dispatchEvent(event);
    flushSync();

    expect(event.defaultPrevented).toBe(false);
    expect(host.querySelector('[role="menu"]')).not.toBeNull();
  });
});

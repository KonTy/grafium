import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import ContextMenuHost from "./ContextMenuHost.svelte";
import {
  CONTEXT_MENU_CLASS,
  closeContextMenu,
  getOpenContextMenu,
  openContextMenu,
} from "../lib/contextMenuHost.svelte";
import type { ContextMenuAction, ContextMenuItem, ContextMenuSection } from "../lib/contextMenuSections";

vi.mock("../lib/editActions", () => ({ buildEditSection: () => null }));

function action(id: string, extra: Partial<ContextMenuAction> = {}): ContextMenuAction {
  return { id, label: id, onSelect: () => {}, ...extra };
}

function section(id: string, items: ContextMenuItem[], extra: Partial<ContextMenuSection> = {}): ContextMenuSection {
  return { id, items, ...extra };
}

async function settle() {
  await Promise.resolve();
  await tick();
  flushSync();
}

const menuEl = () => document.querySelector<HTMLElement>(".ctx-menu");
const itemEls = () => Array.from(document.querySelectorAll<HTMLElement>(".ctx-menu [role='menuitem']"));
const labelsOf = () => itemEls().map((el) => el.querySelector(".ctx-menu-label")?.textContent ?? el.textContent?.trim());

function rect(left: number, top: number, width: number, height: number): DOMRect {
  return {
    x: left,
    y: top,
    left,
    top,
    width,
    height,
    right: left + width,
    bottom: top + height,
    toJSON: () => ({}),
  };
}

interface Geometry {
  zoom: number;
  menu: { width: number; height: number };
  /** Where the ruler lands inside a dialog, as if the dialog were the containing block. */
  originInDialog?: { x: number; y: number };
}

/** Pretends the page is zoomed: everything is `zoom` times larger than its CSS size. */
function stubGeometry(geometry: Geometry) {
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
    if (this.classList.contains("ctx-menu-ruler")) {
      const origin = (this.closest("dialog") && geometry.originInDialog) || { x: 0, y: 0 };
      return rect(origin.x, origin.y, 1000 * geometry.zoom, 0);
    }
    if (this.classList.contains("ctx-menu")) return rect(0, 0, geometry.menu.width, geometry.menu.height);
    return rect(0, 0, 0, 0);
  });
}

function setViewport(width: number, height: number) {
  Object.defineProperty(window, "innerWidth", { value: width, configurable: true });
  Object.defineProperty(window, "innerHeight", { value: height, configurable: true });
}

let host: ReturnType<typeof mount> | null = null;

beforeEach(() => {
  document.body.innerHTML = "";
  setViewport(1000, 800);
  host = mount(ContextMenuHost, { target: document.body });
  flushSync();
});

afterEach(() => {
  closeContextMenu({ restoreFocus: false });
  flushSync();
  if (host) unmount(host);
  host = null;
  vi.restoreAllMocks();
});

describe("ContextMenuHost", () => {
  it("draws nothing while closed", () => {
    expect(menuEl()).toBeNull();
    expect(document.querySelector(".ctx-menu-ruler")).not.toBeNull();
    expect(document.querySelector(".ctx-menu-ruler")?.getAttribute("aria-hidden")).toBe("true");
  });

  it("draws one menu with a section per group and a separator between them", async () => {
    openContextMenu({
      anchor: { x: 10, y: 10 },
      label: "Page actions",
      sections: [
        section("page", [action("page.rename", { label: "Rename page" })], { ariaLabel: "Page" }),
        section("size", [action("size.original", { label: "Original Size", hint: "100%" })], { title: "Size" }),
      ],
    });
    await settle();

    const menu = menuEl()!;
    expect(menu.classList.contains(CONTEXT_MENU_CLASS)).toBe(true);
    expect(menu.classList.contains("app-context-menu")).toBe(true);
    expect(menu.getAttribute("role")).toBe("menu");
    expect(menu.getAttribute("aria-label")).toBe("Page actions");
    expect(document.querySelectorAll(".ctx-menu")).toHaveLength(1);

    const groups = Array.from(menu.querySelectorAll("[role='group']"));
    expect(groups.map((g) => g.getAttribute("aria-label"))).toEqual(["Page", "Size"]);
    expect(menu.querySelectorAll("[role='separator']")).toHaveLength(1);
    expect(menu.querySelector(".ctx-menu-title")?.textContent).toBe("Size");
    expect(menu.querySelector(".ctx-menu-title")?.getAttribute("aria-hidden")).toBe("true");
    expect(menu.querySelector(".ctx-menu-hint")?.textContent).toBe("100%");
    expect(labelsOf()).toEqual(["Rename page", "Original Size"]);
  });

  it("marks danger and emphasis choices, disabled items and status lines", async () => {
    openContextMenu({
      anchor: { x: 10, y: 10 },
      sections: [
        section("a", [
          action("a.delete", { label: "Delete page", danger: true }),
          action("a.fix", { label: "Fix", emphasis: true }),
          action("a.off", { label: "Off", disabled: true, title: "Not now" }),
          { kind: "status", id: "a.note", label: "No suggestions" },
        ]),
      ],
    });
    await settle();

    const [del, fix, off, note] = itemEls();
    expect(del.classList.contains("danger")).toBe(true);
    expect(fix.classList.contains("emphasis")).toBe(true);
    expect((off as HTMLButtonElement).disabled).toBe(true);
    expect(off.getAttribute("title")).toBe("Not now");
    expect(note.getAttribute("aria-disabled")).toBe("true");
    expect(note.tagName).toBe("DIV");
    expect(note.textContent).toBe("No suggestions");
  });

  it("focuses the menu itself, not its first item", async () => {
    openContextMenu({ anchor: { x: 10, y: 10 }, sections: [section("a", [action("a.delete")])] });
    await settle();
    expect(document.activeElement).toBe(menuEl());
  });

  it("runs the chosen action once, closes, and gives focus back", async () => {
    const field = document.createElement("input");
    document.body.append(field);
    field.focus();
    const onSelect = vi.fn();
    openContextMenu({ anchor: { x: 10, y: 10 }, sections: [section("a", [action("a.go", { onSelect })])] });
    await settle();

    (itemEls()[0] as HTMLButtonElement).click();
    await settle();

    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(menuEl()).toBeNull();
    expect(document.activeElement).toBe(field);
  });

  it("does not run a disabled item", async () => {
    const onSelect = vi.fn();
    openContextMenu({
      anchor: { x: 10, y: 10 },
      sections: [section("a", [action("a.go", { onSelect, disabled: true })])],
    });
    await settle();
    (itemEls()[0] as HTMLButtonElement).click();
    await settle();
    expect(onSelect).not.toHaveBeenCalled();
    expect(menuEl()).not.toBeNull();
  });

  it("swallows a right-click on itself instead of opening another menu", async () => {
    openContextMenu({ anchor: { x: 10, y: 10 }, sections: [section("a", [action("a.go")])] });
    await settle();
    const event = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    menuEl()!.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(menuEl()).not.toBeNull();
  });

  it("keeps a click inside from reaching the page", async () => {
    const pageClick = vi.fn();
    document.addEventListener("click", pageClick);
    openContextMenu({ anchor: { x: 10, y: 10 }, sections: [section("a", [action("a.go")])] });
    await settle();
    (itemEls()[0] as HTMLButtonElement).click();
    document.removeEventListener("click", pageClick);
    expect(pageClick).not.toHaveBeenCalled();
  });

  it("does not take focus for a mouse press, which would clear the page's selection", async () => {
    openContextMenu({ anchor: { x: 10, y: 10 }, sections: [section("a", [action("a.go")])] });
    await settle();
    const press = new MouseEvent("mousedown", { bubbles: true, cancelable: true });
    itemEls()[0].dispatchEvent(press);
    expect(press.defaultPrevented).toBe(true);
  });
});

describe("placement", () => {
  it("opens at the pointer when the page is not zoomed", async () => {
    stubGeometry({ zoom: 1, menu: { width: 200, height: 100 } });
    openContextMenu({ anchor: { x: 400, y: 300 }, sections: [section("a", [action("a.go")])] });
    await settle();
    const menu = menuEl()!;
    expect(menu.style.left).toBe("400px");
    expect(menu.style.top).toBe("300px");
    expect(menu.style.visibility).toBe("visible");
  });

  it("opens at the pointer when the page is zoomed in, converting to the zoomed pixels the style needs", async () => {
    stubGeometry({ zoom: 1.25, menu: { width: 250, height: 125 } });
    openContextMenu({ anchor: { x: 500, y: 300 }, sections: [section("a", [action("a.go")])] });
    await settle();
    const menu = menuEl()!;
    expect(menu.style.left).toBe("400px");
    expect(menu.style.top).toBe("240px");
  });

  it("opens at the pointer when the page is zoomed out", async () => {
    stubGeometry({ zoom: 0.7, menu: { width: 140, height: 70 } });
    openContextMenu({ anchor: { x: 350, y: 140 }, sections: [section("a", [action("a.go")])] });
    await settle();
    const menu = menuEl()!;
    expect(parseFloat(menu.style.left)).toBeCloseTo(500, 5);
    expect(parseFloat(menu.style.top)).toBeCloseTo(200, 5);
  });

  it("flips to the other side of the pointer near the edges and stays on screen", async () => {
    stubGeometry({ zoom: 1, menu: { width: 200, height: 100 } });
    openContextMenu({ anchor: { x: 950, y: 780 }, sections: [section("a", [action("a.go")])] });
    await settle();
    const menu = menuEl()!;
    expect(menu.style.left).toBe("750px");
    expect(menu.style.top).toBe("680px");
  });

  it("flips around a button's top edge for a menu opened from that button", async () => {
    stubGeometry({ zoom: 1, menu: { width: 200, height: 100 } });
    openContextMenu({
      anchor: { x: 100, y: 790 },
      flipAnchor: { x: 100, y: 760 },
      sections: [section("a", [action("a.go")])],
    });
    await settle();
    expect(menuEl()!.style.top).toBe("660px");
  });

  it("limits the height to the viewport so a long menu scrolls", async () => {
    stubGeometry({ zoom: 1, menu: { width: 200, height: 2000 } });
    openContextMenu({ anchor: { x: 100, y: 100 }, sections: [section("a", [action("a.go")])] });
    await settle();
    const menu = menuEl()!;
    expect(menu.style.maxHeight).toBe("784px");
    expect(menu.style.top).toBe("8px");
  });

  it("places the menu again when late items change its size", async () => {
    const size = { width: 200, height: 100 };
    vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
      if (this.classList.contains("ctx-menu-ruler")) return rect(0, 0, 1000, 0);
      if (this.classList.contains("ctx-menu")) return rect(0, 0, size.width, size.height);
      return rect(0, 0, 0, 0);
    });
    openContextMenu({
      anchor: { x: 100, y: 700 },
      sections: [
        section("spelling", [{ kind: "status", id: "spelling.loading", label: "Looking…" }], {
          resolve: async () => {
            size.height = 300;
            return [action("spell.one", { label: "one" }), action("spell.two", { label: "two" })];
          },
        }),
      ],
    });
    await settle();
    await settle();

    expect(labelsOf()).toEqual(["one", "two"]);
    expect(menuEl()!.style.top).toBe("400px");
  });

  it("measures from the corner, so an earlier position cannot squeeze the menu", async () => {
    const leftWhenMeasured: string[] = [];
    vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
      if (this.classList.contains("ctx-menu-ruler")) return rect(0, 0, 1000, 0);
      if (this.classList.contains("ctx-menu")) {
        leftWhenMeasured.push((this as HTMLElement).style.left);
        return rect(0, 0, 200, 100);
      }
      return rect(0, 0, 0, 0);
    });
    openContextMenu({ anchor: { x: 900, y: 100 }, sections: [section("a", [action("a.go")])] });
    await settle();
    openContextMenu({ anchor: { x: 100, y: 100 }, sections: [section("b", [action("b.go")])] });
    await settle();
    expect(leftWhenMeasured.every((left) => left === "0px")).toBe(true);
  });
});

describe("keyboard", () => {
  async function openThree() {
    openContextMenu({
      anchor: { x: 10, y: 10 },
      sections: [
        section("a", [action("a.one", { label: "One" }), action("a.two", { label: "Two", disabled: true })]),
        section("b", [action("b.three", { label: "Three" })]),
      ],
    });
    await settle();
  }

  const press = (key: string) => {
    const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
    (document.activeElement ?? document.body).dispatchEvent(event);
    return event;
  };

  it("moves between enabled items with the arrow keys, wrapping around", async () => {
    await openThree();
    press("ArrowDown");
    expect(document.activeElement?.textContent).toContain("One");
    press("ArrowDown");
    expect(document.activeElement?.textContent).toContain("Three");
    press("ArrowDown");
    expect(document.activeElement?.textContent).toContain("One");
    press("ArrowUp");
    expect(document.activeElement?.textContent).toContain("Three");
    press("Home");
    expect(document.activeElement?.textContent).toContain("One");
    press("End");
    expect(document.activeElement?.textContent).toContain("Three");
  });

  it("closes on Escape and gives focus back", async () => {
    const field = document.createElement("input");
    document.body.append(field);
    field.focus();
    await openThree();
    const event = press("Escape");
    await settle();
    expect(event.defaultPrevented).toBe(true);
    expect(menuEl()).toBeNull();
    expect(document.activeElement).toBe(field);
  });

  it("closes on Tab instead of moving focus into the page", async () => {
    const field = document.createElement("input");
    document.body.append(field);
    field.focus();
    await openThree();
    const event = press("Tab");
    await settle();
    expect(event.defaultPrevented).toBe(true);
    expect(menuEl()).toBeNull();
    expect(document.activeElement).toBe(field);
  });

  it("runs the focused item on Enter's click", async () => {
    const onSelect = vi.fn();
    openContextMenu({ anchor: { x: 10, y: 10 }, sections: [section("a", [action("a.go", { onSelect })])] });
    await settle();
    press("ArrowDown");
    (document.activeElement as HTMLElement).click();
    expect(onSelect).toHaveBeenCalledTimes(1);
  });

  it("keeps keys from reaching the page behind it, where Delete would remove the selected blocks", async () => {
    const seen: string[] = [];
    const listener = (event: KeyboardEvent) => seen.push(event.key);
    window.addEventListener("keydown", listener);
    try {
      await openThree();
      for (const key of ["Delete", "Backspace", "x", "F1", "ArrowDown", "Tab"]) {
        await openThree();
        press(key);
      }
      await openThree();
      press("Escape");
    } finally {
      window.removeEventListener("keydown", listener);
    }
    expect(seen).toEqual([]);
  });
});

describe("dismissal", () => {
  beforeEach(async () => {
    openContextMenu({ anchor: { x: 10, y: 10 }, sections: [section("a", [action("a.go")])] });
    await settle();
  });

  it("closes on a press outside, and gives focus back so the page behaves as usual", async () => {
    const field = document.createElement("input");
    document.body.append(field);
    closeContextMenu({ restoreFocus: false });
    field.focus();
    openContextMenu({ anchor: { x: 10, y: 10 }, sections: [section("a", [action("a.go")])] });
    await settle();
    field.blur();

    document.body.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    await settle();
    expect(menuEl()).toBeNull();
    expect(document.activeElement).toBe(field);
  });

  it("stays open for a press inside", async () => {
    itemEls()[0].dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    menuEl()!.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    await settle();
    expect(menuEl()).not.toBeNull();
  });

  it("closes when the page scrolls under it, but not when the menu itself scrolls", async () => {
    itemEls()[0].dispatchEvent(new Event("wheel", { bubbles: true }));
    await settle();
    expect(menuEl()).not.toBeNull();

    document.body.dispatchEvent(new Event("wheel", { bubbles: true }));
    await settle();
    expect(menuEl()).toBeNull();
  });

  it("closes when the window is resized", async () => {
    window.dispatchEvent(new Event("resize"));
    await settle();
    expect(menuEl()).toBeNull();
  });

  it("closes, and gives focus back, when the window loses focus", async () => {
    const other = document.createElement("input");
    document.body.append(other);
    other.focus();
    openContextMenu({ anchor: { x: 10, y: 10 }, sections: [section("a", [action("a.go")])] });
    await settle();
    other.blur();
    window.dispatchEvent(new Event("blur"));
    await settle();
    expect(menuEl()).toBeNull();
    expect(document.activeElement).toBe(other);
  });

  it("ignores blur events from elements, which do not bubble but are captured", async () => {
    const button = itemEls()[0];
    button.dispatchEvent(new Event("blur", { bubbles: false }));
    await settle();
    expect(menuEl()).not.toBeNull();
  });

  it("closes without taking focus when something outside takes it", async () => {
    const other = document.createElement("input");
    document.body.append(other);
    menuEl()!.dispatchEvent(new FocusEvent("focusout", { bubbles: true, relatedTarget: other }));
    await settle();
    expect(menuEl()).toBeNull();
  });

  it("stays open when focus moves within the menu or nowhere in particular", async () => {
    const item = itemEls()[0];
    menuEl()!.dispatchEvent(new FocusEvent("focusout", { bubbles: true, relatedTarget: item }));
    menuEl()!.dispatchEvent(new FocusEvent("focusout", { bubbles: true, relatedTarget: null }));
    await settle();
    expect(menuEl()).not.toBeNull();
  });

  it("stops listening once closed", async () => {
    closeContextMenu();
    await settle();
    expect(getOpenContextMenu()).toBeNull();
    openContextMenu({ anchor: { x: 10, y: 10 }, sections: [section("b", [action("b.go")])] });
    await settle();
    const removed = vi.spyOn(window, "removeEventListener");
    closeContextMenu();
    await settle();
    expect(removed.mock.calls.map(([type]) => type)).toEqual(
      expect.arrayContaining(["pointerdown", "wheel", "resize", "blur"]),
    );
  });
});

const containerEl = () => document.querySelector<HTMLElement>(".ctx-menu-container");
const rulerEl = () => document.querySelector<HTMLElement>(".ctx-menu-ruler");

/** jsdom has no top layer, so say which elements the engine would report as in it. */
function stubTopLayer(known: { modal?: Element[]; popover?: Element[] }) {
  const real = Element.prototype.matches;
  vi.spyOn(Element.prototype, "matches").mockImplementation(function (this: Element, selector: string) {
    if (selector === ":modal") return known.modal?.includes(this) ?? false;
    if (selector === ":popover-open") return known.popover?.includes(this) ?? false;
    return real.call(this, selector);
  });
}

function modalDialogWith(...children: HTMLElement[]): HTMLDialogElement {
  const dialog = document.createElement("dialog");
  dialog.setAttribute("open", "");
  dialog.append(...children);
  document.body.append(dialog);
  return dialog;
}

function toggle(newState: "open" | "closed"): Event {
  return Object.assign(new Event("toggle"), { newState, oldState: newState === "open" ? "closed" : "open" });
}

describe("where the menu is drawn", () => {
  const sections = () => [section("a", [action("a.go")])];

  it("is a node of its own in the body, holding the ruler and the menu", async () => {
    expect(containerEl()?.parentElement).toBe(document.body);
    expect(containerEl()?.contains(rulerEl())).toBe(true);
    openContextMenu({ anchor: { x: 10, y: 10 }, sections: sections() });
    await settle();
    expect(containerEl()?.contains(menuEl())).toBe(true);
  });

  it("is taken out of the page with the host", () => {
    unmount(host!);
    host = null;
    flushSync();
    expect(containerEl()).toBeNull();
    expect(rulerEl()).toBeNull();
  });

  it("stays where it is for menus on the page itself", async () => {
    const field = document.createElement("input");
    document.body.append(field);
    const moved = vi.spyOn(document.body, "append");
    openContextMenu({ anchor: { x: 10, y: 10 }, target: field, sections: sections() });
    await settle();
    openContextMenu({ anchor: { x: 20, y: 20 }, target: field, sections: sections() });
    await settle();
    expect(moved).not.toHaveBeenCalled();
    expect(containerEl()?.parentElement).toBe(document.body);
  });

  it("moves into the modal dialog around what was right-clicked, which is drawn over the page", async () => {
    const field = document.createElement("input");
    const dialog = modalDialogWith(field);
    stubTopLayer({ modal: [dialog] });
    field.focus();

    openContextMenu({ anchor: { x: 10, y: 10 }, target: field, sections: sections() });
    await settle();

    expect(containerEl()?.parentElement).toBe(dialog);
    expect(dialog.contains(menuEl())).toBe(true);
    expect(menuEl()?.style.visibility).toBe("visible");
    expect(document.activeElement).toBe(menuEl());
  });

  it("goes back to the page when the menu closes, so nothing is left in the dialog", async () => {
    const field = document.createElement("input");
    const dialog = modalDialogWith(field);
    stubTopLayer({ modal: [dialog] });
    openContextMenu({ anchor: { x: 10, y: 10 }, target: field, sections: sections() });
    await settle();
    expect(containerEl()?.parentElement).toBe(dialog);

    closeContextMenu();
    await settle();
    expect(containerEl()?.parentElement).toBe(document.body);
    expect(dialog.querySelector(".ctx-menu, .ctx-menu-ruler")).toBeNull();
  });

  it("follows the next menu to another dialog, and to the page", async () => {
    const first = document.createElement("input");
    const second = document.createElement("input");
    const one = modalDialogWith(first);
    const two = modalDialogWith(second);
    const field = document.createElement("input");
    document.body.append(field);
    stubTopLayer({ modal: [one, two] });

    openContextMenu({ anchor: { x: 10, y: 10 }, target: first, sections: sections() });
    await settle();
    expect(containerEl()?.parentElement).toBe(one);

    openContextMenu({ anchor: { x: 10, y: 10 }, target: second, sections: sections() });
    await settle();
    expect(containerEl()?.parentElement).toBe(two);
    expect(getOpenContextMenu()).not.toBeNull();
    expect(document.activeElement).toBe(menuEl());

    openContextMenu({ anchor: { x: 10, y: 10 }, target: field, returnFocus: field, sections: sections() });
    await settle();
    expect(containerEl()?.parentElement).toBe(document.body);
    expect(document.activeElement).toBe(menuEl());
  });

  it("is placed from where it is drawn, so a dialog that offsets its content does not shift it", async () => {
    const field = document.createElement("input");
    const dialog = modalDialogWith(field);
    stubTopLayer({ modal: [dialog] });
    stubGeometry({ zoom: 1.5, menu: { width: 150, height: 75 }, originInDialog: { x: 90, y: 30 } });

    openContextMenu({ anchor: { x: 690, y: 330 }, target: field, sections: sections() });
    await settle();

    expect(menuEl()?.style.left).toBe("400px");
    expect(menuEl()?.style.top).toBe("200px");
  });

  it("closes, without taking focus, when the dialog closes", async () => {
    const field = document.createElement("input");
    const dialog = modalDialogWith(field);
    stubTopLayer({ modal: [dialog] });
    field.focus();
    openContextMenu({ anchor: { x: 10, y: 10 }, target: field, sections: sections() });
    await settle();

    dialog.dispatchEvent(new Event("close"));
    await settle();
    expect(getOpenContextMenu()).toBeNull();
    expect(menuEl()).toBeNull();
    expect(document.activeElement).not.toBe(field);
    expect(containerEl()?.parentElement).toBe(document.body);
  });

  it("closes when the popover it is drawn in is hidden, but not when it is shown", async () => {
    const button = document.createElement("button");
    const popover = document.createElement("div");
    popover.setAttribute("popover", "");
    popover.append(button);
    document.body.append(popover);
    stubTopLayer({ popover: [popover] });
    openContextMenu({ anchor: { x: 10, y: 10 }, target: button, sections: sections() });
    await settle();
    expect(containerEl()?.parentElement).toBe(popover);

    popover.dispatchEvent(toggle("open"));
    await settle();
    expect(getOpenContextMenu()).not.toBeNull();

    popover.dispatchEvent(toggle("closed"));
    await settle();
    expect(getOpenContextMenu()).toBeNull();
    expect(containerEl()?.parentElement).toBe(document.body);
  });

  it("is not closed by another dialog closing", async () => {
    const field = document.createElement("input");
    document.body.append(field);
    const other = modalDialogWith(document.createElement("input"));
    openContextMenu({ anchor: { x: 10, y: 10 }, target: field, sections: sections() });
    await settle();

    other.dispatchEvent(new Event("close"));
    await settle();
    expect(getOpenContextMenu()).not.toBeNull();
  });

  it("is dropped if the dialog was taken out of the page before it could be drawn", async () => {
    const field = document.createElement("input");
    const dialog = modalDialogWith(field);
    stubTopLayer({ modal: [dialog] });
    openContextMenu({ anchor: { x: 10, y: 10 }, target: field, sections: sections() });
    dialog.remove();
    await settle();
    expect(getOpenContextMenu()).toBeNull();
    expect(containerEl()?.parentElement).toBe(document.body);
  });

  it("keeps Escape from reaching the dialog, so the first press closes only the menu", async () => {
    const field = document.createElement("input");
    const dialog = modalDialogWith(field);
    stubTopLayer({ modal: [dialog] });
    const seen = vi.fn();
    dialog.addEventListener("keydown", seen);
    field.focus();
    openContextMenu({ anchor: { x: 10, y: 10 }, target: field, sections: sections() });
    await settle();

    const escape = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
    menuEl()!.dispatchEvent(escape);
    await settle();

    expect(getOpenContextMenu()).toBeNull();
    expect(escape.defaultPrevented).toBe(true);
    expect(seen).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(field);
  });

  it("keeps a click on an item from reaching the dialog, where a click outside its content closes it", async () => {
    const field = document.createElement("input");
    const dialog = modalDialogWith(field);
    stubTopLayer({ modal: [dialog] });
    const seen = vi.fn();
    dialog.addEventListener("click", seen);
    const onSelect = vi.fn();
    openContextMenu({
      anchor: { x: 10, y: 10 },
      target: field,
      sections: [section("a", [action("a.go", { onSelect })])],
    });
    await settle();

    itemEls()[0].click();
    await settle();
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(seen).not.toHaveBeenCalled();
  });
});

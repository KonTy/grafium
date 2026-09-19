import { describe, expect, it, beforeEach, vi } from "vitest";
import { handleMenuKeydown, menuItems } from "./menuKeyboard";

function buildMenu(disabledIndex: number | null = null): HTMLElement {
  const menu = document.createElement("div");
  menu.setAttribute("role", "menu");
  menu.tabIndex = -1;
  ["First", "Second", "Third"].forEach((label, index) => {
    const item = document.createElement("button");
    item.type = "button";
    item.setAttribute("role", "menuitem");
    item.textContent = label;
    if (index === disabledIndex) item.disabled = true;
    menu.appendChild(item);
  });
  document.body.appendChild(menu);
  return menu;
}

function press(menu: HTMLElement, key: string): KeyboardEvent {
  const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
  menu.dispatchEvent(event);
  return event;
}

describe("menuKeyboard", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("closes the menu on Escape and keeps the key from leaking outwards", () => {
    const menu = buildMenu();
    const close = vi.fn();
    menu.addEventListener("keydown", (event) => handleMenuKeydown(event, close));

    const event = press(menu, "Escape");

    expect(close).toHaveBeenCalledTimes(1);
    expect(event.defaultPrevented).toBe(true);
  });

  it("moves focus to the first item on ArrowDown and wraps at the end", () => {
    const menu = buildMenu();
    menu.addEventListener("keydown", (event) => handleMenuKeydown(event, () => {}));
    menu.focus();

    press(menu, "ArrowDown");
    expect(document.activeElement?.textContent).toBe("First");
    press(menu, "ArrowDown");
    press(menu, "ArrowDown");
    expect(document.activeElement?.textContent).toBe("Third");
    press(menu, "ArrowDown");
    expect(document.activeElement?.textContent).toBe("First");
  });

  it("wraps backwards on ArrowUp and jumps with Home/End", () => {
    const menu = buildMenu();
    menu.addEventListener("keydown", (event) => handleMenuKeydown(event, () => {}));
    menu.focus();

    press(menu, "ArrowUp");
    expect(document.activeElement?.textContent).toBe("Third");
    press(menu, "Home");
    expect(document.activeElement?.textContent).toBe("First");
    press(menu, "End");
    expect(document.activeElement?.textContent).toBe("Third");
  });

  it("skips disabled items", () => {
    const menu = buildMenu(1);
    menu.addEventListener("keydown", (event) => handleMenuKeydown(event, () => {}));
    menu.focus();

    expect(menuItems(menu)).toHaveLength(2);
    press(menu, "ArrowDown");
    expect(document.activeElement?.textContent).toBe("First");
    press(menu, "ArrowDown");
    expect(document.activeElement?.textContent).toBe("Third");
  });

  it("ignores keys that are not menu navigation", () => {
    const menu = buildMenu();
    const close = vi.fn();
    menu.addEventListener("keydown", (event) => handleMenuKeydown(event, close));
    menu.focus();

    const event = press(menu, "a");

    expect(close).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(menu);
  });
});

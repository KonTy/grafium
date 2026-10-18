import { flushSync, mount, tick, unmount, type ComponentProps } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import MobileNav from "./MobileNav.svelte";

let host: HTMLDivElement;
let component: ReturnType<typeof mount> | undefined;

function render(props: Record<string, unknown> = {}) {
  const handlers = {
    onNavigate: vi.fn(),
    onToggleKnowledgePanel: vi.fn(),
    onOpenGraph: vi.fn(),
    onCreateGraph: vi.fn(),
    onReindex: vi.fn(),
  };
  // Merge descriptors, not values, so a getter/setter pair stays a live binding.
  const merged = Object.defineProperties({}, {
    ...Object.getOwnPropertyDescriptors({ currentView: "journal", ...handlers }),
    ...Object.getOwnPropertyDescriptors(props),
  }) as ComponentProps<typeof MobileNav>;
  component = mount(MobileNav, { target: host, props: merged });
  flushSync();
  return handlers;
}

const text = (element: Element) => element.textContent?.trim();
const barButtons = () => [...host.querySelectorAll<HTMLButtonElement>("nav.bottom-nav > button")];
const moreButton = () => barButtons().at(-1)!;
const menu = () => host.querySelector<HTMLElement>('[role="menu"]');
const menuItems = () => [...host.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
const menuItem = (label: string) => menuItems().find((item) => text(item) === label)!;

async function settle() {
  await tick();
  flushSync();
}

async function openMenu() {
  moreButton().click();
  await settle();
}

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  host.remove();
});

describe("phone bottom bar", () => {
  it("shows the sidebar's first destinations in order, with More in the last slot", () => {
    render();
    expect(barButtons().map(text)).toEqual(["Journal", "Tasks", "Library", "Studies", "Pages", "More"]);
  });

  it("navigates from a bar button without opening the menu", () => {
    const { onNavigate } = render();
    barButtons()[2].click();
    expect(onNavigate).toHaveBeenCalledWith("__library__");
    expect(menu()).toBeNull();
  });

  it("marks the open destination as current", () => {
    render({ currentView: "library" });
    expect(barButtons().map((button) => button.getAttribute("aria-current"))).toEqual([null, null, "page", null, null, null]);
    expect(moreButton().classList.contains("active")).toBe(false);
  });

  it("highlights More when the open view lives behind it", async () => {
    render({ currentView: "settings" });
    expect(moreButton().classList.contains("active")).toBe(true);
    await openMenu();
    expect(menuItem("Settings").getAttribute("aria-current")).toBe("page");
  });
});

describe("More menu", () => {
  it("is a labelled menu button that reports whether it is open", async () => {
    render();
    expect(moreButton().getAttribute("aria-haspopup")).toBe("menu");
    expect(moreButton().getAttribute("aria-expanded")).toBe("false");
    expect(menu()).toBeNull();

    await openMenu();
    expect(moreButton().getAttribute("aria-expanded")).toBe("true");
    expect(menu()?.getAttribute("aria-label")).toBe("More");
  });

  it("puts New Page first, then the remaining destinations in sidebar order, then tools", async () => {
    render();
    await openMenu();
    expect(menuItems().map(text)).toEqual([
      "New Page",
      "Graph View", "Flashcards", "Chat", "Jobs", "Settings",
      "Knowledge Panel", "Import Media", "Import Books", "Open Graph", "New Graph", "Re-index Graph (Manual)",
    ]);
  });

  it.each([
    ["New Page", "__new_page__"],
    ["Graph View", "__graph__"],
    ["Flashcards", "__flashcards__"],
    ["Chat", "__chat__"],
    ["Jobs", "__jobs__"],
    ["Settings", "__settings__"],
    ["Import Media", "__import_media__"],
    ["Import Books", "__import_books__"],
  ])("opens %s and closes the menu", async (label, target) => {
    const { onNavigate } = render();
    await openMenu();
    menuItem(label).click();
    await settle();
    expect(onNavigate).toHaveBeenCalledTimes(1);
    expect(onNavigate).toHaveBeenCalledWith(target);
    expect(menu()).toBeNull();
    expect(moreButton().getAttribute("aria-expanded")).toBe("false");
  });

  it.each([
    ["Knowledge Panel", "onToggleKnowledgePanel"],
    ["Open Graph", "onOpenGraph"],
    ["New Graph", "onCreateGraph"],
    ["Re-index Graph (Manual)", "onReindex"],
  ] as const)("runs %s once and closes the menu", async (label, callback) => {
    const handlers = render();
    await openMenu();
    menuItem(label).click();
    await settle();
    expect(handlers[callback]).toHaveBeenCalledTimes(1);
    expect(handlers.onNavigate).not.toHaveBeenCalled();
    expect(menu()).toBeNull();
  });

  it("toggles closed from the More button and from the backdrop", async () => {
    render();
    await openMenu();
    await openMenu();
    expect(menu()).toBeNull();

    await openMenu();
    host.querySelector<HTMLElement>(".more-menu-backdrop")!.click();
    await settle();
    expect(menu()).toBeNull();
  });

  it("closes on Escape and hands focus back to More", async () => {
    render();
    moreButton().focus();
    await openMenu();
    expect(menu()?.contains(document.activeElement)).toBe(true);

    menu()!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await settle();
    expect(menu()).toBeNull();
    expect(document.activeElement).toBe(moreButton());
  });

  it("walks the items with the arrow keys, skipping separators", async () => {
    render();
    await openMenu();
    const first = menuItems()[0];
    first.focus();
    menu()!.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
    expect(document.activeElement).not.toBe(first);
    expect(document.activeElement).toBe(menuItems()[1]);
  });

  it("closes the app's bound state when the bar goes away, such as entering Zen mode", async () => {
    let open = true;
    render({
      get menuOpen() {
        return open;
      },
      set menuOpen(value: boolean) {
        open = value;
      },
    });
    await settle();
    expect(menu()).not.toBeNull();

    await unmount(component!);
    component = undefined;
    expect(open).toBe(false);
  });
});

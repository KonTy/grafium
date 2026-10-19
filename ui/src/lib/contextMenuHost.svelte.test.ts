import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ContextMenuAction, ContextMenuItem, ContextMenuSection } from "./contextMenuSections";

const buildEditSection = vi.hoisted(() => vi.fn());
vi.mock("./editActions", () => ({ buildEditSection }));

// jsdom has no top layer; the real lookup is tested on its own.
const enclosingTopLayer = vi.hoisted(() => vi.fn<(start: Element | null | undefined) => HTMLElement | null>());
vi.mock("./topLayer", () => ({ enclosingTopLayer }));

import {
  CONTEXT_MENU_CLASS,
  NO_CONTEXT_MENU_ATTRIBUTE,
  activateContextMenuItem,
  closeContextMenu,
  contextMenu,
  getOpenContextMenu,
  installContextMenu,
  openContextMenu,
  openContextMenuBelow,
  registerContextMenu,
} from "./contextMenuHost.svelte";
import { editorHasDialogFocus } from "./editorDialogFocus";
import { toasts } from "./toast.svelte";

function action(id: string, extra: Partial<ContextMenuAction> = {}): ContextMenuAction {
  return { id, label: id, onSelect: () => {}, ...extra };
}

function section(id: string, items: ContextMenuItem[], extra: Partial<ContextMenuSection> = {}): ContextMenuSection {
  return { id, items, ...extra };
}

const openIds = () => getOpenContextMenu()?.sections.map((s) => s.id) ?? null;
const itemIdsOf = (id: string) =>
  getOpenContextMenu()?.sections.find((s) => s.id === id)?.items.map((i) => i.id) ?? null;

async function flushPromises() {
  for (let i = 0; i < 4; i++) await Promise.resolve();
}

function rightClick(target: Element, init: MouseEventInit = {}): MouseEvent {
  const event = new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 40, clientY: 60, ...init });
  target.dispatchEvent(event);
  return event;
}

let uninstall: (() => void) | null = null;

beforeEach(() => {
  buildEditSection.mockReset();
  buildEditSection.mockReturnValue(null);
  enclosingTopLayer.mockReset();
  enclosingTopLayer.mockReturnValue(null);
  toasts.splice(0, toasts.length);
  document.body.innerHTML = "";
});

afterEach(() => {
  uninstall?.();
  uninstall = null;
  closeContextMenu({ restoreFocus: false });
  vi.restoreAllMocks();
});

describe("openContextMenu", () => {
  it("shows nothing when the sections have no items", () => {
    expect(openContextMenu({ anchor: { x: 1, y: 2 }, sections: [section("empty", [])] })).toBe(false);
    expect(getOpenContextMenu()).toBeNull();
  });

  it("opens one merged, ordered menu at the anchor", () => {
    const opened = openContextMenu({
      anchor: { x: 10, y: 20 },
      sections: [
        section("edit", [action("edit.copy")], { order: 90 }),
        section("page", [action("page.rename")]),
        section("spelling", [action("spell.fix")], { order: 10 }),
      ],
    });
    expect(opened).toBe(true);
    const menu = getOpenContextMenu();
    expect(menu?.sections.map((s) => s.id)).toEqual(["spelling", "page", "edit"]);
    expect(menu?.anchor).toEqual({ x: 10, y: 20 });
    expect(menu?.flipAnchor).toEqual({ x: 10, y: 20 });
    expect(menu?.label).toBe("Context menu");
  });

  it("keeps the flip anchor and label it was given", () => {
    openContextMenu({
      anchor: { x: 10, y: 40 },
      flipAnchor: { x: 10, y: 20 },
      label: "Page actions",
      sections: [section("a", [action("a1")])],
    });
    expect(getOpenContextMenu()?.flipAnchor).toEqual({ x: 10, y: 20 });
    expect(getOpenContextMenu()?.label).toBe("Page actions");
  });

  it("replaces an open menu and puts focus back first", () => {
    const field = document.createElement("input");
    document.body.append(field);
    field.focus();

    openContextMenu({ anchor: { x: 0, y: 0 }, sections: [section("first", [action("a")])] });
    const first = getOpenContextMenu();
    field.blur();
    openContextMenu({ anchor: { x: 5, y: 5 }, sections: [section("second", [action("b")])] });

    expect(getOpenContextMenu()).not.toBe(first);
    expect(openIds()).toEqual(["second"]);
    expect(document.activeElement).toBe(field);
  });

  it("returns focus to the element that had it, and not to the body", () => {
    const field = document.createElement("input");
    document.body.append(field);
    field.focus();
    openContextMenu({ anchor: { x: 0, y: 0 }, sections: [section("a", [action("a")])] });
    expect(getOpenContextMenu()?.returnFocus).toBe(field);
    closeContextMenu({ restoreFocus: false });

    field.blur();
    openContextMenu({ anchor: { x: 0, y: 0 }, sections: [section("a", [action("a")])] });
    expect(getOpenContextMenu()?.returnFocus).toBeNull();
  });

  it("returns focus to the element it is told to", () => {
    const button = document.createElement("button");
    document.body.append(button);
    openContextMenu({ anchor: { x: 0, y: 0 }, returnFocus: button, sections: [section("a", [action("a")])] });
    closeContextMenu();
    expect(document.activeElement).toBe(button);
  });
});

describe("openContextMenuBelow", () => {
  function buttonAt(left: number, top: number, right: number, bottom: number): HTMLButtonElement {
    const button = document.createElement("button");
    document.body.append(button);
    button.getBoundingClientRect = () =>
      ({ left, top, right, bottom, x: left, y: top, width: right - left, height: bottom - top }) as DOMRect;
    return button;
  }

  it("opens just under the button and flips around its top and right edges", () => {
    const button = buttonAt(100, 40, 132, 72);
    const opened = openContextMenuBelow(button, {
      label: "Actions for tech",
      sections: [section("page", [action("page.rename")])],
    });
    expect(opened).toBe(true);
    const menu = getOpenContextMenu();
    expect(menu?.anchor).toEqual({ x: 100, y: 76 });
    expect(menu?.flipAnchor).toEqual({ x: 132, y: 36 });
    expect(menu?.label).toBe("Actions for tech");
  });

  it("hands focus back to the button, wherever focus was when it opened", () => {
    const button = buttonAt(0, 0, 10, 10);
    const other = document.createElement("input");
    document.body.append(other);
    other.focus();

    openContextMenuBelow(button, { sections: [section("a", [action("a1")])] });
    expect(getOpenContextMenu()?.returnFocus).toBe(button);
    closeContextMenu();
    expect(document.activeElement).toBe(button);
  });

  it("shows nothing when there is nothing to offer", () => {
    expect(openContextMenuBelow(buttonAt(0, 0, 10, 10), { sections: [section("none", [])] })).toBe(false);
    expect(getOpenContextMenu()).toBeNull();
  });
});

describe("closeContextMenu", () => {
  it("closes, and hands focus back unless told not to", () => {
    const field = document.createElement("input");
    document.body.append(field);
    field.focus();
    openContextMenu({ anchor: { x: 0, y: 0 }, sections: [section("a", [action("a")])] });
    field.blur();

    closeContextMenu({ restoreFocus: false });
    expect(getOpenContextMenu()).toBeNull();
    expect(document.activeElement).not.toBe(field);

    field.focus();
    openContextMenu({ anchor: { x: 0, y: 0 }, sections: [section("a", [action("a")])] });
    field.blur();
    closeContextMenu();
    expect(document.activeElement).toBe(field);
  });

  it("does nothing when no menu is open", () => {
    expect(() => closeContextMenu()).not.toThrow();
  });
});

describe("keeping an editor open while its menu has focus", () => {
  it("retains the editor around the target until the menu closes", () => {
    const editor = document.createElement("div");
    editor.className = "cm-editor";
    const content = document.createElement("div");
    editor.append(content);
    document.body.append(editor);

    openContextMenu({ anchor: { x: 0, y: 0 }, target: content, sections: [section("a", [action("a")])] });
    expect(editorHasDialogFocus(editor)).toBe(true);
    closeContextMenu();
    expect(editorHasDialogFocus(editor)).toBe(false);
  });

  it("is retained before the menu could take focus", () => {
    const editor = document.createElement("div");
    editor.className = "cm-editor";
    const content = document.createElement("div");
    content.tabIndex = 0;
    editor.append(content);
    document.body.append(editor);
    content.focus();

    let retainedAtBlur = false;
    content.addEventListener("blur", () => {
      retainedAtBlur = editorHasDialogFocus(editor);
    });
    openContextMenu({ anchor: { x: 0, y: 0 }, sections: [section("a", [action("a")])] });
    document.body.focus();
    content.blur();
    expect(retainedAtBlur).toBe(true);
  });

  it("releases the editor when a new menu replaces the old one", () => {
    const editor = document.createElement("div");
    editor.className = "cm-editor";
    const content = document.createElement("div");
    editor.append(content);
    document.body.append(editor);

    openContextMenu({ anchor: { x: 0, y: 0 }, target: content, sections: [section("a", [action("a")])] });
    openContextMenu({ anchor: { x: 0, y: 0 }, sections: [section("b", [action("b")])] });
    expect(editorHasDialogFocus(editor)).toBe(false);
  });
});

describe("activateContextMenuItem", () => {
  it("closes the menu before running the action, and runs it synchronously", () => {
    const calls: string[] = [];
    const item = action("do", {
      onSelect: () => {
        calls.push(getOpenContextMenu() === null ? "closed" : "open");
      },
    });
    openContextMenu({ anchor: { x: 0, y: 0 }, sections: [section("a", [item])] });
    activateContextMenuItem(item);
    expect(calls).toEqual(["closed"]);
  });

  it("restores focus before the action runs, so it acts on the right field", () => {
    const field = document.createElement("input");
    document.body.append(field);
    field.focus();
    let focusedDuringAction: Element | null = null;
    const item = action("do", {
      onSelect: () => {
        focusedDuringAction = document.activeElement;
      },
    });
    openContextMenu({ anchor: { x: 0, y: 0 }, sections: [section("a", [item])] });
    field.blur();
    activateContextMenuItem(item);
    expect(focusedDuringAction).toBe(field);
  });

  it("ignores a disabled item", () => {
    const onSelect = vi.fn();
    const item = action("do", { disabled: true, onSelect });
    openContextMenu({ anchor: { x: 0, y: 0 }, sections: [section("a", [item])] });
    activateContextMenuItem(item);
    expect(onSelect).not.toHaveBeenCalled();
    expect(getOpenContextMenu()).not.toBeNull();
  });

  it("reports an action that throws, instead of leaving the menu half open", () => {
    const item = action("boom", {
      onSelect: () => {
        throw new Error("exploded");
      },
    });
    openContextMenu({ anchor: { x: 0, y: 0 }, sections: [section("a", [item])] });
    activateContextMenuItem(item);
    expect(getOpenContextMenu()).toBeNull();
    expect(toasts.map((t) => t.message)).toEqual(["exploded"]);
  });

  it("reports an action that rejects", async () => {
    const item = action("later", { onSelect: () => Promise.reject(new Error("failed later")) });
    openContextMenu({ anchor: { x: 0, y: 0 }, sections: [section("a", [item])] });
    activateContextMenuItem(item);
    await flushPromises();
    expect(toasts.map((t) => t.message)).toEqual(["failed later"]);
  });
});

describe("items that arrive late", () => {
  it("swaps in the fetched items and keeps the section's place", async () => {
    let resolveItems!: (items: ContextMenuItem[]) => void;
    const pending = new Promise<ContextMenuItem[]>((resolve) => {
      resolveItems = resolve;
    });
    openContextMenu({
      anchor: { x: 0, y: 0 },
      sections: [
        section("spelling", [{ kind: "status", id: "spelling.loading", label: "Looking…" }], {
          order: 10,
          resolve: () => pending,
        }),
        section("edit", [action("edit.copy")], { order: 90 }),
      ],
    });
    const before = getOpenContextMenu();
    expect(itemIdsOf("spelling")).toEqual(["spelling.loading"]);

    resolveItems([action("spell.one"), action("spell.two")]);
    await flushPromises();

    expect(getOpenContextMenu()).not.toBe(before);
    expect(openIds()).toEqual(["spelling", "edit"]);
    expect(itemIdsOf("spelling")).toEqual(["spell.one", "spell.two"]);
    expect(getOpenContextMenu()?.sections[0].resolve).toBeUndefined();
  });

  it("does not touch a menu that was replaced in the meantime", async () => {
    let resolveItems!: (items: ContextMenuItem[]) => void;
    openContextMenu({
      anchor: { x: 0, y: 0 },
      sections: [
        section("slow", [{ kind: "status", id: "slow.loading", label: "…" }], {
          resolve: () => new Promise<ContextMenuItem[]>((resolve) => (resolveItems = resolve)),
        }),
      ],
    });
    await flushPromises();
    openContextMenu({ anchor: { x: 0, y: 0 }, sections: [section("fresh", [action("fresh.a")])] });
    resolveItems([action("stale")]);
    await flushPromises();
    expect(openIds()).toEqual(["fresh"]);
  });

  it("does not reopen a menu that was closed in the meantime", async () => {
    let resolveItems!: (items: ContextMenuItem[]) => void;
    openContextMenu({
      anchor: { x: 0, y: 0 },
      sections: [
        section("slow", [{ kind: "status", id: "slow.loading", label: "…" }], {
          resolve: () => new Promise<ContextMenuItem[]>((resolve) => (resolveItems = resolve)),
        }),
      ],
    });
    await flushPromises();
    closeContextMenu();
    resolveItems([action("stale")]);
    await flushPromises();
    expect(getOpenContextMenu()).toBeNull();
  });

  it("does not fetch for a menu that is already gone", async () => {
    const resolve = vi.fn(async () => [action("late")]);
    openContextMenu({
      anchor: { x: 0, y: 0 },
      sections: [section("slow", [{ kind: "status", id: "slow.loading", label: "…" }], { resolve })],
    });
    closeContextMenu();
    await flushPromises();
    expect(resolve).not.toHaveBeenCalled();
  });

  it("drops the section when fetching fails and keeps the others", async () => {
    const error = new Error("no");
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    openContextMenu({
      anchor: { x: 0, y: 0 },
      sections: [
        section("flaky", [{ kind: "status", id: "flaky.loading", label: "…" }], {
          resolve: () => Promise.reject(error),
        }),
        section("page", [action("page.rename")]),
      ],
    });
    await flushPromises();
    expect(openIds()).toEqual(["page"]);
    expect(log).toHaveBeenCalledWith('The "flaky" context menu section could not be loaded:', error);
  });

  it("says so when a section that has a failure label cannot be fetched", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    openContextMenu({
      anchor: { x: 0, y: 0 },
      sections: [
        section("flaky", [{ kind: "status", id: "flaky.loading", label: "Loading…" }], {
          resolve: () => Promise.reject(new Error("no")),
          errorLabel: "Could not load",
        }),
        section("page", [action("page.rename")]),
      ],
    });
    await flushPromises();
    expect(openIds()).toEqual(["flaky", "page"]);
    expect(getOpenContextMenu()?.sections[0].items).toEqual([
      { kind: "status", id: "flaky.error", label: "Could not load" },
    ]);
    expect(getOpenContextMenu()?.sections[0].resolve).toBeUndefined();
  });

  it("keeps a menu that has only the failure notice open", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    openContextMenu({
      anchor: { x: 0, y: 0 },
      sections: [
        section("only", [{ kind: "status", id: "only.loading", label: "Loading…" }], {
          resolve: () => Promise.reject(new Error("no")),
          errorLabel: "Could not load",
        }),
      ],
    });
    await flushPromises();
    expect(itemIdsOf("only")).toEqual(["only.error"]);
  });

  it("does not show the failure notice for a menu that is no longer open", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    let fail!: (error: Error) => void;
    openContextMenu({
      anchor: { x: 0, y: 0 },
      sections: [
        section("flaky", [{ kind: "status", id: "flaky.loading", label: "Loading…" }], {
          resolve: () => new Promise<ContextMenuItem[]>((_, reject) => (fail = reject)),
          errorLabel: "Could not load",
        }),
      ],
    });
    await flushPromises();
    closeContextMenu();
    openContextMenu({ anchor: { x: 0, y: 0 }, sections: [section("fresh", [action("fresh.a")])] });
    fail(new Error("late"));
    await flushPromises();
    expect(openIds()).toEqual(["fresh"]);
  });

  it("closes when fetching leaves nothing to show", async () => {
    openContextMenu({
      anchor: { x: 0, y: 0 },
      sections: [
        section("only", [{ kind: "status", id: "only.loading", label: "…" }], {
          resolve: () => Promise.resolve([]),
        }),
      ],
    });
    await flushPromises();
    expect(getOpenContextMenu()).toBeNull();
  });

  it("drops fetched items another section already offers", async () => {
    openContextMenu({
      anchor: { x: 0, y: 0 },
      sections: [
        section("late", [{ kind: "status", id: "late.loading", label: "…" }], {
          resolve: () => Promise.resolve([action("shared"), action("late.own")]),
        }),
        section("page", [action("shared")]),
      ],
    });
    await flushPromises();
    expect(itemIdsOf("late")).toEqual(["late.own"]);
    expect(itemIdsOf("page")).toEqual(["shared"]);
  });

  it("copes with a resolver that throws synchronously", async () => {
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    openContextMenu({
      anchor: { x: 0, y: 0 },
      sections: [
        section("bad", [{ kind: "status", id: "bad.loading", label: "…" }], {
          resolve: () => {
            throw new Error("sync");
          },
        }),
        section("page", [action("page.rename")]),
      ],
    });
    await flushPromises();
    expect(openIds()).toEqual(["page"]);
    expect(log).toHaveBeenCalledTimes(1);
  });
});

describe("the layer a menu is drawn in", () => {
  function dialogWith(...children: HTMLElement[]): HTMLDialogElement {
    const dialog = document.createElement("dialog");
    dialog.append(...children);
    document.body.append(dialog);
    return dialog;
  }

  it("is the top layer around what was right-clicked", () => {
    const field = document.createElement("input");
    const dialog = dialogWith(field);
    enclosingTopLayer.mockImplementation((start) => (start === field ? dialog : null));

    openContextMenu({ anchor: { x: 0, y: 0 }, target: field, sections: [section("a", [action("a1")])] });
    expect(getOpenContextMenu()?.layer).toBe(dialog);
  });

  it("is found from the element that gets focus back when there is no target", () => {
    const button = document.createElement("button");
    const dialog = dialogWith(button);
    enclosingTopLayer.mockImplementation((start) => (start === button ? dialog : null));

    openContextMenu({ anchor: { x: 0, y: 0 }, returnFocus: button, sections: [section("a", [action("a1")])] });
    expect(getOpenContextMenu()?.layer).toBe(dialog);
  });

  it("is none for a menu on the page itself", () => {
    const para = document.createElement("p");
    document.body.append(para);
    openContextMenu({ anchor: { x: 0, y: 0 }, target: para, sections: [section("a", [action("a1")])] });
    expect(getOpenContextMenu()?.layer).toBeNull();
  });

  it("stays when items arrive late", async () => {
    const field = document.createElement("input");
    const dialog = dialogWith(field);
    enclosingTopLayer.mockImplementation((start) => (start === field ? dialog : null));
    openContextMenu({
      anchor: { x: 0, y: 0 },
      target: field,
      sections: [
        section("slow", [{ kind: "status", id: "slow.loading", label: "…" }], {
          resolve: () => Promise.resolve([action("slow.a")]),
        }),
      ],
    });
    await flushPromises();
    expect(itemIdsOf("slow")).toEqual(["slow.a"]);
    expect(getOpenContextMenu()?.layer).toBe(dialog);
  });

  it("is looked up again for the next menu", () => {
    const field = document.createElement("input");
    const dialog = dialogWith(field);
    const para = document.createElement("p");
    document.body.append(para);
    enclosingTopLayer.mockImplementation((start) => (start === field ? dialog : null));

    openContextMenu({ anchor: { x: 0, y: 0 }, target: field, sections: [section("a", [action("a1")])] });
    expect(getOpenContextMenu()?.layer).toBe(dialog);
    openContextMenu({ anchor: { x: 0, y: 0 }, target: para, sections: [section("b", [action("b1")])] });
    expect(getOpenContextMenu()?.layer).toBeNull();
  });
});

describe("providers", () => {
  it("are asked innermost first, and unregister", () => {
    const outer = document.createElement("div");
    const inner = document.createElement("span");
    outer.append(inner);
    document.body.append(outer);
    const order: string[] = [];
    const stopOuter = registerContextMenu(outer, () => {
      order.push("outer");
      return section("outer", [action("outer.a")]);
    });
    registerContextMenu(inner, () => {
      order.push("inner");
      return [section("inner", [action("inner.a")])];
    });
    uninstall = installContextMenu();

    rightClick(inner);
    expect(order).toEqual(["inner", "outer"]);
    expect(openIds()).toEqual(["inner", "outer"]);

    closeContextMenu();
    stopOuter();
    order.length = 0;
    rightClick(inner);
    expect(order).toEqual(["inner"]);
  });

  it("can be several on one element", () => {
    const el = document.createElement("div");
    document.body.append(el);
    registerContextMenu(el, () => section("a", [action("a1")]));
    registerContextMenu(el, () => section("b", [action("b1")]));
    uninstall = installContextMenu();
    rightClick(el);
    expect(openIds()).toEqual(["a", "b"]);
  });

  it("hear about the click", () => {
    const el = document.createElement("div");
    document.body.append(el);
    const seen = vi.fn();
    registerContextMenu(el, (context) => {
      seen(context.target, context.x, context.y, context.event.type);
    });
    uninstall = installContextMenu();
    rightClick(el, { clientX: 33, clientY: 44 });
    expect(seen).toHaveBeenCalledWith(el, 33, 44, "contextmenu");
  });

  it("do not stop each other when one fails", () => {
    const el = document.createElement("div");
    document.body.append(el);
    vi.spyOn(console, "error").mockImplementation(() => {});
    registerContextMenu(el, () => {
      throw new Error("broken provider");
    });
    registerContextMenu(el, () => section("fine", [action("fine.a")]));
    uninstall = installContextMenu();
    rightClick(el);
    expect(openIds()).toEqual(["fine"]);
  });

  it("work through the use: action, including when the provider changes or the node goes", () => {
    const el = document.createElement("div");
    document.body.append(el);
    uninstall = installContextMenu();
    const handle = contextMenu(el, () => section("one", [action("one.a")]));

    rightClick(el);
    expect(openIds()).toEqual(["one"]);
    closeContextMenu();

    handle.update(() => section("two", [action("two.a")]));
    rightClick(el);
    expect(openIds()).toEqual(["two"]);
    closeContextMenu();

    handle.update(null);
    rightClick(el);
    expect(getOpenContextMenu()).toBeNull();

    handle.update(() => section("three", [action("three.a")]));
    handle.destroy();
    rightClick(el);
    expect(getOpenContextMenu()).toBeNull();
  });
});

describe("the window's context menu listener", () => {
  it("always suppresses the native menu, even when there is nothing to show", () => {
    const el = document.createElement("div");
    document.body.append(el);
    uninstall = installContextMenu();
    const event = rightClick(el);
    expect(event.defaultPrevented).toBe(true);
    expect(getOpenContextMenu()).toBeNull();
  });

  it("adds the clipboard section after the providers' own", () => {
    const el = document.createElement("div");
    document.body.append(el);
    registerContextMenu(el, () => section("page", [action("page.rename")]));
    buildEditSection.mockReturnValue(section("edit", [action("edit.copy")], { order: 90 }));
    uninstall = installContextMenu();

    rightClick(el, { clientX: 7, clientY: 9 });
    expect(openIds()).toEqual(["page", "edit"]);
    expect(buildEditSection).toHaveBeenCalledWith(el, { x: 7, y: 9 });
    expect(getOpenContextMenu()?.anchor).toEqual({ x: 7, y: 9 });
  });

  it("offers the clipboard section alone where no provider answers", () => {
    const el = document.createElement("p");
    document.body.append(el);
    buildEditSection.mockReturnValue(section("edit", [action("edit.copy")]));
    uninstall = installContextMenu();
    rightClick(el);
    expect(openIds()).toEqual(["edit"]);
  });

  it("closes the menu that is open and opens a new one", () => {
    const el = document.createElement("div");
    document.body.append(el);
    let n = 0;
    registerContextMenu(el, () => section(`s${++n}`, [action(`a${n}`)]));
    uninstall = installContextMenu();
    rightClick(el);
    rightClick(el);
    expect(openIds()).toEqual(["s2"]);
  });

  it("only suppresses the native menu for clicks on the menu itself", () => {
    const el = document.createElement("div");
    document.body.append(el);
    const menu = document.createElement("div");
    menu.className = CONTEXT_MENU_CLASS;
    const item = document.createElement("button");
    menu.append(item);
    document.body.append(menu);
    registerContextMenu(el, () => section("page", [action("page.rename")]));
    uninstall = installContextMenu();

    openContextMenu({ anchor: { x: 0, y: 0 }, sections: [section("open", [action("open.a")])] });
    const before = getOpenContextMenu();
    const event = rightClick(item);
    expect(event.defaultPrevented).toBe(true);
    expect(getOpenContextMenu()).toBe(before);
  });

  it("opens nothing inside a region that opts out, but still suppresses the native menu", () => {
    const page = document.createElement("div");
    const toolbar = document.createElement("div");
    toolbar.setAttribute(NO_CONTEXT_MENU_ATTRIBUTE, "");
    const button = document.createElement("button");
    toolbar.append(button);
    page.append(toolbar);
    document.body.append(page);
    registerContextMenu(page, () => section("page", [action("page.rename")]));
    buildEditSection.mockReturnValue(section("edit", [action("edit.copy")]));
    uninstall = installContextMenu();

    const event = rightClick(button);
    expect(event.defaultPrevented).toBe(true);
    expect(getOpenContextMenu()).toBeNull();
    expect(buildEditSection).not.toHaveBeenCalled();
  });

  it("closes the menu that is open when the click lands in a region that opts out", () => {
    const toolbar = document.createElement("div");
    toolbar.setAttribute(NO_CONTEXT_MENU_ATTRIBUTE, "");
    document.body.append(toolbar);
    uninstall = installContextMenu();

    openContextMenu({ anchor: { x: 0, y: 0 }, sections: [section("open", [action("open.a")])] });
    rightClick(toolbar);
    expect(getOpenContextMenu()).toBeNull();
  });

  it("anchors a keyboard-opened menu to the element when there is no pointer", () => {
    const el = document.createElement("button");
    document.body.append(el);
    vi.spyOn(el, "getBoundingClientRect").mockReturnValue({
      left: 120,
      bottom: 200,
      top: 170,
      right: 220,
      width: 100,
      height: 30,
      x: 120,
      y: 170,
      toJSON: () => ({}),
    });
    registerContextMenu(el, () => section("page", [action("page.rename")]));
    uninstall = installContextMenu();

    rightClick(el, { clientX: 0, clientY: 0 });
    expect(getOpenContextMenu()?.anchor).toEqual({ x: 120, y: 200 });
  });

  it("returns focus to the right element when the menu opens from a focused field", () => {
    const field = document.createElement("input");
    document.body.append(field);
    registerContextMenu(field, () => section("page", [action("page.rename")]));
    field.focus();
    uninstall = installContextMenu();
    rightClick(field);
    expect(getOpenContextMenu()?.returnFocus).toBe(field);
  });

  it("stops listening and closes when removed", () => {
    const el = document.createElement("div");
    document.body.append(el);
    registerContextMenu(el, () => section("page", [action("page.rename")]));
    const stop = installContextMenu();
    rightClick(el);
    expect(getOpenContextMenu()).not.toBeNull();

    stop();
    expect(getOpenContextMenu()).toBeNull();
    const event = rightClick(el);
    expect(event.defaultPrevented).toBe(false);
  });

  it("copes with a right-click on a text node's parent and on the document", () => {
    const el = document.createElement("p");
    el.textContent = "hello";
    document.body.append(el);
    registerContextMenu(el, () => section("page", [action("page.rename")]));
    uninstall = installContextMenu();

    const textEvent = new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 1, clientY: 1 });
    el.firstChild!.dispatchEvent(textEvent);
    expect(textEvent.defaultPrevented).toBe(true);
    expect(openIds()).toEqual(["page"]);

    closeContextMenu();
    const docEvent = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    document.dispatchEvent(docEvent);
    expect(docEvent.defaultPrevented).toBe(true);
    expect(getOpenContextMenu()).toBeNull();
  });
});

// Drives the one app-wide context menu in a real browser with Tauri IPC stubbed.
//
// Unit tests cannot see what this system exists to get right: where the menu lands
// under the app's CSS zoom, that the browser's own menu never shows, and that one
// right-click yields ONE merged menu instead of several stacked ones.
const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor, focus, frames, row } = require("./keyboardSelection.ui.cjs");

const MENU = ".ctx-menu";
const SPELL = { misspelled: ["teh"], suggestions: { teh: ["the", "tea", "ten"] } };
const SPELL_OPTIONS = { spell: SPELL, blockContents: ["teh quick fox", "second block"] };

const near = (actual, expected, message, tolerance = 1.5) =>
  assert.ok(Math.abs(actual - expected) <= tolerance, `${message}: ${actual} is not within ${tolerance} of ${expected}`);

async function menuState(page) {
  return page.evaluate(() => {
    const menus = [...document.querySelectorAll(".ctx-menu")];
    if (menus.length === 0) return null;
    const box = menus[0].getBoundingClientRect();
    return {
      count: menus.length,
      roleMenus: document.querySelectorAll('[role="menu"]').length,
      box: { left: box.left, top: box.top, right: box.right, bottom: box.bottom, width: box.width },
      groups: [...menus[0].querySelectorAll('[role="group"]')].map((group) => ({
        title: group.getAttribute("aria-label"),
        items: [...group.querySelectorAll('[role="menuitem"]')].map((item) => ({
          label: (item.querySelector(".ctx-menu-label") ?? item).textContent.trim(),
          disabled: item.disabled === true || item.getAttribute("aria-disabled") === "true",
        })),
      })),
    };
  });
}

const outline = (state) => Object.fromEntries(state.groups.map((group) => [
  group.title, group.items.map((item) => item.label + (item.disabled ? " (disabled)" : "")),
]));

async function openMenu(page, x, y) {
  await page.mouse.click(x, y, { button: "right" });
  await page.locator(MENU).first().waitFor();
  return menuState(page);
}

async function blockPoint(page, id, dx = 60) {
  const box = await row(page, id).locator(".block-content").boundingBox();
  return { x: Math.round(box.x + dx), y: Math.round(box.y + box.height / 2) };
}

async function rightClickBlock(page, id) {
  const { x, y } = await blockPoint(page, id);
  return openMenu(page, x, y);
}

// Browser-level view of what happened to each contextmenu event once the app's own handler ran.
// Capture phase, registered after the app's, so it still sees events a menu stops from bubbling.
async function watchContextEvents(page) {
  await page.evaluate(() => {
    window.__contextEvents = [];
    window.addEventListener("contextmenu", (event) =>
      window.__contextEvents.push({ prevented: event.defaultPrevented, x: event.clientX, y: event.clientY }), true);
  });
}
const contextEvents = (page) => page.evaluate(() => window.__contextEvents);

const docText = (page) => page.evaluate(() => window.__activeEditorView.state.doc.toString());
const clipboard = (page) => page.evaluate(() => window.__selectionState.clipboard);
const selectRange = (page, anchor, head) =>
  page.evaluate(([anchor, head]) => window.__activeEditorView.dispatch({ selection: { anchor, head } }), [anchor, head]);
const pointAt = (page, pos) => page.evaluate((pos) => {
  const c = window.__activeEditorView.coordsAtPos(pos);
  return { x: c.left + 1, y: (c.top + c.bottom) / 2 };
}, pos);
const menuClosed = async (page) => assert.equal(await page.locator(MENU).count(), 0, "the menu is closed");
const focusIsOutsideMenu = (page) => page.evaluate(() =>
  document.activeElement !== document.body && !document.activeElement?.closest(".ctx-menu"));
const item = (page, name) => page.getByRole("menuitem", { name });
const activeLabel = (page) => page.evaluate(() =>
  document.activeElement?.querySelector?.(".ctx-menu-label")?.textContent?.trim() ?? document.activeElement?.className);
const blockContent = (page, id) =>
  page.evaluate((id) => window.__selectionState.blocks.find((block) => block.id === id)?.content, id);
const calls = (page, cmd) =>
  page.evaluate((cmd) => window.__selectionState.calls.filter((call) => call.cmd === cmd), cmd);

const scenarios = [];

for (const zoom of [70, 100, 125, 150, 180]) {
  scenarios.push([`opens exactly at the pointer at ${zoom}% zoom, as the only menu`, { zoom }, async (page) => {
    await watchContextEvents(page);
    const applied = await page.evaluate(() => parseFloat(getComputedStyle(document.documentElement).zoom));
    near(applied, zoom / 100, "the app zoom is really applied", 0.001);

    const { x, y } = await blockPoint(page, "b1", 80);
    const state = await openMenu(page, x, y);
    assert.equal(state.count, 1, "one menu element");
    assert.equal(state.roleMenus, 1, "one menu role on the page");
    near(state.box.left, x, "left edge");
    near(state.box.top, y, "top edge");
    assert.deepEqual((await contextEvents(page)).map((event) => event.prevented), [true], "the browser's menu is suppressed");
  }]);
}

scenarios.push(["flips back inside the window instead of running off the bottom-right edge", {}, async (page) => {
  await page.setViewportSize({ width: 1280, height: 317 });
  await frames(page);
  const state = await openMenu(page, 1270, 310);
  assert.ok(state.box.left >= 0 && state.box.top >= 0, "not off the top-left");
  assert.ok(state.box.right <= 1280 && state.box.bottom <= 317, "not off the bottom-right");
  near(state.box.right, 1270, "flipped to the left of the pointer");
  near(state.box.bottom, 310, "flipped above the pointer");
}]);

scenarios.push(["offers block and page actions together for a bullet, and Turn bullet into TODO works", {}, async (page) => {
  const state = await rightClickBlock(page, "b1");
  assert.deepEqual(outline(state), {
    Selection: ["Turn bullet into TODO", "Copy block", "Clear selection"],
    Page: ["Rename page", "Delete page"],
  });
  await item(page, "Turn bullet into TODO").click();
  await page.waitForFunction(() => window.__selectionState.blocks.find((block) => block.id === "b1")?.content === "TODO Second block");
  await menuClosed(page);
}]);

scenarios.push(["a second right-click replaces the menu, and right-clicking the menu keeps it", {}, async (page) => {
  await watchContextEvents(page);
  const first = await rightClickBlock(page, "b1");
  assert.equal(first.count, 1);
  // Well to the right, so the point is not covered by the first menu.
  const target = await blockPoint(page, "b3", 700);
  const second = await openMenu(page, target.x, target.y);
  assert.equal(second.count, 1, "still one menu");
  assert.equal(second.roleMenus, 1);
  assert.ok(second.box.top > first.box.top, "it moved to the new pointer position");
  near(second.box.top, target.y, "top edge");

  await page.locator(".ctx-menu-item").first().click({ button: "right" });
  assert.equal((await menuState(page))?.count, 1, "a right-click inside the menu leaves it open");
  assert.deepEqual((await contextEvents(page)).map((event) => event.prevented), [true, true, true]);
}]);

for (const [name, close] of [
  ["Escape", (page) => page.keyboard.press("Escape")],
  ["scrolling the page", async (page) => {
    await page.mouse.move(1100, 700);
    await page.mouse.wheel(0, 120);
  }],
  ["Tab", (page) => page.keyboard.press("Tab")],
]) {
  scenarios.push([`closes on ${name} and hands focus back to the page`, {}, async (page) => {
    await rightClickBlock(page, "b1");
    await close(page);
    await page.locator(MENU).waitFor({ state: "detached" });
    assert.ok(await focusIsOutsideMenu(page), "focus is on a real element, not lost to the document body");
  }]);
}

scenarios.push(["scrolling over the menu itself leaves it open", {}, async (page) => {
  const state = await rightClickBlock(page, "b1");
  await page.mouse.move((state.box.left + state.box.right) / 2, (state.box.top + state.box.bottom) / 2);
  await page.mouse.wheel(0, 120);
  await frames(page);
  assert.equal((await menuState(page))?.count, 1);
}]);

scenarios.push(["a click outside closes the menu and still reaches what was clicked", {}, async (page) => {
  await rightClickBlock(page, "b1");
  const target = await blockPoint(page, "b3", 700);
  await page.mouse.click(target.x, target.y);
  await page.locator(MENU).waitFor({ state: "detached" });
  await page.waitForFunction(() => window.__activeEditorView?.hasFocus === true);
  assert.equal(await page.evaluate(() => window.__activeEditorView.dom.closest("[data-block-id]")?.dataset.blockId), "b3",
    "the click opened the block that was under it");
}]);

scenarios.push(["works from the keyboard: arrows wrap, Enter chooses, Escape returns focus", {}, async (page) => {
  await rightClickBlock(page, "b1");
  await page.waitForFunction(() => document.activeElement?.classList.contains("ctx-menu"));

  await page.keyboard.press("ArrowDown");
  assert.equal(await activeLabel(page), "Turn bullet into TODO");
  await page.keyboard.press("End");
  assert.equal(await activeLabel(page), "Delete page");
  await page.keyboard.press("ArrowDown");
  assert.equal(await activeLabel(page), "Turn bullet into TODO", "wraps from the last item to the first");
  await page.keyboard.press("ArrowUp");
  assert.equal(await activeLabel(page), "Delete page", "wraps from the first item to the last");
  await page.keyboard.press("Home");
  await page.keyboard.press("Enter");
  await page.waitForFunction(() => window.__selectionState.blocks.find((block) => block.id === "b1")?.content === "TODO Second block");
  await menuClosed(page);
  assert.ok(await focusIsOutsideMenu(page));
}]);

scenarios.push(["Delete does nothing while the menu is open, and works again once it closes", {}, async (page) => {
  await rightClickBlock(page, "b1");
  await page.keyboard.press("Delete");
  await frames(page);
  assert.equal(await page.evaluate(() => window.__selectionState.calls.filter(window.__isDeletion).length), 0,
    "no block is deleted by a key pressed in the menu");
  assert.equal((await menuState(page))?.count, 1, "the menu is still there");

  await page.keyboard.press("Escape");
  await menuClosed(page);
  await page.keyboard.press("Delete");
  await page.waitForFunction(() => window.__selectionState.calls.some(window.__isDeletion));
}]);

scenarios.push(["Rename page puts the cursor in the title, where only the edit actions are offered", {}, async (page) => {
  await rightClickBlock(page, "b1");
  await item(page, "Rename page").click();
  const field = page.locator(".page-title-input");
  await field.waitFor();
  await page.waitForFunction(() => document.activeElement?.classList.contains("page-title-input"));

  const box = await field.boundingBox();
  const state = await openMenu(page, box.x + 20, box.y + box.height / 2);
  assert.deepEqual(outline(state), { Edit: ["Cut", "Copy", "Paste", "Select all"] });
  await page.keyboard.press("Escape");
  await menuClosed(page);
  await page.waitForFunction(() => document.activeElement?.classList.contains("page-title-input"));
}]);

scenarios.push(["selected read-only text can be copied from the same menu as the page actions", {}, async (page) => {
  const title = page.locator("h1.page-title").first();
  await title.waitFor();
  const point = await page.evaluate(() => {
    const heading = document.querySelector("h1.page-title");
    const range = document.createRange();
    range.selectNodeContents(heading);
    const selection = window.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    const rect = range.getBoundingClientRect();
    return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
  });
  const state = await openMenu(page, point.x, point.y);
  assert.deepEqual(outline(state), { Page: ["Rename page", "Delete page"], Edit: ["Copy"] });
  await item(page, /^Copy/).click();
  assert.deepEqual(await clipboard(page), ["Keyboard selection"]);
}]);

scenarios.push(["nothing to offer means no menu, and still no browser menu", {}, async (page) => {
  await watchContextEvents(page);
  const button = page.getByRole("button", { name: "Settings", exact: true }).first();
  const box = await button.boundingBox();
  await page.mouse.click(box.x + 10, box.y + box.height / 2, { button: "right" });
  await frames(page);
  await menuClosed(page);
  assert.deepEqual((await contextEvents(page)).map((event) => event.prevented), [true]);
}]);

scenarios.push(["a favourite in the sidebar offers its page actions", { favorites: ["selection-page"] }, async (page) => {
  const favorite = page.locator(".sidebar-section", { has: page.getByRole("heading", { name: "Favorites" }) })
    .locator("button.page-item").first();
  const box = await favorite.boundingBox();
  const state = await openMenu(page, box.x + 20, box.y + box.height / 2);
  assert.deepEqual(outline(state), { Page: ["Remove from Favorites", "Mark as Book Collection"] });
  await item(page, "Remove from Favorites").click();
  await page.waitForFunction(() => window.__selectionState.calls.some((call) => call.cmd === "remove_favorite"));
  assert.deepEqual((await calls(page, "remove_favorite")).map((call) => call.args), [{ pageId: "selection-page" }]);
  await menuClosed(page);
}]);

scenarios.push(["a journal date offers journal and page actions", { journal: true }, async (page) => {
  const title = page.locator(".journal-entry h1.page-title").first();
  await title.waitFor();
  const box = await title.boundingBox();
  const state = await openMenu(page, box.x + 6, box.y + box.height / 2);
  assert.deepEqual(outline(state), { "Journal entry": ["Import from Media..."], Page: ["Delete page"] });
}]);

scenarios.push(["spelling suggestions, dictionary and edit actions share one menu", SPELL_OPTIONS, async (page) => {
  await focus(page, "b0");
  const squiggle = page.locator(".cm-spell-error").first();
  await squiggle.waitFor();
  await squiggle.click({ button: "right" });
  await item(page, "the").waitFor();

  const state = await menuState(page);
  assert.equal(state.count, 1, "one menu element");
  assert.equal(state.roleMenus, 1, "one menu role on the page");
  assert.deepEqual(outline(state), {
    "Spelling suggestions": ["the", "tea", "ten"],
    Dictionary: ["Add to dictionary", "Ignore for now"],
    Edit: ["Cut (disabled)", "Copy (disabled)", "Paste", "Select all"],
  });
}]);

scenarios.push(["choosing a suggestion fixes the word, returns to the editor and saves it", SPELL_OPTIONS, async (page) => {
  await focus(page, "b0");
  await page.locator(".cm-spell-error").first().click({ button: "right" });
  await item(page, "the").click();
  await page.waitForFunction(() => window.__activeEditorView.state.doc.toString() === "the quick fox");
  await menuClosed(page);
  assert.equal(await page.evaluate(() => window.__activeEditorView.hasFocus), true, "focus is back in the editor");
  assert.equal(await page.locator(".cm-spell-error").count(), 0);

  // Block text is saved when editing ends, as it is for typed changes.
  await page.keyboard.press("Escape");
  await page.waitForFunction(() => window.__selectionState.blocks.find((block) => block.id === "b0")?.content === "the quick fox");
}]);

scenarios.push(["Add to dictionary teaches the word and clears the underline", SPELL_OPTIONS, async (page) => {
  await focus(page, "b0");
  await page.locator(".cm-spell-error").first().click({ button: "right" });
  await item(page, "Add to dictionary").click();
  await page.locator(".cm-spell-error").waitFor({ state: "detached" });
  assert.deepEqual((await calls(page, "spell_add_personal_word")).map((call) => call.args.word), ["teh"]);
  await menuClosed(page);
}]);

scenarios.push(["Ignore for now clears the underline without teaching the word", SPELL_OPTIONS, async (page) => {
  await focus(page, "b0");
  await page.locator(".cm-spell-error").first().click({ button: "right" });
  await item(page, "Ignore for now").click();
  await page.locator(".cm-spell-error").waitFor({ state: "detached" });
  assert.deepEqual(await calls(page, "spell_add_personal_word"), [], "nothing is added to the dictionary");
  await menuClosed(page);
}]);

for (const [where, offset] of [["inside", 1], ["right after", 3]]) {
  scenarios.push([`the menu key opens the same menu at the caret ${where} a misspelled word`, SPELL_OPTIONS, async (page) => {
    await focus(page, "b0", offset);
    await page.locator(".cm-spell-error").first().waitFor();
    const caret = await page.evaluate(() => {
      const view = window.__activeEditorView;
      const c = view.coordsAtPos(view.state.selection.main.head);
      return { left: c.left, top: c.top, bottom: c.bottom };
    });

    await page.keyboard.press("ContextMenu");
    await page.locator(MENU).first().waitFor();
    await item(page, "the").waitFor();
    const state = await menuState(page);
    assert.equal(state.count, 1);
    assert.deepEqual(Object.keys(outline(state)), ["Spelling suggestions", "Dictionary", "Edit"]);
    near(state.box.left, caret.left, "opens at the caret", 3);
    assert.ok(state.box.top >= caret.top - 3 && state.box.top <= caret.bottom + 3, "opens on the caret's line");

    await page.keyboard.press("Escape");
    await menuClosed(page);
    assert.equal(await page.evaluate(() => window.__activeEditorView.hasFocus), true, "focus returns to the editor");
    assert.equal(await page.evaluate(() => window.__activeEditorView.state.selection.main.head), offset, "the caret has not moved");
  }]);
}

scenarios.push(["the menu key away from a misspelled word offers only the edit actions", SPELL_OPTIONS, async (page) => {
  await focus(page, "b0");
  await page.locator(".cm-spell-error").first().waitFor();
  await page.keyboard.press("ContextMenu");
  await page.locator(MENU).first().waitFor();
  const state = await menuState(page);
  assert.deepEqual(outline(state), { Edit: ["Cut (disabled)", "Copy (disabled)", "Paste", "Select all"] });
}]);

scenarios.push(["Copy, Paste, Cut and Select all act on the editor's selection", {
  blockContents: ["alpha beta gamma", "second block"],
}, async (page) => {
  await focus(page, "b0");
  // Chromium's own copy and cut never reach the page's clipboard stub, so mirror what
  // the system clipboard would now hold; the app then pastes it back through the stub.
  await page.evaluate(() => {
    const real = document.execCommand.bind(document);
    document.execCommand = (command, ...args) => {
      if (command === "copy" || command === "cut") window.__selectionState.clipboard.push(window.getSelection()?.toString() ?? "");
      return real(command, ...args);
    };
  });
  const lastCopied = (page) => clipboard(page).then((entries) => entries.at(-1));
  const choose = async (from, to, name) => {
    await selectRange(page, from, to);
    const point = await pointAt(page, Math.floor((from + to) / 2));
    await openMenu(page, point.x, point.y);
    await item(page, name).click();
    await menuClosed(page);
  };

  await choose(6, 10, /^Copy/);
  assert.equal(await lastCopied(page), "beta");
  assert.equal(await docText(page), "alpha beta gamma", "copy leaves the text alone");

  await choose(11, 16, /^Paste/);
  await page.waitForFunction(() => window.__activeEditorView.state.doc.toString() === "alpha beta beta");

  await choose(0, 5, /^Cut/);
  assert.equal(await lastCopied(page), "alpha");
  await page.waitForFunction(() => window.__activeEditorView.state.doc.toString() === " beta beta");

  const point = await pointAt(page, 3);
  await openMenu(page, point.x, point.y);
  await item(page, /^Select all/).click();
  await menuClosed(page);
  assert.deepEqual(await page.evaluate(() => {
    const { from, to } = window.__activeEditorView.state.selection.main;
    return [from, to, window.__activeEditorView.state.doc.length];
  }), [0, 10, 10]);
}]);

// A modal <dialog> and an open popover are drawn in the browser's top layer, above everything
// else whatever its z-index, and a modal makes the rest of the page inert. A menu drawn in the
// page itself is hidden behind them and cannot be clicked, so these scenarios use real ones.
async function openSearchDialog(page) {
  await page.keyboard.press("Control+k");
  const input = page.locator("dialog[open] input").first();
  await input.waitFor();
  return input;
}

async function pointIn(locator, dx = 40) {
  const box = await locator.boundingBox();
  return { x: Math.round(box.x + dx), y: Math.round(box.y + box.height / 2) };
}

// A layer made by the test, so it can be closed or removed in ways the app's own dialogs never are.
function addLayer(page, kind, style = "") {
  return page.evaluate(([kind, style]) => {
    const layer = document.createElement(kind === "dialog" ? "dialog" : "div");
    layer.id = "probe-layer";
    if (kind === "popover") layer.setAttribute("popover", "auto");
    layer.style.cssText = style;
    layer.innerHTML = '<input id="probe-field" value="inside the layer" style="width:300px">';
    document.body.append(layer);
    if (kind === "dialog") layer.showModal();
    else layer.showPopover();
  }, [kind, style]);
}

const layerState = (page) => page.evaluate(() => {
  const menu = document.querySelector(".ctx-menu");
  const container = document.querySelector(".ctx-menu-container");
  const state = {
    menus: document.querySelectorAll(".ctx-menu").length,
    rulers: document.querySelectorAll(".ctx-menu-ruler").length,
    containerParent: container?.parentElement?.tagName ?? null,
    containerInDialog: !!container?.closest("dialog"),
    containerInPopover: !!container?.closest("[popover]"),
    openDialogs: document.querySelectorAll("dialog[open]").length,
  };
  if (menu) {
    const box = menu.getBoundingClientRect();
    const hit = document.elementFromPoint(box.left + box.width / 2, box.top + Math.min(20, box.height / 2));
    state.hitsMenu = !!hit && menu.contains(hit);
    state.focused = document.activeElement === menu;
  }
  return state;
});

const noStaleMenu = async (page) => {
  // The dialog's close event, which sends the menu's node home, arrives a moment after the dialog is gone.
  await page.waitForFunction(() =>
    document.querySelector(".ctx-menu-container")?.parentElement === document.body, null, { timeout: 2000 });
  const state = await layerState(page);
  assert.equal(state.menus, 0, "no menu is left behind");
  assert.equal(state.rulers, 1, "one ruler, and it is back with the page");
  assert.equal(state.containerParent, "BODY", "the menu's node is back in the page");
};

for (const zoom of [70, 150]) {
  scenarios.push([`opens at the pointer inside a modal dialog at ${zoom}% zoom`, { zoom }, async (page) => {
    const input = await openSearchDialog(page);
    const { x, y } = await pointIn(input, 60);
    const state = await openMenu(page, x, y);
    const layer = await layerState(page);
    assert.equal(state.count, 1, "one menu");
    assert.equal(layer.containerInDialog, true, "drawn inside the dialog");
    assert.equal(layer.hitsMenu, true, "the menu is what is under the pointer, not the dialog behind it");
    near(state.box.left, x, "left edge");
    near(state.box.top, y, "top edge");
  }]);
}

scenarios.push(["is on top of a modal dialog and its edit actions work", {}, async (page) => {
  const input = await openSearchDialog(page);
  await input.fill("hello world");
  const { x, y } = await pointIn(input);
  const state = await openMenu(page, x, y);
  const layer = await layerState(page);
  assert.deepEqual(outline(state), { Edit: ["Cut (disabled)", "Copy (disabled)", "Paste", "Select all"] });
  assert.equal(layer.hitsMenu, true, "clickable, though the page behind the dialog is inert");
  assert.equal(layer.focused, true, "the menu has the keyboard");

  // Playwright refuses to click something another element would receive the click for.
  await item(page, /^Select all/).click();
  await menuClosed(page);
  assert.deepEqual(await page.evaluate(() => {
    const field = document.querySelector("dialog[open] input");
    return [document.activeElement === field, field.selectionStart, field.selectionEnd];
  }), [true, 0, 11], "the field has the focus back, with everything selected");
  assert.equal((await layerState(page)).openDialogs, 1, "the dialog is still open");
  await noStaleMenu(page);
}]);

scenarios.push(["Escape closes the menu first and the dialog second", {}, async (page) => {
  const input = await openSearchDialog(page);
  const { x, y } = await pointIn(input);
  await openMenu(page, x, y);

  await page.keyboard.press("Escape");
  await menuClosed(page);
  assert.equal((await layerState(page)).openDialogs, 1, "the first Escape left the dialog open");
  assert.equal(await page.evaluate(() => document.activeElement === document.querySelector("dialog[open] input")), true,
    "the field has the focus back");
  await noStaleMenu(page);

  await page.keyboard.press("Escape");
  await page.locator("dialog[open]").waitFor({ state: "detached" });
  await noStaleMenu(page);
}]);

scenarios.push(["a menu opened again in a reopened dialog works the same", {}, async (page) => {
  for (let round = 0; round < 2; round++) {
    const input = await openSearchDialog(page);
    const { x, y } = await pointIn(input);
    const state = await openMenu(page, x, y);
    assert.equal(state.count, 1, `one menu in round ${round}`);
    assert.equal((await layerState(page)).hitsMenu, true, `on top in round ${round}`);
    await page.keyboard.press("Escape");
    await menuClosed(page);
    await page.keyboard.press("Escape");
    await page.locator("dialog[open]").waitFor({ state: "detached" });
    await noStaleMenu(page);
  }
}]);

scenarios.push(["goes with a dialog that is closed under it, and the page then works as before", {}, async (page) => {
  await addLayer(page, "dialog");
  const { x, y } = await pointIn(page.locator("#probe-field"));
  await openMenu(page, x, y);
  assert.equal((await layerState(page)).containerInDialog, true);

  await page.evaluate(() => {
    const layer = document.getElementById("probe-layer");
    layer.close();
    layer.remove();
  });
  await page.locator(MENU).waitFor({ state: "detached" });
  await noStaleMenu(page);

  const state = await rightClickBlock(page, "b1");
  assert.equal(state.count, 1);
  assert.equal((await layerState(page)).containerInDialog, false, "back to drawing in the page");
}]);

scenarios.push(["does not leave a stale menu when the dialog is removed without being closed", {}, async (page) => {
  await addLayer(page, "dialog");
  const { x, y } = await pointIn(page.locator("#probe-field"));
  await openMenu(page, x, y);
  await page.evaluate(() => document.getElementById("probe-layer").remove());
  assert.equal(await page.locator(MENU).count(), 0, "the menu went with it, not onto the page");

  // The next click or right-click finds nothing in the way, and one menu.
  const target = await blockPoint(page, "b1", 80);
  const state = await openMenu(page, target.x, target.y);
  const layer = await layerState(page);
  assert.equal(state.count, 1, "one menu");
  assert.equal(layer.containerParent, "BODY");
  assert.equal(layer.hitsMenu, true);
  near(state.box.left, target.x, "left edge");
  near(state.box.top, target.y, "top edge");
}]);

scenarios.push(["is drawn inside an open popover, and closes when the popover does", {}, async (page) => {
  await addLayer(page, "popover");
  const { x, y } = await pointIn(page.locator("#probe-field"));
  const state = await openMenu(page, x, y);
  const layer = await layerState(page);
  assert.equal(state.count, 1);
  assert.equal(layer.containerInPopover, true, "drawn inside the popover");
  assert.equal(layer.hitsMenu, true, "on top of the popover");
  near(state.box.left, x, "left edge");
  near(state.box.top, y, "top edge");

  await item(page, /^Select all/).click();
  await menuClosed(page);
  assert.equal(await page.evaluate(() => document.getElementById("probe-layer").matches(":popover-open")), true,
    "using the menu does not light-dismiss the popover");

  await openMenu(page, x, y);
  await page.evaluate(() => document.getElementById("probe-layer").hidePopover());
  await page.locator(MENU).waitFor({ state: "detached" });
  await noStaleMenu(page);
}]);

scenarios.push(["looks the same inside a dialog that styles its own text", {}, async (page) => {
  const read = (page) => page.evaluate(() => {
    const menu = document.querySelector(".ctx-menu");
    const button = menu.querySelector("button");
    const style = (element) => {
      const css = getComputedStyle(element);
      return [css.fontFamily, css.fontSize, css.fontWeight, css.fontStyle, css.letterSpacing, css.textTransform, css.textAlign];
    };
    return { menu: style(menu), button: style(button) };
  });

  const block = await blockPoint(page, "b1", 80);
  await openMenu(page, block.x, block.y);
  const onPage = await read(page);
  await page.keyboard.press("Escape");
  await menuClosed(page);

  await addLayer(page, "dialog",
    "font: italic 700 21px serif; letter-spacing: 4px; text-transform: uppercase; text-align: center;");
  const { x, y } = await pointIn(page.locator("#probe-field"));
  await openMenu(page, x, y);
  assert.deepEqual(await read(page), onPage, "the menu does not inherit the dialog's type");
}]);

module.exports = { scenarios };

if (require.main === module) (async () => {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  let failures = 0;
  try {
    for (const [name, options, run] of scenarios) {
      if (process.env.UI_TEST_CASE && !process.env.UI_TEST_CASE.split("|").some((filter) => name.includes(filter))) continue;
      let fixture;
      try {
        fixture = await openEditor(browser, options);
        await run(fixture.page);
        assert.deepEqual(fixture.errors, [], "no uncaught browser errors");
        assert.deepEqual(await fixture.page.evaluate(() => window.__selectionState.unhandledIpc), []);
        console.log(`PASS ${name}`);
      } catch (error) {
        failures++;
        console.error(`FAIL ${name}\n${error.stack ?? error}`);
        if (fixture) console.error("Menu state:", JSON.stringify(await menuState(fixture.page).catch(() => "unreadable")));
      } finally {
        await fixture?.page.close();
      }
    }
  } finally {
    await browser.close();
  }
  if (failures) process.exitCode = 1;
  else console.log("ALL PASSED");
})().catch((error) => { console.error(error); process.exitCode = 1; });

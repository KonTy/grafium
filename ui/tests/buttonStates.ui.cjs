// Every visible, enabled button must visibly react to hover and press, and
// toggles must show their pressed state. Synthetic fixtures only.
const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor, frames } = require("./keyboardSelection.ui.cjs");

const VIEWS = ["Journal", "Library", "All Pages", "Tasks", "Studies", "Flashcards", "Chat", "Settings"];

async function installLibrary(page) {
  await page.addInitScript(() => {
    if (window !== window.top) return;
    const books = [
      { id: "audio-book", title: "Synthetic audiobook", kind: "audio", available: true, favorite: true,
        tracks: [{ id: "t1", title: "Chapter 1", relativePath: "Fixture/1.mp3", available: true }],
        position: null, bookmarks: [] },
      { id: "epub-book", title: "Synthetic EPUB", kind: "epub", available: true, favorite: false,
        tracks: [], position: null, bookmarks: [] },
    ];
    function install(internals) {
      const original = internals.invoke;
      internals.invoke = async (command, args = {}) => {
        if (command === "reader_snapshot" || command === "reader_rescan")
          return structuredClone({ libraryPath: "/synthetic/library", books });
        if (command === "list_studies") return { items: [], days: [], topics: [] };
        return original(command, args);
      };
      return internals;
    }
    let internals = window.__TAURI_INTERNALS__;
    if (internals) internals = install(internals);
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true, get: () => internals, set: (value) => { internals = install(value); },
    });
  });
}

const STATE = ["backgroundColor", "backgroundImage", "color", "borderTopColor", "boxShadow",
  "textDecorationLine", "filter", "opacity", "outlineStyle"];

async function hoverable(page, view) {
  return page.evaluate((view) => {
    for (const marked of document.querySelectorAll("[data-button-audit]")) marked.removeAttribute("data-button-audit");
    if (view === "Settings") for (const section of document.querySelectorAll("details.settings-section")) section.open = true;
    return [...document.querySelectorAll("button")].filter((button) => {
      if (button.disabled || button.closest("[inert], [hidden], dialog:not([open]), [popover]:not(:popover-open)")) return false;
      const box = button.getBoundingClientRect();
      const style = getComputedStyle(button);
      return box.width >= 4 && box.height >= 4 && style.visibility !== "hidden" && style.pointerEvents !== "none";
    }).slice(0, 150).map((button, index) => {
      button.dataset.buttonAudit = String(index);
      return { index, name: (button.getAttribute("aria-label") || button.textContent || button.title || "").trim().slice(0, 50) };
    });
  }, view);
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const { page, errors } = await openEditor(browser, { beforeNavigate: installLibrary });
    const failures = [];
    let checked = 0;
    for (const view of VIEWS) {
      await page.locator(".sidebar").getByRole("button", { name: view, exact: true }).click();
      await page.waitForTimeout(400);
      await frames(page);
      for (const { index, name } of await hoverable(page, view)) {
        const button = page.locator(`[data-button-audit="${index}"]`);
        if (!(await button.count())) continue;
        try { await button.scrollIntoViewIfNeeded({ timeout: 1000 }); } catch { continue; }
        await page.mouse.move(0, 0);
        await page.waitForTimeout(30);
        // Buttons can legitimately disappear while others are hovered (menus
        // closing, rows re-rendering); those are skipped, not counted.
        const read = () => button.evaluate((node, keys) => {
          const style = getComputedStyle(node);
          return Object.fromEntries(keys.map((key) => [key, style[key]]));
        }, STATE, { timeout: 1000 }).catch(() => null);
        const resting = await read();
        if (!resting) continue;
        try { await button.hover({ timeout: 1000 }); } catch { continue; }
        await page.waitForTimeout(160);
        const hovered = await read();
        if (!hovered) continue;
        checked++;
        if (JSON.stringify(resting) === JSON.stringify(hovered)) failures.push(`${view}: "${name}" has no hover feedback`);
        await page.mouse.down();
        await page.waitForTimeout(40);
        const pressed = await read();
        await page.mouse.move(0, 0);
        await page.mouse.up();
        if (pressed && JSON.stringify(pressed) === JSON.stringify(hovered)) failures.push(`${view}: "${name}" has no press feedback`);
      }
      await page.keyboard.press("Escape");
    }
    const standardBackground = () => page.evaluate(() => {
      const probe = document.createElement("div");
      probe.style.background = "var(--bg-primary)";
      document.body.append(probe);
      const color = getComputedStyle(probe).backgroundColor;
      probe.remove();
      return color;
    });
    const auditDialog = async (dialog, label) => {
      await dialog.waitFor();
      for (const button of await dialog.locator("button:enabled").all()) {
        const name = ((await button.getAttribute("aria-label")) || (await button.innerText())).trim();
        const read = () => button.evaluate((node, keys) => {
          const style = getComputedStyle(node);
          return JSON.stringify(keys.map((key) => style[key]));
        }, STATE);
        await page.mouse.move(0, 0);
        const resting = await read();
        await button.hover();
        await page.waitForTimeout(160);
        checked++;
        if (resting === await read()) failures.push(`${label}: "${name}" has no hover feedback`);
      }
    };
    await page.locator(".sidebar").getByRole("button", { name: "All Pages", exact: true }).click();
    await page.getByRole("button", { name: "Import Media", exact: true }).click();
    const media = page.getByRole("dialog", { name: "Import from Video/Audio", exact: true });
    await auditDialog(media, "Import Video/Audio");
    const target = media.getByLabel("Save as", { exact: true });
    const select = await target.evaluate((node) => ({
      background: getComputedStyle(node).backgroundColor, appearance: getComputedStyle(node).appearance,
    }));
    assert.equal(select.background, await standardBackground(), "the media dropdown uses the standard background");
    assert.equal(select.appearance, "none");
    const ok = await media.getByRole("button", { name: "Import", exact: true }).evaluate((node) => getComputedStyle(node).borderTopStyle);
    const cancel = await media.getByRole("button", { name: "Cancel", exact: true }).evaluate((node) => getComputedStyle(node).borderTopStyle);
    assert.equal(ok, cancel, "dialog buttons share one border treatment");
    await media.getByRole("button", { name: "Cancel", exact: true }).click();
    await media.waitFor({ state: "hidden" });
    await page.keyboard.press("Alt+b");
    const books = page.getByRole("dialog", { name: "Import books", exact: true });
    await auditDialog(books, "Import books");
    await books.getByRole("button", { name: "Cancel", exact: true }).click();
    assert.deepEqual(failures, []);

    await page.locator(".sidebar").getByRole("button", { name: "Library", exact: true }).click();
    const favorites = page.getByRole("button", { name: "★ Favorites", exact: true });
    const before = await favorites.evaluate((node) => getComputedStyle(node).borderTopColor);
    await favorites.click();
    assert.equal(await favorites.getAttribute("aria-pressed"), "true");
    await page.mouse.move(0, 0);
    await page.waitForTimeout(160);
    const after = await favorites.evaluate((node) => getComputedStyle(node).borderTopColor);
    assert.notEqual(after, before, "a pressed Library toggle looks different from an unpressed one");
    assert.ok(checked > 150, `audited ${checked} buttons`);
    assert.deepEqual(failures, []);
    assert.deepEqual(errors, []);
    console.log(`Button states: ${checked} buttons across ${VIEWS.length} views react to hover and press PASS`);
  } finally {
    await browser.close();
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});

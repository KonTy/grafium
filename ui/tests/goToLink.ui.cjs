const { chromium, webkit } = require("playwright");
const assert = require("node:assert/strict");
const BASE_URL = process.env.UI_TEST_URL ?? "http://localhost:5199/";

(async () => {
  const browser = process.env.UI_TEST_BROWSER === "webkit"
    ? await webkit.launch()
    : await chromium.launch({ args: ["--no-sandbox"] });
  try {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    page.setDefaultTimeout(10_000);
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.addInitScript(() => {
      const row = (id, title, is_journal = false) => ({
        id, title, is_journal, file_path: null, properties: {}, created_at: 0, updated_at: 0,
      });
      const pages = [
        row("welcome", "Welcome To Grafium"),
        row("orbital", "Space/Orbital mechanics"),
        row("literal-route", "__graph__"),
        ...Array.from({ length: 1200 }, (_, i) => row(`topic-${i}`, `Topic ${String(i).padStart(4, "0")}`)),
        row("final", "ZZZ Final target"),
        row("journal", "2026-09-13", true),
      ].sort((a, b) => a.title.localeCompare(b.title));
      window.__linkFixture = { pages, calls: 0, writes: [], hold: false, fail: false };
      localStorage.setItem("grafium.session.lastLocation", JSON.stringify({ kind: "page", title: "Welcome To Grafium" }));
      let sequence = 0;
      window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
      window.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
        plugins: {}, transformCallback() { return ++sequence; }, unregisterCallback() {},
        invoke: async (cmd, args = {}) => {
          const fixture = window.__linkFixture;
          switch (cmd) {
            case "get_app_theme": return "github";
            case "get_layout_preferences": return { sidebarVisible: true, wideMode: true };
            case "get_graph_info": return { name: "Welcome test graph", path: "/synthetic/welcome-links" };
            case "list_page_summaries": {
              fixture.calls += 1;
              const result = fixture.pages.slice();
              while (fixture.hold) await new Promise((resolve) => setTimeout(resolve, 20));
              if (fixture.fail) throw new Error("Metadata temporarily unavailable");
              return result;
            }
            case "get_page": {
              const found = fixture.pages.find((p) => p.id === args.id || p.title === args.title);
              if (!found) throw new Error("Page not found");
              return found;
            }
            case "get_parent_page": return null;
            case "list_pages": return fixture.pages.filter((p) => !p.is_journal).slice(0, args.limit ?? 100);
            case "list_journal_pages": return args.offset ? [] : fixture.pages.filter((p) => p.is_journal);
            case "create_page": return { ...row("today", args.title, true) };
            case "list_blocks": return [{
              id: `${args.pageId}-block`, page_id: args.pageId, parent_id: null, order_index: 0,
              content: "Keep this note and cursor unchanged.", block_type: "text",
              properties: {}, created_at: 0, updated_at: 0,
            }];
            case "get_page_source": return `- Keep this note and cursor unchanged.\n  id:: ${args.pageId}-block\n`;
            case "update_page_source":
            case "update_block": fixture.writes.push({ cmd, args }); return;
            case "plugin:event|listen": return ++sequence;
            default: return [];
          }
        },
      };
    });
    await page.goto(BASE_URL, { waitUntil: "networkidle" });
    await page.locator(".block-item").first().waitFor();
    assert.equal(await page.evaluate(() => window.__linkFixture.calls), 0, "no full title-list query at startup");
    await page.evaluate(() => { window.__linkFixture.hold = true; });
    await page.keyboard.press("Control+l");
    const dialog = page.getByRole("dialog", { name: "Go to link", exact: true });
    const input = dialog.getByRole("combobox", { name: "Find a page" });
    await dialog.waitFor();
    assert.equal(await input.evaluate((el) => el === document.activeElement), true);
    const initialInputTop = (await input.boundingBox()).y;
    await page.keyboard.type("sporbme");
    assert.equal(await input.inputValue(), "sporbme");
    await page.evaluate(() => { window.__linkFixture.hold = false; });
    await dialog.getByRole("option", { selected: true }).filter({ hasText: "Space/Orbital mechanics" }).waitFor();
    assert.equal((await input.boundingBox()).y, initialInputTop, "the search box must not jump as results change");
    await page.keyboard.press("Enter");
    await dialog.waitFor({ state: "detached" });
    await page.locator(".main-content .page-title").filter({ hasText: "Space/Orbital mechanics" }).waitFor();
    console.log("PASS Ctrl+L focuses immediately while metadata loads; fuzzy fragments open a page");

    await page.keyboard.press("Control+l");
    await dialog.getByRole("option").first().waitFor();
    assert.equal(await dialog.getByRole("option").first().getAttribute("aria-setsize"), "1205");
    assert.ok(await dialog.getByRole("option").count() < 30, "all metadata, bounded rendered rows");
    for (let i = 0; i < 25; i++) await page.keyboard.press("ArrowDown");
    assert.equal(await dialog.getByRole("option", { selected: true }).getAttribute("aria-posinset"), "26");
    await page.keyboard.press("PageDown");
    assert.ok(Number(await dialog.getByRole("option", { selected: true }).getAttribute("aria-posinset")) > 26);
    assert.equal(await input.evaluate((el) => el === document.activeElement), true);
    await input.fill("zzfin");
    await dialog.getByRole("option", { selected: true }).filter({ hasText: "ZZZ Final target" }).waitFor();
    await page.keyboard.press("Enter");
    await page.locator(".main-content .page-title").filter({ hasText: "ZZZ Final target" }).waitFor();
    console.log("PASS keyboard navigation crosses virtual rows and fuzzy search reaches the last page");

    for (const continuous of [false, true]) {
      if (continuous) await page.getByRole("button", { name: "Experimental continuous editor", exact: true }).click();
      await page.locator(continuous ? ".unified-rendered-block" : ".block-content").first().click();
      const editor = page.locator(".cm-content").first();
      await editor.focus();
      const before = await page.evaluate(() => {
        const view = window.__activeEditorView ?? window.__unifiedPageEditorView;
        return { text: view.state.doc.toString(), selection: view.state.selection.toJSON() };
      });
      await page.keyboard.press("Alt+z");
      assert.equal(await editor.evaluate((el) => el === document.activeElement), true, "editor focused before opening navigator");
      await page.keyboard.press("Control+l");
      await input.waitFor();
      await input.fill("no-such-page-xyz");
      await dialog.getByText("No matching pages. Try fewer letters.").waitFor();
      await page.keyboard.press("Enter");
      assert.equal(await dialog.isVisible(), true);
      await page.keyboard.press("Alt+z");
      assert.equal(await page.locator(".app-shell.zen").count(), 1, "dialog owns shortcuts");
      for (let i = 0; i < 4; i++) {
        await page.keyboard.press("Tab");
        assert.equal(await dialog.evaluate((el) => el.contains(document.activeElement)), true);
      }
      await page.keyboard.press("Escape");
      await dialog.waitFor({ state: "detached" });
      await page.waitForFunction(() => document.activeElement?.classList.contains("cm-content")).catch(async (error) => {
        const state = await page.evaluate(() => ({
          active: document.activeElement?.outerHTML.slice(0, 200),
          editors: document.querySelectorAll(".cm-content").length,
          currentEditor: !!window.__activeEditorView,
          unified: !!window.__unifiedPageEditorView,
        }));
        throw new Error(`${continuous ? "continuous" : "classic"}: ${JSON.stringify(state)}`, { cause: error });
      });
      assert.equal(await page.locator(".app-shell.zen").count(), 1);
      assert.deepEqual(await page.evaluate(() => {
        const view = window.__activeEditorView ?? window.__unifiedPageEditorView;
        return { text: view.state.doc.toString(), selection: view.state.selection.toJSON() };
      }), before);
      await page.keyboard.press("Alt+z");
    }
    assert.deepEqual(await page.evaluate(() => window.__linkFixture.writes), []);
    console.log("PASS both editors preserve source, selection and focus; modal traps Tab and preserves Zen");

    await page.keyboard.press("Control+l");
    await input.fill("__graph__");
    await dialog.getByRole("option", { selected: true }).filter({ hasText: "__graph__" }).waitFor();
    await page.keyboard.press("Enter");
    await page.locator(".main-content .page-title").filter({ hasText: "__graph__" }).waitFor();
    assert.equal(await page.locator(".graph-view-wrapper").count(), 0, "literal page title is not an internal route");
    await page.keyboard.press("Control+g");
    const calendar = page.getByRole("dialog", { name: "Choose date" });
    await calendar.waitFor();
    await page.keyboard.press("Escape");
    const dateButton = page.getByRole("button", { name: "Go to date", exact: true });
    assert.equal((await dateButton.textContent()).trim(), "");
    assert.equal(await dateButton.locator("svg").count(), 1);
    await page.getByRole("button", { name: "Go to link", exact: true }).click();
    await input.waitFor();
    await input.fill("20260913");
    await dialog.getByRole("option", { selected: true }).filter({ hasText: "2026-09-13" }).waitFor();
    await page.keyboard.press("Escape");
    console.log("PASS adjacent icon controls, unchanged calendar keys, journal links, and ID-based navigation");

    await page.setViewportSize({ width: 390, height: 700 });
    await page.keyboard.press("Control+l");
    await dialog.getByRole("option").first().waitFor();
    const box = await dialog.boundingBox();
    assert.ok(box.x >= 0 && box.x + box.width <= 390 && box.y + box.height <= 700);
    assert.equal(await input.evaluate((el) => el === document.activeElement), true);
    await page.keyboard.press("Escape");
    await page.setViewportSize({ width: 1280, height: 900 });

    await page.evaluate(() => { window.__linkFixture.fail = true; });
    await page.keyboard.press("Control+l");
    await dialog.getByRole("alert").filter({ hasText: "Metadata temporarily unavailable" }).waitFor();
    await page.evaluate(() => { window.__linkFixture.fail = false; });
    await dialog.getByRole("button", { name: "Retry", exact: true }).click();
    await dialog.getByRole("option").first().waitFor();
    await page.keyboard.press("Escape");
    await page.evaluate(() => { window.__linkFixture.pages = []; });
    await page.keyboard.press("Control+l");
    await dialog.getByText("No pages in this graph yet.").waitFor();
    await page.keyboard.press("Escape");
    console.log("PASS visible errors with retry and a fresh metadata read on each open");
    assert.deepEqual(errors, []);
  } finally {
    await browser.close();
  }
})().catch((error) => { console.error(error); process.exit(1); });

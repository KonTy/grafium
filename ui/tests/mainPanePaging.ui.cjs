const { chromium, webkit } = require("playwright");
const assert = require("node:assert/strict");
const BASE_URL = process.env.UI_TEST_URL ?? "http://localhost:5199/";

async function fixture(browser) {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  page.setDefaultTimeout(10_000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.clock.setFixedTime(new Date("2026-09-13T12:00:00"));
  await page.addInitScript(() => {
    localStorage.setItem("grafium.session.lastLocation", JSON.stringify({ kind: "page", title: "Keyboard reading" }));
    const pages = Array.from({ length: 180 }, (_, i) => ({
      id: `page-${i}`, title: i === 0 ? "Keyboard reading" : i === 1 ? "Books/Keyboard book" : `Topic ${i}`,
      is_journal: false, file_path: `pages/${i === 1 ? "Books/" : ""}reading-${i}.md`,
      properties: {}, created_at: 0, updated_at: 0,
    }));
    const journal = { ...pages[0], id: "journal-day", title: "2026-09-13",
      is_journal: true, file_path: "journals/2026_09_13.md" };
    const blocks = (id) => Array.from({ length: 180 }, (_, i) => ({
      id: `${id}-block-${i}`, page_id: id, parent_id: null, order_index: i,
      content: i === 0
        ? Array.from({ length: 50 }, (_, line) => `Editable line ${line} stays unchanged.`).join("\n")
        : `Reading paragraph ${i}. A keyboard-only reading surface needs predictable scrolling and stable focus.`,
      block_type: "text", properties: {}, created_at: 0, updated_at: 0,
    }));
    let sequence = 0;
    window.__pagingWrites = [];
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
      plugins: {}, transformCallback() { return ++sequence; }, unregisterCallback() {},
      invoke: async (cmd, args = {}) => {
        switch (cmd) {
          case "get_app_theme": return "github";
          case "get_layout_preferences": return { sidebarVisible: true, wideMode: true };
          case "get_graph_info": return { name: "Keyboard fixture", path: "/synthetic/keyboard-paging" };
          case "get_page": {
            const found = [...pages, journal].find((p) => p.id === args.id || p.title === args.title);
            if (!found) throw new Error("Page not found");
            return found;
          }
          case "get_parent_page": return null;
          case "list_journal_pages":
            await new Promise((resolve) => setTimeout(resolve, 100));
            return args.offset ? [] : [journal];
          case "list_journal_note_dates": return args.year === 2026 && args.month === 9 ? [journal.title] : [];
          case "list_pages":
          case "list_page_summaries": return pages;
          case "list_blocks": return blocks(args.pageId);
          case "get_page_source": return blocks(args.pageId).map((block) => {
            const [first, ...rest] = block.content.split("\n");
            return [`- ${first}`, ...rest.map((line) => `  ${line}`), `  id:: ${block.id}`].join("\n");
          }).join("\n\n") + "\n";
          case "update_block":
          case "update_page_source":
          case "create_block": window.__pagingWrites.push({ cmd, args }); return;
          case "count_pages": return pages.length;
          case "list_pages_window": return pages.slice(args.offset, args.offset + args.limit);
          case "pages_namespace_tree": return pages.map((p) => ({
            key: p.title, label: p.title, page_id: p.id, children: [], descendant_count: 1, updated_at: 0,
          }));
          case "list_open_task_rows": return pages.map((p, i) => ({
            block_id: `task-${i}`, content: `TODO Read topic ${i}`, page_title: p.title,
            state: "TODO", priority: null, scheduled_date: null, scheduled_time: null,
            deadline_date: null, created_at: 0, updated_at: 0,
          }));
          case "task_flow_stats": return null;
          case "get_graph_data": return {
            nodes: [{ id: "a", title: "Alpha", degree: 1 }, { id: "b", title: "Beta", degree: 1 }],
            edges: [{ source: "a", target: "b", weight: 1 }],
          };
          case "plugin:event|listen": return ++sequence;
          default: return [];
        }
      },
    };
  });
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  await page.locator('.block-item[data-block-id="page-0-block-0"]').waitFor();
  return { page, errors };
}

async function pageKey(page, key, selector = ".main-content") {
  const before = await page.locator(selector).evaluate((el) => el.scrollTop);
  await page.keyboard.press(key);
  await page.waitForFunction(({ selector, before, down }) => {
    const top = document.querySelector(selector).scrollTop;
    return down ? top > before + 50 : top < before - 50;
  }, { selector, before, down: key === "PageDown" }).catch(async (error) => {
    error.message += JSON.stringify(await page.evaluate(({ selector, before }) => {
      const el = document.querySelector(selector);
      return { before, top: el.scrollTop, height: el.clientHeight, total: el.scrollHeight,
        active: document.activeElement?.outerHTML.slice(0, 150),
        rows: document.querySelectorAll(".block-item").length,
        overlays: [...document.querySelectorAll("[role='dialog'],[role='menu'],.cm-tooltip-autocomplete")]
          .map((node) => node.outerHTML.slice(0, 150)) };
    }, { selector, before }));
    throw new Error(error.message, { cause: error });
  });
  await page.evaluate(() => new Promise((resolve) =>
    requestAnimationFrame(() => requestAnimationFrame(resolve))));
}

const navigate = (page, title) => page.evaluate((title) =>
  window.dispatchEvent(new CustomEvent("navigate-page", { detail: title })), title);

const editorState = (page) => page.evaluate(() => {
  const view = window.__activeEditorView ?? window.__unifiedPageEditorView;
  return { doc: view.state.doc.toString(), selection: view.state.selection.toJSON(), focused: view.hasFocus };
});

(async () => {
  const browser = process.env.UI_TEST_BROWSER === "webkit"
    ? await webkit.launch()
    : await chromium.launch({ args: ["--no-sandbox"] });
  try {
    const { page, errors } = await fixture(browser);
    await pageKey(page, "PageDown");
    await pageKey(page, "PageUp");
    await page.keyboard.press("Control+b");
    const search = page.locator(".sidebar input").first();
    assert.equal(await search.evaluate((el) => el === document.activeElement), true);
    await pageKey(page, "PageDown");
    assert.equal(await search.evaluate((el) => el === document.activeElement), true);
    await page.keyboard.press("Alt+z");
    await pageKey(page, "PageDown");
    await pageKey(page, "PageUp");
    await page.keyboard.press("Alt+z");
    console.log("PASS page keys scroll the reader from startup, sidebar focus, and Zen without changing focus");

    for (const continuous of [false, true]) {
      if (continuous) await page.getByRole("button", { name: "Experimental continuous editor", exact: true }).click();
      await page.locator(".main-content").evaluate((el) => { el.scrollTop = 0; });
      if (!continuous) await page.locator('[data-block-id="page-0-block-0"] .block-content').click();
      else await page.locator(".unified-rendered-block").first().click();
      const editor = page.locator(".cm-content").first();
      await editor.focus();
      const before = await editorState(page);
      await pageKey(page, "PageDown");
      await pageKey(page, "PageDown");
      await pageKey(page, "PageUp");
      assert.deepEqual(await editorState(page), before);
      await page.keyboard.press("Shift+PageDown");
      const after = await editorState(page);
      assert.notDeepEqual(after.selection, before.selection, "Shift+PageDown remains editor selection");
      assert.equal(after.doc, before.doc);
      assert.deepEqual(await page.evaluate(() => window.__pagingWrites), []);
      console.log(`PASS ${continuous ? "continuous" : "classic"} editor paging preserves caret, focus, text and modified selection`);
    }

    await navigate(page, "Books/Keyboard book");
    await page.locator(".page-content.bookPage").waitFor();
    await page.keyboard.press("Alt+z");
    for (let i = 0; i < 5; i++) await pageKey(page, "PageDown");
    await page.waitForFunction(() => document.querySelectorAll(".bookPage .block-item").length > 32);
    assert.deepEqual(await page.evaluate(() => window.__pagingWrites), []);
    await page.keyboard.press("Alt+z");
    console.log("PASS keyboard paging progressively loads a book without editing or clicking into it");

    await navigate(page, "__statistics__");
    await page.locator(".task-item.open").first().waitFor();
    await pageKey(page, "PageDown", ".statistics-view");
    await pageKey(page, "PageUp", ".statistics-view");
    assert.equal(await page.locator(".main-content").evaluate((el) => el.scrollTop), 0);
    await navigate(page, "__all_pages__");
    await page.locator(".tree-browser").waitFor();
    await pageKey(page, "PageDown");
    await pageKey(page, "PageUp");
    console.log("PASS tasks use their nested scroller and All Pages uses the main pane");

    await navigate(page, "__graph__");
    await page.locator(".graph-view-wrapper canvas").waitFor();
    const prevented = await page.evaluate(() => {
      const event = new KeyboardEvent("keydown", { key: "PageDown", bubbles: true, cancelable: true });
      document.body.dispatchEvent(event);
      return event.defaultPrevented;
    });
    assert.equal(prevented, false, "graph keys must not be intercepted");
    await page.keyboard.press("Control+g");
    const calendar = page.getByRole("dialog", { name: "Choose date" });
    await calendar.waitFor();
    await calendar.locator('[data-date="2026-09-13"].has-notes').waitFor();
    await page.keyboard.press("Enter");
    await calendar.waitFor({ state: "detached" });
    await page.locator(".journal-feed .block-item").first().waitFor();
    await navigate(page, "Keyboard reading");
    await page.locator(".page-title").filter({ hasText: "Keyboard reading" }).waitFor();
    await page.keyboard.press("Control+Shift+J");
    await page.locator(".journal-feed .cm-content").waitFor();
    assert.equal(await calendar.count(), 0, "a consumed calendar request must not reopen on later journal navigation");
    assert.deepEqual(errors, []);
    await page.close();
    console.log("PASS graph keys are unchanged; Ctrl+G opens a journal calendar from another view without stale reopen requests");
  } finally {
    await browser.close();
  }
})().catch((error) => { console.error(error); process.exitCode = 1; });

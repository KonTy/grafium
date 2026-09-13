// Real journal/editor integration, with synthetic notes and controllable IPC.
const { chromium, webkit } = require("playwright");
const assert = require("node:assert/strict");
const BASE_URL = process.env.UI_TEST_URL ?? "http://localhost:5199/";

async function openJournal(browser, { holdFirst = false, failList = false } = {}) {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.clock.setFixedTime(new Date("2026-09-13T12:00:00"));
  await page.addInitScript(({ holdFirst, failList }) => {
    localStorage.setItem("grafium.ui.zoom", "100");
    localStorage.setItem("grafium.session.lastLocation", JSON.stringify({ kind: "journal" }));
    const counts = [29, 14, 3, 1, 2, 26, 23, 4, 5, 14, 60, 24, 34, 15, 34, 37, 191, 29, 26, 129];
    const pages = counts.map((_, i) => {
      const date = new Date(2026, 8, 13 - i);
      const title = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
      return { id: `day-${i}`, title, is_journal: true, properties: {},
        file_path: `journals/${title}.md`, created_at: 0, updated_at: 0 };
    });
    const blocks = pages.flatMap((note, day) => Array.from({ length: counts[day] }, (_, i) => ({
      id: `${note.id}-block-${i}`, page_id: note.id,
      parent_id: i % 3 === 0 ? null : `${note.id}-block-${i - i % 3}`,
      order_index: i, content: `Journal ${day}, note ${i}`,
      block_type: "markdown", properties: {}, created_at: 0, updated_at: 0,
    })));
    blocks.find((block) => block.id === "day-16-block-0").content = "[[Other page]]";
    const otherPage = { ...pages[0], id: "other-page", title: "Other page", is_journal: false, file_path: "pages/Other page.md" };
    blocks.push({ ...blocks[0], id: "other-block", page_id: otherPage.id, content: "A linked page" });
    let sequence = 0;
    window.__journalState = { pages, blocks, calls: [], holdFirst, failList, holdUpdate: false, failBlocks: null, heightObservers: 0 };
    const observe = ResizeObserver.prototype.observe;
    ResizeObserver.prototype.observe = function (node, options) {
      if (node.classList.contains("block-shell")) window.__journalState.heightObservers++;
      return observe.call(this, node, options);
    };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
      plugins: {}, transformCallback() { return ++sequence; }, unregisterCallback() {},
      invoke: async (cmd, args = {}) => {
        const state = window.__journalState;
        state.calls.push({ cmd, args });
        switch (cmd) {
          case "get_graph_info": return { name: "Journal test", path: "/synthetic/journal-test" };
          case "get_app_theme": return "github";
          case "get_page": {
            if (args.id === otherPage.id || args.title === otherPage.title) return structuredClone(otherPage);
            const found = state.pages.find((note) => note.id === args.id || note.title === args.title);
            if (found) return structuredClone(found);
            throw new Error("Database error: Query returned no rows");
          }
          case "list_journal_pages":
            if (state.failList) throw new Error("Simulated journal list failure");
            return structuredClone(state.pages.slice(args.offset ?? 0, (args.offset ?? 0) + args.limit));
          case "list_blocks":
            if (args.pageId === "day-0" && state.holdFirst) {
              state.holdFirst = false;
              await new Promise((resolve) => { window.__releaseFirstJournal = resolve; });
            }
            if (args.pageId === state.failBlocks) throw new Error("Simulated entry failure");
            return structuredClone(state.blocks.filter((block) => block.page_id === args.pageId));
          case "update_block": {
            if (state.holdUpdate) await new Promise((resolve) => { window.__releaseJournalUpdate = resolve; });
            const block = state.blocks.find((item) => item.id === args.id);
            if (!block) throw new Error("Block not found");
            block.content = args.content;
            return;
          }
          case "get_parent_page": return null;
          case "plugin:event|listen": return ++sequence;
          default: return [];
        }
      },
    };
  }, { holdFirst, failList });
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  return { page, errors };
}

const entry = (page, day) => page.locator(`.journal-entry[data-page-id="day-${day}"]`);
const frames = (page) => page.evaluate(() => new Promise((resolve) =>
  requestAnimationFrame(() => requestAnimationFrame(resolve))));
async function loaded(page, day) {
  await entry(page, day).locator(".block-item").first().waitFor();
  await page.waitForFunction((day) =>
    document.querySelector(`[data-page-id="day-${day}"]`)?.getAttribute("aria-busy") === "false", day);
  await frames(page);
}
async function scrollTo(page, day) {
  await page.locator(".journal-feed").hover();
  await page.mouse.wheel(0, 1);
  await entry(page, day).evaluate((node) => node.scrollIntoView({ block: "start" }));
  await loaded(page, day);
}
async function chooseDate(page, month, day) {
  await page.getByRole("button", { name: "Go to date", exact: true }).click();
  if (month === 8) await page.locator(".date-picker").getByRole("button", { name: "Previous", exact: true }).click();
  await page.locator(".dp-grid").getByRole("button", { name: String(day), exact: true }).click();
}

(async () => {
  const browser = process.env.UI_TEST_BROWSER === "webkit"
    ? await webkit.launch()
    : await chromium.launch({ args: ["--no-sandbox"] });
  try {
    const { page, errors } = await openJournal(browser, { holdFirst: true });
    await page.waitForFunction(() => typeof window.__releaseFirstJournal === "function");
    await frames(page);
    const pending = await page.evaluate(() => ({
      loads: window.__journalState.calls.filter((call) => call.cmd === "list_blocks").map((call) => call.args.pageId),
      lists: window.__journalState.calls.filter((call) => call.cmd === "list_journal_pages" && call.args.limit === 10),
    }));
    assert.deepEqual(pending.loads, ["day-0"], "a slow first day must not fan out to offscreen editors");
    assert.equal(pending.lists.length, 1, "initial sentinel must not fetch a second metadata batch");
    await page.evaluate(() => window.__releaseFirstJournal());
    await loaded(page, 0);
    await frames(page);
    const startupPages = await page.locator(".journal-entry .page-content").count();
    const startupBlocks = await page.locator(".journal-entry .block-item").count();
    assert.ok(startupPages <= 2, "only near-viewport days mount at startup");
    assert.ok(startupBlocks < 100, "startup does not mount all 700 blocks");
    assert.equal(await page.evaluate(() => window.__journalState.heightObservers), 0,
      "nonvirtualized journal blocks do not allocate individual height observers");
    console.log(`Startup: ${startupPages} mounted day(s), ${startupBlocks} block rows`);
    assert.equal(await entry(page, 9).locator(".page-content").count(), 0);

    await entry(page, 0).locator('[data-block-id="day-0-block-1"] .block-content').click();
    const editor = entry(page, 0).locator(".cm-content");
    await editor.waitFor();
    await page.evaluate(() => {
      window.__journalState.holdUpdate = true;
      window.__originalJournalEditor = document.querySelector('[data-page-id="day-0"] .cm-content');
      window.__originalJournalRow = document.querySelector('[data-block-id="day-0-block-1"]');
    });
    await editor.fill("Unsaved journal text survives scrolling");
    await scrollTo(page, 5);
    assert.equal(await page.evaluate(() => window.__originalJournalEditor.isConnected), true);
    assert.equal(await editor.textContent(), "Unsaved journal text survives scrolling");
    await scrollTo(page, 0);
    assert.equal(await page.evaluate(() => window.__originalJournalRow.isConnected), true);

    await chooseDate(page, 8, 28);
    await page.waitForFunction(() => typeof window.__releaseJournalUpdate === "function");
    await loaded(page, 16);
    assert.equal(await entry(page, 12).locator(".page-content").count(), 0, "jumping to an old date skips intermediate editors");
    assert.equal(await page.evaluate(() => window.__originalJournalRow.isConnected), true, "date navigation retains earlier editors");
    const position = await entry(page, 16).evaluate((node) =>
      node.getBoundingClientRect().top - node.closest(".journal-feed").getBoundingClientRect().top);
    assert.ok(Math.abs(position) < 3, `date target stays aligned after hydration, offset=${position}`);
    await page.evaluate(() => { window.__journalState.holdUpdate = false; window.__releaseJournalUpdate(); });

    await entry(page, 16).locator('.page-link[data-page="Other page"]').click();
    await page.locator('.block-item[data-block-id="other-block"]').waitFor();
    await page.keyboard.press("Control+[");
    await loaded(page, 16);
    assert.equal(await entry(page, 12).locator(".page-content").count(), 0, "history restore hydrates the destination without mounting intermediate days");
    assert.ok(await page.locator(".journal-entry .page-content").count() <= 3, "history restore is lazy too");

    await page.keyboard.press("Control+Shift+J");
    await entry(page, 0).locator('[data-block-id="day-0-block-28"] .cm-content').waitFor();
    assert.equal(await entry(page, 0).locator(".cm-content").evaluate((node) => {
      const editor = node.getBoundingClientRect();
      const feed = node.closest(".journal-feed").getBoundingClientRect();
      return editor.top >= feed.top && editor.bottom <= feed.bottom;
    }), true, "edit-today scrolls the focused last block into view after lazy hydration");
    assert.equal((await entry(page, 0).locator('[data-block-id="day-0-block-1"] .block-content').textContent()).trim(),
      "Unsaved journal text survives scrolling");

    const beforeMidnight = await entry(page, 0).locator(".cm-content").evaluate((node) => {
      window.__beforeMidnightEditor = node;
      window.__beforeMidnightScroll = document.querySelector(".journal-feed").scrollTop;
      return node.getBoundingClientRect().top;
    });
    await page.evaluate(() => {
      const state = window.__journalState;
      const today = { ...state.pages[0], id: "next-day", title: "2026-09-14", file_path: "journals/2026-09-14.md" };
      state.pages.unshift(today);
      state.blocks.push({ ...state.blocks[0], id: "next-day-block", page_id: today.id, content: "A new day" });
    });
    await page.clock.setFixedTime(new Date("2026-09-14T00:00:01"));
    await page.locator('.journal-entry[data-page-title="2026-09-14"]').waitFor({ state: "attached" });
    await frames(page);
    await page.waitForFunction(() => !document.querySelector('.journal-entry[aria-busy="true"]'));
    await frames(page);
    const afterMidnight = await page.evaluate(() => ({
      connected: window.__beforeMidnightEditor.isConnected,
      top: window.__beforeMidnightEditor.getBoundingClientRect().top,
      beforeScroll: window.__beforeMidnightScroll,
      scroll: document.querySelector(".journal-feed").scrollTop,
      entries: [...document.querySelectorAll(".journal-entry")].slice(0, 3).map((node) => ({
        title: node.dataset.pageTitle, top: node.getBoundingClientRect().top,
        height: node.getBoundingClientRect().height, busy: node.getAttribute("aria-busy"),
      })),
    }));
    assert.equal(afterMidnight.connected, true, "midnight refresh must not remount a focused editor");
    assert.ok(Math.abs(afterMidnight.top - beforeMidnight) < 3,
      `midnight insertion preserves the current scroll anchor (${beforeMidnight} -> ${JSON.stringify(afterMidnight)})`);
    assert.deepEqual(errors, []);
    await page.close();
    console.log("PASS bounded startup, slow-load pagination, scrolling, pending edits, far-date jump, history restore, edit-today and midnight anchoring");

    const failure = await openJournal(browser, { failList: true });
    await failure.page.getByRole("button", { name: "Retry loading journals", exact: true }).waitFor();
    await failure.page.evaluate(() => { window.__journalState.failList = false; });
    await failure.page.getByRole("button", { name: "Retry loading journals", exact: true }).click();
    await loaded(failure.page, 0);
    await failure.page.evaluate(() => { window.__journalState.failBlocks = "day-5"; });
    await entry(failure.page, 5).evaluate((node) => node.scrollIntoView({ block: "start" }));
    await entry(failure.page, 5).getByText(/Simulated entry failure/).waitFor();
    await failure.page.evaluate(() => { window.__journalState.failBlocks = null; });
    await entry(failure.page, 5).getByRole("button", { name: "Retry", exact: true }).click();
    await loaded(failure.page, 5);
    await failure.page.evaluate(() => { window.__journalState.failList = true; });
    await scrollTo(failure.page, 9);
    await failure.page.getByRole("button", { name: "Retry loading older journals", exact: true }).waitFor();
    await failure.page.evaluate(() => { window.__journalState.failList = false; });
    await failure.page.getByRole("button", { name: "Retry loading older journals", exact: true }).click();
    await entry(failure.page, 10).waitFor({ state: "attached" });
    await scrollTo(failure.page, 10);
    assert.deepEqual(failure.errors, []);
    await failure.page.close();
    console.log("PASS journal-list, entry and pagination failures remain visible and retryable; scrolling loads the next batch");

    const delayedEdit = await openJournal(browser, { holdFirst: true });
    await delayedEdit.page.waitForFunction(() => typeof window.__releaseFirstJournal === "function");
    await delayedEdit.page.keyboard.press("Control+Shift+J");
    // Longer than the old forty-frame focus retry: the intent must survive IPC.
    await delayedEdit.page.waitForTimeout(900);
    await delayedEdit.page.evaluate(() => window.__releaseFirstJournal());
    await entry(delayedEdit.page, 0).locator('[data-block-id="day-0-block-28"] .cm-content').waitFor();
    assert.deepEqual(delayedEdit.errors, []);
    await delayedEdit.page.close();
    console.log("PASS edit-today survives a delayed initial block load");
  } finally {
    await browser.close();
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});

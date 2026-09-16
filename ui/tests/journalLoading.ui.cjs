// Real journal/editor integration, with synthetic notes and controllable IPC.
const { chromium, webkit } = require("playwright");
const assert = require("node:assert/strict");
const BASE_URL = process.env.UI_TEST_URL ?? "http://localhost:5199/";

async function openJournal(browser, {
  holdFirst = false, failList = false, navigation = false, unifiedDay = null,
  now = "2026-09-13T12:00:00", lookupFailure = null, failCreate = false,
} = {}) {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (unifiedDay !== null && message.type() === "error") console.error(message.text());
  });
  await page.clock.setFixedTime(new Date(now));
  await page.addInitScript(({ holdFirst, failList, navigation, unifiedDay, lookupFailure, failCreate }) => {
    localStorage.setItem("grafium.ui.zoom", "100");
    localStorage.setItem("grafium.session.lastLocation", JSON.stringify({ kind: "journal" }));
    const counts = [29, 14, navigation ? 0 : 3, 1, 2, 26, 23, 4, 5, 14, 60, 24, 34, 15, 34, 37, 191, 29, 26, 129];
    if (unifiedDay !== null) localStorage.setItem(`grafium.experimental.unifiedPageEditor:day-${unifiedDay}`, "1");
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
    window.__journalState = {
      pages, blocks, calls: [], holdFirst, failList, lookupFailure, failCreate,
      holdUpdate: false, failBlocks: null, heightObservers: 0,
      originalPages: structuredClone(pages), originalBlocks: structuredClone(blocks),
    };
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
          case "get_layout_preferences": return { sidebarVisible: true, wideMode: true };
          case "list_journal_note_dates": {
            if (state.failMarkers) throw new Error("Simulated marker failure");
            if (args.month === state.holdMarkerMonth) {
              await new Promise((resolve) => { window.__releaseCalendarMarkers = resolve; });
            }
            const prefix = `${String(args.year).padStart(4, "0")}-${String(args.month).padStart(2, "0")}-`;
            return state.pages.filter((note) => note.title.startsWith(prefix)
              && state.blocks.some((block) => block.page_id === note.id && block.content.trim()))
              .map((note) => note.title);
          }
          case "get_page": {
            if (state.lookupFailure) throw state.lookupFailure;
            if (args.id === otherPage.id || args.title === otherPage.title) return structuredClone(otherPage);
            const found = state.pages.find((note) => note.id === args.id || note.title === args.title);
            if (found) return structuredClone(found);
            throw { code: "page_not_found", message: `No page has the title or approved alias '${args.title ?? args.id}'.` };
          }
          case "create_page": {
            if (state.failCreate) throw new Error("Synthetic journal creation failed.");
            if (state.pages.some(({ title }) => title === args.title)) throw new Error("Duplicate journal creation.");
            const created = {
              ...state.pages[0], id: `created-${args.title}`, title: args.title,
              is_journal: args.isJournal, properties: {}, file_path: `journals/${args.title}.md`,
            };
            state.pages.push(created);
            state.pages.sort((a, b) => b.title.localeCompare(a.title));
            return structuredClone(created);
          }
          case "list_journal_pages":
            if (state.failList) throw new Error("Simulated journal list failure");
            if (args.limit === 10 && args.offset === state.holdListOffset) {
              state.holdListOffset = null;
              await new Promise((resolve) => { window.__releaseJournalList = resolve; });
            }
            return structuredClone(state.pages.slice(args.offset ?? 0, (args.offset ?? 0) + args.limit));
          case "list_blocks":
            if (args.pageId === "day-0" && state.holdFirst) {
              state.holdFirst = false;
              await new Promise((resolve) => { window.__releaseFirstJournal = resolve; });
            }
            if (args.pageId === state.failBlocks) throw new Error("Simulated entry failure");
            if (args.pageId === state.holdBlocks) {
              state.holdBlocks = null;
              await new Promise((resolve) => { window.__releaseJournalBlocks = resolve; });
            }
            return structuredClone(state.blocks.filter((block) => block.page_id === args.pageId));
          case "get_page_source":
            return state.blocks.filter((block) => block.page_id === args.pageId)
              .map((block) => `- ${block.content}\n  id:: ${block.id}\n`).join("");
          case "create_block": {
            const block = { ...state.blocks[0], id: `${args.pageId}-empty`, page_id: args.pageId,
              parent_id: args.parentId, order_index: args.orderIndex, content: args.content };
            state.blocks.push(block);
            return structuredClone(block);
          }
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
  }, { holdFirst, failList, navigation, unifiedDay, lookupFailure, failCreate });
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
  await page.waitForFunction(() => document.activeElement?.hasAttribute("data-date"));
  const currentMonth = await page.evaluate(() => Number(document.activeElement.dataset.date.slice(5, 7)));
  for (let i = 0; i < Math.abs(month - currentMonth); i++) {
    await page.locator(".date-picker").getByRole("button", {
      name: month < currentMonth ? "Previous month" : "Next month", exact: true,
    }).click();
  }
  await page.locator(`.dp-grid button[data-date="2026-${String(month).padStart(2, "0")}-${String(day).padStart(2, "0")}"]`).click();
}

async function focusJournalBlock(page, id, edge = "top") {
  await page.locator(`.block-item[data-block-id="${id}"] .block-content`).click();
  await page.waitForFunction((id) => document.activeElement?.closest(".block-item")?.dataset.blockId === id, id);
  await page.evaluate((edge) => {
    const view = window.__activeEditorView;
    view.dispatch({ selection: { anchor: edge === "top" ? 0 : view.state.doc.length }, scrollIntoView: true });
  }, edge);
  await frames(page);
}

async function checkJournalCreation(browser) {
    {
      const { page, errors } = await openJournal(browser, { now: "2026-09-16T12:00:00" });
      await page.locator('.journal-entry[data-page-title="2026-09-16"] .block-item').first().waitFor({ timeout: 6000 });
      const state = await page.evaluate(() => window.__journalState);
      assert.deepEqual(state.calls.filter(({ cmd }) => cmd === "create_page").map(({ args }) => args),
        [{ title: "2026-09-16", isJournal: true }]);
      assert.deepEqual(state.pages.filter(({ id }) => !id.startsWith("created-")), state.originalPages);
      assert.deepEqual(state.blocks.filter(({ page_id }) => !page_id.startsWith("created-")), state.originalBlocks);
      assert.equal(await page.getByRole("button", { name: "Retry loading journals", exact: true }).count(), 0);
      assert.deepEqual(errors, []);
      await page.close();
      console.log("PASS a genuinely missing current journal is created once without changing existing notes");
    }

    for (const lookupFailure of [
      { code: "page_lookup_failed", message: "Database error: Query returned no rows" },
      { code: "page_lookup_failed", message: "The requested journal alias is ambiguous." },
      "Database is locked",
    ]) {
      const { page, errors } = await openJournal(browser, { lookupFailure });
      await page.getByRole("button", { name: "Retry loading journals", exact: true }).waitFor();
      assert.equal(await page.evaluate(() => window.__journalState.calls.some(({ cmd }) => cmd === "create_page")), false);
      await page.evaluate(() => { window.__journalState.lookupFailure = null; });
      await page.getByRole("button", { name: "Retry loading journals", exact: true }).click();
      await loaded(page, 0);
      assert.deepEqual(errors, []);
      await page.close();
    }
    console.log("PASS database and ambiguous-name failures remain retryable and never authorize journal creation");

    {
      const { page, errors } = await openJournal(browser, { now: "2026-09-16T12:00:00", failCreate: true });
      await page.getByRole("alert").filter({ hasText: "Synthetic journal creation failed." }).waitFor();
      await page.evaluate(() => { window.__journalState.failCreate = false; });
      await page.getByRole("button", { name: "Retry loading journals", exact: true }).click();
      await page.locator('.journal-entry[data-page-title="2026-09-16"] .block-item').first().waitFor();
      assert.equal(await page.evaluate(() => window.__journalState.pages.filter(({ title }) => title === "2026-09-16").length), 1);
      assert.deepEqual(errors, []);
      await page.close();
      console.log("PASS journal creation failures remain visible and retry succeeds without duplicates");
    }

    {
      const { page, errors } = await openJournal(browser);
      await loaded(page, 0);
      await chooseDate(page, 9, 14);
      await page.locator('.journal-entry[data-page-title="2026-09-14"] .block-item').first().waitFor();
      assert.deepEqual(await page.evaluate(() => window.__journalState.calls
        .filter(({ cmd }) => cmd === "create_page").map(({ args }) => args)),
      [{ title: "2026-09-14", isJournal: true }]);
      assert.deepEqual(errors, []);
      await page.close();
      console.log("PASS the calendar creates a genuinely missing chosen journal date");
    }
}

async function expectFocusedBlock(page, id) {
  try {
    await page.waitForFunction((id) => document.activeElement?.closest(".block-item")?.dataset.blockId === id, id);
  } catch (error) {
    console.error("Journal focus mismatch", id, await page.evaluate(() => ({
      active: document.activeElement?.className,
      block: document.activeElement?.closest(".block-item")?.dataset.blockId,
      text: window.__activeEditorView?.state.doc.toString(),
      selection: window.__activeEditorView?.state.selection.toJSON(),
    })));
    throw error;
  }
  await frames(page);
}

(async () => {
  const browser = process.env.UI_TEST_BROWSER === "webkit"
    ? await webkit.launch()
    : await chromium.launch({ args: ["--no-sandbox"] });
  try {
    await checkJournalCreation(browser);
    {
      const { page, errors } = await openJournal(browser, { navigation: true });
      await loaded(page, 0);
      await focusJournalBlock(page, "day-0-block-28");
      await entry(page, 0).locator(".cm-content").fill("First visual line");
      await page.keyboard.press("Shift+Enter");
      await page.keyboard.type("Second visual line");
      await frames(page);
      await page.keyboard.press("ArrowUp");
      assert.equal(await page.evaluate(() => window.__activeEditorView.state.doc.lineAt(
        window.__activeEditorView.state.selection.main.head).number), 1);
      await page.keyboard.press("ArrowDown");
      await expectFocusedBlock(page, "day-0-block-28");
      assert.equal(await page.evaluate(() => window.__activeEditorView.state.doc.lineAt(
        window.__activeEditorView.state.selection.main.head).number), 2, "Down moves within a multiline block first");
      await page.keyboard.press("Shift+ArrowUp");
      await expectFocusedBlock(page, "day-0-block-28");
      await page.keyboard.press("ArrowRight");
      await page.keyboard.press("Home");
      await page.keyboard.press("ArrowRight");
      await page.keyboard.press("ArrowRight");
      const caretX = await page.evaluate(() => window.__activeEditorView.coordsAtPos(
        window.__activeEditorView.state.selection.main.head).left);
      await page.keyboard.press("ArrowDown");
      await expectFocusedBlock(page, "day-1-block-0");
      const landedX = await page.evaluate(() => window.__activeEditorView.coordsAtPos(
        window.__activeEditorView.state.selection.main.head).left);
      assert.ok(Math.abs(landedX - caretX) < 8, `cross-day navigation preserves the visual column: ${caretX} -> ${landedX}`);
      await page.waitForFunction(() => window.__journalState.blocks.find((b) => b.id === "day-0-block-28")
        .content === "First visual line\nSecond visual line");
      await page.keyboard.press("ArrowUp");
      await expectFocusedBlock(page, "day-0-block-28");
      assert.equal(await page.evaluate(() => window.__activeEditorView.state.doc.lineAt(
        window.__activeEditorView.state.selection.main.head).number), 2, "Up enters the previous day's last visual line");

      await entry(page, 0).locator('.block-item[data-block-id="day-0-block-27"] .bullet-container').click();
      await focusJournalBlock(page, "day-1-block-0");
      await page.keyboard.press("ArrowUp");
      await expectFocusedBlock(page, "day-0-block-27");
      assert.equal(await entry(page, 0).locator('.block-item[data-block-id="day-0-block-28"]').count(), 0,
        "cross-day focus respects collapsed children");
      await page.keyboard.press("ArrowDown");
      await expectFocusedBlock(page, "day-1-block-0");
      await focusJournalBlock(page, "day-1-block-13", "bottom");
      await page.keyboard.press("ArrowDown");
      await expectFocusedBlock(page, "day-2-empty");
      assert.equal(await page.evaluate(() => window.__journalState.calls.filter((call) => call.cmd === "create_block"
        && call.args.pageId === "day-2").length), 1, "an empty existing day gets one editable block");
      await page.keyboard.press("ArrowDown");
      await expectFocusedBlock(page, "day-3-block-0");
      await page.keyboard.press("ArrowUp");
      await expectFocusedBlock(page, "day-2-empty");

      await page.evaluate(() => {
        window.__journalState.holdBlocks = "day-8";
        window.__journalState.failBlocks = "day-7";
      });
      await chooseDate(page, 9, 4);
      await loaded(page, 9);
      await focusJournalBlock(page, "day-9-block-0");
      await page.keyboard.press("ArrowUp");
      await page.waitForFunction(() => typeof window.__releaseJournalBlocks === "function");
      await page.keyboard.press("ArrowUp");
      await page.keyboard.type("Keep typing here");
      await page.evaluate(() => window.__releaseJournalBlocks());
      await loaded(page, 8);
      await expectFocusedBlock(page, "day-9-block-0");
      await page.keyboard.press("Home");
      await page.keyboard.press("ArrowUp");
      await expectFocusedBlock(page, "day-8-block-4");
      await focusJournalBlock(page, "day-8-block-0");
      await page.keyboard.press("ArrowUp");
      await page.getByText(/Could not move to 2026-09-06:/).waitFor();
      await expectFocusedBlock(page, "day-8-block-0");
      await page.evaluate(() => { window.__journalState.failBlocks = null; });
      await entry(page, 7).getByRole("button", { name: "Retry", exact: true }).click();
      await loaded(page, 7);
      await focusJournalBlock(page, "day-8-block-0");
      await page.keyboard.press("ArrowUp");
      await expectFocusedBlock(page, "day-7-block-3");
      assert.deepEqual(errors, []);
      await page.close();
      console.log("PASS cross-day arrows, multiline edges, caret column, saved edits, collapsed/empty days, deferred loads, cancellation and errors");
    }

    {
      const { page, errors } = await openJournal(browser, { holdFirst: true });
      await page.waitForFunction(() => typeof window.__releaseFirstJournal === "function");
      await page.evaluate(() => {
        window.__journalState.holdBlocks = "day-1";
        window.__releaseFirstJournal();
      });
      await loaded(page, 0);
      await focusJournalBlock(page, "day-0-block-28", "bottom");
      await page.keyboard.press("ArrowDown");
      await page.waitForFunction(() => typeof window.__releaseJournalBlocks === "function");
      await page.keyboard.press("ArrowDown");
      await page.keyboard.press("ArrowDown");
      await expectFocusedBlock(page, "day-0-block-28");
      await page.evaluate(() => window.__releaseJournalBlocks());
      await expectFocusedBlock(page, "day-1-block-0");
      assert.deepEqual(errors, []);
      await page.close();
      console.log("PASS delayed next-day loading completes the arrow move without extra keypresses or skipped days");
    }

    {
      const { page, errors } = await openJournal(browser);
      await loaded(page, 0);
      await page.evaluate(() => { window.__journalState.holdListOffset = 10; });
      await chooseDate(page, 9, 4);
      await loaded(page, 9);
      await focusJournalBlock(page, "day-9-block-13", "bottom");
      await page.keyboard.press("ArrowDown");
      await page.waitForFunction(() => typeof window.__releaseJournalList === "function");
      await expectFocusedBlock(page, "day-9-block-13");
      await page.evaluate(() => window.__releaseJournalList());
      await expectFocusedBlock(page, "day-10-block-0");
      await page.keyboard.press("ArrowUp");
      await expectFocusedBlock(page, "day-9-block-13");
      await chooseDate(page, 8, 25);
      await loaded(page, 19);
      await focusJournalBlock(page, "day-19-block-128", "bottom");
      await page.keyboard.press("ArrowDown");
      await expectFocusedBlock(page, "day-19-block-128");
      await chooseDate(page, 9, 13);
      await loaded(page, 0);
      await focusJournalBlock(page, "day-0-block-0");
      await page.keyboard.press("ArrowUp");
      await expectFocusedBlock(page, "day-0-block-0");
      assert.equal(await page.evaluate(() => window.__journalState.calls.filter((call) => call.cmd === "create_page").length), 0,
        "arrows at feed endpoints do not invent new dates");
      assert.deepEqual(errors, []);
      await page.close();
      console.log("PASS arrows await metadata pagination and stay put at the feed endpoints");
    }

    {
      const { page, errors } = await openJournal(browser, { unifiedDay: 1 });
      await loaded(page, 0);
      await focusJournalBlock(page, "day-0-block-28", "bottom");
      await page.keyboard.press("ArrowDown");
      await page.waitForFunction(() => document.activeElement?.closest(".journal-entry")?.dataset.pageId === "day-1");
      await page.keyboard.press("ArrowUp");
      await expectFocusedBlock(page, "day-0-block-28");
      await scrollTo(page, 2);
      await focusJournalBlock(page, "day-2-block-0");
      await page.keyboard.press("ArrowUp");
      await page.waitForFunction(() => document.activeElement?.closest(".journal-entry")?.dataset.pageId === "day-1");
      assert.equal(await page.evaluate(() => window.__activeEditorView.state.doc.lineAt(
        window.__activeEditorView.state.selection.main.head).text), "- Journal 1, note 13");
      await page.keyboard.press("ArrowDown");
      await expectFocusedBlock(page, "day-2-block-0");
      assert.deepEqual(errors, []);
      await page.close();
      console.log("PASS navigation between classic and continuous journal entries in both directions");
    }

    {
      const { page, errors } = await openJournal(browser);
      await loaded(page, 0);
      const feed = page.locator(".journal-feed");
      assert.equal(await page.evaluate(() => window.__journalState.calls.filter((call) => call.cmd === "list_journal_note_dates").length), 0,
        "calendar metadata is not requested on ordinary startup");
      await page.keyboard.press("PageDown");
      await page.waitForFunction(() => document.querySelector(".journal-feed").scrollTop > 200);
      await frames(page);
      assert.equal(await page.locator(".main-content").evaluate((el) => el.scrollTop), 0);
      await page.keyboard.press("PageUp");
      await page.waitForFunction(() => document.querySelector(".journal-feed").scrollTop < 100);
      await page.keyboard.press("Control+b");
      await page.keyboard.press("Alt+z");
      await page.keyboard.press("PageDown");
      await page.waitForFunction(() => document.querySelector(".journal-feed").scrollTop > 200);
      await page.keyboard.press("PageUp");
      await page.waitForFunction(() => document.querySelector(".journal-feed").scrollTop < 100);
      await page.keyboard.press("Control+g");
      const calendar = page.getByRole("dialog", { name: "Choose date" });
      await calendar.waitFor();
      await page.waitForFunction(() => document.activeElement?.getAttribute("data-date") === "2026-09-13");
      await calendar.locator('[data-date="2026-09-13"].has-notes').waitFor();
      assert.equal(await calendar.locator('[data-date="2026-09-14"].has-notes').count(), 0);
      const before = await feed.evaluate((el) => el.scrollTop);
      for (const [key, date] of [
        ["ArrowLeft", "2026-09-12"], ["ArrowUp", "2026-09-05"],
        ["PageUp", "2026-08-05"], ["Home", "2026-08-03"], ["End", "2026-08-09"],
        ["PageDown", "2026-09-09"], ["Shift+PageUp", "2025-09-09"], ["Shift+PageDown", "2026-09-09"],
      ]) {
        await page.keyboard.press(key);
        await page.waitForFunction((date) => document.activeElement?.getAttribute("data-date") === date, date);
      }
      assert.equal(await feed.evaluate((el) => el.scrollTop), before, "calendar keys must not scroll the journal behind it");
      for (let i = 0; i < 3; i++) await page.keyboard.press("Shift+Tab");
      assert.match(await page.evaluate(() => document.activeElement?.getAttribute("aria-label")), /^Choose month/);
      await page.keyboard.press("Enter");
      await calendar.locator(".dp-month-grid").waitFor();
      await page.keyboard.press("ArrowRight");
      await page.keyboard.press("Enter");
      await page.waitForFunction(() => document.activeElement?.getAttribute("data-date") === "2026-10-09");
      await page.keyboard.press("PageUp");
      await page.waitForFunction(() => document.activeElement?.getAttribute("data-date") === "2026-09-09");
      await page.keyboard.press("Shift+Tab");
      await page.keyboard.press("Shift+Tab");
      assert.equal(await page.evaluate(() => document.activeElement?.getAttribute("aria-label")), "Choose year");
      await page.keyboard.press("Enter");
      await calendar.locator(".dp-month-grid").waitFor();
      await page.keyboard.press("ArrowRight");
      await page.keyboard.press("Enter");
      await page.waitForFunction(() => document.activeElement?.getAttribute("aria-label") === "September 2027");
      await page.keyboard.press("Enter");
      await page.waitForFunction(() => document.activeElement?.getAttribute("data-date") === "2027-09-09");
      await page.keyboard.press("Shift+PageUp");
      await page.waitForFunction(() => document.activeElement?.getAttribute("data-date") === "2026-09-09");
      await page.keyboard.press("Tab");
      assert.equal(await page.evaluate(() => document.activeElement?.textContent), "Today");
      await page.keyboard.press("Tab");
      assert.equal(await page.evaluate(() => document.activeElement?.getAttribute("aria-label")), "Previous month");
      await page.keyboard.press("Shift+Tab");
      assert.equal(await page.evaluate(() => document.activeElement?.textContent), "Today");
      await page.keyboard.press("Escape");
      await calendar.waitFor({ state: "detached" });
      assert.equal(await page.locator(".app-shell.zen").count(), 1, "Escape closes the calendar, not Zen");

      await page.keyboard.press("Control+g");
      await calendar.waitFor();
      await page.keyboard.press("ArrowLeft");
      await page.keyboard.press("Enter");
      await calendar.waitFor({ state: "detached" });
      await loaded(page, 1);
      assert.ok(Math.abs(await entry(page, 1).evaluate((el) => el.getBoundingClientRect().top
        - el.closest(".journal-feed").getBoundingClientRect().top)) < 3);
      await page.keyboard.press("Escape");
      await page.keyboard.press("Control+Shift+J");
      await entry(page, 0).locator(".cm-content").waitFor();
      const savedEditor = await page.evaluate(() => {
        window.__calendarInvoker = document.activeElement;
        return window.__activeEditorView.state.selection.toJSON();
      });
      await page.keyboard.press("Control+g");
      await calendar.waitFor();
      await page.keyboard.press("Escape");
      await page.waitForFunction(() => document.activeElement === window.__calendarInvoker);
      assert.deepEqual(await page.evaluate(() => window.__activeEditorView.state.selection.toJSON()), savedEditor);
      await page.keyboard.press("PageDown");
      const afterPaging = await feed.evaluate((el) => el.scrollTop);
      assert.ok(afterPaging > 0);
      await page.waitForTimeout(150);
      assert.ok(await feed.evaluate((el) => el.scrollTop) >= afterPaging - 3,
        "late journal hydration must not pull keyboard paging back");
      assert.equal(await page.evaluate(() => document.activeElement === window.__calendarInvoker), true);
      assert.deepEqual(await page.evaluate(() => window.__activeEditorView.state.selection.toJSON()), savedEditor);

      await page.keyboard.press("Control+g");
      await calendar.waitFor();
      await page.evaluate(() => { window.__journalState.holdMarkerMonth = 8; });
      await page.keyboard.press("PageUp");
      await page.waitForFunction(() => typeof window.__releaseCalendarMarkers === "function");
      await page.keyboard.press("PageDown");
      await calendar.locator('[data-date="2026-09-13"].has-notes').waitFor();
      await page.evaluate(() => window.__releaseCalendarMarkers());
      await frames(page);
      assert.equal(await calendar.locator('[data-date="2026-09-13"].has-notes').count(), 1,
        "an old month's late response must not replace current markers");
      await page.keyboard.press("Escape");
      await page.evaluate(() => { window.__journalState.failMarkers = true; });
      await page.keyboard.press("Control+g");
      await page.getByText(/Could not load calendar note markers:/).waitFor();
      await page.keyboard.press("Escape");
      await page.evaluate(() => { window.__journalState.failMarkers = false; });
      await page.keyboard.press("Control+g");
      await calendar.locator(".has-notes").first().waitFor();
      assert.deepEqual(errors, []);
      await page.close();
      console.log("PASS keyboard-only journal paging, Ctrl+G, date navigation, note circles, focus restoration, stale marker responses and visible failures");
    }

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
    await page.clock.setFixedTime(new Date("2026-09-14T00:00:01"));
    await page.locator('.journal-entry[data-page-title="2026-09-14"]').waitFor({ state: "attached" });
    assert.deepEqual(await page.evaluate(() => window.__journalState.calls
      .filter(({ cmd }) => cmd === "create_page").map(({ args }) => args)),
    [{ title: "2026-09-14", isJournal: true }]);
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

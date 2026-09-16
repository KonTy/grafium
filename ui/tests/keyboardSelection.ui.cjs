// Real classic/continuous editors; all notes and persistence live in this IPC fixture.
const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const BASE_URL = process.env.UI_TEST_URL ?? "http://localhost:5199/";
const ROOT = '[data-keyboard-block-selection="true"]';
const TOOLBAR = ".keyboard-selection-toolbar";

async function openEditor(browser, { beforeNavigate, ...options } = {}) {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  page.setDefaultTimeout(6000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.clock.setFixedTime(new Date("2026-09-13T12:00:00"));
  await page.addInitScript((options) => {
    localStorage.setItem("grafium.ui.zoom", "100");
    localStorage.setItem("grafium.session.lastLocation", JSON.stringify(options.journal ? { kind: "journal" }
      : { kind: "page", title: options.book ? "Books/Keyboard selection" : "Keyboard selection" }));
    for (const day of options.unifiedDays ?? []) {
      localStorage.setItem(`grafium.experimental.unifiedPageEditor:day-${day}`, "1");
    }
    if (options.unifiedPage) localStorage.setItem("grafium.experimental.unifiedPageEditor:selection-page", "1");
    const makePage = (id, title, journal = false) => ({
      id, title, is_journal: journal, properties: {},
      file_path: `${journal ? "journals" : "pages"}/${title}.md`, created_at: 0, updated_at: 0,
    });
    const note = makePage("selection-page", options.book ? "Books/Keyboard selection" : "Keyboard selection");
    const days = Array.from({ length: 12 }, (_, day) =>
      makePage(`day-${day}`, `2026-09-${String(13 - day).padStart(2, "0")}`, true));
    const block = (pageId, id, order, content, parent = null) => ({
      id, page_id: pageId, parent_id: parent, order_index: order, content,
      block_type: "Text", properties: {}, created_at: 0, updated_at: 0,
    });
    const standalone = options.blockCount ? Array.from({ length: options.blockCount }, (_, i) =>
      block(note.id, `large-${i}`, i, `Virtual block ${String(i).padStart(4, "0")}`)) : [
      block(note.id, "b0", 0, "First visual line"),
      block(note.id, "b1", 1, "Second block"),
      block(note.id, "b2", 2, "Folded branch"),
      block(note.id, "hidden-child", 0, "Hidden child", "b2"),
      block(note.id, "hidden-grandchild", 0, "Hidden grandchild", "hidden-child"),
      block(note.id, "b3", 3, "After branch"),
      block(note.id, "b4", 4, "Final block"),
    ];
    if (options.typedChild) Object.assign(standalone.find((block) => block.id === "hidden-grandchild"), {
      block_type: "Audio", properties: { asset: "assets/synthetic-audio.ogg", label: "Retain nested metadata" },
    });
    const journal = days.flatMap((note, day) =>
      Array.from({ length: day === 0 ? 16 : day === 2 ? 0 : 2 }, (_, i) =>
        block(note.id, `${note.id}-b${i}`, i, `Journal ${day}, block ${i}`)));
    if (!options.flatJournal) {
      journal.push(block("day-0", "journal-hidden", 0, "Hidden journal child", "day-0-b15"));
      journal.push(block("day-0", "journal-grandchild", 0, "Hidden journal grandchild", "journal-hidden"));
    }
    let sequence = 0;
    const state = window.__selectionState = {
      pages: [note, ...days], blocks: [...standalone, ...journal], calls: [], completed: [],
      holdUpdate: false, failUpdate: false, updateWaiters: [],
      holdBlocks: options.holdBlocks ?? null, holdListOffset: options.holdListOffset ?? null,
      clipboard: [], clipboardRequests: [], clipboardWaiters: [], holdClipboard: false, failClipboard: false,
      fallbackClipboard: null, clipboardFallbacks: [], deleteFocus: [],
      holdDelete: false, deleteWaiters: [], holdReload: false, reloadReads: [],
    };
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: {
      writeText: async (text) => {
        state.clipboardRequests.push(text);
        if (state.holdClipboard) await new Promise((resolve) => state.clipboardWaiters.push(resolve));
        if (state.failClipboard) throw new Error("Simulated clipboard API failure");
        state.clipboard.push(text);
      },
      readText: async () => state.clipboard.at(-1) ?? "",
    } });
    const execCommand = document.execCommand.bind(document);
    document.execCommand = (command, ...args) => {
      if (command !== "copy" || state.fallbackClipboard == null) return execCommand(command, ...args);
      window.__fallbackClipboardHost = document.activeElement;
      const text = document.activeElement?.value ?? window.getSelection()?.toString() ?? "";
      const succeeded = state.fallbackClipboard === "success";
      state.clipboardFallbacks.push({ text, succeeded });
      if (succeeded) state.clipboard.push(text);
      return succeeded;
    };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
      plugins: {}, transformCallback() { return ++sequence; }, unregisterCallback() {},
      invoke: async (cmd, args = {}) => {
        state.calls.push({ cmd, args: structuredClone(args) });
        switch (cmd) {
          case "get_graph_info": return { name: "Keyboard selection fixture", path: "/synthetic/keyboard-selection" };
          case "ai_get_config": return { enabled: false, mode: "local" };
          case "ai_health_check": return {
            enabled: false, llm_available: false, embedder_available: false,
            vector_store_available: false, vector_count: 0, mode: "local",
          };
          case "assistant_context_info": {
            const source = state.pages.find(({ id }) => id === args.pageId);
            if (!source) throw new Error("The synthetic source page no longer exists.");
            const isBook = Boolean(options.book && source.id === "selection-page");
            return {
              pageId: source.id, pageTitle: source.title, isJournal: source.is_journal, isBook,
              blockCount: state.blocks.filter(({ page_id }) => page_id === source.id).length,
              section: null, book: isBook ? { pageId: source.id, title: source.title } : null,
            };
          }
          case "get_app_theme": return "github";
          case "get_layout_preferences": return { sidebarVisible: true, wideMode: true };
          case "get_page": {
            const found = state.pages.find((note) => note.id === args.id || note.title === args.title);
            if (!found) throw new Error("Database error: Query returned no rows");
            return structuredClone(found);
          }
          case "get_parent_page": return null;
          case "list_pages": return structuredClone(state.pages);
          case "list_journal_note_dates":
            return days.filter((note) => note.title.startsWith(`${args.year}-${String(args.month).padStart(2, "0")}-`))
              .filter((note) => state.blocks.some((block) => block.page_id === note.id && block.content.trim()))
              .map((note) => note.title);
          case "list_journal_pages":
            if (args.limit === 10 && args.offset === state.holdListOffset) {
              state.listPromise ??= new Promise((resolve) => { window.__releaseList = resolve; });
              await state.listPromise;
            }
            return structuredClone(days.slice(args.offset ?? 0, (args.offset ?? 0) + args.limit));
          case "list_blocks":
            await waitForReload(args.pageId, cmd);
            if (args.pageId === state.holdBlocks) {
              state.blocksPromise ??= new Promise((resolve) => { window.__releaseBlocks = resolve; });
              await state.blocksPromise;
            }
            return structuredClone(orderedBlocks(args.pageId));
          case "get_page_source":
            await waitForReload(args.pageId, cmd);
            return sourceFor(args.pageId);
          case "update_page_source": {
            if (state.holdUpdate) await new Promise((resolve) => state.updateWaiters.push(resolve));
            if (state.failUpdate) throw new Error("Simulated selection source save failure");
            const { parsePageSourceMap } = await import("/src/lib/pageSourceMap.ts");
            const parents = [];
            const siblingCounts = new Map();
            const restored = parsePageSourceMap(args.content).blocks.map((item) => {
              const id = item.id ?? `source-${++sequence}`;
              const parent = item.depth ? parents[item.depth - 1] : null;
              parents[item.depth] = id;
              const order = siblingCounts.get(parent) ?? 0;
              siblingCounts.set(parent, order + 1);
              return block(args.pageId, id, order, item.content, parent);
            });
            state.blocks = [...state.blocks.filter((block) => block.page_id !== args.pageId), ...restored];
            state.completed.push({ cmd, pageId: args.pageId });
            return;
          }
          case "create_block": {
            const created = { ...block(args.pageId, args.id ?? `${args.pageId}-created-${++sequence}`,
              args.orderIndex, args.content, args.parentId ?? null),
              block_type: nativeBlockType(args.blockType), properties: structuredClone(args.properties ?? {}) };
            state.blocks.push(created);
            return structuredClone(created);
          }
          case "create_blocks": {
            const created = [];
            for (const item of args.blocks) {
              const parent = item.parentIndex == null ? item.parentId ?? null : created[item.parentIndex].id;
              assertUnusedId(item.id);
              created.push({ ...block(args.pageId, item.id ?? `created-${++sequence}`, item.orderIndex, item.content, parent),
                block_type: nativeBlockType(item.blockType), properties: structuredClone(item.properties ?? {}) });
            }
            state.blocks.push(...created);
            state.completed.push({ cmd, pageId: args.pageId });
            return structuredClone(created);
          }
          case "update_block": {
            if (state.holdUpdate) await new Promise((resolve) => state.updateWaiters.push(resolve));
            if (state.failUpdate) throw new Error("Simulated selection source save failure");
            const found = state.blocks.find((block) => block.id === args.id);
            if (!found) throw new Error(`Missing update block ${args.id}`);
            found.content = args.content;
            state.completed.push({ cmd, id: args.id, content: args.content });
            return;
          }
          case "delete_blocks": {
            state.deleteFocus.push({
              tag: document.activeElement?.tagName,
              restored: document.activeElement === window.__expectedClipboardFocus,
            });
            if (state.holdDelete) await new Promise((resolve) => state.deleteWaiters.push(resolve));
            const ids = new Set(args.ids);
            for (let previous = -1; previous !== ids.size;) {
              previous = ids.size;
              for (const block of state.blocks) if (ids.has(block.parent_id)) ids.add(block.id);
            }
            const removed = state.blocks.filter((block) => ids.has(block.id));
            if (removed.some((block) => block.page_id !== args.pageId)) throw new Error("Deletion must be per-page");
            state.blocks = state.blocks.filter((block) => !ids.has(block.id));
            state.completed.push({ cmd, pageId: args.pageId });
            return structuredClone(removed);
          }
          case "reorder_blocks":
            args.blockIds.forEach((id, order) => {
              const found = state.blocks.find((block) => block.id === id);
              if (found) found.order_index = order;
            });
            return;
          case "plugin:clipboard-manager|write_text":
            state.clipboard.push(args.text);
            return;
          case "plugin:event|listen": return ++sequence;
          default: return [];
        }
      },
    };
    function assertUnusedId(id) {
      if (id && state.blocks.some((block) => block.id === id)) throw new Error(`Duplicate restored ID ${id}`);
    }
    function nativeBlockType(value) {
      return { handwriting: "Handwriting", audio: "Audio", mixed: "Mixed", flashcard: "Flashcard", query: "Query" }[value] ?? "Text";
    }
    async function waitForReload(pageId, cmd) {
      if (!state.holdReload || !state.completed.some((call) => call.cmd === "delete_blocks" && call.pageId === pageId)) return;
      state.reloadReads.push({ cmd, pageId });
      state.reloadPromise ??= new Promise((resolve) => { window.__releaseReload = resolve; });
      await state.reloadPromise;
    }
    function compareBlocks(a, b) {
      return a.order_index - b.order_index || a.created_at - b.created_at || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
    }
    function orderedBlocks(pageId) {
      const blocks = state.blocks.filter((block) => block.page_id === pageId);
      const visit = (parent) => blocks.filter((block) => block.parent_id === parent)
        .sort(compareBlocks)
        .flatMap((block) => [block, ...visit(block.id)]);
      return visit(null);
    }
    function sourceFor(pageId) {
      const blocks = state.blocks.filter((block) => block.page_id === pageId);
      const visit = (parent, depth) => blocks.filter((block) => block.parent_id === parent)
        .sort(compareBlocks).map((block) => {
          const indent = "  ".repeat(depth);
          const [first, ...rest] = block.content.split("\n");
          return [`${indent}- ${first}`, ...rest.map((line) => `${indent}  ${line}`),
            `${indent}  id:: ${block.id}`, visit(block.id, depth + 1)].filter(Boolean).join("\n");
        }).join("\n");
      return `${visit(null, 0)}\n`;
    }
  }, options);
  if (beforeNavigate) await beforeNavigate(page);
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  try {
    if (!options.journal) {
      const initialBlockId = options.initialBlockId ?? (options.blockCount ? "large-0" : "b0");
      if (options.unifiedPage) await page.locator(`[data-source-block-id="${initialBlockId}"]`).waitFor();
      else await row(page, initialBlockId).waitFor();
    } else {
      if (options.unifiedDays?.includes(0)) await page.locator('[data-source-block-id="day-0-b0"]').waitFor();
      else await row(page, "day-0-b0").waitFor();
    }
  } catch (error) {
    console.error("Fixture startup:", errors, (await page.locator("body").innerText()).slice(0, 1500));
    await page.close();
    throw error;
  }
  return { page, errors };
}

const row = (page, id) => page.locator(`.block-item[data-block-id="${id}"]`);
const frames = (page) => page.evaluate(() => new Promise((resolve) =>
  requestAnimationFrame(() => requestAnimationFrame(resolve))));
async function focus(page, id, anchor = "end") {
  await row(page, id).locator(".block-content").click();
  await page.waitForFunction((id) => document.activeElement?.closest(".block-item")?.dataset.blockId === id, id);
  await page.evaluate((anchor) => {
    const view = window.__activeEditorView;
    view.dispatch({ selection: { anchor: anchor === "end" ? view.state.doc.length : anchor }, scrollIntoView: true });
  }, anchor);
  await frames(page);
}
const nativeRange = (page) => page.evaluate(() => {
  const { anchor, head } = window.__activeEditorView.state.selection.main;
  return { anchor, head };
});
async function selected(page, ids) {
  await page.waitForFunction((ids) => {
    const actual = [...document.querySelectorAll(".block-item.selected")].map((node) => node.dataset.blockId);
    return JSON.stringify(actual) === JSON.stringify(ids);
  }, ids);
  assert.equal(await page.locator(ROOT).count() > 0, ids.length > 0);
  if (ids.length) {
    await page.locator(TOOLBAR).waitFor();
    assert.match(await page.locator(TOOLBAR).innerText(), new RegExp(`\\b${ids.length}\\b`), "toolbar counts visible selected rows");
  } else {
    assert.equal(await page.locator(TOOLBAR).count(), 0);
  }
}
async function shift(page, direction, ids) {
  await page.keyboard.press(`Shift+Arrow${direction}`);
  await frames(page);
  await selected(page, ids);
}
async function copy(page, type = "copy") {
  return page.evaluate((type) => {
    const data = new DataTransfer();
    const event = new ClipboardEvent(type, { clipboardData: data, bubbles: true, cancelable: true });
    (document.activeElement ?? document.body).dispatchEvent(event);
    return { text: data.getData("text/plain"), markdown: data.getData("text/markdown"), prevented: event.defaultPrevented };
  }, type);
}
async function snapshot(page, pageIds = ["selection-page"]) {
  return page.evaluate((pageIds) => window.__selectionState.blocks.filter((block) => pageIds.includes(block.page_id))
    .map(({ id, page_id, parent_id, order_index, content, block_type, properties }) =>
      ({ id, page_id, parent_id, order_index, content, block_type, properties }))
    .sort((a, b) => a.page_id.localeCompare(b.page_id) || a.order_index - b.order_index || a.id.localeCompare(b.id)), pageIds);
}
async function saved(page, id, content) {
  await page.waitForFunction(({ id, content }) =>
    window.__selectionState.blocks.find((block) => block.id === id)?.content === content, { id, content });
}
async function release(page, kind) {
  await page.evaluate((kind) => {
    const state = window.__selectionState;
    if (kind === "Update") {
      state.holdUpdate = false;
      state.updateWaiters.splice(0).forEach((resolve) => resolve());
    } else {
      if (kind === "Blocks") state.holdBlocks = null;
      else state.holdListOffset = null;
      window[`__release${kind}`]();
    }
  }, kind);
  await frames(page);
}
async function focused(page, id) {
  await page.waitForFunction((id) => document.activeElement?.closest(".block-item")?.dataset.blockId === id, id);
}
async function fold(page, id, child) {
  await row(page, id).locator(".bullet-container").click();
  await row(page, child).waitFor({ state: "detached" });
}
async function chooseDay(page, day) {
  await page.getByRole("button", { name: "Go to date", exact: true }).click();
  await page.locator(`.dp-grid button[data-date="2026-09-${String(13 - day).padStart(2, "0")}"]`).click();
  await row(page, `day-${day}-b0`).waitFor();
}
async function focusContinuous(page, pageId, blockId, edge = "end") {
  const scope = pageId === "selection-page" ? page.locator(".page-content").first()
    : page.locator(`.journal-entry[data-page-id="${pageId}"]`);
  await scope.locator(`[data-source-block-id="${blockId}"] .unified-rendered-content`).click();
  await page.waitForFunction(() => document.activeElement?.closest(".unified-page-editor") != null);
  await page.evaluate(async ({ blockId, edge }) => {
    const view = window.__activeEditorView;
    const { parsePageSourceMap } = await import("/src/lib/pageSourceMap.ts");
    const block = parsePageSourceMap(view.state.doc.toString()).blocks.find((block) => block.id === blockId);
    if (!block) throw new Error(`Missing continuous source block ${blockId}`);
    const anchor = edge === "page-end" ? view.state.doc.length : edge === "page-start" ? 0
      : edge === "end" ? block.contentTo : block.contentFrom;
    view.dispatch({ selection: { anchor }, scrollIntoView: true });
  }, { blockId, edge });
  await frames(page);
}
async function selectionCount(page, count) {
  await page.waitForFunction((count) => {
    const toolbar = document.querySelector(".keyboard-selection-toolbar");
    return toolbar && new RegExp(`\\b${count}\\b`).test(toolbar.textContent);
  }, count);
  assert.ok(await page.locator(ROOT).count() > 0);
}
async function nativeArrow(page, direction, extend = true) {
  await page.evaluate(({ direction, extend }) => {
    if (typeof window.__handleNativeVerticalArrow !== "function") throw new Error("Native arrow bridge is not installed");
    window.__handleNativeVerticalArrow(direction, extend);
  }, { direction, extend });
  await frames(page);
}
async function expectCaret(page, id, edge = "start", continuous = false) {
  const isAtCaret = async ({ id, edge, continuous }) => {
    const view = window.__activeEditorView;
    if (!view?.hasFocus || !view.state.selection.main.empty) return false;
    let position;
    if (continuous) {
      if (!document.activeElement?.closest(".unified-page-editor")) return false;
      const { parsePageSourceMap } = await import("/src/lib/pageSourceMap.ts");
      const block = parsePageSourceMap(view.state.doc.toString()).blocks.find((block) => block.id === id);
      if (!block) return false;
      position = edge === "end" ? block.contentTo : block.contentFrom;
    } else {
      if (document.activeElement?.closest(".block-item")?.dataset.blockId !== id) return false;
      position = edge === "end" ? view.state.doc.length : 0;
    }
    return view.state.selection.main.head === position;
  };
  const expected = { id, edge, continuous };
  await page.waitForFunction(isAtCaret, expected);
  await frames(page);
  assert.equal(await page.evaluate(isAtCaret, expected), true, "deletion-gap focus and exact caret survive the following render frames");
}
async function typeAfterDelete(page, id, expected, text, continuous = false) {
  await page.keyboard.type(text);
  const actual = await page.evaluate(async ({ id, continuous }) => {
    const view = window.__activeEditorView;
    if (!continuous) return view.state.doc.toString();
    const { parsePageSourceMap } = await import("/src/lib/pageSourceMap.ts");
    return parsePageSourceMap(view.state.doc.toString()).blocks.find((block) => block.id === id)?.content;
  }, { id, continuous });
  assert.equal(actual, expected, "typing immediately after deletion edits the deletion gap without a click");
  if (continuous) {
    const pageId = await page.evaluate((id) => window.__selectionState.blocks.find((block) => block.id === id).page_id, id);
    const scope = pageId === "selection-page" ? page.locator(".page-content").first()
      : page.locator(`.journal-entry[data-page-id="${pageId}"]`);
    await scope.getByRole("button", { name: "Save source", exact: true }).click();
  } else await page.evaluate(() => document.activeElement.blur());
  await saved(page, id, expected);
}

const cases = [
  ["multiline text before blocks, extension and origin restoration", {}, async (page) => {
    await focus(page, "b0");
    await page.keyboard.press("Shift+Enter");
    await page.keyboard.type("Second visual line");
    await page.evaluate(() => window.__activeEditorView.dispatch({ selection: { anchor: 2 } }));
    await frames(page);
    await shift(page, "Down", []);
    const original = await nativeRange(page);
    assert.ok(original.head > original.anchor, "first Shift+Down extends native text selection");
    assert.equal(await page.evaluate(() => window.__activeEditorView.state.doc.lineAt(
      window.__activeEditorView.state.selection.main.head).number), 2);
    await shift(page, "Down", ["b0", "b1"]);
    await saved(page, "b0", "First visual line\nSecond visual line");
    await shift(page, "Down", ["b0", "b1", "b2"]);
    await shift(page, "Up", ["b0", "b1"]);
    await shift(page, "Up", []);
    await focused(page, "b0");
    assert.deepEqual(await nativeRange(page), original, "shrinking to origin restores the exact text range");
  }],
  ["wrapped visual lines and reverse upward selection", {}, async (page) => {
    await focus(page, "b2");
    await row(page, "b2").locator(".cm-content").fill("Upper line");
    await page.keyboard.press("Shift+Enter");
    await page.keyboard.type("Lower line");
    await page.evaluate(() => window.__activeEditorView.dispatch({ selection: { anchor: 19 } }));
    await frames(page);
    await shift(page, "Up", []);
    const original = await nativeRange(page);
    assert.ok(original.head < original.anchor);
    await shift(page, "Up", ["b1", "b2"]);
    await shift(page, "Up", ["b0", "b1", "b2"]);
    await shift(page, "Down", ["b1", "b2"]);
    await shift(page, "Down", []);
    assert.deepEqual(await nativeRange(page), original);
    await focus(page, "b0");
    await row(page, "b0").locator(".cm-content").fill("Wrapped text should remain native inside the block. ".repeat(15));
    await row(page, "b0").evaluate((node) => { node.style.maxWidth = "360px"; });
    await page.evaluate(() => window.__activeEditorView.dispatch({ selection: { anchor: 3 } }));
    await frames(page);
    const top = await page.evaluate(() => window.__activeEditorView.coordsAtPos(3).top);
    await shift(page, "Down", []);
    assert.ok(await page.evaluate((top) => {
      const view = window.__activeEditorView;
      return view.coordsAtPos(view.state.selection.main.head).top > top;
    }, top), "Shift+Down moves one wrapped visual line without promoting the block");
    await page.evaluate(() => {
      const view = window.__activeEditorView;
      view.dispatch({ selection: { anchor: view.state.doc.length }, scrollIntoView: true });
    });
    await frames(page);
    await shift(page, "Down", ["b0", "b1"]);
  }],
  ["clipboard, toolbar clear, Escape and plain arrows", {}, async (page) => {
    await fold(page, "b2", "hidden-child");
    await focus(page, "b1");
    await shift(page, "Down", ["b1", "b2"]);
    const payload = await copy(page);
    assert.equal(payload.prevented, true);
    for (const content of ["Second block", "Folded branch", "Hidden child", "Hidden grandchild"]) {
      assert.ok(payload.text.includes(content), `copy includes ${content}`);
    }
    assert.ok(!payload.text.includes("After branch"));
    assert.match(payload.markdown, /\n\s+- Hidden child\n\s+- Hidden grandchild/);
    await page.locator(TOOLBAR).getByRole("button", { name: "Copy", exact: true }).click();
    await page.waitForFunction(() => window.__selectionState.clipboard.length > 0);
    assert.equal(await page.evaluate(() => window.__selectionState.clipboard.at(-1)), payload.markdown);
    await page.locator(TOOLBAR).getByRole("button", { name: "Clear", exact: true }).click();
    await selected(page, []);
    await focus(page, "b1");
    await shift(page, "Down", ["b1", "b2"]);
    const sidebar = await page.locator(".sidebar").isVisible();
    await page.keyboard.press("Escape");
    await selected(page, []);
    assert.equal(await page.locator(".sidebar").isVisible(), sidebar, "Escape only clears selection");
    await page.keyboard.press("Alt+z");
    await page.locator(".app-shell.zen").waitFor();
    await focus(page, "b1");
    await shift(page, "Down", ["b1", "b2"]);
    await page.keyboard.press("Escape");
    await selected(page, []);
    assert.equal(await page.locator(".app-shell.zen").count(), 1, "Escape does not close Zen");
    for (const direction of ["Down", "Up"]) {
      await focus(page, "b1");
      await shift(page, "Down", ["b1", "b2"]);
      await page.keyboard.press(`Arrow${direction}`);
      await selected(page, []);
      assert.ok(await page.evaluate(() => document.activeElement?.closest(".block-item") != null),
        "unmodified arrows return to an editor");
    }
  }],
  ["Delete, Backspace and cut include folded descendants; single undo and redo", { typedChild: true }, async (page) => {
    await fold(page, "b2", "hidden-child");
    const initial = await snapshot(page);
    for (const operation of ["Delete", "Backspace", "cut", "toolbar"]) {
      await focus(page, "b1");
      await shift(page, "Down", ["b1", "b2"]);
      if (operation === "cut") assert.match((await copy(page, "cut")).text, /Hidden grandchild/);
      else if (operation === "toolbar") await page.locator(TOOLBAR).getByRole("button", { name: "Delete", exact: true }).click();
      else await page.keyboard.press(operation);
      await page.waitForFunction(() => !window.__selectionState.blocks.some((block) => block.id === "b1"));
      const deleted = await snapshot(page);
      assert.deepEqual(deleted, initial.filter((block) =>
        !["b1", "b2", "hidden-child", "hidden-grandchild"].includes(block.id)));
      await page.keyboard.press("Control+z");
      await row(page, "b1").waitFor();
      assert.deepEqual(await snapshot(page), initial, `${operation}: one undo restores exact IDs, hierarchy, order and content`);
      await page.keyboard.press("Control+y");
      await row(page, "b1").waitFor({ state: "detached" });
      assert.deepEqual(await snapshot(page), deleted, `${operation}: one redo repeats the deletion`);
      await page.keyboard.press("Control+z");
      await row(page, "b1").waitFor();
    }
  }],
  ["search focus cannot copy or delete an old block selection", {}, async (page) => {
    await page.keyboard.press("Control+b");
    await focus(page, "b0");
    await shift(page, "Down", ["b0", "b1"]);
    const before = await snapshot(page);
    const search = page.locator(".sidebar input").first();
    await search.click();
    await search.fill("find this");
    await search.press("Control+a");
    const payload = await copy(page);
    assert.ok(!payload.text.includes("First visual line"), "copy from search must not capture old selected blocks");
    await search.press("Delete");
    await selected(page, []);
    assert.deepEqual(await snapshot(page), before);
    assert.equal(await search.inputValue(), "");
  }],
  ["slow source save blocks stale copy and deletion; typing cancels activation", {}, async (page) => {
    await focus(page, "b0");
    await page.evaluate(() => { window.__selectionState.holdUpdate = true; });
    await row(page, "b0").locator(".cm-content").fill("Unsaved source");
    await page.keyboard.press("End");
    await page.keyboard.press("Shift+ArrowDown");
    await page.waitForFunction(() => window.__selectionState.updateWaiters.length > 0);
    assert.ok(!(await copy(page)).text.includes("Second block"), "pending activation cannot copy stale block snapshots");
    await page.keyboard.press("Delete");
    assert.equal(await page.evaluate(() => window.__selectionState.calls.filter((call) => call.cmd === "delete_blocks").length), 0);
    await page.keyboard.type(" + keep typing");
    await release(page, "Update");
    assert.equal(await page.evaluate(() => window.__activeEditorView.state.doc.toString()), "Unsaved source + keep typing");
    await selected(page, []);
    await focus(page, "b1");
    await saved(page, "b0", "Unsaved source + keep typing");
    await selected(page, []);
  }],
  ["failed source save never activates destructive selection", {}, async (page) => {
    await focus(page, "b0");
    await page.evaluate(() => { window.__selectionState.failUpdate = true; });
    await row(page, "b0").locator(".cm-content").fill("Recoverable edited source");
    await page.keyboard.press("End");
    await page.keyboard.press("Shift+ArrowDown");
    await page.waitForFunction(() => window.__selectionState.calls.some((call) => call.cmd === "update_block"));
    await frames(page);
    await selected(page, []);
    assert.ok(!(await copy(page)).text.includes("Second block"));
    await page.keyboard.press("Delete");
    assert.equal(await page.evaluate(() => window.__selectionState.calls.filter((call) => call.cmd === "delete_blocks").length), 0);
    assert.equal(await page.evaluate(() => window.__activeEditorView.state.doc.toString()), "Recoverable edited source");
    await page.evaluate(() => { window.__selectionState.failUpdate = false; });
    await shift(page, "Down", ["b0", "b1"]);
    await saved(page, "b0", "Recoverable edited source");
  }],
  ["journal boundary clipboard and grouped per-page delete undo", { journal: true }, async (page) => {
    await fold(page, "day-0-b15", "journal-hidden");
    await focus(page, "day-0-b15");
    await row(page, "day-0-b15").locator(".cm-content").fill("Edited journal source");
    await page.keyboard.press("End");
    await shift(page, "Down", ["day-0-b15", "day-1-b0"]);
    await saved(page, "day-0-b15", "Edited journal source");
    const payload = await copy(page);
    for (const text of ["Edited journal source", "Hidden journal child", "Hidden journal grandchild", "Journal 1, block 0"]) {
      assert.ok(payload.text.includes(text));
    }
    assert.ok(!payload.text.includes("Journal 0, block 15"), "clipboard uses the saved source snapshot");
    const before = await snapshot(page, ["day-0", "day-1"]);
    await page.keyboard.press("Delete");
    await page.waitForFunction(() => !window.__selectionState.blocks.some((block) => block.id === "day-1-b0"));
    const after = await snapshot(page, ["day-0", "day-1"]);
    assert.deepEqual(after, before.filter((block) =>
      !["day-0-b15", "journal-hidden", "journal-grandchild", "day-1-b0"].includes(block.id)));
    const writes = await page.evaluate(() => window.__selectionState.calls.filter((call) => call.cmd === "delete_blocks"));
    assert.deepEqual(writes.map((call) => call.args.pageId).sort(), ["day-0", "day-1"]);
    await page.keyboard.press("Control+z");
    await page.waitForFunction(() => window.__selectionState.blocks.some((block) => block.id === "day-1-b0")
      && window.__selectionState.blocks.some((block) => block.id === "day-0-b15"));
    assert.deepEqual(await snapshot(page, ["day-0", "day-1"]), before, "one undo restores both pages, not just the last page");
    await page.keyboard.press("Control+y");
    await page.waitForFunction(() => !window.__selectionState.blocks.some((block) => block.id === "day-1-b0"));
    assert.deepEqual(await snapshot(page, ["day-0", "day-1"]), after);
  }],
  ["lazy journal mounting coalesces repeats and crosses an empty day", { journal: true, holdBlocks: "day-1" }, async (page) => {
    await fold(page, "day-0-b15", "journal-hidden");
    await focus(page, "day-0-b15");
    await page.keyboard.press("Shift+ArrowDown");
    await page.waitForFunction(() => typeof window.__releaseBlocks === "function");
    await page.keyboard.press("Shift+ArrowDown");
    await page.keyboard.press("Shift+ArrowDown");
    await release(page, "Blocks");
    await selected(page, ["day-0-b15", "day-1-b0"]);
    await shift(page, "Down", ["day-0-b15", "day-1-b0", "day-1-b1"]);
    await page.keyboard.press("Shift+ArrowDown");
    await page.waitForFunction(() => window.__selectionState.blocks.some((block) => block.page_id === "day-2"));
    const emptyId = await page.evaluate(() => window.__selectionState.blocks.find((block) => block.page_id === "day-2").id);
    await selected(page, ["day-0-b15", "day-1-b0", "day-1-b1", emptyId]);
    await shift(page, "Down", ["day-0-b15", "day-1-b0", "day-1-b1", emptyId, "day-3-b0"]);
    await shift(page, "Up", ["day-0-b15", "day-1-b0", "day-1-b1", emptyId]);
    assert.equal(await page.evaluate(() => window.__selectionState.calls.filter((call) =>
      call.cmd === "create_block" && call.args.pageId === "day-2").length), 1, "empty days create only one editable row");
  }],
  ...["typing", "click"].map((cancel) => [
    `pending lazy journal selection is canceled by ${cancel}`, { journal: true, holdBlocks: "day-1" }, async (page) => {
      await fold(page, "day-0-b15", "journal-hidden");
      await focus(page, "day-0-b15");
      await page.keyboard.press("Shift+ArrowDown");
      await page.waitForFunction(() => typeof window.__releaseBlocks === "function");
      if (cancel === "typing") await page.keyboard.type(" stays here");
      else await focus(page, "day-0-b14");
      await release(page, "Blocks");
      await row(page, "day-1-b0").waitFor();
      await selected(page, []);
      await focused(page, cancel === "typing" ? "day-0-b15" : "day-0-b14");
      assert.equal(await page.evaluate(() => window.__selectionState.calls.filter((call) => call.cmd === "delete_blocks").length), 0);
    },
  ]),
  ["journal selection awaits metadata pagination without skipping a date", { journal: true, holdListOffset: 10 }, async (page) => {
    await chooseDay(page, 9);
    await focus(page, "day-9-b1");
    await page.keyboard.press("Shift+ArrowDown");
    await page.waitForFunction(() => typeof window.__releaseList === "function");
    await page.keyboard.press("Shift+ArrowDown");
    await release(page, "List");
    await selected(page, ["day-9-b1", "day-10-b0"]);
    await shift(page, "Down", ["day-9-b1", "day-10-b0", "day-10-b1"]);
    await shift(page, "Down", ["day-9-b1", "day-10-b0", "day-10-b1", "day-11-b0"]);
    await page.keyboard.press("Escape");
    await selected(page, []);
    assert.equal(await page.evaluate(() => window.__selectionState.calls.filter((call) => call.cmd === "create_page").length), 0);
  }],
  ["continuous within-page ranges preserve source, editor identity and native undo", { unifiedPage: true }, async (page) => {
    await focusContinuous(page, "selection-page", "b0");
    const before = await snapshot(page);
    const original = await page.evaluate(() => {
      window.__selectionTestView = window.__activeEditorView;
      return window.__activeEditorView.state.doc.toString();
    });
    await page.keyboard.type(" edited");
    const edited = await page.evaluate(() => window.__activeEditorView.state.doc.toString());
    await page.evaluate(async () => {
      const view = window.__activeEditorView;
      const { parsePageSourceMap } = await import("/src/lib/pageSourceMap.ts");
      const block = parsePageSourceMap(view.state.doc.toString()).blocks.find((block) => block.id === "b0");
      view.dispatch({ selection: { anchor: block.contentFrom + 2 } });
    });
    await frames(page);
    for (let i = 0; i < 3; i++) await page.keyboard.press("Shift+ArrowDown");
    await frames(page);
    assert.equal(await page.locator(TOOLBAR).count(), 0, "within-page continuous selection remains native");
    assert.ok(await page.evaluate(async () => {
      const view = window.__activeEditorView;
      const { parsePageSourceMap } = await import("/src/lib/pageSourceMap.ts");
      const second = parsePageSourceMap(view.state.doc.toString()).blocks.find((block) => block.id === "b1");
      return view.state.selection.main.head >= second.contentFrom;
    }));
    assert.equal(await page.evaluate(() => window.__activeEditorView.state.doc.toString()), edited);
    assert.equal(await page.evaluate(() => window.__activeEditorView === window.__selectionTestView), true);
    await page.keyboard.press("ArrowLeft");
    await page.keyboard.press("Control+z");
    await page.waitForFunction((original) => window.__activeEditorView.state.doc.toString() === original, original);
    assert.equal(await page.locator(TOOLBAR).count(), 0);
    await page.getByRole("heading", { name: "Keyboard selection", exact: true }).click();
    await saved(page, "b0", "First visual line");
    assert.deepEqual(await snapshot(page), before);
    assert.equal(await page.evaluate(() => window.__selectionState.calls.filter((call) => call.cmd === "delete_blocks").length), 0);
  }],
  ...[true, false].map((bothContinuous) => [
    `${bothContinuous ? "continuous" : "mixed classic-continuous"} journal boundary copy/delete/undo and reverse`,
    { journal: true, flatJournal: true, unifiedDays: bothContinuous ? [0, 1] : [1] },
    async (page) => {
      if (bothContinuous) await focusContinuous(page, "day-0", "day-0-b15", "page-end");
      else await focus(page, "day-0-b15");
      const before = await snapshot(page, ["day-0", "day-1"]);
      await page.keyboard.press("Shift+ArrowDown");
      await selectionCount(page, 2);
      let payload = await copy(page);
      assert.ok(payload.text.includes("Journal 0, block 15"));
      assert.ok(payload.text.includes("Journal 1, block 0"));
      assert.ok(!payload.text.includes("Journal 1, block 1"));
      await page.keyboard.press("Delete");
      await page.waitForFunction(() => !window.__selectionState.blocks.some((block) => block.id === "day-1-b0"));
      const after = await snapshot(page, ["day-0", "day-1"]);
      assert.deepEqual(after, before.filter((block) => !["day-0-b15", "day-1-b0"].includes(block.id)));
      await expectCaret(page, "day-1-b1", "start", true);
      await page.keyboard.press("Control+z");
      await page.waitForFunction(() => window.__selectionState.blocks.some((block) => block.id === "day-1-b0")
        && window.__selectionState.blocks.some((block) => block.id === "day-0-b15"));
      assert.deepEqual(await snapshot(page, ["day-0", "day-1"]), before);
      await focusContinuous(page, "day-1", "day-1-b0", "page-start");
      const original = await nativeRange(page);
      await page.keyboard.press("Shift+ArrowUp");
      await selectionCount(page, 2);
      payload = await copy(page);
      assert.ok(payload.text.includes("Journal 0, block 15") && payload.text.includes("Journal 1, block 0"),
        `reverse clipboard must include both days: ${JSON.stringify(payload)}`);
      await page.keyboard.press("Shift+ArrowDown");
      await selected(page, []);
      assert.deepEqual(await nativeRange(page), original, "reverse shrink restores the continuous source range");
    },
  ]),
  ["empty journal day deletion restores its exact editable row in one undo", { journal: true }, async (page) => {
    await chooseDay(page, 1);
    await focus(page, "day-1-b1");
    await page.keyboard.press("Shift+ArrowDown");
    await selectionCount(page, 2);
    const originalEmpty = await page.evaluate(() => window.__selectionState.blocks.find((block) => block.page_id === "day-2").id);
    const before = await snapshot(page, ["day-1", "day-2"]);
    await page.keyboard.press("Delete");
    await page.waitForFunction((id) => !window.__selectionState.blocks.some((block) => block.id === id), originalEmpty);
    const after = await snapshot(page, ["day-1", "day-2"]);
    const placeholder = after.filter((block) => block.page_id === "day-2");
    assert.equal(placeholder.length, 1, "an emptied day retains one editable row");
    assert.equal(placeholder[0].content, "");
    assert.notEqual(placeholder[0].id, originalEmpty);
    await page.keyboard.press("Control+z");
    await page.waitForFunction(({ originalEmpty, placeholderId }) =>
      window.__selectionState.blocks.some((block) => block.id === originalEmpty)
      && !window.__selectionState.blocks.some((block) => block.id === placeholderId),
    { originalEmpty, placeholderId: placeholder[0].id });
    assert.deepEqual(await snapshot(page, ["day-1", "day-2"]), before, "undo restores original blank ID and removes the temporary placeholder");
    await page.keyboard.press("Control+y");
    await page.waitForFunction((id) => !window.__selectionState.blocks.some((block) => block.id === id), originalEmpty);
    assert.deepEqual(await snapshot(page, ["day-1", "day-2"]), after, "redo reuses the same stable placeholder ID");
  }],
  ...[false, true].map((book) => [
    `${book ? "progressive book" : "virtual standalone"} selection crosses the rendered window`,
    { book, blockCount: book ? 180 : 540 },
    async (page) => {
      const initial = await page.locator(".block-item").evaluateAll((rows) => rows.map((row) => row.dataset.blockId));
      assert.ok(initial.length < (book ? 180 : 540), "fixture starts with only a partial render");
      const lastRendered = Math.max(...initial.map((id) => Number(id.replace("large-", ""))));
      const target = Math.min(lastRendered + (book ? 10 : 100), book ? 179 : 539);
      await focus(page, "large-0");
      for (let index = 1; index <= target; index++) {
        await page.keyboard.press("Shift+ArrowDown");
        await selectionCount(page, index + 1);
      }
      const payload = await copy(page);
      const expected = Array.from({ length: target + 1 }, (_, i) => `Virtual block ${String(i).padStart(4, "0")}`);
      for (const content of expected) assert.ok(payload.text.includes(content), `copy includes ${content}`);
      assert.ok(!payload.text.includes(`Virtual block ${String(target + 1).padStart(4, "0")}`));
      assert.deepEqual(payload.markdown.trim().split("\n"), expected.map((content) => `- ${content}`));
      await row(page, `large-${target}`).waitFor();
      if (!book) assert.ok(await page.locator(".block-item").count() < 540, "keyboard extension retains virtualization");
      await page.keyboard.press("Shift+ArrowUp");
      await selectionCount(page, target);
      await page.keyboard.press("Escape");
      await selected(page, []);
      assert.equal(await page.evaluate(() => window.__selectionState.calls.filter((call) =>
        ["delete_blocks", "create_blocks", "update_block"].includes(call.cmd)).length), 0);
    },
  ]),
  ["native bridge selects text first, promotes and extends the active shared range", {}, async (page) => {
    await focus(page, "b0");
    await page.keyboard.press("Shift+Enter");
    await page.keyboard.type("Second visual line");
    await page.evaluate(() => window.__activeEditorView.dispatch({ selection: { anchor: 2 } }));
    await frames(page);
    await nativeArrow(page, "down");
    await selected(page, []);
    const original = await nativeRange(page);
    assert.ok(original.head > original.anchor, "injected native arrows use normal in-block selection first");
    await nativeArrow(page, "down");
    await selected(page, ["b0", "b1"]);
    await nativeArrow(page, "down");
    await selected(page, ["b0", "b1", "b2"]);
    await nativeArrow(page, "up");
    await selected(page, ["b0", "b1"]);
    await nativeArrow(page, "up");
    await selected(page, []);
    assert.deepEqual(await nativeRange(page), original);
    await saved(page, "b0", "First visual line\nSecond visual line");
  }],
  ["native bridge respects focused inputs rather than stale editor selection", {}, async (page) => {
    await page.keyboard.press("Control+b");
    await focus(page, "b0");
    await nativeArrow(page, "down");
    await selected(page, ["b0", "b1"]);
    const before = await snapshot(page);
    const search = page.locator(".sidebar input").first();
    await search.fill("independent input");
    for (const direction of ["down", "down", "up"]) await nativeArrow(page, direction);
    assert.equal(await search.evaluate((node) => node === document.activeElement), true);
    const remaining = await page.locator(".block-item.selected").evaluateAll((rows) => rows.map((row) => row.dataset.blockId));
    assert.ok(remaining.length === 0 || JSON.stringify(remaining) === JSON.stringify(["b0", "b1"]),
      "native input arrows must not extend an old block range");
    assert.ok(!(await copy(page)).text.includes("First visual line"));
    await search.press("Control+a");
    await search.press("Delete");
    assert.equal(await search.inputValue(), "");
    assert.deepEqual(await snapshot(page), before);
  }],
  ["native bridge continuous boundary extension and range restoration", {
    journal: true, flatJournal: true, unifiedDays: [0, 1],
  }, async (page) => {
    await focusContinuous(page, "day-0", "day-0-b15", "page-end");
    const original = await nativeRange(page);
    await nativeArrow(page, "down");
    await selectionCount(page, 2);
    await nativeArrow(page, "down");
    await selectionCount(page, 3);
    const payload = await copy(page);
    for (const content of ["Journal 0, block 15", "Journal 1, block 0", "Journal 1, block 1"]) {
      assert.ok(payload.text.includes(content), `native bridge clipboard includes ${content}`);
    }
    await nativeArrow(page, "up");
    await selectionCount(page, 2);
    await nativeArrow(page, "up");
    await selected(page, []);
    assert.deepEqual(await nativeRange(page), original);
  }],
  ["native bridge coalesces repeated arrows while the adjacent day loads", {
    journal: true, holdBlocks: "day-1",
  }, async (page) => {
    await fold(page, "day-0-b15", "journal-hidden");
    await focus(page, "day-0-b15");
    await nativeArrow(page, "down");
    await page.waitForFunction(() => typeof window.__releaseBlocks === "function");
    await nativeArrow(page, "down");
    await nativeArrow(page, "down");
    await release(page, "Blocks");
    await selected(page, ["day-0-b15", "day-1-b0"]);
    await nativeArrow(page, "down");
    await selected(page, ["day-0-b15", "day-1-b0", "day-1-b1"]);
  }],
  ...["extend", "shrink"].map((change) => [
    `async Ctrl+X does not delete after the selection ${change}s`, {}, async (page) => {
      await focus(page, "b0");
      await shift(page, "Down", ["b0", "b1"]);
      const before = await snapshot(page);
      const original = (await copy(page)).markdown;
      await page.evaluate(() => { window.__selectionState.holdClipboard = true; });
      await page.keyboard.press("Control+x");
      await page.waitForFunction(() => window.__selectionState.clipboardWaiters.length > 0);
      if (change === "extend") await shift(page, "Down", ["b0", "b1", "b2"]);
      else await shift(page, "Up", []);
      await page.evaluate(() => {
        window.__selectionState.holdClipboard = false;
        window.__selectionState.clipboardWaiters.splice(0).forEach((resolve) => resolve());
      });
      await page.waitForFunction(() => window.__selectionState.clipboard.length > 0);
      await frames(page);
      assert.equal(await page.evaluate(() => window.__selectionState.clipboard.at(-1)), original,
        "in-flight clipboard write retains its original payload");
      assert.equal(await page.evaluate(() => window.__selectionState.calls.filter((call) => call.cmd === "delete_blocks").length), 0,
        "changing the selection invalidates the pending destructive cut");
      assert.deepEqual(await snapshot(page), before);
    },
  ]),
  ["Ctrl+X preserves blocks when both clipboard API and fallback fail", {}, async (page) => {
    await focus(page, "b0");
    await shift(page, "Down", ["b0", "b1"]);
    const before = await snapshot(page);
    await page.evaluate(() => Object.assign(window.__selectionState, { failClipboard: true, fallbackClipboard: "fail" }));
    await page.keyboard.press("Control+x");
    await page.waitForFunction(() => window.__selectionState.clipboardFallbacks.length > 0);
    await frames(page);
    assert.equal(await page.evaluate(() => window.__selectionState.calls.filter((call) => call.cmd === "delete_blocks").length), 0);
    assert.deepEqual(await page.evaluate(() => window.__selectionState.clipboard), []);
    assert.deepEqual(await snapshot(page), before);
    await selected(page, ["b0", "b1"]);
  }],
  ["Ctrl+X fallback restores focus and deletes only after a successful copy", {}, async (page) => {
    await focus(page, "b0");
    await shift(page, "Down", ["b0", "b1"]);
    const before = await snapshot(page);
    const original = (await copy(page)).markdown;
    await page.evaluate(() => {
      Object.assign(window.__selectionState, { failClipboard: true, fallbackClipboard: "success" });
      window.__expectedClipboardFocus = document.activeElement;
    });
    await page.keyboard.press("Control+x");
    await page.waitForFunction(() => !window.__selectionState.blocks.some((block) => ["b0", "b1"].includes(block.id)));
    assert.equal(await page.evaluate(() => window.__selectionState.clipboard.at(-1)), original);
    assert.deepEqual(await page.evaluate(() => window.__selectionState.clipboardFallbacks), [{ text: original, succeeded: true }]);
    assert.ok(await page.evaluate(() => window.__selectionState.deleteFocus.every((focus) => focus.restored)),
      "fallback restores the original selection host before starting deletion");
    assert.equal(await page.evaluate(() => document.contains(window.__fallbackClipboardHost)), false, "fallback textarea is removed");
    assert.deepEqual(await snapshot(page), before.filter((block) => !["b0", "b1"].includes(block.id)));
    await page.keyboard.press("Control+z");
    await row(page, "b0").waitFor();
    assert.deepEqual(await snapshot(page), before);
  }],
  ["deletion gap: native single-line text deletion retains its editor and caret", {}, async (page) => {
    const original = "First visual line";
    for (const key of ["Delete", "Backspace"]) {
      await focus(page, "b0");
      await row(page, "b0").locator(".cm-content").fill(original);
      await page.evaluate(() => window.__activeEditorView.dispatch({ selection: { anchor: 2, head: 7 } }));
      await frames(page);
      await selected(page, []);
      await page.keyboard.press(key);
      await focused(page, "b0");
      assert.deepEqual(await nativeRange(page), { anchor: 2, head: 2 });
      await page.keyboard.type("X");
      assert.equal(await page.evaluate(() => window.__activeEditorView.state.doc.toString()),
        `${original.slice(0, 2)}X${original.slice(7)}`);
    }
    assert.equal(await page.evaluate(() => window.__selectionState.calls.filter((call) => call.cmd === "delete_blocks").length), 0);
  }],
  ...[
    ["Delete", "Down"], ["Backspace", "Down"], ["Control+x", "Down"], ["Delete", "Up"],
  ].map(([key, direction]) => [
    `deletion gap: classic ${direction} selection ${key} focuses the following block start`, {}, async (page) => {
      await fold(page, "b2", "hidden-child");
      await focus(page, direction === "Up" ? "b2" : "b1", direction === "Up" ? 0 : "end");
      await shift(page, direction, ["b1", "b2"]);
      await page.keyboard.press(key);
      await page.waitForFunction(() => !window.__selectionState.blocks.some((block) => block.id === "b1"));
      await expectCaret(page, "b3");
      await typeAfterDelete(page, "b3", "Next After branch", "Next ");
      assert.equal(await page.evaluate(() => window.__selectionState.blocks.some((block) =>
        ["b1", "b2", "hidden-child", "hidden-grandchild"].includes(block.id))), false);
    },
  ]),
  ["deletion gap: a trailing range focuses the previous visible block end", {}, async (page) => {
    await fold(page, "b2", "hidden-child");
    await focus(page, "b3");
    await shift(page, "Down", ["b3", "b4"]);
    await page.keyboard.press("Delete");
    await expectCaret(page, "b2", "end");
    await typeAfterDelete(page, "b2", "Folded branch tail", " tail");
    assert.equal(await row(page, "hidden-child").count(), 0, "deletion does not unfold the preceding subtree");
  }],
  ["deletion gap: deleting an entire page focuses its tracked blank", { blockCount: 2 }, async (page) => {
    await focus(page, "large-0");
    await shift(page, "Down", ["large-0", "large-1"]);
    await page.keyboard.press("Delete");
    await page.waitForFunction(() => {
      const blocks = window.__selectionState.blocks.filter((block) => block.page_id === "selection-page");
      return blocks.length === 1 && !["large-0", "large-1"].includes(blocks[0].id);
    });
    const blank = await page.evaluate(() => window.__selectionState.blocks.find((block) => block.page_id === "selection-page"));
    assert.equal(blank.content, "");
    assert.equal(blank.block_type, "Text");
    await expectCaret(page, blank.id);
    await typeAfterDelete(page, blank.id, "A fresh paragraph", "A fresh paragraph");
  }],
  ...["classic", "mixed", "continuous"].map((mode) => [
    `deletion gap: ${mode} cross-day selection focuses the exact next block content offset`,
    { journal: true, flatJournal: true, unifiedDays: mode === "continuous" ? [0, 1] : mode === "mixed" ? [1] : [] },
    async (page) => {
      if (mode === "continuous") await focusContinuous(page, "day-0", "day-0-b15", "page-end");
      else await focus(page, "day-0-b15");
      await page.keyboard.press("Shift+ArrowDown");
      await selectionCount(page, 2);
      await page.keyboard.press("Delete");
      await page.waitForFunction(() => !window.__selectionState.blocks.some((block) => block.id === "day-1-b0"));
      await expectCaret(page, "day-1-b1", "start", mode !== "classic");
      await typeAfterDelete(page, "day-1-b1", "Next Journal 1, block 1", "Next ", mode !== "classic");
    },
  ]),
  ...[false, true].map((book) => [
    `deletion gap: ${book ? "progressive book" : "virtual page"} focuses a next block outside the viewport`,
    { book, blockCount: book ? 180 : 540 },
    async (page) => {
      const initial = await page.locator(".block-item").evaluateAll((rows) =>
        rows.map((row) => Number(row.dataset.blockId.replace("large-", ""))));
      const last = Math.max(...initial) + (book ? 12 : 65);
      const target = `large-${last + 1}`;
      await focus(page, "large-0");
      for (let index = 1; index <= last; index++) {
        await page.keyboard.press("Shift+ArrowDown");
        await selectionCount(page, index + 1);
      }
      await page.locator(".main-content").evaluate((node) => { node.scrollTop = 0; });
      await frames(page);
      if (!book) await row(page, target).waitFor({ state: "detached" });
      else if (await row(page, target).count()) {
        assert.ok(await row(page, target).evaluate((node) => node.getBoundingClientRect().top >= window.innerHeight));
      }
      await page.keyboard.press("Delete");
      await expectCaret(page, target);
      const content = `Virtual block ${String(last + 1).padStart(4, "0")}`;
      await typeAfterDelete(page, target, `Next ${content}`, "Next ");
    },
  ]),
  ...["delete", "reload"].flatMap((phase) => ["click", "typing", "dialog"].map((interaction) => [
    `deletion gap: pending ${phase} does not steal focus after external ${interaction}`, {}, async (page) => {
      await page.keyboard.press("Control+b");
      await fold(page, "b2", "hidden-child");
      await focus(page, "b1");
      await shift(page, "Down", ["b1", "b2"]);
      await page.evaluate((phase) => {
        if (phase === "delete") window.__selectionState.holdDelete = true;
        else window.__selectionState.holdReload = true;
      }, phase);
      await page.keyboard.press("Delete");
      await page.waitForFunction((phase) => phase === "delete"
        ? window.__selectionState.deleteWaiters.length > 0 : typeof window.__releaseReload === "function", phase);
      const search = page.locator(".sidebar input").first();
      if (interaction === "dialog") {
        await page.keyboard.press("Control+g");
        await page.getByRole("dialog", { name: "Choose date" }).waitFor();
      } else if (interaction === "typing") {
        await search.focus();
        await page.keyboard.type("Keep this query");
      } else await search.click();
      await page.evaluate(() => { window.__pendingDeleteExternalFocus = document.activeElement; });
      await page.evaluate((phase) => {
        if (phase === "delete") {
          window.__selectionState.holdDelete = false;
          window.__selectionState.deleteWaiters.splice(0).forEach((resolve) => resolve());
        } else {
          window.__selectionState.holdReload = false;
          window.__releaseReload();
        }
      }, phase);
      await page.waitForFunction(() => !window.__selectionState.blocks.some((block) => block.id === "b1"));
      await page.evaluate(() => new Promise((resolve, reject) => {
        let remaining = 10;
        const check = () => {
          if (document.activeElement !== window.__pendingDeleteExternalFocus) {
            reject(new Error(`Pending deletion stole focus into ${document.activeElement?.className}`));
          } else if (--remaining) requestAnimationFrame(check);
          else resolve();
        };
        requestAnimationFrame(check);
      }));
      if (interaction === "typing") assert.equal(await search.inputValue(), "Keep this query");
      if (interaction === "dialog") assert.equal(await page.getByRole("dialog", { name: "Choose date" }).count(), 1);
    },
  ])),
  ...[false, true].map((typeAfterFocus) => [
    `deletion gap: undo respects saved survivor history${typeAfterFocus ? " after undoing new typing first" : ""}`, {},
    async (page) => {
      await fold(page, "b2", "hidden-child");
      await focus(page, "b3");
      await page.keyboard.type(" already edited");
      await focus(page, "b1");
      const survivor = "After branch already edited";
      await saved(page, "b3", survivor);
      const before = await snapshot(page);
      await shift(page, "Down", ["b1", "b2"]);
      await page.keyboard.press("Delete");
      await expectCaret(page, "b3");
      if (typeAfterFocus) {
        await page.keyboard.type("New ");
        assert.equal(await page.evaluate(() => window.__activeEditorView.state.doc.toString()), `New ${survivor}`);
        await page.keyboard.press("Control+z");
        await page.waitForFunction((survivor) => window.__activeEditorView?.state.doc.toString() === survivor, survivor);
        assert.equal(await page.evaluate(() => window.__selectionState.blocks.some((block) => block.id === "b1")), false,
          "first undo removes only newly typed text, not the preceding grouped deletion");
      }
      await page.keyboard.press("Control+z");
      await page.waitForFunction(() => ["b1", "b2", "hidden-child", "hidden-grandchild"].every((id) =>
        window.__selectionState.blocks.some((block) => block.id === id)));
      assert.deepEqual(await snapshot(page), before, "app undo restores the deleted range without undoing older survivor edits");
      if (typeAfterFocus) {
        await page.keyboard.press("Control+y");
        await page.waitForFunction(() => !window.__selectionState.blocks.some((block) => block.id === "b1"));
        await page.waitForFunction((survivor) => window.__activeEditorView?.hasFocus
          && window.__activeEditorView.state.doc.toString() === survivor, survivor);
        await page.keyboard.press("Control+y");
        await page.waitForFunction((expected) => window.__activeEditorView?.state.doc.toString() === expected, `New ${survivor}`);
        assert.equal(await page.evaluate(() => window.__selectionState.blocks.some((block) => block.id === "b1")), false,
          "redo reapplies the grouped deletion before redoing newer local typing");
      }
    },
  ]),
];

module.exports = { openEditor, focus, frames, row, selected, shift, snapshot, nativeRange, copy, cases, focusContinuous, selectionCount, nativeArrow, expectCaret };

if (require.main === module) (async () => {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  let failures = 0;
  try {
    for (const [name, options, run] of cases) {
      if (process.env.UI_TEST_CASE && !process.env.UI_TEST_CASE.split("|").some((filter) => name.includes(filter))) continue;
      let fixture;
      try {
        fixture = await openEditor(browser, options);
        await run(fixture.page);
        assert.deepEqual(fixture.errors, [], "no uncaught browser errors");
        console.log(`PASS ${name}`);
      } catch (error) {
        failures++;
        console.error(`FAIL ${name}\n${error.stack ?? error}`);
        if (fixture) console.error("Selection state:", JSON.stringify(await fixture.page.evaluate(() => ({
          selected: [...document.querySelectorAll(".block-item.selected")].map((node) => node.dataset.blockId),
          toolbar: document.querySelector(".keyboard-selection-toolbar")?.textContent,
          activeTag: document.activeElement?.tagName,
          activeClass: document.activeElement?.className,
          activeBlock: document.activeElement?.closest(".block-item")?.dataset.blockId,
          editor: window.__activeEditorView?.state.doc.toString().slice(0, 240),
          docLength: window.__activeEditorView?.state.doc.length,
          range: window.__activeEditorView?.state.selection.main.toJSON(),
          recentWrites: window.__selectionState.calls.filter((call) =>
            ["update_block", "update_page_source", "delete_blocks", "create_blocks"].includes(call.cmd)).slice(-5),
        })), null, 2));
      } finally {
        await fixture?.page.close();
      }
    }
  } finally {
    await browser.close();
  }
  if (failures) process.exitCode = 1;
})().catch((error) => { console.error(error); process.exitCode = 1; });

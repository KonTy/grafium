// Real AI panel, JournalView, and CodeMirror editors; only synthetic IPC is replaced.
const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { mkdir } = require("node:fs/promises");
const path = require("node:path");
const { openEditor, focus, focusContinuous, frames, row } = require("./keyboardSelection.ui.cjs");

const PANEL = ".writing-panel";
const rewritten = (content) => `Naturally revised: ${content}`;
const button = (page, name) => page.locator(PANEL).getByRole("button", { name, exact: true });
const calls = (page, cmd) => page.evaluate((cmd) =>
  window.__selectionState.calls.filter((call) => call.cmd === cmd), cmd);
const undo = (page) => page.evaluate(() => structuredClone(window.__undoStack ?? []));
const allNotes = (page) => page.evaluate(() => structuredClone(window.__selectionState.blocks));

async function installWritingFixture(page, options) {
  await page.addInitScript((options) => {
    function install(internals) {
      const state = window.__selectionState;
      const original = internals.invoke;
      const callbacks = new Map();
      const listeners = new Map();
      const transformCallback = internals.transformCallback.bind(internals);
      const unregisterCallback = internals.unregisterCallback.bind(internals);
      internals.transformCallback = (callback, ...args) => {
        const id = transformCallback(callback, ...args);
        callbacks.set(id, callback);
        return id;
      };
      internals.unregisterCallback = (id) => {
        callbacks.delete(id);
        unregisterCallback(id);
      };
      window.__emitWritingProgress = (operationId, message) => {
        for (const [id, listener] of listeners) {
          if (listener.event === "ai-writing-progress") {
            callbacks.get(listener.handler)?.({
              event: listener.event, id, payload: { operationId, message },
            });
          }
        }
      };
      window.__writingProgressListenerCount = () => [...listeners.values()]
        .filter(({ event }) => event === "ai-writing-progress").length;
      const writing = window.__writingState = {
        graphPath: "/synthetic/keyboard-selection", hold: false, pending: [], mode: "valid", applied: [],
        connection: options.connection, holdApply: false, applyWaiters: [],
      };
      for (const block of state.blocks) {
        block.content = `I wrote this synthetic note about ${block.id}. It describes an ordinary afternoon in the garden.`;
        block.properties = { fixture: block.id, tags: ["synthetic", "keep"] };
      }
      const base = state.blocks.find((block) => block.id === "b4");
      state.blocks.push(
        { ...structuredClone(base), id: "code", order_index: 5, content: "```js\nconst preserved = true;\n```" },
        { ...structuredClone(base), id: "query", order_index: 6, block_type: "Query", content: "{{query SELECT 1}}" },
        { ...structuredClone(base), id: "audio", order_index: 7, block_type: "Audio", content: "Synthetic audio caption" },
        { ...structuredClone(base), id: "empty", order_index: 8, content: "" },
        { ...structuredClone(base), id: "day-1-child", page_id: "day-1", parent_id: "day-1-b0", order_index: 0,
          content: "This nested synthetic paragraph must follow the selected journal day." },
      );
      const canonical = (blocks) => JSON.stringify([...blocks].sort((a, b) => a.id.localeCompare(b.id))
        .map(({ id, page_id, parent_id, order_index, content, block_type, properties, created_at, updated_at }) =>
          ({ id, page_id, parent_id, order_index, content, block_type, properties, created_at, updated_at })));
      internals.invoke = async (cmd, args = {}) => {
        const custom = ["get_graph_info", "get_app_theme", "ai_health_check", "ai_index_status",
          "ai_analyze_writing", "ai_rewrite_writing", "ai_cancel_writing", "apply_writing_changes", "ai_ask"].includes(cmd);
        if (custom) state.calls.push({ cmd, args: structuredClone(args) });
        switch (cmd) {
          case "plugin:event|listen": {
            const id = await original(cmd, args);
            listeners.set(id, args);
            return id;
          }
          case "plugin:event|unlisten":
            listeners.delete(args.eventId);
            return original(cmd, args);
          case "get_graph_info": return { name: "Synthetic writing fixture", path: writing.graphPath };
          case "get_app_theme": return options.theme ?? "github";
          case "ai_health_check": return {
            enabled: writing.connection !== "disabled", llm_available: !["disabled", "embedding-only"].includes(writing.connection),
            embedder_available: writing.connection !== "disabled", vector_store_available: true,
            vector_count: 0, mode: "synthetic-current-model",
          };
          case "ai_index_status": return {
            indexed_chunks: 0, total_blocks: state.blocks.length, pending_pages: 0,
            embedder_ready: true, llm_ready: true, accelerator: null,
          };
          case "ai_get_config": return { enabled: true, mode: "local", local: { provider: "openai_compatible" } };
          case "ai_ask": return { answer: "A merged draft ready for manual review.", sources: [] };
          case "assistant_chat":
            for (const [id, listener] of listeners) {
              if (listener.event === "ai://chat_stream") callbacks.get(listener.handler)?.({
                event: listener.event, id,
                payload: { request_id: args.requestId, delta: "Additional context worth considering.", done: true },
              });
            }
            return;
          case "research_get_config": throw new Error("unknown command research_get_config");
          case "get_smplos_theme": return null;
          case "get_app_version": return "0.0.123";
          case "ai_analyze_writing":
          case "ai_rewrite_writing": {
            if (writing.hold) await new Promise((resolve) => writing.pending.push(resolve));
            if (writing.mode === "error") throw new Error("Synthetic model connection failed");
            if (cmd === "ai_analyze_writing") return {
              score: writing.mode === "inconclusive" ? null : 67,
              summary: "Some repeated sentence patterns; this is a subjective style assessment.",
              findings: options.longFindings ? Array.from({ length: 12 }, (_, index) => ({
                label: `Pattern ${index + 1}`,
                detail: "This synthetic explanation describes a repeated writing pattern and its surrounding context. ".repeat(6),
                quote: args.blocks[0].content.split(" ").slice(index, index + 6).join(" "),
              })) : [{ label: "Repeated rhythm", detail: "Several sentences follow the same cadence.",
                quote: args.blocks[0].content }],
              wordCount: 120, analyzedWordCount: 90, chunksAnalyzed: 2, chunksTotal: 3,
            };
            const result = args.blocks.map(({ id, content }) => ({ id, content: `Naturally revised: ${content}` }));
            if (writing.mode === "invalid-id") result[0].id = "unknown-block";
            if (writing.mode === "empty") result[0].content = " ";
            if (writing.mode === "incomplete") result.pop();
            if (writing.mode === "no-op") return { blocks: structuredClone(args.blocks), skipped: [] };
            const skipped = [];
            if (["some-skipped", "all-skipped"].includes(writing.mode)) {
              const retained = writing.mode === "all-skipped" ? args.blocks : args.blocks.slice(0, 1);
              for (const [index, block] of retained.entries()) {
                result[index] = structuredClone(block);
                skipped.push({
                  blockId: block.id, blockOrdinal: index + 1, lineOrdinal: 1,
                  reason: "The model did not provide a safe wording edit after two attempts.",
                });
              }
            }
            return { blocks: result, skipped };
          }
          case "ai_cancel_writing": return;
          case "apply_writing_changes": {
            if (writing.holdApply) await new Promise((resolve) => writing.applyWaiters.push(resolve));
            if (args.graphPath !== writing.graphPath) throw new Error("The graph changed. Nothing was changed.");
            if (!state.pages.some((note) => note.id === args.pageId)) throw new Error("The page no longer exists.");
            const blocks = state.blocks.filter((block) => block.page_id === args.pageId);
            if (args.expectedBlocks && canonical(blocks) !== canonical(args.expectedBlocks)) {
              throw new Error("The page changed during generation. Nothing was changed.");
            }
            const ids = new Set();
            for (const change of args.changes) {
              const block = blocks.find((block) => block.id === change.blockId);
              if (ids.has(change.blockId) || !block || block.content !== change.beforeContent
                || typeof change.afterContent !== "string" || !change.afterContent.trim()) {
                throw new Error("Writing compare-and-swap failed. Nothing was changed.");
              }
              ids.add(change.blockId);
            }
            // Validate the entire batch before changing any content.
            for (const change of args.changes) blocks.find((block) => block.id === change.blockId).content = change.afterContent;
            writing.applied.push(structuredClone(args));
            return;
          }
          case "update_page_source": {
            const previous = new Map(state.blocks.map((block) => [block.id, structuredClone(block)]));
            await original(cmd, args);
            // The shared fixture parses structure; emulate native metadata preservation too.
            for (const block of state.blocks) {
              const old = previous.get(block.id);
              if (old) Object.assign(block, { properties: old.properties, block_type: old.block_type,
                created_at: old.created_at, updated_at: old.updated_at });
            }
            return;
          }
          default: return original(cmd, args);
        }
      };
      return internals;
    }
    // Playwright does not guarantee ordering among separately registered init scripts.
    let internals = window.__TAURI_INTERNALS__;
    if (internals) internals = install(internals);
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true,
      get: () => internals, set: (value) => { internals = install(value); } });
  }, options);
}

async function openWriting(browser, options = {}) {
  const fixture = await openEditor(browser, {
    ...options, beforeNavigate: (page) => installWritingFixture(page, options),
  });
  const page = fixture.page;
  await focusTarget(page, options.journal ? "day-0-b0" : "b0", options);
  await page.evaluate(() => window.dispatchEvent(new CustomEvent("toggle-reference-panel")));
  await page.locator(".reference-panel summary").filter({ hasText: "Page / selection tools" }).click();
  await page.locator(".reference-panel summary").filter({ hasText: /^Writing assistance$/ }).click();
  await page.locator(PANEL).waitFor();
  await page.waitForFunction(() => !document.querySelector(".writing-panel")?.textContent.includes("Checking AI connection"));
  return fixture;
}

async function focusTarget(page, id, options = {}) {
  const pageId = id.startsWith("day-") ? id.split("-").slice(0, 2).join("-") : "selection-page";
  if (options.unifiedPage || options.unifiedDays?.includes(Number(pageId.slice(4)))) {
    await focusContinuous(page, pageId, id);
  } else await focus(page, id);
}
async function finish(page) {
  await page.waitForFunction(() => !document.querySelector(".writing-panel .progress"));
  await frames(page);
}
async function releaseModel(page) {
  await page.evaluate(() => {
    window.__writingState.hold = false;
    window.__writingState.pending.splice(0).forEach((resolve) => resolve());
  });
  await finish(page);
}
async function startHeld(page, kind = "Rewrite naturally") {
  await page.evaluate(() => { window.__writingState.hold = true; });
  await button(page, kind).click();
  await page.waitForFunction(() => window.__writingState.pending.length > 0);
}
async function waitContent(page, id, content) {
  await page.waitForFunction(({ id, content }) =>
    window.__selectionState.blocks.find((block) => block.id === id)?.content === content, { id, content });
}
async function editorText(page, continuous = false) {
  return page.evaluate((continuous) => (continuous ? window.__unifiedPageEditorView : window.__activeEditorView)
    ?.state.doc.toString(), continuous);
}
async function editedBlockText(page, id, continuous) {
  return page.evaluate(async ({ id, continuous }) => {
    const view = continuous ? window.__unifiedPageEditorView : window.__activeEditorView;
    const content = view?.state.doc.toString();
    if (!continuous || content == null) return content;
    const { parsePageSourceMap } = await import("/src/lib/pageSourceMap.ts");
    return parsePageSourceMap(content).blocks.find((block) => block.id === id)?.content;
  }, { id, continuous });
}
async function expectNoRewrite(page, before, actions) {
  assert.deepEqual(await allNotes(page), before, "no persisted block or metadata changed");
  assert.deepEqual(await undo(page), actions, "no undo action created");
  assert.deepEqual(await page.evaluate(() => window.__writingState.applied), [], "no atomic rewrite committed");
}

const cases = [];
async function askForToolAnswer(page) {
  const chat = page.locator(".reference-panel");
  await chat.getByRole("textbox", { name: "Message", exact: true }).fill("Explain this source.");
  await chat.getByRole("button", { name: "Send", exact: true }).click();
  await chat.getByText("Additional context worth considering.", { exact: true }).waitFor();
}

cases.push(["explicit answer merge stays editable and replaces only the captured block with one undo", {}, async (page) => {
  await askForToolAnswer(page);
  const source = await allNotes(page);
  const tools = page.locator(".page-tools");
  await tools.getByRole("button", { name: "Merge answer with block", exact: true }).click();
  const preview = tools.getByRole("region", { name: "Merged block draft", exact: true });
  const draft = preview.getByRole("textbox", { name: "Replacement Markdown", exact: true });
  await draft.waitFor();
  assert.deepEqual(await allNotes(page), source, "drafting must not change notes");
  assert.deepEqual(await undo(page), []);
  await draft.fill("Reviewed and edited before keeping this merged text.");
  await preview.getByRole("button", { name: "Replace captured block", exact: true }).click();
  await waitContent(page, "b0", "Reviewed and edited before keeping this merged text.");
  assert.equal((await undo(page)).length, 1);
  const expected = source.map((block) => block.id === "b0"
    ? { ...block, content: "Reviewed and edited before keeping this merged text." } : block);
  assert.deepEqual(await allNotes(page), expected);
  await focus(page, "b0");
  await page.keyboard.press("Control+z");
  await waitContent(page, "b0", source.find(({ id }) => id === "b0").content);
  assert.deepEqual(await allNotes(page), source);
}]);

cases.push(["explicit answer merge refuses a stale page and preserves the reviewed draft", {}, async (page) => {
  await askForToolAnswer(page);
  const tools = page.locator(".page-tools");
  await tools.getByRole("button", { name: "Merge answer with block", exact: true }).click();
  const preview = tools.getByRole("region", { name: "Merged block draft", exact: true });
  const draft = preview.getByRole("textbox", { name: "Replacement Markdown", exact: true });
  await draft.waitFor();
  await draft.fill("Keep this reviewed draft after a conflict.");
  await page.evaluate(() => { window.__selectionState.blocks.find(({ id }) => id === "b1").content += " External edit."; });
  const changed = await allNotes(page);
  await preview.getByRole("button", { name: "Replace captured block", exact: true }).click();
  await tools.getByRole("alert").filter({ hasText: "The page changed during generation." }).waitFor();
  assert.deepEqual(await allNotes(page), changed);
  assert.deepEqual(await undo(page), []);
  assert.equal(await draft.inputValue(), "Keep this reviewed draft after a conflict.");
}]);

cases.push(["assistant accordions have visible enclosing boundaries in every theme", {}, async (page) => {
  await button(page, "Analyze AI style").click();
  await finish(page);
  await page.mouse.move(0, 0);
  const measurements = await page.evaluate(async () => {
    const { themes, applyTheme } = await import("/src/lib/themes.ts");
    const { contrastRatio } = await import("/src/lib/contrast.ts");
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = 1;
    const ctx = canvas.getContext("2d");
    const hex = (color) => {
      ctx.clearRect(0, 0, 1, 1);
      ctx.fillStyle = color;
      ctx.fillRect(0, 0, 1, 1);
      return "#" + [...ctx.getImageData(0, 0, 1, 1).data].slice(0, 3)
        .map((channel) => channel.toString(16).padStart(2, "0")).join("");
    };
    const results = [];
    for (const theme of themes) {
      applyTheme(theme.colors);
      for (const details of document.querySelectorAll(".reference-panel details")) {
        const wasOpen = details.open;
        const summary = details.querySelector(":scope > summary");
        for (const open of [false, true]) {
          details.open = open;
          const frame = getComputedStyle(details);
          const header = getComputedStyle(summary);
          const marker = getComputedStyle(summary, "::marker");
          const body = details.querySelector(":scope > .assistant-disclosure-body");
          const rect = details.getBoundingClientRect();
          const headerRect = summary.getBoundingClientRect();
          results.push({
            label: `${theme.id}: ${summary.textContent} (${open ? "expanded" : "collapsed"})`,
            sharedStyle: details.classList.contains("assistant-disclosure"),
            textContrast: contrastRatio(hex(header.color), hex(header.backgroundColor)),
            markerContrast: contrastRatio(hex(marker.color), hex(header.backgroundColor)),
            borderContrast: contrastRatio(hex(frame.borderBottomColor), hex(frame.backgroundColor)),
            headerBorderContrast: contrastRatio(hex(frame.borderTopColor), hex(header.backgroundColor)),
            frameWidths: [frame.borderTopWidth, frame.borderRightWidth, frame.borderBottomWidth, frame.borderLeftWidth].map(parseFloat),
            headerHeight: headerRect.height,
            fontWeight: Number(header.fontWeight),
            separated: !open || parseFloat(header.borderBottomWidth) >= 1,
            contained: !open || !!body && body.getBoundingClientRect().bottom <= rect.bottom
              && body.scrollWidth <= body.clientWidth + 1 && parseFloat(getComputedStyle(body).paddingBottom) >= 10,
          });
        }
        details.open = wasOpen;
      }
    }
    applyTheme(themes.find(({ id }) => id === "github").colors);
    return { results, themeCount: themes.length };
  });
  assert.equal(measurements.results.length, measurements.themeCount * 4 * 2, "all four disclosures in both states");
  for (const metrics of measurements.results) {
    assert.ok(metrics.sharedStyle && metrics.separated && metrics.contained, JSON.stringify(metrics));
    assert.ok(metrics.textContrast >= 4.5 && metrics.markerContrast >= 3
      && metrics.borderContrast >= 3 && metrics.headerBorderContrast >= 3, JSON.stringify(metrics));
    assert.ok(metrics.frameWidths.every((width) => width >= 2), metrics.label);
    assert.ok(metrics.headerHeight >= 40 && metrics.fontWeight >= 600, metrics.label);
  }

  const writingHeader = page.locator(".writing-tools > summary");
  await writingHeader.focus();
  await page.keyboard.press("Space");
  assert.equal(await page.locator(PANEL).isVisible(), false, "Space collapses writing assistance");
  assert.equal(await writingHeader.evaluate((node) => {
    const style = getComputedStyle(node);
    return node.matches(":focus-visible") && parseFloat(style.outlineWidth) >= 2;
  }), true, "keyboard focus is clearly visible");
  await page.keyboard.press("Enter");
  assert.equal(await page.locator(`${PANEL} .score-number`).textContent(), "67", "reopening retains analysis");
  assert.equal((await calls(page, "ai_analyze_writing")).length, 1, "disclosures do not repeat analysis");
  assert.equal((await calls(page, "apply_writing_changes")).length, 0, "presentation never edits notes");

  if (process.env.UI_TEST_SCREENSHOTS) {
    await mkdir(process.env.UI_TEST_SCREENSHOTS, { recursive: true });
    for (const themeId of ["github", "github-dark", "oled"]) {
      await page.evaluate(async (themeId) => {
        const { themes, applyTheme } = await import("/src/lib/themes.ts");
        applyTheme(themes.find(({ id }) => id === themeId).colors);
      }, themeId);
      await page.locator(".page-tools > summary").scrollIntoViewIfNeeded();
      await page.screenshot({ path: path.join(process.env.UI_TEST_SCREENSHOTS, `accordions-${themeId}.png`) });
    }
  }
}]);

cases.push(["writing labels include scientific text without calling it prose", {}, async (page) => {
  const before = await allNotes(page);
  await button(page, "Page").click();
  assert.match(await page.locator(`${PANEL} .target-hint`).innerText(), /Text on Keyboard selection/);
  assert.match(await page.locator(PANEL).innerText(), /protected numbers, citations, and formatting/);
  assert.match(await page.locator(PANEL).innerText(), /Review meaning and scientific details/);
  assert.doesNotMatch(await page.locator(PANEL).innerText(), /\bprose\b/i);
  await startHeld(page);
  assert.match(await page.locator(`${PANEL} .progress`).innerText(), /Rewriting \d+ text blocks/);
  assert.doesNotMatch(await page.locator(PANEL).innerText(), /\bprose\b/i);
  await button(page, "Cancel").click();
  await releaseModel(page);
  assert.deepEqual(await allNotes(page), before);
  assert.equal((await calls(page, "apply_writing_changes")).length, 0);
}]);
cases.push(["wording progress follows only the active rewrite and unregisters on completion", {}, async (page) => {
  await button(page, "Page").click();
  await startHeld(page);
  const operationId = (await calls(page, "ai_rewrite_writing")).at(-1).args.operationId;
  const initial = await page.locator(`${PANEL} .progress span`).innerText();
  assert.equal(await page.evaluate(() => window.__writingProgressListenerCount()), 1);
  await page.evaluate(() => window.__emitWritingProgress("another-operation", "Unrelated progress"));
  await frames(page);
  assert.equal(await page.locator(`${PANEL} .progress span`).innerText(), initial);
  const message = "Rewriting text block 2 of 5, line 1...";
  await page.evaluate(({ operationId, message }) => window.__emitWritingProgress(operationId, message),
    { operationId, message });
  await page.waitForFunction((message) =>
    document.querySelector(".writing-panel .progress span")?.textContent === message, message);
  await button(page, "Cancel").click();
  await releaseModel(page);
  assert.equal(await page.evaluate(() => window.__writingProgressListenerCount()), 0);
  assert.equal((await calls(page, "apply_writing_changes")).length, 0);
  await startHeld(page);
  const next = await page.locator(`${PANEL} .progress span`).innerText();
  await page.evaluate((operationId) => window.__emitWritingProgress(operationId, "Stale progress"), operationId);
  await frames(page);
  assert.equal(await page.locator(`${PANEL} .progress span`).innerText(), next);
  await button(page, "Cancel").click();
  await releaseModel(page);
}]);
for (const viewport of [{ width: 1280, height: 600 }, { width: 390, height: 720 }]) cases.push([
  `pattern explanations scroll fully with the wheel at ${viewport.width}px`,
  { longFindings: true }, async (page) => {
    await page.setViewportSize(viewport);
    await button(page, "Analyze AI style").click();
    await finish(page);
    const foundScroller = await page.locator(PANEL).evaluate((panel) => {
      for (let node = panel.parentElement; node; node = node.parentElement) {
        if (getComputedStyle(node).overflowY === "auto" && node.scrollHeight > node.clientHeight) {
          node.setAttribute("data-writing-test-scroller", "");
          return true;
        }
      }
      return false;
    });
    assert.equal(foundScroller, true, "writing findings have a scrollable container");
    const scroller = page.locator("[data-writing-test-scroller]");
    const bounds = await scroller.boundingBox();
    assert.ok(bounds && bounds.height > 100);
    assert.equal(await scroller.evaluate((node) => getComputedStyle(node).overflowY), "auto");
    assert.ok(await scroller.evaluate((node) => node.scrollHeight > node.clientHeight * 2),
      "the fixture must overflow the visible panel");
    const mainScroll = await page.locator(".main-content").evaluate((node) => node.scrollTop);
    await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2);
    await page.mouse.wheel(0, 50000);
    await page.waitForFunction(() => {
      const node = document.querySelector("[data-writing-test-scroller]");
      return node.scrollTop >= node.scrollHeight - node.clientHeight - 2;
    });
    const lastBefore = await page.locator(`${PANEL} .finding:last-child blockquote`).boundingBox();
    assert.ok(lastBefore);
    if (lastBefore.y < bounds.y) {
      await page.mouse.wheel(0, lastBefore.y - bounds.y - 8);
      await page.waitForFunction(() => {
        const container = document.querySelector("[data-writing-test-scroller]").getBoundingClientRect();
        const quote = document.querySelector(".writing-panel .finding:last-child blockquote").getBoundingClientRect();
        return quote.top >= container.top - 1 && quote.bottom <= container.bottom + 1;
      });
    }
    const last = await page.locator(`${PANEL} .finding:last-child blockquote`).boundingBox();
    assert.ok(last && last.y >= bounds.y - 1 && last.y + last.height <= bounds.y + bounds.height + 1,
      "the final pattern's evidence must be reachable and fully visible");
    assert.equal(await page.locator(".main-content").evaluate((node) => node.scrollTop), mainScroll,
      "scrolling the findings must not move the notes");
    assert.equal(await page.locator(".reference-panel .panel-header").isVisible(), true);
    await page.mouse.wheel(0, -50000);
    await page.waitForFunction(() => document.querySelector("[data-writing-test-scroller]").scrollTop === 0);
    const headingBefore = await page.locator(`${PANEL} h3`).boundingBox();
    assert.ok(headingBefore);
    if (headingBefore.y + headingBefore.height > bounds.y + bounds.height) {
      await page.mouse.wheel(0, headingBefore.y - bounds.y - 8);
      await page.waitForFunction(() => {
        const container = document.querySelector("[data-writing-test-scroller]").getBoundingClientRect();
        const heading = document.querySelector(".writing-panel h3").getBoundingClientRect();
        return heading.top >= container.top - 1 && heading.bottom <= container.bottom + 1;
      });
    }
    const heading = await page.locator(`${PANEL} h3`).boundingBox();
    assert.ok(heading && heading.y >= bounds.y - 1 && heading.y + heading.height <= bounds.y + bounds.height);
  },
]);
for (const unifiedPage of [false, true]) cases.push([
  `Ctrl-Shift-B toggles the right pane without editing ${unifiedPage ? "continuous" : "classic"} text`,
  { unifiedPage }, async (page) => {
    await page.keyboard.press("Control+Shift+b");
    await page.locator(".reference-panel").waitFor({ state: "detached" });
    await focusTarget(page, "b0", { unifiedPage });
    const original = await editorText(page, unifiedPage);
    await page.evaluate(() => {
      const view = window.__activeEditorView;
      const anchor = view.state.doc.toString().indexOf("synthetic note");
      view.dispatch({ selection: { anchor, head: anchor + "synthetic note".length } });
      window.__shortcutEditor = view;
    });
    await page.keyboard.press("Control+Shift+b");
    await page.locator(".reference-panel").waitFor();
    assert.equal(await page.evaluate(() => window.__shortcutEditor.state.doc.toString()), original,
      "opening the pane must not bold the selection or alter Markdown");
    await page.keyboard.press("Control+Shift+b");
    await page.locator(".reference-panel").waitFor({ state: "detached" });
    assert.equal(await page.evaluate(() => window.__shortcutEditor.state.doc.toString()), original);
  },
]);
cases.push(["Ctrl-Alt-B formats bold in the block editor instead of toggling the right pane", {}, async (page) => {
  await page.keyboard.press("Control+Shift+b");
  await page.locator(".reference-panel").waitFor({ state: "detached" });
  await focus(page, "b0");
  const original = await editorText(page);
  await page.evaluate(() => {
    const view = window.__activeEditorView;
    const anchor = view.state.doc.toString().indexOf("synthetic note");
    view.dispatch({ selection: { anchor, head: anchor + "synthetic note".length } });
  });
  await page.keyboard.press("Control+Alt+b");
  assert.equal(await editorText(page), original.replace("synthetic note", "**synthetic note**"));
  assert.equal(await page.locator(".reference-panel").count(), 0);
  await page.keyboard.press("Control+z");
  assert.equal(await editorText(page), original);
}]);
for (const connection of ["disabled", "embedding-only"]) cases.push([
  `${connection} gate and Configure AI navigation`, { connection }, async (page) => {
    assert.equal(await button(page, "Configure AI").isVisible(), true);
    assert.equal(await button(page, "Analyze AI style").count(), 0);
    assert.equal(await button(page, "Rewrite naturally").count(), 0);
    await button(page, "Configure AI").click();
    await page.locator(".settings-page").waitFor();
    assert.equal((await calls(page, "ai_analyze_writing")).length, 0);
    assert.equal((await calls(page, "ai_rewrite_writing")).length, 0);
  },
]);
cases.push(["visible panel refreshes the generation gate after AI configuration changes", { connection: "embedding-only" }, async (page) => {
  assert.equal(await button(page, "Configure AI").isVisible(), true);
  await page.evaluate(() => {
    window.__writingState.connection = "configured";
    window.dispatchEvent(new CustomEvent("ai-configuration-changed"));
  });
  await button(page, "Rewrite naturally").waitFor();
  assert.equal(await button(page, "Rewrite naturally").isEnabled(), true);
  await page.evaluate(() => {
    window.__writingState.connection = "disabled";
    window.dispatchEvent(new CustomEvent("ai-configuration-changed"));
  });
  await button(page, "Configure AI").waitFor();
  assert.equal(await button(page, "Rewrite naturally").count(), 0);
  assert.equal((await calls(page, "ai_rewrite_writing")).length, 0);
}]);
cases.push(["command palette opens writing assistance without losing the selected block", {}, async (page) => {
  await page.evaluate(() => window.dispatchEvent(new CustomEvent("toggle-reference-panel")));
  await page.locator(PANEL).waitFor({ state: "detached" });
  await focus(page, "b1");
  await page.keyboard.press("Control+Shift+p");
  await page.getByPlaceholder("Run a command…").fill("Writing assistance");
  await page.getByRole("button", { name: "Open writing assistance", exact: true }).click();
  await button(page, "Analyze AI style").click();
  await finish(page);
  assert.equal((await calls(page, "ai_analyze_writing")).at(-1).args.blocks[0].id, "b1");
}]);
cases.push(["default block analyzes last-focused prose without mutations or web calls", {}, async (page) => {
  await focus(page, "b1");
  const before = await allNotes(page);
  const actions = await undo(page);
  assert.equal(await button(page, "Block").getAttribute("aria-pressed"), "true");
  await button(page, "Analyze AI style").click();
  await finish(page);
  const request = (await calls(page, "ai_analyze_writing")).at(-1).args;
  assert.deepEqual(request.blocks, [{ id: "b1", content: before.find((block) => block.id === "b1").content }]);
  assert.ok(request.operationId);
  await page.getByRole("img", { name: "AI-like style score: 67 out of 100. Not an authorship probability.", exact: true }).waitFor();
  assert.match(await page.locator(`${PANEL} .finding`).innerText(), /Repeated rhythm/);
  assert.equal(await page.locator(`${PANEL} blockquote`).innerText(), request.blocks[0].content);
  assert.match(await page.locator(`${PANEL} .coverage`).innerText(), /90 of 120 words.*2 of 3 sections/);
  assert.match(await page.locator(PANEL).innerText(), /not proof of AI authorship/i);
  await expectNoRewrite(page, before, actions);
  assert.deepEqual(await page.evaluate(() => window.__selectionState.calls.filter(({ cmd }) =>
    /^(ai_ask|research_|web_|ai_search)/.test(cmd))), []);
}]);
cases.push(["block rewrite preserves children and metadata; actual editor Ctrl+Z/Y is one action", {}, async (page) => {
  await focus(page, "b2");
  const before = await allNotes(page);
  const original = before.find((block) => block.id === "b2").content;
  await button(page, "Rewrite naturally").click();
  await finish(page);
  assert.deepEqual(await allNotes(page), before.map((block) =>
    block.id === "b2" ? { ...block, content: rewritten(original) } : block));
  const actions = await undo(page);
  assert.equal(actions.length, 1);
  assert.equal(actions[0].type, "rewrite_writing");
  assert.equal(actions[0].changes.length, 1);
  await focus(page, "b2");
  assert.equal(await editorText(page), rewritten(original));
  await page.keyboard.press("Control+z");
  await waitContent(page, "b2", original);
  assert.deepEqual(await allNotes(page), before);
  await page.keyboard.press("Control+y");
  await waitContent(page, "b2", rewritten(original));
  const applies = await calls(page, "apply_writing_changes");
  assert.equal(applies.length, 3);
  assert.equal(applies[0].args.expectedBlocks.length, before.filter((block) => block.page_id === "selection-page").length);
  assert.equal(applies[1].args.expectedBlocks, undefined, "undo uses content CAS, not the stale page snapshot");
  assert.equal(applies[1].args.changes[0].beforeContent, rewritten(original));
}]);
cases.push(["journal Page scope rewrites focused middle day with nested prose and grouped undo", { journal: true }, async (page) => {
  await row(page, "day-1-b0").scrollIntoViewIfNeeded();
  await focus(page, "day-1-b0");
  await button(page, "Page").click();
  const before = await allNotes(page);
  await button(page, "Analyze AI style").click();
  await finish(page);
  assert.deepEqual((await calls(page, "ai_analyze_writing")).at(-1).args.blocks.map((block) => block.id),
    ["day-1-b0", "day-1-child", "day-1-b1"]);
  assert.match(await page.locator(`${PANEL} .result-target`).innerText(), /2026-09-12/);
  assert.deepEqual(await allNotes(page), before);
  await button(page, "Rewrite naturally").click();
  await finish(page);
  const after = before.map((block) => block.page_id === "day-1" ? { ...block, content: rewritten(block.content) } : block);
  assert.deepEqual(await allNotes(page), after);
  assert.equal((await undo(page)).length, 1);
  assert.equal((await undo(page))[0].changes.length, 3);
  await focus(page, "day-1-b1");
  assert.equal(await editorText(page), after.find((block) => block.id === "day-1-b1").content);
  await page.keyboard.press("Control+z");
  await waitContent(page, "day-1-b0", before.find((block) => block.id === "day-1-b0").content);
  assert.deepEqual(await allNotes(page), before, "one undo restores all day blocks");
  await page.keyboard.press("Control+y");
  await waitContent(page, "day-1-child", after.find((block) => block.id === "day-1-child").content);
  assert.deepEqual(await allNotes(page), after);
}]);
cases.push(["Page scope excludes unsupported blocks and preserves every ID and property", {}, async (page) => {
  await button(page, "Page").click();
  const before = await allNotes(page);
  await button(page, "Rewrite naturally").click();
  await finish(page);
  const expectedIds = ["b0", "b1", "b2", "hidden-child", "hidden-grandchild", "b3", "b4"];
  assert.deepEqual((await calls(page, "ai_rewrite_writing")).at(-1).args.blocks.map((block) => block.id), expectedIds);
  assert.deepEqual(await allNotes(page), before.map((block) =>
    expectedIds.includes(block.id) ? { ...block, content: rewritten(block.content) } : block));
}]);
for (const unifiedPage of [false, true]) cases.push([
  `validated edits report retained wording and undo together in ${unifiedPage ? "continuous" : "classic"} editor`,
  { unifiedPage }, async (page) => {
    await button(page, "Page").click();
    const before = await allNotes(page);
    await page.evaluate(() => { window.__writingState.mode = "some-skipped"; });
    await button(page, "Rewrite naturally").click();
    await finish(page);
    const requested = (await calls(page, "ai_rewrite_writing")).at(-1).args.blocks;
    const changedIds = requested.slice(1).map(({ id }) => id);
    const after = before.map((block) => changedIds.includes(block.id)
      ? { ...block, content: rewritten(block.content) } : block);
    assert.deepEqual(await allNotes(page), after);
    const warning = page.getByRole("region", { name: "Wording left unchanged" });
    assert.match(await warning.innerText(), /Text block 1, line 1/);
    assert.match(await warning.innerText(), /original text was kept/);
    assert.equal((await calls(page, "apply_writing_changes")).length, 1);
    assert.equal((await undo(page)).length, 1);
    assert.deepEqual((await undo(page))[0].changes.map(({ blockId }) => blockId), changedIds);
    await focusTarget(page, "b1", { unifiedPage });
    await page.keyboard.press("Control+z");
    await waitContent(page, "b1", before.find(({ id }) => id === "b1").content);
    assert.deepEqual(await allNotes(page), before);
    await page.keyboard.press("Control+y");
    await waitContent(page, "b1", after.find(({ id }) => id === "b1").content);
    assert.deepEqual(await allNotes(page), after);
  },
]);
cases.push(["all rejected proposals keep notes and undo untouched and report the affected part", {}, async (page) => {
  const before = await allNotes(page);
  const actions = await undo(page);
  await page.evaluate(() => { window.__writingState.mode = "all-skipped"; });
  await button(page, "Rewrite naturally").click();
  await finish(page);
  await expectNoRewrite(page, before, actions);
  assert.match(await page.locator(`${PANEL} .result-status`).innerText(), /No safe wording changes were available/);
  assert.match(await page.getByRole("region", { name: "Wording left unchanged" }).innerText(), /Text block 1, line 1/);
  await page.evaluate(() => { window.__writingState.mode = "no-op"; });
  await button(page, "Rewrite naturally").click();
  await finish(page);
  assert.equal(await page.locator(`${PANEL} .skipped-result`).count(), 0);
  assert.match(await page.locator(`${PANEL} .result-status`).innerText(), /model kept the original wording/);
}]);
for (const unifiedPage of [false, true]) {
  const name = unifiedPage ? "continuous" : "classic";
  cases.push([`${name} flushes unsaved draft and updates editor after rewrite`, { unifiedPage }, async (page) => {
    await focusTarget(page, "b0", { unifiedPage });
    const original = (await allNotes(page)).find((block) => block.id === "b0").content;
    await page.keyboard.insertText(" Unsaved draft.");
    await button(page, "Rewrite naturally").click();
    await finish(page);
    const request = (await calls(page, "ai_rewrite_writing")).at(-1).args;
    assert.equal(request.blocks[0].content, `${original} Unsaved draft.`);
    await focusTarget(page, "b0", { unifiedPage });
    assert.ok((await editorText(page)).includes(rewritten(request.blocks[0].content)), "live CodeMirror shows replacement");
    await page.keyboard.press("Control+z");
    await waitContent(page, "b0", request.blocks[0].content);
    await page.keyboard.press("Control+y");
    await waitContent(page, "b0", rewritten(request.blocks[0].content));
  }]);
  cases.push([`${name} edits during generation reject stale rewrite atomically`, { unifiedPage }, async (page) => {
    await startHeld(page);
    await focusTarget(page, "b0", { unifiedPage });
    const original = (await allNotes(page)).find((block) => block.id === "b0").content;
    await page.keyboard.insertText(" A newer unsaved thought.");
    await releaseModel(page);
    assert.equal(await editedBlockText(page, "b0", unifiedPage), `${original} A newer unsaved thought.`,
      "newer unsaved draft remains in the real editor");
    const completionMessage = await page.locator(PANEL).innerText();
    await page.locator(`${PANEL} h3`).click();
    await waitContent(page, "b0", `${original} A newer unsaved thought.`);
    assert.equal((await undo(page)).filter((action) => action.type === "rewrite_writing").length, 0);
    assert.deepEqual(await page.evaluate(() => window.__writingState.applied), []);
    assert.match(completionMessage, /page changed|Cancelled/i);
  }]);
  cases.push([`${name} blocks typing during apply then restores editing and undo order`, { unifiedPage }, async (page) => {
    await focusTarget(page, "b0", { unifiedPage });
    const original = (await allNotes(page)).find((block) => block.id === "b0").content;
    await page.keyboard.insertText(" Before apply.");
    const captured = `${original} Before apply.`;
    await page.evaluate(() => { window.__writingState.holdApply = true; });
    await button(page, "Rewrite naturally").click();
    await page.waitForFunction(() => window.__writingState.applyWaiters.length === 1);
    const beforeApply = await allNotes(page);
    const actions = await undo(page);
    assert.equal(beforeApply.find((block) => block.id === "b0").content, captured,
      "pending draft is flushed before the native apply begins");
    assert.equal(await button(page, "Cancel").isDisabled(), true);
    const pendingRequest = (await calls(page, "apply_writing_changes")).at(-1).args;
    assert.equal(pendingRequest.changes[0].beforeContent, captured);

    for (const id of ["b0", "b1"]) {
      const content = unifiedPage
        ? page.locator(`[data-source-block-id="${id}"] .unified-rendered-content`)
        : row(page, id).locator(".block-content");
      await content.click();
      const documentBeforeTyping = await page.evaluate((continuous) => {
        window.__writingLockedView = continuous ? window.__unifiedPageEditorView : window.__activeEditorView;
        return window.__writingLockedView?.state.doc.toString();
      }, unifiedPage);
      await page.keyboard.type("MUST NOT ENTER DURING APPLY");
      await page.keyboard.press("Enter");
      await frames(page);
      assert.equal(await page.evaluate(() => window.__writingLockedView?.state.doc.toString()), documentBeforeTyping,
        `the ${id} editor rejects keyboard edits while its page is locked`);
      assert.equal(await page.locator(".page-content").getByText("MUST NOT ENTER DURING APPLY", { exact: false }).count(), 0);
    }
    assert.deepEqual(await allNotes(page), beforeApply, "pending apply makes no partial writes");
    assert.deepEqual(await undo(page), actions, "blocked typing creates no undo action");
    await page.evaluate(() => {
      window.__writingState.holdApply = false;
      window.__writingState.applyWaiters.splice(0).forEach((resolve) => resolve());
    });
    await finish(page);
    await focusTarget(page, "b0", { unifiedPage });
    const replacement = rewritten(captured);
    assert.equal(await editedBlockText(page, "b0", unifiedPage), replacement);
    await page.keyboard.insertText(" After apply.");
    assert.equal(await editedBlockText(page, "b0", unifiedPage), `${replacement} After apply.`,
      "editor accepts normal typing once apply and notification complete");
    await page.keyboard.press("Control+z");
    assert.equal(await editedBlockText(page, "b0", unifiedPage), replacement,
      "new typing undoes before the grouped rewrite");
    await page.keyboard.press("Control+z");
    await waitContent(page, "b0", captured);
    assert.equal(await editedBlockText(page, "b0", unifiedPage), captured,
      "the next undo restores the complete pre-rewrite draft");
    await page.keyboard.press("Control+y");
    await waitContent(page, "b0", replacement);
    assert.equal(await editedBlockText(page, "b0", unifiedPage), replacement);
  }]);
}
cases.push(["newer classic typing undoes before rewrite and older local typing after it", {}, async (page) => {
  await focus(page, "b0");
  const original = (await allNotes(page)).find((block) => block.id === "b0").content;
  await page.keyboard.insertText(" Earlier thought.");
  await button(page, "Rewrite naturally").click();
  await finish(page);
  const replacement = rewritten(`${original} Earlier thought.`);
  await focus(page, "b0");
  await page.keyboard.insertText(" Newer thought.");
  await page.keyboard.press("Control+z");
  assert.equal(await editorText(page), replacement, "newest typing must undo first");
  await page.keyboard.press("Control+z");
  await waitContent(page, "b0", `${original} Earlier thought.`);
  await page.keyboard.press("Control+z");
  assert.equal(await editorText(page), original, "preexisting local history remains after grouped rewrite undo");
}]);
cases.push(["continuous retains its view and older native history across grouped rewrites", { unifiedPage: true }, async (page) => {
  await focusContinuous(page, "selection-page", "b0");
  const original = (await allNotes(page)).find((block) => block.id === "b0").content;
  await page.keyboard.insertText(" Earlier thought.");
  await page.evaluate(() => { window.__writingViewBeforeRewrite = window.__unifiedPageEditorView; });
  await button(page, "Rewrite naturally").click();
  await finish(page);
  assert.equal(await page.evaluate(() => window.__unifiedPageEditorView === window.__writingViewBeforeRewrite), true,
    "a successful rewrite updates, rather than recreates, the continuous editor");
  await focusContinuous(page, "selection-page", "b0");
  const replacement = rewritten(`${original} Earlier thought.`);
  await page.keyboard.insertText(" Newer thought.");
  await page.keyboard.press("Control+z");
  assert.equal(await editedBlockText(page, "b0", true), replacement);
  await page.keyboard.press("Control+z");
  await waitContent(page, "b0", `${original} Earlier thought.`);
  await focusContinuous(page, "selection-page", "b0");
  await page.keyboard.press("Control+z");
  assert.equal(await editedBlockText(page, "b0", true), original,
    "older native typing remains undoable after the grouped rewrite is undone");
}]);
cases.push(["continuous Page rewrite updates all blocks and restores one grouped action", { unifiedPage: true }, async (page) => {
  await button(page, "Page").click();
  const before = await allNotes(page);
  await button(page, "Rewrite naturally").click();
  await finish(page);
  const rewrittenIds = (await calls(page, "ai_rewrite_writing")).at(-1).args.blocks.map((block) => block.id);
  assert.equal(rewrittenIds.length, 7);
  const after = before.map((block) => rewrittenIds.includes(block.id) ? { ...block, content: rewritten(block.content) } : block);
  assert.deepEqual(await allNotes(page), after);
  await focusContinuous(page, "selection-page", "b0");
  assert.ok((await editorText(page)).includes(rewritten(before.find((block) => block.id === "b4").content)),
    "continuous source reloads all rewritten blocks");
  await page.keyboard.press("Control+z");
  await waitContent(page, "b0", before.find((block) => block.id === "b0").content);
  assert.deepEqual(await allNotes(page), before);
  await page.keyboard.press("Control+y");
  await waitContent(page, "b0", after.find((block) => block.id === "b0").content);
  assert.deepEqual(await allNotes(page), after);
}]);
cases.push(["page snapshot rejects changes outside the selected block", {}, async (page) => {
  const actions = await undo(page);
  await startHeld(page);
  await page.evaluate(() => {
    window.__selectionState.blocks.find((block) => block.id === "b4").properties.concurrent = "new metadata";
  });
  const concurrent = await allNotes(page);
  await releaseModel(page);
  await expectNoRewrite(page, concurrent, actions);
  assert.match(await page.locator(`${PANEL} [role="alert"]`).innerText(), /page changed/i);
}]);
for (const mode of ["error", "invalid-id", "empty", "incomplete", "no-op"]) cases.push([
  `${mode} rewrite never mutates or creates undo`, {}, async (page) => {
    const before = await allNotes(page);
    const actions = await undo(page);
    await page.evaluate((mode) => { window.__writingState.mode = mode; }, mode);
    await button(page, "Rewrite naturally").click();
    await finish(page);
    if (mode === "no-op") assert.match(await page.locator(PANEL).innerText(), /Nothing was changed/);
    else await page.locator(`${PANEL} [role="alert"]`).waitFor();
    await expectNoRewrite(page, before, actions);
    assert.equal((await calls(page, "apply_writing_changes")).length, 0);
  },
]);
cases.push(["cancel disables controls and ignores delayed model completion", {}, async (page) => {
  const before = await allNotes(page);
  const actions = await undo(page);
  await startHeld(page);
  for (const name of ["Block", "Page", "Analyze AI style", "Rewrite naturally"]) assert.equal(await button(page, name).isDisabled(), true);
  const operationId = (await calls(page, "ai_rewrite_writing")).at(-1).args.operationId;
  await button(page, "Cancel").click();
  assert.equal((await calls(page, "ai_cancel_writing")).at(-1).args.operationId, operationId);
  await releaseModel(page);
  await expectNoRewrite(page, before, actions);
  assert.equal((await calls(page, "apply_writing_changes")).length, 0);
  assert.match(await page.locator(PANEL).innerText(), /Cancelled/);
}]);
cases.push(["changing focused target ignores a delayed rewrite", {}, async (page) => {
  const before = await allNotes(page);
  const actions = await undo(page);
  await startHeld(page);
  await focus(page, "b1");
  await releaseModel(page);
  await expectNoRewrite(page, before, actions);
  assert.equal((await calls(page, "apply_writing_changes")).length, 0);
}]);
cases.push(["graph change rejects delayed rewrite", {}, async (page) => {
  const before = await allNotes(page);
  const actions = await undo(page);
  await startHeld(page);
  await page.evaluate(() => { window.__writingState.graphPath = "/synthetic/different-writing-graph"; });
  await releaseModel(page);
  await expectNoRewrite(page, before, actions);
  assert.match(await page.locator(`${PANEL} [role="alert"]`).innerText(), /graph changed/i);
}]);
for (const theme of ["github", "github-dark"]) cases.push([
  `${theme} narrow panel retains visible accessible score and controls`, { theme }, async (page) => {
    await page.setViewportSize({ width: 900, height: 850 });
    const separator = page.getByRole("separator", { name: "Resize right panel", exact: true });
    const bounds = await separator.boundingBox();
    await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + 180);
    await page.mouse.down();
    await page.mouse.move(895, bounds.y + 180);
    await page.mouse.up();
    const tabs = page.locator(".reference-panel .panel-tabs");
    for (const name of ["Chat", "Notes"]) {
      const tab = tabs.getByRole("tab", { name, exact: true });
      assert.equal(await tab.isVisible(), true);
      assert.equal(await tab.evaluate((node) => {
        const text = document.createRange();
        text.selectNodeContents(node);
        const label = text.getBoundingClientRect();
        const button = node.getBoundingClientRect();
        const group = node.closest(".panel-tabs").getBoundingClientRect();
        return label.left >= button.left && label.right <= button.right
          && label.top >= button.top && label.bottom <= button.bottom
          && button.left >= group.left && button.right <= group.right
          && button.top >= group.top && button.bottom <= group.bottom;
      }), true, `${name} label is fully visible, not clipped at narrow width`);
    }
    const close = page.locator(".reference-panel").getByRole("button", { name: "Close reading panel", exact: true });
    assert.equal(await close.isVisible(), true);
    assert.equal(await close.evaluate((node) => {
      const rect = node.getBoundingClientRect();
      const firstTab = node.closest(".panel-header").querySelector(".panel-tabs button").getBoundingClientRect();
      return rect.width >= 20 && Math.abs(rect.top - firstTab.top) <= 4;
    }), true, "close button retains its width and top alignment");
    await button(page, "Analyze AI style").click();
    await finish(page);
    const svg = page.getByRole("img", { name: /AI-like style score: 67 out of 100/ });
    assert.equal(await svg.isVisible(), true);
    assert.equal(await page.locator(`${PANEL} .score-number`).textContent(), "67");
    const metrics = await svg.evaluate((node) => {
      const panel = node.closest(".reference-panel").getBoundingClientRect();
      const rect = node.getBoundingClientRect();
      const text = getComputedStyle(node.querySelector("text"));
      const ring = getComputedStyle(node.querySelector(".score-value"));
      const background = getComputedStyle(node.closest(".result")).backgroundColor;
      const luminance = (color) => {
        const channels = color.match(/[\d.]+/g).slice(0, 3).map((channel) => {
          const value = Number(channel) / 255;
          return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
        });
        return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
      };
      const contrast = (color) => {
        const values = [luminance(color), luminance(background)].sort((a, b) => b - a);
        return (values[0] + 0.05) / (values[1] + 0.05);
      };
      return { panelWidth: panel.width, inPanel: rect.left >= panel.left && rect.right <= panel.right,
        fill: text.fill, stroke: ring.stroke, width: rect.width, height: rect.height,
        textContrast: contrast(text.fill), ringContrast: contrast(ring.stroke),
        noOverflow: node.closest(".writing-panel").scrollWidth <= node.closest(".writing-panel").clientWidth + 1 };
    });
    assert.equal(metrics.panelWidth, 280, "regressions exercise the minimum panel width");
    assert.ok(metrics.inPanel && metrics.noOverflow && metrics.width >= 100 && metrics.height >= 100);
    assert.notEqual(metrics.fill, "none");
    assert.notEqual(metrics.stroke, "none");
    assert.ok(metrics.textContrast >= 4.5, `score text contrast: ${metrics.textContrast.toFixed(2)}:1`);
    assert.ok(metrics.ringContrast >= 3, `score ring contrast: ${metrics.ringContrast.toFixed(2)}:1`);
    for (const name of ["Block", "Page", "Analyze AI style", "Rewrite naturally"]) {
      assert.equal(await button(page, name).isVisible(), true);
      await button(page, name).focus();
      assert.equal(await button(page, name).evaluate((node) => node === document.activeElement), true);
    }
    if (process.env.UI_TEST_SCREENSHOTS) {
      await mkdir(process.env.UI_TEST_SCREENSHOTS, { recursive: true });
      await page.screenshot({ path: path.join(process.env.UI_TEST_SCREENSHOTS, `writing-${theme}-narrow.png`) });
    }
  },
]);

module.exports = { cases, openWriting };
if (require.main === module) (async () => {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  let failures = 0;
  let passes = 0;
  try {
    for (const [name, options, run] of cases) {
      if (process.env.UI_TEST_CASE && !process.env.UI_TEST_CASE.split("|").some((filter) => name.includes(filter))) continue;
      let fixture;
      try {
        fixture = await openWriting(browser, options);
        await run(fixture.page);
        assert.deepEqual(fixture.errors, [], "no uncaught browser errors");
        passes++;
        console.log(`PASS ${name}`);
      } catch (error) {
        failures++;
        console.error(`FAIL ${name}\n${error.stack ?? error}`);
        if (fixture) console.error("Writing state:", JSON.stringify(await fixture.page.evaluate(() => ({
          panel: document.querySelector(".writing-panel")?.textContent,
          undo: window.__undoStack,
          activeBlock: document.activeElement?.closest(".block-item")?.dataset.blockId,
          editor: window.__activeEditorView?.state.doc.toString().slice(0, 500),
          requests: window.__selectionState.calls.filter(({ cmd }) =>
            /writing|update_block|update_page_source/.test(cmd)).slice(-6),
        })), null, 2));
      } finally {
        await fixture?.page.close();
      }
    }
  } finally {
    await browser.close();
  }
  console.log(`Writing UI: ${passes} passed, ${failures} failed`);
  if (failures) process.exitCode = 1;
})().catch((error) => { console.error(error); process.exitCode = 1; });

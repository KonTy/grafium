// Real shared Chat and editors; every source, model, search and write is synthetic.
process.env.UI_TEST_URL ??= "http://127.0.0.1:5286";
const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor, focus, focusContinuous, row, frames } = require("./keyboardSelection.ui.cjs");

async function installAssistantFixture(page, options = {}) {
  page.assistantExternalRequests = [];
  const origin = new URL(process.env.UI_TEST_URL).origin;
  await page.route("**/*", (route) => {
    if (new URL(route.request().url()).origin === origin) return route.continue();
    page.assistantExternalRequests.push(route.request().url());
    return route.abort("blockedbyclient");
  });
  await page.addInitScript((options) => {
    function install(internals) {
      const state = window.__selectionState;
      const original = internals.invoke;
      state.blocks.find(({ id }) => id === "b1").content = "Alpha\nBeta";
      state.pages.push({
        ...structuredClone(state.pages[0]), id: "other-research", title: "Other research source",
        file_path: "pages/other-research.md",
      });
      state.blocks.push({
        ...structuredClone(state.blocks[0]), id: "other-research-block", page_id: "other-research",
      }, {
        ...structuredClone(state.blocks[0]), id: "long-book-chunk", order_index: 8,
        content: "LONG-BOOK-MARKER " + "Synthetic book background. ".repeat(2800),
      });
      // The editor fixture deliberately has an empty journal day for its own tests.
      // Populate it here so editor initialization cannot obscure Chat's no-write invariant.
      state.blocks.push({
        ...structuredClone(state.blocks[0]), id: "day-2-b0", page_id: "day-2", content: "Synthetic journal entry",
      });
      if (options.video) {
        const root = state.blocks.find(({ id }) => id === "b0");
        root.content = "Synthetic imported video";
        state.blocks.push({
          ...structuredClone(root), id: "video-transcript", parent_id: root.id,
          content: "LONG-TRANSCRIPT-MARKER " + "Unrelated synthetic video background. ".repeat(1500)
            + "The lantern uses the cobalt dial.",
        });
      }
      const callbacks = new Map(), listeners = new Map(), pending = new Map();
      let sequence = 5000;
      const fixture = window.__assistantFixture = {
        calls: [], requests: [], contextCalls: [], cancellations: [], writes: [], legacyCalls: [],
        hold: false, connected: true, healthChecks: 0, configChecks: 0,
        indexFailure: false, indexBuilds: 0, gpuRetries: 0,
        config: {
          enabled: true, mode: "cloud",
          cloud: {
            llm_provider: "openai", llm_model: "Spark synthetic model",
            llm_base_url: "http://spark.lan:8000/v1", llm_api_key: "DO_NOT_RENDER",
            embedding_provider: "", embedding_model: "",
          },
        },
        index: {
          indexed_chunks: 0, total_blocks: state.blocks.length, pending_pages: 0,
          embedder_ready: false, llm_ready: true, accelerator: null,
        },
      };
      fixture.originalSources = structuredClone({ pages: state.pages, blocks: state.blocks });
      const graphs = [
        { name: "Keyboard selection fixture", path: "/synthetic/keyboard-selection" },
        { name: "Other test graph", path: "/synthetic/assistant-other" },
      ];
      let graph = graphs[0];
      fixture.emit = (event, payload) => {
        for (const [id, listener] of listeners) {
          if (listener.event === event) callbacks.get(listener.handler)?.({ event, id, payload });
        }
      };
      fixture.finish = (requestId, reply = { delta: "Synthetic scoped answer.", done: true }) => {
        const resolve = pending.get(requestId);
        if (!resolve) throw new Error(`No pending request: ${requestId}`);
        resolve(typeof reply === "string" ? { delta: reply, done: true } : reply);
      };
      fixture.listenerCount = () => [...listeners.values()]
        .filter(({ event }) => event === "ai://chat_stream" || event === "ai://chat_sources").length;
      internals.transformCallback = (callback) => { const id = ++sequence; callbacks.set(id, callback); return id; };
      internals.unregisterCallback = (id) => callbacks.delete(id);
      window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener(_event, id) { listeners.delete(id); } };
      internals.invoke = async (cmd, args = {}) => {
        fixture.calls.push({ cmd, args: structuredClone(args) });
        if (/^(create|update|delete|save|insert|apply|rewrite|restore|move|reorder)_(block|blocks|page|page_source|reading_note)/.test(cmd)
          || /^(research_insert_summary|reading_note_save|reading_note_delete|writing_apply)/.test(cmd)) {
          fixture.writes.push({ cmd, args: structuredClone(args) });
        }
        if (["ai_ask", "ai_ask_stream", "research_scoped", "research_deep", "research_scope_info",
          "get_chat_preferences", "set_chat_preferences", "ai_cancel_stream"].includes(cmd)) {
          fixture.legacyCalls.push(cmd);
          throw new Error(`Shared Chat must not invoke obsolete endpoint ${cmd}`);
        }
        if (cmd === "plugin:event|listen") { const id = ++sequence; listeners.set(id, args); return id; }
        if (cmd === "plugin:event|unlisten") { listeners.delete(args.eventId); return; }
        if (cmd === "get_graph_info") return structuredClone(graph);
        if (cmd === "list_graphs") return structuredClone(graphs);
        if (cmd === "validate_graph") return { is_valid: true };
        if (cmd === "open_graph") {
          graph = graphs.find(({ path }) => path === args.path);
          if (!graph) throw new Error("Unknown synthetic graph");
          return structuredClone(graph);
        }
        if (cmd === "ai_health_check") {
          fixture.healthChecks++;
          return { enabled: true, llm_available: fixture.connected, embedder_available: false, vector_count: 0 };
        }
        if (cmd === "ai_get_config") { fixture.configChecks++; return structuredClone(fixture.config); }
        if (cmd === "ai_index_status") {
          if (fixture.indexFailure) throw new Error("Synthetic index status failure");
          return structuredClone(fixture.index);
        }
        if (cmd === "ai_index_all_pages") {
          fixture.indexBuilds++;
          fixture.index.indexed_chunks = 12;
          return { pages_processed: 2, pages_failed: 0 };
        }
        if (cmd === "ai_retry_llm_on_gpu") {
          fixture.gpuRetries++;
          fixture.index.accelerator.on_gpu = true;
          return;
        }
        if (cmd === "assistant_context_info") {
          fixture.contextCalls.push(structuredClone(args));
          const source = state.pages.find(({ id }) => id === args.pageId);
          if (!source) throw new Error("Missing context source");
          const isBook = source.title.startsWith("Books/");
          return {
            pageId: source.id, pageTitle: source.title, isJournal: source.is_journal, isBook,
            blockCount: state.blocks.filter(({ page_id }) => page_id === source.id).length,
            section: args.blockId && !source.is_journal ? { title: "Synthetic chapter", blockId: "b0" } : null,
            book: isBook ? { pageId: source.id, title: source.title } : null,
          };
        }
        if (cmd === "assistant_chat") {
          const keys = Object.keys(args).sort().join(",");
          if (keys !== "context,graphPath,history,mode,question,requestId") {
            throw new Error(`Unexpected assistant_chat request shape: ${keys}`);
          }
          fixture.requests.push({
            cmd, args: structuredClone(args),
            rootContent: state.blocks.find(({ id }) => id === "b0").content,
          });
          if (/LONG-(BOOK|TRANSCRIPT)-MARKER/.test(args.question)) throw new Error("Source was inlined into question");
          fixture.emit("ai://chat_stream", { request_id: args.requestId, phase: "retrieving", delta: "", done: false });
          const reply = fixture.hold
            ? await new Promise((resolve) => pending.set(args.requestId, resolve))
            : { delta: "Synthetic scoped answer.", done: true };
          pending.delete(args.requestId);
          fixture.emit("ai://chat_sources", {
            request_id: args.requestId,
            sources: args.context.kind === "none" ? [] : [{
              index: 1, page_id: args.context.pageId ?? "selection-page",
              page_title: "Synthetic source", block_id: "b0", date: null,
            }],
            web_sources: args.mode === "answer" ? [] : [{
              number: 1, title: "Synthetic evidence", url: "https://example.org/evidence",
            }],
          });
          fixture.emit("ai://chat_stream", { request_id: args.requestId, ...reply });
          return;
        }
        if (cmd === "research_cancel") {
          fixture.cancellations.push(args.requestId);
          pending.get(args.requestId)?.({ delta: "", done: true });
          return;
        }
        return original(cmd, args);
      };
      return internals;
    }
    let internals = window.__TAURI_INTERNALS__;
    if (internals) internals = install(internals);
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true, get: () => internals, set: (value) => { internals = install(value); },
    });
  }, options);
}

const panel = (page) => page.locator(".assistant-conversation:visible");
const input = (page) => panel(page).getByRole("textbox", { name: "Message", exact: true });
const context = (page) => panel(page).getByRole("combobox", { name: "Context", exact: true });
const mode = (page) => panel(page).getByRole("combobox", { name: "Mode", exact: true });
const button = (page, name) => panel(page).getByRole("button", { name, exact: true });
const composer = (page) => panel(page).locator("form").filter({
  has: page.getByRole("textbox", { name: "Message", exact: true }),
});

async function openAssistant(browser, options = {}) {
  const fixture = await openEditor(browser, {
    ...options, beforeNavigate: (page) => installAssistantFixture(page, options),
  });
  const { page } = fixture;
  if (options.global) {
    assert.equal(await page.locator(".assistant-conversation").count(), 0, "global Chat stays lazy");
    await page.keyboard.press("Alt+c");
  } else {
    if (options.journal) {
      await row(page, "day-1-b0").scrollIntoViewIfNeeded();
      await focus(page, "day-1-b0");
    } else if (options.unifiedPage) await focusContinuous(page, "selection-page", "b0");
    else await focus(page, "b0");
    await page.evaluate(() => window.dispatchEvent(new CustomEvent("toggle-reference-panel")));
    await page.getByRole("tab", { name: "Chat", exact: true }).click();
  }
  await input(page).waitFor();
  await page.waitForFunction(() => window.__assistantFixture.healthChecks > 0 && window.__assistantFixture.configChecks > 0);
  await frames(page);
  return fixture;
}

async function send(page, question, held = false) {
  const before = await page.evaluate(() => window.__assistantFixture.requests.length);
  await input(page).fill(question);
  await button(page, "Send").click();
  await page.waitForFunction((before) => window.__assistantFixture.requests.length === before + 1, before);
  if (!held) await button(page, "Send").waitFor();
  return page.evaluate(() => window.__assistantFixture.requests.at(-1));
}

async function finish(page, requestId, reply) {
  await page.evaluate(({ requestId, reply }) => window.__assistantFixture.finish(requestId, reply), { requestId, reply });
  await page.waitForFunction(() => window.__assistantFixture.listenerCount() === 0);
  await frames(page);
}

async function runCases(cases, open = openAssistant) {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  let passed = 0, failed = 0;
  try {
    for (const [name, options, run] of cases) {
      if (process.env.UI_TEST_CASE && !process.env.UI_TEST_CASE.split("|").some((part) => name.includes(part))) continue;
      let fixture;
      try {
        fixture = await open(browser, options);
        await run(fixture.page);
        await frames(fixture.page);
        assert.deepEqual(fixture.errors, []);
        assert.deepEqual(fixture.page.assistantExternalRequests, [], "all model/search transport goes through native IPC");
        const safety = await fixture.page.evaluate(() => ({
          legacyCalls: window.__assistantFixture.legacyCalls,
          writes: window.__assistantFixture.writes.map(({ cmd }) => cmd),
          original: window.__assistantFixture.originalSources,
          current: { pages: window.__selectionState.pages, blocks: window.__selectionState.blocks },
        }));
        assert.deepEqual(safety.legacyCalls, [], "only the unified context/mode API is used");
        if (!options.allowEditorWrites) {
          assert.deepEqual(safety.writes, [], "asking, navigation and context/mode changes never write sources");
          assert.deepEqual(safety.current, safety.original, "all synthetic source text and identities are unchanged");
        } else {
          assert.ok(safety.writes.every((call) => ["update_block", "update_page_source"].includes(call)),
            `only explicit editor flushes may write: ${JSON.stringify(safety.writes)}`);
        }
        console.log(`PASS ${name}`);
        passed++;
      } catch (error) {
        failed++;
        console.error(`FAIL ${name}\n${error.stack ?? error}`);
        if (fixture) console.error(await fixture.page.evaluate(() => ({
          requests: window.__assistantFixture?.requests.map(({ args }) => ({
            question: args.question.slice(0, 100), context: args.context.kind, mode: args.mode,
          })),
          panel: document.querySelector(".assistant-conversation")?.textContent?.slice(-2000),
          overlay: document.querySelector("vite-error-overlay")?.shadowRoot?.textContent?.slice(0, 1000),
        })));
      } finally { await fixture?.page.close(); }
    }
  } finally { await browser.close(); }
  console.log(`Unified Chat UI: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
}

module.exports = {
  assert, openAssistant, installAssistantFixture, panel, input, context, mode, button, composer,
  send, finish, runCases, focus, focusContinuous, row, frames,
};

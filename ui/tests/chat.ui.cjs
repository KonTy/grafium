// Real ChatView with stubbed IPC: no model, graph mutation, or internet searches.
const { chromium } = require("playwright");
const assert = require("node:assert/strict");

const BASE_URL = process.env.UI_TEST_URL ?? "http://localhost:5199/";

(async () => {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  try {
    const page = await browser.newPage({ viewport: { width: 1200, height: 900 } });
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    let savedPreferences = null;
    let failPreferenceSave = false;
    await page.exposeFunction("__loadChatPreferences", async () => {
      await new Promise((resolve) => setTimeout(resolve, 100));
      return savedPreferences;
    });
    await page.exposeFunction("__saveChatPreferences", (preferences) => {
      if (failPreferenceSave) throw new Error("Preference storage unavailable");
      savedPreferences = preferences;
    });
    await page.addInitScript(() => {
      const callbacks = new Map();
      const listeners = new Map();
      let sequence = 0;
      window.__chatRequests = [];
      window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
        unregisterListener(_event, id) { listeners.delete(id); },
      };
      window.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
        plugins: {},
        transformCallback(callback) {
          const id = ++sequence;
          callbacks.set(id, callback);
          return id;
        },
        unregisterCallback(id) { callbacks.delete(id); },
        invoke: async (cmd, args = {}) => {
          switch (cmd) {
            case "get_page": throw new Error("Page not found");
            case "get_app_theme": return "dark";
            case "get_chat_preferences": return window.__loadChatPreferences();
            case "set_chat_preferences": return window.__saveChatPreferences(args.preferences);
            case "get_graph_info": return { name: "Test", path: "/tmp/chat-test-graph" };
            case "ai_health_check": return { enabled: true, llm_available: true };
            case "ai_index_status": return {
              indexed_chunks: 4, total_blocks: 4, pending_pages: 0, embedder_ready: true,
              llm_ready: true, accelerator: null,
            };
            case "plugin:event|listen": {
              const id = ++sequence;
              listeners.set(id, args);
              return id;
            }
            case "plugin:event|unlisten": listeners.delete(args.eventId); return;
            case "ai_ask_stream":
            case "research_deep": {
              window.__chatRequests.push({ cmd, args });
              for (const [id, listener] of listeners) {
                if (listener.event !== "ai://chat_stream") continue;
                callbacks.get(listener.handler)?.({
                  event: listener.event, id,
                  payload: { request_id: args.requestId, delta: "A test answer.", done: true },
                });
              }
              return;
            }
            default: return [];
          }
        },
      };
    });
    const openChat = async () => {
      await page.getByRole("button", { name: "Chat", exact: true }).first().click();
      await page.locator(".chat-view").waitFor();
      await page.waitForFunction(() => !document.querySelector(".chat-view textarea")?.disabled);
    };
    const scope = page.getByRole("combobox", { name: "Scope", exact: true });
    const research = page.getByRole("checkbox", { name: "Research", exact: true });
    const send = async (question) => {
      await page.locator(".chat-view textarea").fill(question);
      await page.getByRole("button", { name: "Send", exact: true }).click();
      await page.waitForFunction(() => !document.querySelector(".chat-view textarea").disabled);
      return page.evaluate(() => window.__chatRequests.at(-1));
    };
    await page.goto(BASE_URL, { waitUntil: "networkidle" });
    await openChat();
    await page.waitForFunction(() => document.activeElement === document.querySelector(".chat-view textarea"));
    assert.equal(await scope.inputValue(), "local");
    assert.equal(await research.isDisabled(), true);
    let request = await send("Search the internet for test results");
    assert.equal(request.cmd, "ai_ask_stream");
    assert.equal(request.args.scope, "local");

    await scope.selectOption("internet");
    await scope.focus();
    await page.waitForTimeout(100);
    assert.equal(await scope.evaluate((element) => element === document.activeElement), true);
    assert.equal(await research.isEnabled(), true);
    request = await send("What are the latest results?");
    assert.equal(request.cmd, "ai_ask_stream");
    assert.equal(request.args.scope, "internet");
    assert.equal(request.args.history.length, 2);

    await research.check();
    request = await send("Investigate the test results in depth");
    assert.equal(request.cmd, "research_deep");
    assert.equal(request.args.scope, "internet");
    assert.equal(request.args.history.length, 4);

    await page.evaluate(() => localStorage.clear());
    await page.reload({ waitUntil: "networkidle" });
    await openChat();
    assert.equal(await scope.inputValue(), "internet");
    assert.equal(await research.isChecked(), true);
    assert.deepEqual(savedPreferences, { scope: "internet", research: true });
    await send("A fresh conversation after reload");
    await page.getByRole("button", { name: "New chat", exact: true }).click();
    assert.equal(await scope.inputValue(), "internet");

    await scope.selectOption("local");
    assert.equal(await research.isDisabled(), true);
    assert.equal(await research.isChecked(), false);
    request = await send("Look up the results online");
    assert.equal(request.cmd, "ai_ask_stream");
    assert.equal(request.args.scope, "local");
    await page.evaluate(() => localStorage.clear());
    await page.reload({ waitUntil: "networkidle" });
    await openChat();
    assert.equal(await scope.inputValue(), "local");
    assert.equal(await research.isDisabled(), true);
    await scope.selectOption("internet");
    assert.equal(await research.isChecked(), true);
    await page.waitForFunction(() => !document.querySelector(".chat-scope select").disabled);
    failPreferenceSave = true;
    await scope.selectOption("local");
    await page.getByText("Could not save Chat preferences:", { exact: false }).waitFor();
    assert.equal(await research.isDisabled(), true);
    request = await send("Keep this question local despite a persistence failure");
    assert.equal(request.args.scope, "local");
    assert.equal(savedPreferences.scope, "internet");
    assert.deepEqual(errors, []);
    console.log("PASS Chat scope defaults, persistence, focus, routing, history and Research gating");
  } finally {
    await browser.close();
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});

const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const BASE_URL = process.env.UI_TEST_URL ?? "http://localhost:5199/";

(async () => {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  try {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    page.setDefaultTimeout(10_000);
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.addInitScript(() => {
      let sequence = 0;
      window.__settingsWrites = [];
      window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
      window.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
        plugins: {}, transformCallback() { return ++sequence; }, unregisterCallback() {},
        invoke: async (cmd) => {
          switch (cmd) {
            case "get_page": throw new Error("Page not found");
            case "get_app_theme": return "github";
            case "get_smplos_theme": return null;
            case "get_layout_preferences": return { sidebarVisible: true, wideMode: true };
            case "get_graph_info": return { name: "Welcome test graph", path: "/synthetic/welcome-settings" };
            case "get_app_version": return "0.0.123";
            case "get_chat_preferences": return null;
            case "ai_default_concept_edge_prompt": return "Extract concepts from the selected notes.";
            case "ai_get_config": return { enabled: true, mode: "local", local: { provider: "openai_compatible" } };
            case "ai_health_check": return { enabled: true, llm_available: true, embedder_available: true, vector_count: 0 };
            case "ai_index_status": return {
              indexed_chunks: 0, total_blocks: 0, pending_pages: 0, embedder_ready: true,
              llm_ready: true, accelerator: null,
            };
            case "research_get_config": throw new Error("unknown command research_get_config");
            case "ai_set_config":
            case "set_media_config":
            case "research_set_config": window.__settingsWrites.push(cmd); return;
            case "plugin:event|listen": return ++sequence;
            default: return [];
          }
        },
      };
    });
    await page.goto(BASE_URL, { waitUntil: "networkidle" });
    await page.keyboard.press("Alt+s");
    const search = page.getByRole("searchbox", { name: "Filter settings", exact: true });
    await search.waitFor();
    await page.locator(".ai-settings .choice-row").first().waitFor({ state: "attached" });
    const sections = page.locator(".settings-page > details.settings-section");
    const titles = () => page.locator(".settings-page > details.settings-section:visible > summary .section-title").allTextContents();
    const totalSections = await sections.count();
    await search.fill("chat");
    await page.waitForFunction(() => [...document.querySelectorAll(".settings-page > details.settings-section")]
      .filter((el) => !el.hidden).length === 3);
    assert.deepEqual(await titles(), ["AI / Knowledge Engine", "Research", "Keyboard Shortcuts"]);
    const shortcuts = page.locator(".keymap-row:visible");
    assert.equal(await shortcuts.count(), 1);
    assert.match(await shortcuts.innerText(), /Go to Chat tab/);
    assert.match(await shortcuts.innerText(), /Alt-C/);
    assert.doesNotMatch(await shortcuts.innerText(), /Ctrl-Alt-C|Ctrl-Shift-C/);
    assert.equal(await page.locator(".keymap-category:visible").count(), 1);
    assert.equal(await page.locator(".research-settings .engine-item:visible").count(), 0);
    assert.equal(await page.locator(".research-settings .field-group:visible").count(), 0);
    assert.equal(await page.locator(".settings-page [hidden]:visible").count(), 0, "flex/grid styles must not override hidden");
    console.log("PASS chat shows only literal matches, not whole sections or unrelated keyboard categories");

    await search.fill("narrow padding");
    await page.waitForFunction(() => document.querySelector(".settings-page > details").hidden === false);
    assert.deepEqual(await titles(), ["General"]);
    assert.equal(await page.locator(".setting-row:visible").count(), 1);
    assert.match(await page.locator(".setting-row:visible").innerText(), /Narrow view side padding/);
    await search.fill("rust");
    await page.waitForFunction(() => [...document.querySelectorAll(".detail-row")].some((el) => !el.hidden));
    assert.deepEqual(await titles(), ["About"]);
    assert.equal(await page.locator(".detail-row:visible").count(), 1);
    await search.fill("theme");
    await page.locator(".theme-card").first().waitFor();
    assert.deepEqual(await titles(), ["Theme", "Keyboard Shortcuts"]);
    assert.ok(await page.locator(".theme-card:visible").count() > 4);
    await search.fill("no-settings-have-this-token");
    await page.locator(".settings-empty").waitFor();
    assert.deepEqual(await titles(), []);
    await search.fill("");
    await page.waitForFunction(() => !document.querySelector(".settings-page [hidden]"));
    assert.equal((await titles()).length, totalSections);
    assert.deepEqual(await page.evaluate(() => window.__settingsWrites), []);
    console.log("PASS multiword matching, section-title matches, no results, clearing and unchanged settings");

    await search.fill("chat");
    await page.evaluate(() => {
      const row = document.createElement("div");
      row.className = "setting-row";
      row.id = "async-setting-fixture";
      row.textContent = "Transport status";
      document.querySelector(".settings-page > details.settings-section").append(row);
    });
    await page.waitForFunction(() => document.getElementById("async-setting-fixture").hidden);
    await page.evaluate(() => { document.getElementById("async-setting-fixture").firstChild.data = "Chat transport status"; });
    await page.locator("#async-setting-fixture").waitFor();
    await page.evaluate(() => document.getElementById("async-setting-fixture").remove());
    await page.waitForFunction(() => document.querySelector(".settings-page > details.settings-section").hidden);
    console.log("PASS asynchronously updated labels re-filter without changing the query");

    await page.keyboard.press("Alt+c");
    await page.locator(".chat-view").waitFor();
    await page.waitForFunction(() => document.activeElement === document.querySelector(".chat-view textarea"));
    await page.keyboard.press("Alt+s");
    await search.waitFor();
    await search.focus();
    await page.keyboard.press("Control+Shift+c");
    assert.equal(await page.locator(".chat-view").count(), 0);
    await search.focus();
    await page.keyboard.press("Control+Alt+c");
    assert.equal(await page.locator(".chat-view").count(), 0);
    await search.focus();
    await page.keyboard.press("Alt+c");
    await page.locator(".chat-view").waitFor();
    console.log("PASS Alt+C opens Chat from Settings; old Ctrl combos do not");
    assert.deepEqual(errors, []);
  } finally {
    await browser.close();
  }
})().catch((error) => { console.error(error); process.exitCode = 1; });

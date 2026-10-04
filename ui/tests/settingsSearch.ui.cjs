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
            case "get_system_appearance": return { themeName: null, backgroundOpacity: 1, nativeTransparency: false };
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
    // Hidden ? help is searchable too: Asset Cleanup's help mentions chat history,
    // and Library explains when Chat may use Library excerpts.
    await page.waitForFunction(() => [...document.querySelectorAll(".settings-page > details.settings-section")]
      .filter((el) => !el.hidden).length === 5);
    assert.deepEqual(await titles(), ["Library", "AI / Knowledge Engine (optional)", "AI Research & Web Search", "Keyboard Shortcuts", "Asset Cleanup"]);
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
    // A whole stack name: plain "rust" also matches "trusted" in Library voice help.
    await search.fill("rust tauri");
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
    // Collapsed ? help keeps its searchable text in hidden containers by design.
    await page.waitForFunction(() => !document.querySelector(".settings-page [hidden]:not([data-settings-help-text])"));
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
    await page.locator(".chat-view textarea").fill("Keep this conversation draft");
    await page.keyboard.press("Alt+s");
    await search.waitFor();
    assert.equal(await page.locator(".chat-session[hidden][inert] .chat-view").count(), 1,
      "inactive Chat stays mounted to preserve its conversation");
    await search.focus();
    await page.keyboard.press("Control+Shift+c");
    assert.equal(await page.locator(".chat-view").isVisible(), false);
    assert.equal(await search.evaluate((el) => el === document.activeElement), true);
    await search.focus();
    await page.keyboard.press("Control+Alt+c");
    assert.equal(await page.locator(".chat-view").isVisible(), false);
    assert.equal(await search.evaluate((el) => el === document.activeElement), true);
    await search.focus();
    await page.keyboard.press("Alt+c");
    await page.locator(".chat-view").waitFor();
    assert.equal(await page.locator(".chat-view textarea").inputValue(), "Keep this conversation draft");
    console.log("PASS Alt+C opens Chat from Settings; old Ctrl combos do not");

    // Shortcuts can be changed in Settings, are saved on this device, and work at once.
    await page.keyboard.press("Alt+s");
    await search.waitFor();
    await search.fill("");
    const shortcutSection = page.locator(".settings-page > details.settings-section").filter({
      has: page.locator(".section-title", { hasText: "Keyboard Shortcuts" }),
    });
    await shortcutSection.locator("summary").click();
    const chatRow = shortcutSection.locator('[data-shortcut-id="go-chat"]');
    await chatRow.getByRole("button", { name: "Add a shortcut for Go to Chat tab" }).click();
    await page.keyboard.press("Control+Alt+h");
    await chatRow.locator(".keymap-binding", { hasText: "Ctrl-Alt-H" }).waitFor();
    assert.deepEqual(await page.evaluate(() => JSON.parse(localStorage.getItem("grafium.shortcuts.v1"))),
      { version: 1, bindings: { "go-chat": ["alt+c", "mod+alt+h"] } });
    await page.locator("body").click({ position: { x: 5, y: 5 } });
    await page.keyboard.press("Control+Alt+h");
    await page.locator(".chat-view").waitFor();
    console.log("PASS a recorded shortcut is saved on this device and works straight away");

    await page.keyboard.press("Alt+s");
    await search.waitFor();
    if (!(await shortcutSection.evaluate((el) => el.open))) await shortcutSection.locator("summary").click();
    await chatRow.getByRole("button", { name: "Remove Alt-C from Go to Chat tab" }).click();
    await search.focus();
    await page.keyboard.press("Alt+c");
    await page.waitForTimeout(300);
    assert.equal(await page.locator(".chat-view").isVisible(), false, "a removed shortcut stops working");
    await chatRow.getByRole("button", { name: "Add a shortcut for Go to Chat tab" }).click();
    await page.keyboard.press("Control+k");
    const conflict = chatRow.locator(".keymap-conflict");
    await conflict.waitFor();
    assert.match(await conflict.innerText(), /Ctrl-K is used by Global search/);
    await conflict.getByRole("button", { name: "Use here", exact: true }).click();
    await chatRow.locator(".keymap-binding", { hasText: "Ctrl-K" }).waitFor();
    assert.match(await shortcutSection.locator('[data-shortcut-id="search-global"]').innerText(), /None/);
    await chatRow.getByRole("button", { name: "Add a shortcut for Go to Chat tab" }).click();
    await page.keyboard.press("Enter");
    await chatRow.locator(".keymap-notice").waitFor();
    assert.match(await chatRow.locator(".keymap-notice").innerText(), /can't be changed/);
    const graphRow = shortcutSection.locator('[data-shortcut-id="go-graph"]');
    await graphRow.getByRole("button", { name: "Add a shortcut for Go to graph view" }).click();
    await page.keyboard.press("x");
    await page.keyboard.press("y");
    await graphRow.locator(".keymap-binding", { hasText: "x y" }).waitFor();
    console.log("PASS removing, moving a key in use, refusing typing keys and recording sequences");

    // Getting a default back never takes it from the action that has it now.
    const globalRow = shortcutSection.locator('[data-shortcut-id="search-global"]');
    await globalRow.getByRole("button", { name: "Reset Global search to its default" }).click();
    assert.match(await globalRow.locator(".keymap-notice").innerText(), /Ctrl-K stays with Go to Chat tab/);
    assert.match(await globalRow.innerText(), /None/);

    // The window takes Ctrl-Z and Ctrl-. before the page on Linux: while
    // recording, they go to the recorder instead of undoing or toggling.
    await page.evaluate(() => {
      window.__appUndoCount = 0;
      window.addEventListener("app-undo", () => { window.__appUndoCount += 1; });
    });
    await graphRow.getByRole("button", { name: "Add a shortcut for Go to graph view" }).click();
    await page.evaluate(() => window.__handleNativeUndo());
    assert.match(await graphRow.locator(".keymap-notice").innerText(), /undoes, so it can't be changed/);
    await graphRow.getByRole("button", { name: "Add a shortcut for Go to graph view" }).click();
    await page.keyboard.press("Control+z");
    assert.match(await graphRow.locator(".keymap-notice").innerText(), /undoes, so it can't be changed/);
    assert.equal(await page.evaluate(() => window.__appUndoCount), 0, "recording a shortcut must not undo anything");
    await chatRow.getByRole("button", { name: "Add a shortcut for Go to Chat tab" }).click();
    await page.evaluate(() => window.__toggleReferencePanel());
    assert.match(await chatRow.locator(".keymap-conflict").innerText(), /Ctrl-\. is used by Toggle right sidebar/);
    await chatRow.locator(".keymap-conflict").getByRole("button", { name: "Use here", exact: true }).click();
    await chatRow.locator(".keymap-binding", { hasText: "Ctrl-." }).waitFor();
    // Outside the recorder, Ctrl-. runs whatever it is bound to now.
    await search.focus();
    await page.evaluate(() => window.__toggleReferencePanel());
    await page.locator(".chat-view").waitFor();
    console.log("PASS native Ctrl-Z/Ctrl-. reach the recorder, follow Settings, and resets keep moved keys");

    // Commands without keys stay in the command palette.
    await page.keyboard.press("Control+Shift+p");
    await page.getByPlaceholder("Run a command…").fill("Global search");
    const paletteRow = page.locator(".command-palette-item", { hasText: "Global search" });
    await paletteRow.waitFor();
    assert.equal((await paletteRow.locator(".command-palette-keys").innerText()).trim(), "");
    await page.keyboard.press("Escape");
    await page.keyboard.press("Alt+s");
    await search.waitFor();
    if (!(await shortcutSection.evaluate((el) => el.open))) await shortcutSection.locator("summary").click();
    console.log("PASS the command palette lists actions whose keys were removed");

    await shortcutSection.getByRole("button", { name: "Reset all shortcuts", exact: true }).click();
    await shortcutSection.getByRole("button", { name: "Reset all", exact: true }).click();
    await chatRow.locator(".keymap-binding", { hasText: "Alt-C" }).waitFor();
    assert.equal(await chatRow.locator(".keymap-binding").count(), 1);
    assert.equal(await graphRow.locator(".keymap-binding", { hasText: "x y" }).count(), 0);
    assert.equal(await shortcutSection.locator(".keymap-category-title", { hasText: "Journal" }).count(), 1);
    console.log("PASS reset all restores the defaults; journal shortcuts have their own section");
    assert.deepEqual(errors, []);
  } finally {
    await browser.close();
  }
})().catch((error) => { console.error(error); process.exitCode = 1; });

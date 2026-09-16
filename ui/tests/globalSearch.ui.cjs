const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor, focus } = require("./keyboardSelection.ui.cjs");

const dialog = (page) => page.getByRole("dialog", { name: "Search your graph", exact: true });
const input = (page) => dialog(page).getByRole("textbox", { name: "Search pages and blocks", exact: true });
const search = (page, semantic = false) => dialog(page).getByRole("button", { name: semantic ? "AI Search" : "Search", exact: true });

async function openSearch(browser, options = {}) {
  const fixture = await openEditor(browser, {
    beforeNavigate: (page) => page.addInitScript((options) => {
      function install(internals) {
        const state = window.__selectionState;
        const original = internals.invoke;
        const result = { ...structuredClone(state.pages[0]), id: "search-source", title: "Synthetic astronomy",
          file_path: "pages/synthetic-astronomy.md" };
        const block = { ...structuredClone(state.blocks[0]), id: "search-passage", page_id: result.id,
          content: "Synthetic astronomy explains how nearby stars form clusters." };
        state.pages.push(result);
        state.blocks.push(block);
        const test = window.__globalSearch = { calls: [], hold: false, pending: [], fail: false };
        internals.invoke = async (cmd, args = {}) => {
          if (cmd === "get_layout_preferences") return { sidebarVisible: false, wideMode: true };
          if (cmd === "ai_health_check") {
            if (options.healthFailure) throw new Error("Synthetic model status is unavailable.");
            return {
              enabled: Boolean(options.semantic), llm_available: false,
              embedder_available: Boolean(options.semantic), vector_store_available: true,
              vector_count: options.semantic ? 1 : 0, mode: "local",
            };
          }
          if (["search_page_titles", "search_fts", "ai_search"].includes(cmd)) {
            test.calls.push({ cmd, args: structuredClone(args) });
            if (test.hold) await new Promise((resolve) => test.pending.push(resolve));
            if (test.fail) throw new Error("Synthetic search is unavailable.");
            if (args.query === "unmatched") return [];
            if (cmd === "search_page_titles") return [structuredClone(result)];
            if (cmd === "search_fts") return [structuredClone(block)];
            return [{
              chunk_id: "search-chunk", graph_id: "synthetic-graph", page_id: result.id,
              page_title: result.title, block_id: block.id, content: block.content, score: 0.92, metadata: {},
            }];
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
    }, options),
  });
  await focus(fixture.page, "b0");
  await fixture.page.keyboard.press("Control+k");
  await input(fixture.page).waitFor();
  return fixture;
}

const cases = [
  ["global exact search works without AI or either sidebar and navigates by source identity", {}, async (page) => {
    assert.equal(await input(page).evaluate((node) => node === document.activeElement), true);
    assert.equal(await page.locator(".reference-panel").count(), 0);
    await input(page).fill("astronomy");
    await search(page).click();
    const result = dialog(page).getByRole("button").filter({ hasText: "Synthetic astronomy" }).first();
    await result.waitFor();
    assert.equal(await search(page, true).isDisabled(), true);
    assert.equal(await page.evaluate(() => window.__globalSearch.calls.some(({ cmd }) => cmd === "ai_search")), false);
    await result.click();
    await page.locator('[data-block-id="search-passage"]').first().waitFor();
    assert.equal(await dialog(page).count(), 0);
    assert.equal(await page.evaluate(() => window.__selectionState.calls.some(({ cmd }) => cmd === "create_page")), false);
  }],
  ["semantic search uses embeddings without requiring a generation model", { semantic: true }, async (page) => {
    await input(page).fill("star formation");
    await search(page, true).click();
    await dialog(page).getByRole("button").filter({ hasText: "Synthetic astronomy" }).first().waitFor();
    const calls = await page.evaluate(() => window.__globalSearch.calls);
    assert.equal(calls.filter(({ cmd }) => cmd === "ai_search").length, 1);
    assert.equal(calls.find(({ cmd }) => cmd === "ai_search").args.query, "star formation");
    assert.equal(await page.evaluate(() => window.__selectionState.calls.some(({ cmd }) =>
      ["assistant_chat", "ai_ask_stream", "research_deep"].includes(cmd))), false);
  }],
  ["global search owns Escape and prevents right-panel shortcuts leaking behind its dialog", {}, async (page) => {
    await page.keyboard.press("Control+Shift+b");
    assert.equal(await page.locator(".reference-panel").count(), 0);
    assert.equal(await dialog(page).isVisible(), true);
    await page.keyboard.press("Escape");
    await dialog(page).waitFor({ state: "detached" });
    await page.keyboard.press("Control+k");
    assert.equal(await input(page).evaluate((node) => node === document.activeElement), true);
  }],
  ["search from the command palette opens after the palette has closed", {}, async (page) => {
    await page.keyboard.press("Escape");
    await page.keyboard.press("Control+Shift+p");
    await page.getByPlaceholder("Run a command…").fill("Global search");
    await page.locator(".command-palette").getByRole("button").first().click();
    await input(page).waitFor();
    assert.equal(await page.getByPlaceholder("Run a command…").count(), 0);
  }],
  ["failed searches report the error and can be retried", {}, async (page) => {
    await page.evaluate(() => { window.__globalSearch.fail = true; });
    await input(page).fill("astronomy");
    await search(page).click();
    await dialog(page).getByRole("alert").filter({ hasText: "Synthetic search is unavailable." }).waitFor();
    await page.evaluate(() => { window.__globalSearch.fail = false; });
    await search(page).click();
    await dialog(page).getByRole("button").filter({ hasText: "Synthetic astronomy" }).first().waitFor();
    assert.equal(await dialog(page).getByRole("alert").count(), 0);
  }],
  ["editing a query discards its older pending search results", {}, async (page) => {
    await page.evaluate(() => { window.__globalSearch.hold = true; });
    await input(page).fill("astronomy");
    await page.waitForFunction(() => window.__globalSearch.pending.length > 0);
    await input(page).fill("unmatched");
    await page.evaluate(() => {
      window.__globalSearch.hold = false;
      window.__globalSearch.pending.splice(0).forEach((resolve) => resolve());
    });
    await dialog(page).getByText(/No matches/i).waitFor();
    assert.equal(await dialog(page).getByRole("button").filter({ hasText: "Synthetic astronomy" }).count(), 0);
  }],
  ["model status failures are explicit without disabling ordinary search", { healthFailure: true }, async (page) => {
    await dialog(page).getByRole("alert").filter({ hasText: "Synthetic model status is unavailable." }).waitFor();
    await input(page).fill("astronomy");
    await dialog(page).getByRole("button").filter({ hasText: "Synthetic astronomy" }).first().waitFor();
    assert.equal(await search(page, true).isDisabled(), true);
  }],
  ...[false, true].map((semantic) => [
    `${semantic ? "semantic" : "exact block"} result navigation retains source identity after a rename`, { semantic }, async (page) => {
      await input(page).fill("astronomy");
      if (semantic) await search(page, true).click();
      const result = dialog(page).getByRole("button").filter({ hasText: "Synthetic astronomy" }).last();
      await result.waitFor();
      if (semantic) await dialog(page).getByRole("heading", { name: "AI semantic results", exact: true }).waitFor();
      await page.evaluate(() => { window.__selectionState.pages.find(({ id }) => id === "search-source").title = "Renamed source"; });
      await result.click();
      await page.locator('[data-block-id="search-passage"]').first().waitFor();
      assert.equal(await page.evaluate(() => window.__selectionState.calls.some(({ cmd }) => cmd === "create_page")), false);
      assert.equal(await page.evaluate(() => window.__selectionState.calls.filter(({ cmd }) => cmd === "get_page").at(-1).args.id), "search-source");
    },
  ]),
];

if (require.main === module) (async () => {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  let failed = 0;
  try {
    for (const [name, options, run] of cases) {
      let fixture;
      try {
        fixture = await openSearch(browser, options);
        await run(fixture.page);
        assert.deepEqual(fixture.errors, []);
        console.log(`PASS ${name}`);
      } catch (error) {
        failed++;
        console.error(`FAIL ${name}\n${error.stack ?? error}`);
      } finally { await fixture?.page.close(); }
    }
  } finally { await browser.close(); }
  if (failed) process.exitCode = 1;
})().catch((error) => { console.error(error); process.exitCode = 1; });

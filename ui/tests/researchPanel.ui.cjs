const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor, focus, frames, row } = require("./keyboardSelection.ui.cjs");

async function installResearchFixture(page, options) {
  await page.addInitScript((options) => {
    function install(internals) {
      const state = window.__selectionState;
      const original = internals.invoke;
      state.pages.push({
        ...structuredClone(state.pages[0]), id: "second-source", title: "Second research source",
        file_path: "pages/second-source.md",
      });
      state.blocks.push({ ...structuredClone(state.blocks[0]), id: "second-block", page_id: "second-source" });
      const research = window.__researchFixture = { hold: false, pending: [], calls: [] };
      internals.invoke = async (cmd, args = {}) => {
        if (["get_graph_info", "ai_health_check", "ai_generate_references", "ai_insert_page_summary"].includes(cmd)) {
          research.calls.push({ cmd, args: structuredClone(args), content: state.blocks.find((b) => b.id === "b0").content });
        }
        if (cmd === "get_graph_info") return { name: "Synthetic research", path: "/synthetic/research" };
        if (cmd === "ai_health_check") return {
          enabled: options.enabled, llm_available: options.enabled, embedder_available: false, vector_count: 0,
        };
        if (cmd === "ai_generate_references") {
          if (research.hold) await new Promise((resolve) => research.pending.push(resolve));
          return {
            page_id: args.pageId, generated_at: Date.now(), content_hash: "synthetic", reference_count: 0, references: [],
            summary: { title_answer: null, topics: [{
              topic: "Sleep", summary: "A synthetic summary about sleep quality.", tags: [{ term: "sleep_quality" }],
            }] },
          };
        }
        if (cmd === "ai_insert_page_summary") throw new Error("This fixture must not insert into the wrong source.");
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

async function openResearch(browser, options = {}) {
  const fixture = await openEditor(browser, {
    beforeNavigate: (page) => installResearchFixture(page, { enabled: true, ...options }),
  });
  await focus(fixture.page, "b0");
  await fixture.page.evaluate(() => window.dispatchEvent(new CustomEvent("toggle-reference-panel")));
  await fixture.page.locator(".reference-panel summary").filter({ hasText: "Page / selection tools" }).click();
  return fixture;
}

const button = (page, name) => page.locator(".reference-panel").getByRole("button", { name, exact: true });
const cases = [
  ["Chat tools expose exact link discovery without an AI model", { enabled: false }, async (page) => {
    assert.equal(await button(page, "Exact page links").isEnabled(), true);
    assert.equal(await button(page, "Summarize this Page").isEnabled(), false);
    await button(page, "Exact page links").click();
    await page.waitForFunction(() => window.__selectionState.calls.some((c) => c.cmd === "discover_link_candidates"));
    assert.equal(await page.evaluate(() => window.__selectionState.calls.some((c) => c.cmd === "ai_create_concept_edges")), false);
  }],
  ["Chat tools pin a delayed summary to its source page", {}, async (page) => {
    await page.evaluate(() => { window.__researchFixture.hold = true; });
    await button(page, "Summarize this Page").click();
    await page.waitForFunction(() => window.__researchFixture.pending.length === 1);
    await page.evaluate(() => window.dispatchEvent(new CustomEvent("navigate-page", { detail: "Second research source" })));
    await row(page, "second-block").waitFor();
    await page.evaluate(() => window.__researchFixture.pending.splice(0).forEach((resolve) => resolve()));
    await page.getByRole("region", { name: "Page summary", exact: true }).waitFor();
    assert.match(await page.getByRole("region", { name: "Page summary", exact: true }).getByRole("heading").first().innerText(), /Keyboard selection/i);
    assert.equal(await button(page, "Insert into page").isEnabled(), false);
    assert.equal(await page.evaluate(() => window.__researchFixture.calls.some((c) => c.cmd === "ai_insert_page_summary")), false);
  }],
  ["Chat tools wait for draft persistence before summarizing", {}, async (page) => {
    await focus(page, "b0");
    await page.evaluate(() => { window.__selectionState.holdUpdate = true; });
    await page.keyboard.press("End");
    await page.keyboard.type(" fresh research draft");
    await button(page, "Summarize this Page").click();
    await page.waitForFunction(() => window.__selectionState.updateWaiters.length > 0);
    assert.equal(await page.evaluate(() => window.__researchFixture.calls.some((c) => c.cmd === "ai_generate_references")), false);
    await page.evaluate(() => {
      window.__selectionState.holdUpdate = false;
      window.__selectionState.updateWaiters.splice(0).forEach((resolve) => resolve());
    });
    await page.getByRole("region", { name: "Page summary", exact: true }).waitFor();
    assert.match(await page.evaluate(() => window.__researchFixture.calls.find((c) => c.cmd === "ai_generate_references").content), /fresh research draft/);
    await frames(page);
  }],
];

if (require.main === module) (async () => {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  let failed = 0;
  try {
    for (const [name, options, run] of cases) {
      let fixture;
      try {
        fixture = await openResearch(browser, options);
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

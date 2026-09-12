// Real editor/clipboard events with synthetic notes and controllable persistence.
const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const BASE_URL = process.env.UI_TEST_URL ?? "http://localhost:5199/";
const PASTE = [
  "list of motorcycles", "**Honda CRF300L**", "✅", "✅✅✅", "❌", "$5,599",
  "**Best for Deep Woods**", "**Kawasaki Versys 650**", "✅", "✅✅", "✅✅",
  "$6,000 (Used)", "**Best for Cross-Country**", "**Royal Enfield**", "❌ (India)",
  "✅✅✅", "❌", "$5,999", "**Best for Value (But Indian)**", "**Kawasaki Eliminator**",
  "✅", "❌", "✅✅✅", "$6,799", "**Best for City (No Off-Road)**",
];

async function openEditor(browser) {
    const page = await browser.newPage({ viewport: { width: 1400, height: 1000 } });
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.addInitScript(() => {
      const note = {
        id: "paste-page", title: "Paste regression", properties: {}, is_journal: false,
        file_path: "pages/Paste regression.md", created_at: 0, updated_at: 0,
      };
      const otherNote = { ...note, id: "other-page", title: "Other page" };
      const makeBlock = (id, parent, order, content) => ({
        id, page_id: note.id, parent_id: parent, order_index: order, content,
        block_type: "text", properties: {}, created_at: 0, updated_at: 0,
      });
      let sequence = 0;
      window.__pasteState = {
        blocks: [makeBlock("anchor", null, 0, ""), { ...makeBlock("other-block", null, 0, "Other page text"), page_id: otherNote.id }],
        calls: [], holdReorder: false, failReorder: false, holdCreate: false, failCreate: false,
        holdUpdate: false, failUpdate: false, reorderFinished: 0,
      };
      window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
      window.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
        plugins: {}, transformCallback() { return ++sequence; }, unregisterCallback() {},
        invoke: async (cmd, args = {}) => {
          const state = window.__pasteState;
          state.calls.push({ cmd, args });
          switch (cmd) {
            case "get_page":
              if (args.id === note.id || args.title === note.title) return structuredClone(note);
              if (args.id === otherNote.id || args.title === otherNote.title) return structuredClone(otherNote);
              throw new Error("Page not found");
            case "get_graph_info": return { name: "Paste test", path: "/tmp/paste-test" };
            case "get_app_theme": return "dark";
            case "list_blocks": return structuredClone(state.blocks.filter((block) => block.page_id === args.pageId));
            case "create_block": {
              const block = makeBlock(`created-${++sequence}`, args.parentId ?? null, args.orderIndex, args.content);
              state.blocks.push(block);
              return structuredClone(block);
            }
            case "update_block": {
              if (state.holdUpdate) {
                state.holdUpdate = false;
                await new Promise((resolve) => { window.__releaseUpdate = resolve; });
              }
              if (state.failUpdate) throw new Error("Simulated anchor save failure");
              const block = state.blocks.find((item) => item.id === args.id);
              if (!block) throw new Error("Block not found");
              block.content = args.content;
              return;
            }
            case "create_blocks": {
              if (state.holdCreate) await new Promise((resolve) => { window.__releaseCreate = resolve; });
              if (state.failCreate) throw new Error("Simulated batch save failure");
              const created = [];
              for (const item of args.blocks) {
                const parent = item.parentIndex === undefined ? item.parentId ?? null : created[item.parentIndex].id;
                const block = makeBlock(item.id ?? `pasted-${++sequence}`, parent, item.orderIndex, item.content);
                created.push(block);
              }
              state.blocks.push(...created);
              return structuredClone(created);
            }
            case "delete_blocks": {
              const removed = state.blocks.filter((block) => args.ids.includes(block.id));
              state.blocks = state.blocks.filter((block) => !args.ids.includes(block.id));
              return structuredClone(removed);
            }
            case "reorder_blocks":
              if (state.holdReorder) await new Promise((resolve) => { window.__releaseReorder = resolve; });
              if (state.failReorder) throw new Error("Simulated ordering failure");
              args.blockIds.forEach((id, order) => {
                const block = state.blocks.find((item) => item.id === id);
                if (block) block.order_index = order;
              });
              state.reorderFinished++;
              return;
            case "plugin:event|listen": return ++sequence;
            default: return [];
          }
        },
      };
    });
    await page.goto(BASE_URL, { waitUntil: "networkidle" });
    await page.evaluate(() => window.dispatchEvent(new CustomEvent("navigate-page", { detail: "Paste regression" })));
    await page.locator('[data-block-id="anchor"] .block-content').click();
    await page.locator('[data-block-id="anchor"] .cm-content').fill("[[motorcycle]]");
    await page.keyboard.press("Enter");
    const child = page.locator('[data-block-id^="created-"] .cm-content');
    await child.waitFor();
    return { page, child, errors };
}

async function pasteInto(child, lines = PASTE, html = true) {
    await child.evaluate((element, { lines, html }) => {
      const data = new DataTransfer();
      data.setData("text/plain", lines.join("\n"));
      if (html) data.setData("text/html", lines.map((line) => `<p>${line.replace(/\*\*(.*?)\*\*/g, "<strong>$1</strong>")}</p>`).join(""));
      element.dispatchEvent(new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }));
    }, { lines, html });
}

async function finishCase({ page, errors }, message) {
  assert.deepEqual(errors, []);
  await page.close();
  console.log(`PASS ${message}`);
}

(async () => {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  try {
    for (const failReorder of [false, true]) {
      const fixture = await openEditor(browser);
      const { page, child } = fixture;
      await page.evaluate((failReorder) => Object.assign(window.__pasteState, { holdReorder: true, failReorder }), failReorder);
      await pasteInto(child);
      await page.waitForFunction(() => typeof window.__releaseReorder === "function");
      assert.ok(await page.evaluate(() => window.__pasteState.blocks.some((block) => block.content.includes("Kawasaki Eliminator"))));
      await page.getByText("Kawasaki Eliminator", { exact: true }).waitFor({ timeout: 1000 });
      await page.locator('[data-block-id^="pasted-"] .cm-content').filter({ hasText: "Best for City (No Off-Road)" }).waitFor();
      assert.equal(await page.evaluate(() => {
        const view = window.__activeEditorView;
        return view.state.selection.main.head === view.state.doc.length;
      }), true);
      assert.equal(await page.locator(".paste-preview").count(), 0);
      await page.evaluate(() => { window.__pasteState.holdReorder = false; window.__releaseReorder(); });
      if (failReorder) {
        await page.getByText(/Pasted blocks were saved, but their order could not be confirmed/).waitFor();
      } else {
        await page.waitForFunction(() => window.__pasteState.reorderFinished > 0);
      }
      await page.keyboard.press("Escape");
      const expected = await page.evaluate(() => window.__pasteState.blocks
        .filter((block) => block.parent_id === "anchor").sort((a, b) => a.order_index - b.order_index)
        .map((block) => block.content));
      assert.deepEqual(expected, PASTE);
      await page.evaluate(() => window.dispatchEvent(new CustomEvent("page-content-reload-blocks", { detail: { pageId: "paste-page" } })));
      await page.getByText("Kawasaki Eliminator", { exact: true }).waitFor();
      await page.getByText("Best for City (No Off-Road)", { exact: true }).waitFor();
      if (!failReorder) {
        await page.evaluate(() => window.dispatchEvent(new Event("app-undo")));
        await page.waitForFunction(() => !window.__pasteState.blocks.some((block) => block.id.startsWith("pasted-")));
        await page.evaluate(() => window.dispatchEvent(new Event("app-redo")));
        await page.getByText("Kawasaki Eliminator", { exact: true }).waitFor();
      }
      await finishCase(fixture, `saved paste stays visible through ${failReorder ? "failed" : "delayed"} ordering and reload`);
    }

    for (const phase of ["Update", "Create"]) {
      const fixture = await openEditor(browser);
      const { page, child } = fixture;
      await page.evaluate((phase) => { window.__pasteState[`hold${phase}`] = true; }, phase);
      await pasteInto(child);
      await page.waitForFunction((phase) => typeof window[`__release${phase}`] === "function", phase);
      assert.match(await page.locator(".paste-preview").innerText(), /Kawasaki Eliminator/);
      await page.getByText("Saving pasted blocks...", { exact: true }).waitFor();
      if (phase === "Create") await child.pressSequentially(" + my note");
      await page.evaluate((phase) => {
        window.__pasteState[`hold${phase}`] = false;
        window[`__release${phase}`]();
      }, phase);
      await page.getByText("Kawasaki Eliminator", { exact: true }).waitFor();
      if (phase === "Create") {
        await page.waitForFunction(() => window.__pasteState.blocks.some((block) => block.content === "list of motorcycles + my note"));
      }
      await finishCase(fixture, `complete clipboard preview is visible during delayed ${phase.toLowerCase()}`);
    }

    for (const phase of ["Update", "Create"]) {
      const fixture = await openEditor(browser);
      const { page, child } = fixture;
      await page.evaluate((phase) => { window.__pasteState[`fail${phase}`] = true; }, phase);
      await pasteInto(child);
      await page.getByRole("textbox", { name: "Unfinished pasted text" }).waitFor();
      assert.match(await page.getByRole("textbox", { name: "Unfinished pasted text" }).inputValue(), /Kawasaki Eliminator/);
      assert.equal(await page.evaluate(() => window.__pasteState.blocks.filter((block) => block.id.startsWith("pasted-")).length), 0);
      await finishCase(fixture, `failed ${phase.toLowerCase()} keeps recoverable pasted text and shows an error`);
    }

    {
      const fixture = await openEditor(browser);
      const { page, child } = fixture;
      await pasteInto(child, ["Topic", "", "- Item A", "    - Item A child", "- Item B"], false);
      await page.locator('[data-block-id^="pasted-"] .cm-content').filter({ hasText: "Item B" }).waitFor();
      const hierarchy = await page.evaluate(() => {
        const blocks = window.__pasteState.blocks;
        return blocks.filter((block) => block.content.startsWith("Item"))
          .map((block) => [block.content, blocks.find((parent) => parent.id === block.parent_id)?.content]);
      });
      assert.deepEqual(hierarchy, [["Item A", "Topic"], ["Item A child", "Item A"], ["Item B", "Topic"]]);
      await finishCase(fixture, "nested list paste retains its parent and child block hierarchy");
    }

    {
      const fixture = await openEditor(browser);
      const { page, child } = fixture;
      const lines = Array.from({ length: 510 }, (_, index) => `Pasted paragraph ${index}`);
      await pasteInto(child, lines);
      await page.locator('[data-block-id^="pasted-"] .cm-content').filter({ hasText: "Pasted paragraph 509" }).waitFor();
      assert.ok(await page.locator(".block-item").count() < lines.length);
      assert.equal(await page.evaluate(() => window.__pasteState.blocks.filter((block) => block.parent_id === "anchor").length), lines.length);
      await finishCase(fixture, "large paste reveals and focuses the final block beyond the virtualized window");
    }

    {
      const fixture = await openEditor(browser);
      const { page, child } = fixture;
      await pasteInto(child, PASTE, false);
      assert.equal(await child.innerText(), PASTE.join("\n"));
      await page.keyboard.press("Escape");
      await page.waitForFunction((text) => window.__pasteState.blocks.some((block) => block.content === text), PASTE.join("\n"));
      assert.equal(await page.locator(".paste-preview").count(), 0);
      await finishCase(fixture, "plain multiline clipboard content remains intact without artificial block splitting");
    }

    {
      const fixture = await openEditor(browser);
      const { page, child } = fixture;
      await page.evaluate(() => { window.__pasteState.holdUpdate = true; });
      await pasteInto(child);
      await page.waitForFunction(() => typeof window.__releaseUpdate === "function");
      await page.evaluate(() => window.dispatchEvent(new CustomEvent("navigate-page", { detail: "Other page" })));
      await page.getByText("Other page text", { exact: true }).waitFor();
      await page.evaluate(() => window.__releaseUpdate());
      await page.waitForFunction(() => window.__pasteState.reorderFinished > 0);
      assert.equal(await page.locator('[data-block-id^="pasted-"]').count(), 0);
      assert.equal(await page.evaluate(() => window.__pasteState.calls.find((call) => call.cmd === "create_blocks").args.pageId), "paste-page");
      assert.equal(await page.getByText("Other page text", { exact: true }).isVisible(), true);
      await finishCase(fixture, "navigation during anchor save does not retarget or inject the paste into another page");
    }
  } finally {
    await browser.close();
  }
})().catch((error) => { console.error(error); process.exitCode = 1; });

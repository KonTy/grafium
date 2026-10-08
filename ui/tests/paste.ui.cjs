// Real editor/clipboard events with synthetic notes and controllable persistence.
const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor: openContinuousEditor, focusContinuous } = require("./keyboardSelection.ui.cjs");
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
            case "get_layout_preferences": return { sidebarVisible: true, wideMode: true };
            case "list_blocks": return structuredClone(state.blocks.filter((block) => block.page_id === args.pageId));
            case "insert_block": {
              // Mirrors the native insert: number the displayed siblings 0..n
              // around the new block's position.
              const siblings = state.blocks.filter((sibling) => sibling.page_id === args.pageId
                && sibling.parent_id === (args.parentId ?? null)).sort((a, b) => a.order_index - b.order_index);
              const at = Math.min(args.position, siblings.length);
              siblings.forEach((sibling, index) => { sibling.order_index = index < at ? index : index + 1; });
              const block = makeBlock(`created-${++sequence}`, args.parentId ?? null, at, args.content);
              state.blocks.push(block);
              return structuredClone(block);
            }
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
    const createdEditor = page.locator('[data-block-id^="created-"] .cm-content');
    await createdEditor.waitFor();
    const childId = await createdEditor.evaluate((element) => element.closest("[data-block-id]").dataset.blockId);
    const child = page.locator(`[data-block-id="${childId}"] .cm-content`);
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

async function renderedCode(page) {
  return page.locator(".code-block-wrapper .code-line").evaluateAll((lines) =>
    lines.map((line) => line.textContent).join("\n"));
}

async function expectSavedCode(fixture, content, code) {
  const { page } = fixture;
  assert.equal(await page.evaluate(() => window.__activeEditorView.state.doc.toString()), content);
  await page.keyboard.press("Escape");
  await page.waitForFunction((content) => window.__pasteState.blocks.some((block) => block.content === content), content);
  assert.deepEqual(await page.evaluate(() => window.__pasteState.blocks
    .filter((block) => block.parent_id === "anchor").map((block) => block.content)), [content]);
  await page.locator(".code-block-wrapper").waitFor();
  assert.equal(await renderedCode(page), code);
  assert.equal(await page.evaluate(() => window.__pasteState.calls.some((call) => call.cmd === "create_blocks")), false);
  await page.evaluate(() => window.dispatchEvent(new CustomEvent("page-content-reload-blocks", { detail: { pageId: "paste-page" } })));
  await page.locator(".code-block-wrapper").waitFor();
  assert.equal(await renderedCode(page), code);
}

(async () => {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  try {
    for (const { code, escapeFirst, ime } of [
      { code: "can we crate examples foder in workflows,", escapeFirst: false, ime: false },
      { code: "can we crate examples foder in workflows,", escapeFirst: true, ime: false },
      { code: "  first  \n\n\t- literal bullet\nid:: literal property\n ", escapeFirst: false, ime: false },
      { code: "  first  \n\n\t- literal bullet\nid:: literal property\n ", escapeFirst: true, ime: true },
    ]) {
      const fixture = await openContinuousEditor(browser, { journal: true, flatJournal: true });
      const { page } = fixture;
      const editor = page.locator(".cm-content:focus");
      await page.locator('[data-block-id="day-0-b0"] .block-content').click();
      await editor.fill("11:53");
      await page.keyboard.press("Enter");
      await page.waitForFunction(() => window.__activeEditorView?.hasFocus
        && window.__activeEditorView.state.doc.length === 0);
      const heading = "Figuring out comfyui for exercise videos exmples";
      await editor.fill(heading);
      await page.keyboard.press("Enter");
      await page.waitForFunction(() => window.__activeEditorView?.hasFocus
        && window.__activeEditorView.state.doc.length === 0);
      await page.keyboard.press("Tab");
      await page.waitForFunction(() => window.__activeEditorView?.hasFocus
        && window.__selectionState.blocks.find((block) => block.id ===
          document.activeElement.closest("[data-block-id]").dataset.blockId)?.parent_id !== "day-0-b0");
      const codeId = await editor.evaluate((element) => element.closest("[data-block-id]").dataset.blockId);
      await editor.pressSequentially("```");
      await pasteInto(editor, code.split("\n"), false);
      const fenced = `\`\`\`\n${code}\n\`\`\``;
      assert.equal(await page.evaluate(() => window.__activeEditorView.state.doc.toString()), fenced);
      if (escapeFirst) {
        await page.keyboard.press("Escape");
        await page.waitForFunction(({ codeId, fenced }) => window.__selectionState.blocks
          .find((block) => block.id === codeId)?.content === fenced, { codeId, fenced });
        await page.locator(`[data-block-id="${codeId}"] .code-block-wrapper`).waitFor();
        assert.equal(await renderedCode(page), code, "code renders correctly before creating a following block");
        await page.locator(`[data-block-id="${codeId}"] .block-content`).click();
      }
      await page.evaluate(() => {
        const view = window.__activeEditorView;
        view.dispatch({ selection: { anchor: view.state.doc.length }, scrollIntoView: true });
      });
      await page.keyboard.press("Enter");
      assert.equal(await page.evaluate(() => window.__activeEditorView.state.doc.toString()), `${fenced}\n`);
      if (ime) {
        await editor.evaluate((element) => element.dispatchEvent(new InputEvent("beforeinput", {
          inputType: "insertParagraph", bubbles: true, cancelable: true,
        })));
      } else {
        await page.keyboard.press("Enter");
      }
      await page.waitForFunction((codeId) => window.__activeEditorView?.hasFocus
        && document.activeElement.closest("[data-block-id]")?.dataset.blockId !== codeId, codeId);
      assert.equal(await page.evaluate((codeId) => window.__selectionState.blocks
        .find((block) => block.id === codeId).content, codeId), fenced,
      "creating a following block must not split the code at its first newline");
      await page.locator(`[data-block-id="${codeId}"] .code-block-wrapper`).waitFor();
      assert.equal(await renderedCode(page), code);
      await editor.fill("Continue writing below the code");
      await page.keyboard.press("Escape");
      await page.waitForFunction(() => window.__selectionState.blocks
        .some((block) => block.content === "Continue writing below the code"));
      const structure = await page.evaluate(({ codeId, heading }) => {
        const blocks = window.__selectionState.blocks;
        const codeBlock = blocks.find((block) => block.id === codeId);
        return {
          parent: blocks.find((block) => block.id === codeBlock.parent_id)?.content,
          siblings: blocks.filter((block) => block.parent_id === codeBlock.parent_id)
            .sort((a, b) => a.order_index - b.order_index).map((block) => block.content),
          headingCount: blocks.filter((block) => block.content === heading).length,
        };
      }, { codeId, heading });
      assert.deepEqual(structure, {
        parent: heading, siblings: [fenced, "Continue writing below the code"], headingCount: 1,
      });
      await page.evaluate(() => window.dispatchEvent(new CustomEvent("page-content-reload-blocks",
        { detail: { pageId: "day-0" } })));
      await page.locator(".code-block-wrapper").waitFor();
      assert.equal(await renderedCode(page), code, "saved/reloaded code keeps all literal newlines and spaces");
      await finishCase(fixture, `closing-fence Enter twice preserves nested journal code${escapeFirst ? " after Escape" : ""}${ime ? " via native beforeinput" : ""}`);
    }

    {
      const fixture = await openEditor(browser);
      const { page, child } = fixture;
      await child.fill("Figuring out comfyui for exercise videos examples");
      await page.keyboard.press("Shift+Enter");
      await child.pressSequentially("```");
      const code = "  can we create examples folder in workflows,\n\n\t- keep this literal  \nid:: literal code";
      await pasteInto(child, code.split("\n"));
      assert.equal(await page.evaluate(() => window.__activeEditorView.state.doc.toString()),
        `Figuring out comfyui for exercise videos examples\n\`\`\`\n${code}\n\`\`\``);
      await page.keyboard.press("Enter");
      await page.evaluate(() => {
        const view = window.__activeEditorView;
        const anchor = view.state.doc.toString().lastIndexOf("```");
        view.dispatch({ selection: { anchor, head: anchor + 3 } });
      });
      await child.pressSequentially("```");
      await expectSavedCode(fixture,
        `Figuring out comfyui for exercise videos examples\n\`\`\`\n${code}\n\n\`\`\``, `${code}\n`);
      await finishCase(fixture, "typed fence retains pasted code and whitespace in one saved/rendered block");
    }

    for (const kind of ["plain", "rich", "markdown", "html-only"]) {
      const fixture = await openEditor(browser);
      const { page, child } = fixture;
      const code = "\n  first <tag>  \n\n\t- literal\nTODO not a task\nid:: not metadata\n ";
      await pasteInto(child, ["~~~text", "", "~~~"], false);
      await page.evaluate(() => window.__activeEditorView.dispatch({ selection: { anchor: 8 } }));
      await child.evaluate((element, { kind, code }) => {
        const data = new DataTransfer();
        if (kind === "markdown") data.setData("text/markdown", code);
        else if (kind !== "html-only") data.setData("text/plain", code.replace(/\n/g, "\r\n"));
        if (kind === "rich" || kind === "html-only") {
          data.setData("text/html", `<pre><code>${code.replace(/</g, "&lt;").replace(/>/g, "&gt;")}</code></pre>`);
        }
        element.dispatchEvent(new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }));
      }, { kind, code });
      await expectSavedCode(fixture, `~~~text\n${code}\n~~~`, code);
      await finishCase(fixture, `${kind} paste in a tilde fence keeps literal whitespace, Markdown and properties`);
    }

    for (const html of [false, true]) {
      const fixture = await openEditor(browser);
      const code = "  first  \n\n```\n- literal\nid:: literal\n~~~~ not a closer\n![Example](https://example.com/image.png)\nlast";
      const snippet = `\`\`\`\`md\n${code}\n\`\`\`\``;
      await pasteInto(fixture.child, snippet.split("\n"), html);
      await expectSavedCode(fixture, snippet, code);
      await finishCase(fixture, `whole fenced source paste with ${html ? "rich" : "plain"} clipboard keeps one code block`);
    }

    {
      const fixture = await openEditor(browser);
      await fixture.child.evaluate((element) => {
        const data = new DataTransfer();
        data.setData("text/html", '<pre><code class="language-md"><span class="code-line">  first  </span><span class="code-line">```</span><span class="code-line"></span><span class="code-line">- literal</span></code></pre>');
        element.dispatchEvent(new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }));
      });
      await expectSavedCode(fixture, "````md\n  first  \n```\n\n- literal\n````", "  first  \n```\n\n- literal");
      await finishCase(fixture, "HTML-only rendered code paste retains lines and literal fence markers");
    }

    {
      const fixture = await openEditor(browser);
      await pasteInto(fixture.child, ["- ```", "    indented  ", "  - literal", "  ```", "- After"], false);
      await fixture.page.locator('[data-block-id^="pasted-"] .cm-content').filter({ hasText: "After" }).waitFor();
      await fixture.page.keyboard.press("Escape");
      assert.deepEqual(await fixture.page.evaluate(() => window.__pasteState.blocks
        .filter((block) => block.parent_id === "anchor").sort((a, b) => a.order_index - b.order_index)
        .map((block) => block.content)), ["```\n  indented  \n- literal\n```", "After"]);
      await fixture.page.locator(".code-block-wrapper").waitFor();
      assert.equal(await renderedCode(fixture.page), "  indented  \n- literal");
      await finishCase(fixture, "fenced outline paste keeps code together and the following outline block separate");
    }

    for (const html of [false, true]) {
      const fixture = await openContinuousEditor(browser, {
        unifiedPage: true, componentHarness: true, blockContents: ["Heading\n```\n\n```", "Unrelated block"],
      });
      const { page } = fixture;
      await focusContinuous(page, "selection-page", "b0");
      await page.evaluate(async () => {
        const view = window.__activeEditorView;
        const { parsePageSourceMap } = await import("/src/lib/pageSourceMap.ts");
        const block = parsePageSourceMap(view.state.doc.toString()).blocks[0];
        view.dispatch({ selection: { anchor: block.contentSegments[2].from } });
      });
      const code = "  source code  \n\n- literal bullet\nid:: literal property\n\tlast";
      await pasteInto(page.locator(".unified-page-editor .cm-content"), code.split("\n"), html);
      await page.getByRole("button", { name: "Save source", exact: true }).click();
      await page.waitForFunction((content) => window.__selectionState.blocks.some((block) => block.content === content),
        `Heading\n\`\`\`\n${code}\n\`\`\``);
      assert.deepEqual(await page.evaluate(() => window.__selectionState.blocks
        .filter((block) => block.page_id === "selection-page").map((block) => [block.id, block.parent_id, block.content])),
        [["b0", null, `Heading\n\`\`\`\n${code}\n\`\`\``], ["b1", null, "Unrelated block"]]);
      await page.locator(".code-block-wrapper").waitFor();
      assert.equal(await renderedCode(page), code);
      await finishCase(fixture, `retained continuous editor preserves stored code and siblings on ${html ? "rich" : "plain"} paste`);
    }

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
      assert.deepEqual(await page.evaluate(() => window.__pasteState.blocks
        .filter((block) => block.parent_id === "anchor").map((block) => block.content)), [PASTE.join("\n")]);
      assert.equal(await page.evaluate(() => window.__pasteState.calls.some((call) => call.cmd === "create_blocks")), false);
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

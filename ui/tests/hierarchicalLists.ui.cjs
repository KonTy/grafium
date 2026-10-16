const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor, focus } = require("./keyboardSelection.ui.cjs");

const active = (page) => page.locator(".cm-content:focus");
const doc = (page) => page.evaluate(() => window.__activeEditorView.state.doc.toString());
const blocks = (page) => page.evaluate(() => window.__selectionState.blocks
  .filter((block) => block.page_id === "selection-page")
  .map(({ id, content, parent_id }) => ({ id, content, parent_id })));
const id = (page) => active(page).evaluate((element) => element.closest("[data-block-id]").dataset.blockId);
async function moved(page, blockId, parent, content) {
  await page.waitForFunction(({ blockId, parent, content }) => {
    const block = window.__selectionState.blocks.find((block) => block.id === blockId);
    return block.parent_id === parent && block.content === content
      && window.__activeEditorView?.hasFocus && window.__activeEditorView.state.doc.toString() === content;
  }, { blockId, parent, content });
}
async function enter(page, previous, content) {
  await page.keyboard.press("Enter");
  await page.waitForFunction(({ previous, content }) => window.__activeEditorView?.hasFocus
    && document.activeElement.closest("[data-block-id]")?.dataset.blockId !== previous
    && window.__activeEditorView.state.doc.toString() === content, { previous, content });
  return id(page);
}
async function escape(page) {
  await page.keyboard.press("Escape");
  await page.waitForFunction(() => !document.querySelector(".block-item .cm-content"));
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
}
async function clearGuide(page, blockId) {
  const value = await page.locator(`.block-item[data-block-id="${blockId}"]`).evaluate((row) => {
    const elbow = row.querySelector(".indent-guide-elbow")?.getBoundingClientRect();
    const text = row.querySelector(".block-content").getBoundingClientRect();
    return { gap: elbow && text.left - elbow.right, bullets: row.querySelectorAll(".bullet, .bullet-default").length };
  });
  assert.ok(value.gap >= 3, JSON.stringify(value));
  assert.equal(value.bullets, 0);
}

(async () => {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  try {
    for (const delimiter of [".", ")"]) {
      const { page, errors } = await openEditor(browser, {
        blockContents: ["# MINISFORUM", `1${delimiter} First`, `2${delimiter} Second`, `3${delimiter}`],
        beforeNavigate: (page) => page.addInitScript(() => localStorage.setItem("grafium.pageContent.showBlockGuides", "true")),
      });
      try {
        await page.evaluate(() => {
          for (const block of window.__selectionState.blocks.filter((block) => ["b1", "b2", "b3"].includes(block.id))) {
            block.parent_id = "b0";
          }
          window.dispatchEvent(new CustomEvent("page-content-reload-blocks", { detail: { pageId: "selection-page" } }));
        });
        await focus(page, "b3");
        const left = () => page.locator('[data-block-id="b2"] .block-content').evaluate((el) => el.getBoundingClientRect().left);
        const before = await left();
        await page.keyboard.press("Tab");
        await moved(page, "b3", "b2", `2.1${delimiter} `);
        assert.equal(await left(), before, "2 must not shift when it gains a collapse control");
        assert.deepEqual((await blocks(page)).find((block) => block.id === "b2"),
          { id: "b2", content: `2${delimiter} Second`, parent_id: "b0" });
        assert.equal(await page.evaluate(() => window.__activeEditorView.state.selection.main.head), `2.1${delimiter} `.length);
        await clearGuide(page, "b3");
        await active(page).pressSequentially("Child one");
        const second = await enter(page, "b3", `2.2${delimiter} `);
        await active(page).pressSequentially("Child two");
        const third = await enter(page, second, `2.3${delimiter} `);
        assert.equal((await blocks(page)).find((block) => block.id === third).parent_id, "b2");
        await page.keyboard.press("Tab");
        await moved(page, third, second, `2.2.1${delimiter} `);
        await active(page).pressSequentially("Grandchild");
        await page.keyboard.press("Shift+Tab");
        await moved(page, third, "b2", `2.3${delimiter} Grandchild`);
        await page.keyboard.press("Shift+Tab");
        await moved(page, third, "b0", `3${delimiter} Grandchild`);
        await escape(page);
        const expected = await blocks(page);
        for (const child of ["b3", second]) {
          await page.locator(`[data-block-id="${child}"] .ordered-list-label`).waitFor();
          assert.equal(await page.locator(`[data-block-id="${child}"] .bullet`).count(), 0);
        }
        await page.evaluate(() => window.dispatchEvent(new CustomEvent("page-content-reload-blocks", { detail: { pageId: "selection-page" } })));
        await page.locator('[data-block-id="b3"] .ordered-list-label').waitFor();
        assert.deepEqual(await blocks(page), expected);
        await focus(page, "b3");
        await clearGuide(page, "b3");
        await escape(page);
        await clearGuide(page, "b3");
        if (process.env.UI_TEST_SCREENSHOT) {
          await page.locator(".page-content").screenshot({ path: process.env.UI_TEST_SCREENSHOT });
        }
        await focus(page, second);
        const empty = await enter(page, second, `2.3${delimiter} `);
        await page.keyboard.press("Enter");
        assert.equal(await id(page), empty);
        assert.equal(await doc(page), "");
        await escape(page);
        await focus(page, second);
        const escaped = await enter(page, second, `2.3${delimiter} `);
        await escape(page);
        assert.equal((await blocks(page)).find((block) => block.id === escaped).content, "");
        assert.deepEqual(errors, []);
        console.log(`PASS ${delimiter} Tab only moves 3, stable 2 geometry, 2.1/2.2/2.3, deeper nesting, Shift+Tab, empty exits and reload`);
      } finally { await page.close(); }
    }
    {
      const { page, errors } = await openEditor(browser, {
        blockContents: ["1. First", "2. Parent", "2.4. Existing", "3. Alpha selected Omega", "1. Child", "Code stays literal"],
      });
      try {
        await page.evaluate(() => {
          const state = window.__selectionState;
          state.blocks.find((block) => block.id === "b2").parent_id = "b1";
          state.blocks.find((block) => block.id === "b4").parent_id = "b3";
          state.blocks.find((block) => block.id === "b5").parent_id = "b4";
          window.dispatchEvent(new CustomEvent("page-content-reload-blocks", { detail: { pageId: "selection-page" } }));
        });
        await focus(page, "b3");
        await page.evaluate(() => window.__activeEditorView.dispatch({ selection: { anchor: 9, head: 17 } }));
        await page.keyboard.press("Tab");
        await moved(page, "b3", "b1", "2.5. Alpha selected Omega");
        assert.deepEqual(await page.evaluate(() => window.__activeEditorView.state.selection.main.toJSON()), { anchor: 11, head: 19 });
        assert.equal(await page.evaluate(() => window.__activeEditorView.state.sliceDoc(
          window.__activeEditorView.state.selection.main.from, window.__activeEditorView.state.selection.main.to)), "selected");
        await page.keyboard.press("Shift+Tab");
        await moved(page, "b3", null, "3. Alpha selected Omega");
        assert.deepEqual(await page.evaluate(() => window.__activeEditorView.state.selection.main.toJSON()), { anchor: 9, head: 17 });
        await escape(page);
        const stored = await blocks(page);
        assert.deepEqual(stored.find((block) => block.id === "b2"), { id: "b2", content: "2.4. Existing", parent_id: "b1" });
        assert.deepEqual(stored.find((block) => block.id === "b4"), { id: "b4", content: "3.1. Child", parent_id: "b3" });
        assert.deepEqual(stored.find((block) => block.id === "b5"), { id: "b5", content: "Code stays literal", parent_id: "b4" });
        assert.deepEqual(errors, []);
        console.log("PASS existing children append numbers, moved subtree retains identities/text and Tab/Shift+Tab preserve text selections");
      } finally { await page.close(); }
    }
    {
      const { page, errors } = await openEditor(browser, {
        blockContents: ["2.1. Child", "```\n2.1. Literal\n```", "Plain parent", "3. Item"],
      });
      try {
        await focus(page, "b0");
        await page.keyboard.press("Shift+Enter");
        assert.equal(await doc(page), "2.1. Child\n");
        assert.equal((await blocks(page)).length, 4);
        await escape(page);
        await focus(page, "b1", 7);
        await page.keyboard.press("Tab");
        assert.equal(await doc(page), "```\n2.1  . Literal\n```");
        await escape(page);
        await focus(page, "b3");
        await page.keyboard.press("Tab");
        await moved(page, "b3", "b2", "3. Item");
        await escape(page);
        assert.deepEqual(errors, []);
        console.log("PASS dotted Shift+Enter and code stay literal; ordinary parents do not invent numbered prefixes");
      } finally { await page.close(); }
    }
    for (const contents of [["Parent", "First", "Second"], ["1. Parent", "2. First", "3. Second"]]) {
      const { page, errors } = await openEditor(browser, { blockContents: contents });
      try {
        await page.locator('.block-item[data-block-id="b1"] .bullet-container').click();
        await page.locator('.block-item[data-block-id="b2"] .bullet-container').click({ modifiers: ["Shift"] });
        assert.equal(await page.locator(".block-item.selected").count(), 2);
        await page.keyboard.press("Tab");
        await page.waitForFunction(() => ["b1", "b2"].every((id) =>
          window.__selectionState.blocks.find((block) => block.id === id).parent_id === "b0"));
        assert.equal(await page.locator(".block-item.selected").count(), 2);
        await page.keyboard.press("Shift+Tab");
        await page.waitForFunction(() => ["b1", "b2"].every((id) =>
          window.__selectionState.blocks.find((block) => block.id === id).parent_id === null));
        assert.deepEqual(await blocks(page), contents.map((content, index) => ({ id: `b${index}`, content, parent_id: null })));
        assert.deepEqual(errors, []);
        console.log(`PASS explicit ${contents[0].startsWith("1.") ? "numbered" : "ordinary"} multi-block Tab/Shift+Tab preserves group intent and text`);
      } finally { await page.close(); }
    }
  } finally { await browser.close(); }
})().catch((error) => { console.error(error); process.exitCode = 1; });

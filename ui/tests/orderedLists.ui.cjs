const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor, focus } = require("./keyboardSelection.ui.cjs");

const active = (page) => page.locator(".cm-content:focus");
const doc = (page) => page.evaluate(() => window.__activeEditorView.state.doc.toString());
const activeId = (page) => active(page).evaluate((element) => element.closest("[data-block-id]").dataset.blockId);
async function entered(page, previous) {
  await page.waitForFunction((previous) => window.__activeEditorView?.hasFocus
    && document.activeElement.closest("[data-block-id]")?.dataset.blockId !== previous, previous);
}
async function escapeAndSettle(page) {
  await page.keyboard.press("Escape");
  await page.waitForFunction(() => document.querySelectorAll(".block-item .cm-content").length === 0);
  await page.evaluate(() => new Promise((resolve) =>
    requestAnimationFrame(() => requestAnimationFrame(resolve))));
}
async function sourceBlocks(page) {
  return page.evaluate(() => window.__selectionState.blocks
    .filter((block) => block.page_id === "selection-page")
    .sort((a, b) => a.order_index - b.order_index)
    .map(({ id, content, parent_id }) => ({ id, content, parent_id })));
}

(async () => {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  try {
    for (const delimiter of [".", ")"]) {
      const { page, errors } = await openEditor(browser, { blockContents: [""] });
      try {
        await focus(page, "b0");
        await active(page).pressSequentially(`7${delimiter} First`);
        assert.equal(await page.locator('[data-block-id="b0"] .bullet').count(), 0,
          "typing a number removes the extra outline dot immediately");
        await page.keyboard.press("Enter");
        await entered(page, "b0");
        assert.equal(await doc(page), `8${delimiter} `);
        assert.equal(await page.evaluate(() => window.__activeEditorView.state.selection.main.head), 3);
        await active(page).pressSequentially("Second");
        const second = await activeId(page);
        await page.keyboard.press("Enter");
        await entered(page, second);
        assert.equal(await doc(page), `9${delimiter} `);
        const empty = await activeId(page);
        await page.keyboard.press("Enter");
        assert.equal(await activeId(page), empty, "empty item exits in place without an extra block");
        assert.equal(await doc(page), "");
        await active(page).pressSequentially("Ordinary writing");
        await escapeAndSettle(page);
        await page.locator(`[data-block-id="${empty}"] .rendered-content`).waitFor();
        const expected = [
          { id: "b0", content: `7${delimiter} First`, parent_id: null },
          { id: second, content: `8${delimiter} Second`, parent_id: null },
          { id: empty, content: "Ordinary writing", parent_id: null },
        ];
        assert.deepEqual(await sourceBlocks(page), expected, "continuations stay siblings, not nested children");
        for (const id of ["b0", second]) {
          const row = page.locator(`[data-block-id="${id}"]`);
          await row.locator("ol > li").waitFor();
          assert.equal(await row.locator(".bullet, .bullet-default").count(), 0);
          assert.equal(await row.locator("ol > li").count(), 1);
        }
        await page.evaluate(() => window.dispatchEvent(new CustomEvent("page-content-reload-blocks",
          { detail: { pageId: "selection-page" } })));
        await page.locator('[data-block-id="b0"] ol').waitFor();
        assert.deepEqual(await sourceBlocks(page), expected);
        assert.equal(await page.locator('[data-block-id="b0"] .bullet').count(), 0);
        assert.deepEqual(errors, []);
        console.log(`PASS ${delimiter} numbering, empty-marker exit, no double bullets, sibling structure and reload`);
      } finally { await page.close(); }
    }

    for (const delimiter of [".", ")"]) {
      const { page, errors } = await openEditor(browser, { blockContents: [`1${delimiter} Step`] });
      try {
        await focus(page, "b0");
        assert.equal(await page.locator('[data-block-id="b0"] .bullet').count(), 0);
        await page.keyboard.press("Enter");
        await entered(page, "b0");
        const id = await activeId(page);
        assert.equal(await doc(page), `2${delimiter} `);
        await escapeAndSettle(page);
        await page.waitForFunction((id) => !document.querySelector(`[data-block-id="${id}"] .cm-content`), id);
        assert.deepEqual(await sourceBlocks(page), [
          { id: "b0", content: `1${delimiter} Step`, parent_id: null },
          { id, content: "", parent_id: null },
        ]);
        await focus(page, id);
        await active(page).pressSequentially("Plain after Escape");
        await escapeAndSettle(page);
        await page.waitForFunction((id) => window.__selectionState.blocks
          .find((block) => block.id === id)?.content === "Plain after Escape", id);
        assert.deepEqual(errors, []);
        console.log(`PASS ${delimiter} Escape clears empty numbers, preserves completed text and permits plain writing`);
      } finally { await page.close(); }
    }

    {
      const { page, errors } = await openEditor(browser, {
        blockContents: ["4. Alpha selected Omega", "Existing child", "Unrelated root"],
      });
      try {
        await page.evaluate(() => {
          window.__selectionState.blocks.find((block) => block.id === "b1").parent_id = "b0";
          window.dispatchEvent(new CustomEvent("page-content-reload-blocks", { detail: { pageId: "selection-page" } }));
        });
        await focus(page, "b0", 9);
        await page.evaluate(() => window.__activeEditorView.dispatch({ selection: { anchor: 9, head: 17 } }));
        await page.keyboard.press("Enter");
        await entered(page, "b0");
        assert.equal(await doc(page), "5.  Omega", "selection is replaced and the untouched suffix follows the new marker");
        assert.equal(await page.evaluate(() => window.__activeEditorView.state.selection.main.head), 3);
        await active(page).pressSequentially("Inserted");
        const id = await activeId(page);
        await escapeAndSettle(page);
        await page.waitForFunction((id) => window.__selectionState.blocks
          .find((block) => block.id === id)?.content === "5. Inserted Omega", id);
        const blocks = await sourceBlocks(page);
        assert.deepEqual(blocks.find((block) => block.id === "b0"), { id: "b0", content: "4. Alpha ", parent_id: null });
        assert.equal(blocks.find((block) => block.id === id).parent_id, null,
          "numbering stays a sibling even when the previous item has children");
        assert.deepEqual(blocks.find((block) => block.id === "b1"),
          { id: "b1", content: "Existing child", parent_id: "b0" });
        assert.deepEqual(blocks.find((block) => block.id === "b2"),
          { id: "b2", content: "Unrelated root", parent_id: null });
        const parent = page.locator('[data-block-id="b0"]');
        await parent.locator("ol").waitFor();
        assert.equal(await parent.locator(".bullet, .bullet-default").count(), 0);
        await parent.locator(".bullet-container").click();
        assert.equal(await parent.locator(".bullet-container").getAttribute("aria-expanded"), "false");
        assert.deepEqual(errors, []);
        console.log("PASS caret/selection splitting retains suffix, existing children, sibling level and collapse controls");
      } finally { await page.close(); }
    }

    for (const input of ["beforeinput", "ime"]) {
      const { page, errors } = await openEditor(browser, { blockContents: ["3) Native input"] });
      try {
        await focus(page, "b0");
        if (input === "beforeinput") {
          await active(page).evaluate((element) => element.dispatchEvent(new InputEvent("beforeinput", {
            inputType: "insertParagraph", bubbles: true, cancelable: true,
          })));
        } else {
          await page.keyboard.insertText("\n");
        }
        await entered(page, "b0");
        assert.equal(await doc(page), "4) ");
        const id = await activeId(page);
        await page.keyboard.press("Enter");
        assert.equal(await activeId(page), id);
        assert.equal(await doc(page), "");
        await escapeAndSettle(page);
        await page.waitForFunction((id) => window.__selectionState.blocks.find((block) => block.id === id)?.content === "", id);
        assert.deepEqual(errors, []);
        console.log(`PASS ${input} continues numbering and exits empty item without extra blocks`);
      } finally { await page.close(); }
    }

    for (const marker of ["2.", "2)"]) {
      const { page, errors } = await openEditor(browser, { blockContents: [marker] });
      try {
        await focus(page, "b0");
        await page.keyboard.press("Enter");
        assert.equal(await activeId(page), "b0");
        assert.equal(await doc(page), "");
        await escapeAndSettle(page);
        await page.waitForFunction(() => window.__selectionState.blocks.find((block) => block.id === "b0").content === "");
        assert.equal((await sourceBlocks(page)).length, 1);
        assert.deepEqual(errors, []);
        console.log(`PASS bare ${marker} exits numbering without trailing whitespace or an extra block`);
      } finally { await page.close(); }
    }

    {
      const { page, errors } = await openEditor(browser, { blockContents: ["1. Keep", "Plain", "```\n1. Literal\n```"] });
      try {
        await focus(page, "b0");
        await page.keyboard.press("Shift+Enter");
        assert.equal(await doc(page), "1. Keep\n");
        assert.equal((await sourceBlocks(page)).length, 3, "Shift+Enter must not create an outline item");
        await escapeAndSettle(page);
        await focus(page, "b2", 14);
        await page.keyboard.press("Enter");
        assert.equal(await doc(page), "```\n1. Literal\n\n```", "numbered text in code stays literal");
        await escapeAndSettle(page);
        await focus(page, "b1");
        await active(page).fill("1. [[");
        await page.locator(".cm-tooltip-autocomplete").waitFor();
        await page.keyboard.press("Escape");
        await page.locator(".cm-tooltip-autocomplete").waitFor({ state: "detached" });
        assert.equal(await doc(page), "1. [[", "first Escape dismisses completion without losing text");
        await escapeAndSettle(page);
        await page.locator('[data-block-id="b1"] ol').waitFor();
        assert.equal(await page.locator('[data-block-id="b1"] .bullet').count(), 0);
        await focus(page, "b1");
        await active(page).fill("Plain again");
        await page.locator('[data-block-id="b1"] .bullet').waitFor();
        await escapeAndSettle(page);
        assert.deepEqual(errors, []);
        console.log("PASS Shift+Enter, fenced-code exclusion, completion Escape and live marker removal");
      } finally { await page.close(); }
    }

    {
      const { page, errors } = await openEditor(browser, {
        blockContents: ["1.text", "1.5 metres", "3)word", "Intro\n5. AlphaBeta"],
      });
      try {
        for (const id of ["b0", "b1", "b2"]) {
          await focus(page, id);
          await page.keyboard.press("Enter");
          await entered(page, id);
          assert.equal(await doc(page), "", "non-list numeric text must not continue numbers");
          await escapeAndSettle(page);
        }
        await focus(page, "b3", "Intro\n5. Alpha".length);
        await page.keyboard.press("Enter");
        await entered(page, "b3");
        assert.equal(await doc(page), "6. Beta", "continuation follows the caret's line, not the block's first line");
        await escapeAndSettle(page);
        assert.deepEqual(errors, []);
        console.log("PASS numeric prose exclusions and current-line continuation in a multiline block");
      } finally { await page.close(); }
    }
  } finally { await browser.close(); }
})().catch((error) => { console.error(error); process.exitCode = 1; });

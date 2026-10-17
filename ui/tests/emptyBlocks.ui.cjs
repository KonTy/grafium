const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor, focus, frames } = require("./keyboardSelection.ui.cjs");

const active = (page) => page.locator(".cm-content:focus");
const doc = (page) => page.evaluate(() => window.__activeEditorView.state.doc.toString());
const stored = (page) => page.evaluate(() => window.__selectionState.blocks.filter((block) => block.page_id === "selection-page")
  .map(({ id, content, parent_id }) => ({ id, content, parent_id })));
async function caret(page, id, text, head = text.length) {
  await page.waitForFunction(({ id, text, head }) => window.__activeEditorView?.hasFocus
    && document.activeElement.closest("[data-block-id]")?.dataset.blockId === id
    && window.__activeEditorView.state.doc.toString() === text
    && window.__activeEditorView.state.selection.main.head === head, { id, text, head });
}
async function escape(page) {
  await page.keyboard.press("Escape");
  await page.waitForFunction(() => !document.querySelector(".block-item .cm-content"));
  await frames(page);
}

(async () => {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  try {
    for (const marker of ["2.", "2)", "2. \t", "2.\u00a0", "2.1.", "2.1) ", "  2.1.\t\u200b", "2. \u200b"]) {
      const { page, errors } = await openEditor(browser, { blockContents: ["Previous", marker] });
      try {
        await focus(page, "b1");
        await page.keyboard.press("Enter");
        await caret(page, "b1", "");
        await active(page).pressSequentially("Plain text");
        await escape(page);
        assert.deepEqual(await stored(page), [
          { id: "b0", content: "Previous", parent_id: null },
          { id: "b1", content: "Plain text", parent_id: null },
        ]);
        assert.deepEqual(errors, []);
      } finally { await page.close(); }
    }
    console.log("PASS bare, dotted, Unicode-space and invisible-only markers exit in place and accept plain writing");

    for (const delimiter of [".", ")"]) {
      const { page, errors } = await openEditor(browser, { blockContents: [`1${delimiter} Written text`] });
      try {
        await focus(page, "b0", 0);
        await escape(page);
        await focus(page, "b0");
        await page.keyboard.press("Enter");
        await page.waitForFunction((text) => window.__activeEditorView?.hasFocus
          && window.__activeEditorView.state.doc.toString() === text, `2${delimiter} `);
        await page.keyboard.press("Backspace");
        await caret(page, "b0", `1${delimiter} Written text`);
        await page.keyboard.press("Backspace");
        await caret(page, "b0", `1${delimiter} Written tex`);
        await escape(page);
        assert.deepEqual(await stored(page), [{ id: "b0", content: `1${delimiter} Written tex`, parent_id: null }]);
        assert.deepEqual(errors, []);
      } finally { await page.close(); }
    }
    console.log("PASS generated empty number Backspace removes its row, focuses previous END and continues deleting text");

    for (const content of ["", " \t\u200b", "-", "2.1.\u00a0"]) {
      const { page, errors } = await openEditor(browser, { blockContents: ["Previous", content, "Following"] });
      try {
        await focus(page, "b0", 0);
        await escape(page);
        await focus(page, "b1");
        await page.keyboard.press("Backspace");
        await caret(page, "b0", "Previous");
        await page.keyboard.press("Backspace");
        await caret(page, "b0", "Previou");
        await escape(page);
        assert.deepEqual(await stored(page), [
          { id: "b0", content: "Previou", parent_id: null },
          { id: "b2", content: "Following", parent_id: null },
        ]);
        assert.deepEqual(errors, []);
      } finally { await page.close(); }
    }
    console.log("PASS blank bullets and number-only rows never reuse a cached start caret or delete following text");

    {
      const { page, errors } = await openEditor(browser, { blockContents: ["Keep", "", "", ""] });
      try {
        await focus(page, "b3");
        for (const id of ["b2", "b1", "b0"]) {
          await page.keyboard.press("Backspace");
          await caret(page, id, id === "b0" ? "Keep" : "");
        }
        await page.keyboard.press("Backspace");
        await caret(page, "b0", "Kee");
        await escape(page);
        assert.deepEqual(await stored(page), [{ id: "b0", content: "Kee", parent_id: null }]);
        assert.deepEqual(errors, []);
      } finally { await page.close(); }
    }
    console.log("PASS consecutive blank rows delete backwards one at a time before preceding text");

    {
      const { page, errors } = await openEditor(browser, { blockContents: ["Previous", "Keep this block"] });
      try {
        await focus(page, "b1", 0);
        await page.keyboard.press("Backspace");
        await caret(page, "b0", "Previous");
        await page.keyboard.press("Backspace");
        await caret(page, "b0", "Previou");
        await escape(page);
        assert.deepEqual(await stored(page), [
          { id: "b0", content: "Previou", parent_id: null },
          { id: "b1", content: "Keep this block", parent_id: null },
        ]);
        assert.deepEqual(errors, []);
      } finally { await page.close(); }
    }
    console.log("PASS nonempty start boundary moves to previous end without deleting or merging written blocks");

    {
      const { page, errors } = await openEditor(browser, { blockContents: ["Previous", "Parent", "Child", "Grandchild", "Following"] });
      try {
        await page.evaluate(() => {
          const state = window.__selectionState;
          state.blocks.find((block) => block.id === "b2").parent_id = "b1";
          state.blocks.find((block) => block.id === "b3").parent_id = "b2";
          window.dispatchEvent(new CustomEvent("page-content-reload-blocks", { detail: { pageId: "selection-page" } }));
        });
        await page.locator('.block-item[data-block-id="b1"] .bullet-container').click();
        await focus(page, "b0", 0);
        await active(page).pressSequentially("Old ");
        await escape(page);
        await focus(page, "b1");
        await active(page).fill("");
        await page.keyboard.press("Backspace");
        await caret(page, "b0", "Old Previous");
        assert.deepEqual((await stored(page)).filter((block) => ["b2", "b3"].includes(block.id)), [
          { id: "b2", content: "Child", parent_id: null },
          { id: "b3", content: "Grandchild", parent_id: "b2" },
        ]);
        await page.keyboard.press("Control+z");
        await page.waitForFunction(() => window.__selectionState.blocks.some((block) => block.id === "b1")
          && window.__selectionState.blocks.find((block) => block.id === "b2").parent_id === "b1");
        assert.equal((await stored(page)).find((block) => block.id === "b0").content, "Old Previous",
          "undo restores structural deletion before older cached text edits");
        assert.deepEqual(errors, []);
      } finally { await page.close(); }
    }
    console.log("PASS deleting a folded empty parent preserves children and undo restores its exact hierarchy");

    for (const content of ["", "2.\u00a0"]) {
      const { page, errors } = await openEditor(browser, { blockContents: [content] });
      try {
        await focus(page, "b0");
        for (let index = 0; index < 5; index++) await page.keyboard.press("Backspace");
        await caret(page, "b0", "");
        await active(page).pressSequentially("Still writable");
        await escape(page);
        assert.deepEqual(await stored(page), [{ id: "b0", content: "Still writable", parent_id: null }]);
        assert.deepEqual(errors, []);
      } finally { await page.close(); }
    }
    console.log("PASS final empty row remains writable and empty numbering can be removed");

    {
      const { page, errors } = await openEditor(browser, { blockContents: ["Previous", "", "Following"] });
      try {
        await focus(page, "b1");
        await page.evaluate(() => { window.__selectionState.holdDelete = true; });
        for (let index = 0; index < 6; index++) {
          await page.keyboard.down("Backspace");
          await page.keyboard.up("Backspace");
        }
        assert.equal(await page.evaluate(() => window.__selectionState.calls.filter(window.__isDeletion).length), 1);
        assert.equal(await page.locator('[data-block-id="b1"] .cm-content').getAttribute("contenteditable"), "false",
          "an in-flight structural deletion cannot acquire unsaved text");
        await page.evaluate(() => {
          window.__selectionState.holdDelete = false;
          for (const resolve of window.__selectionState.deleteWaiters.splice(0)) resolve();
        });
        await caret(page, "b0", "Previous");
        await page.keyboard.press("Backspace");
        await caret(page, "b0", "Previou");
        await escape(page);
        assert.deepEqual(errors, []);
      } finally { await page.close(); }
    }
    console.log("PASS rapid/repeated Backspace saves one deletion, then continues from the preceding text");

    {
      const { page, errors } = await openEditor(browser, { blockContents: ["Previous", ""] });
      try {
        await focus(page, "b1");
        await active(page).evaluate((element) => element.dispatchEvent(new InputEvent("beforeinput", {
          inputType: "deleteContentBackward", bubbles: true, cancelable: true,
        })));
        await caret(page, "b0", "Previous");
        assert.deepEqual(await stored(page), [{ id: "b0", content: "Previous", parent_id: null }]);
        assert.deepEqual(errors, []);
      } finally { await page.close(); }
    }
    console.log("PASS native beforeinput Backspace uses the same structural deletion and end caret");

    for (const failure of ["failure", "external-focus", "typing-during-save"]) {
      const { page, errors } = await openEditor(browser, { blockContents: ["Previous", "2. ", "Following"] });
      try {
        await focus(page, "b1");
        await page.evaluate((failure) => {
          const state = window.__selectionState;
          if (failure === "failure") state.failDelete = true;
          else if (failure === "external-focus") state.holdDelete = true;
          else state.holdUpdate = true;
        }, failure);
        await page.keyboard.press("Backspace");
        if (failure === "failure") {
          await page.getByText("Could not delete the block: Simulated block deletion failure", { exact: true }).waitFor();
          assert.equal((await stored(page)).length, 3);
          await page.evaluate(() => { window.__selectionState.failDelete = false; });
          await page.keyboard.press("Backspace");
          await caret(page, "b0", "Previous");
        } else if (failure === "external-focus") {
          await page.evaluate(() => {
            const input = document.createElement("input");
            input.id = "independent-editor";
            document.body.append(input);
            input.focus();
            window.__selectionState.holdDelete = false;
            for (const resolve of window.__selectionState.deleteWaiters.splice(0)) resolve();
          });
          await page.waitForFunction(() => !window.__selectionState.blocks.some((block) => block.id === "b1"));
          await frames(page);
          assert.equal(await page.evaluate(() => document.activeElement.id), "independent-editor");
        } else {
          await active(page).pressSequentially("Keep new writing");
          await page.evaluate(() => {
            window.__selectionState.holdUpdate = false;
            for (const resolve of window.__selectionState.updateWaiters.splice(0)) resolve();
          });
          await frames(page);
          assert.equal(await doc(page), "Keep new writing");
          assert.equal((await stored(page)).length, 3, "new writing cancels the pending empty-block deletion");
        }
        assert.deepEqual(errors, []);
      } finally { await page.close(); }
    }
    console.log("PASS delete errors are visible/retryable and async deletion neither steals focus nor discards new writing");

    {
      const { page, errors } = await openEditor(browser, {
        blockContents: ["Parent", "Written child", "Written grandchild"],
      });
      try {
        await page.evaluate(() => {
          const state = window.__selectionState;
          state.blocks.find((block) => block.id === "b1").parent_id = "b0";
          state.blocks.find((block) => block.id === "b2").parent_id = "b1";
          window.dispatchEvent(new CustomEvent("page-content-reload-blocks", { detail: { pageId: "selection-page" } }));
        });
        await page.locator('.block-item[data-block-id="b0"] .bullet-container').click();
        await focus(page, "b0");
        await active(page).fill("");
        await page.keyboard.press("Backspace");
        await caret(page, "b1", "Written child", 0);
        assert.deepEqual(await stored(page), [
          { id: "b1", content: "Written child", parent_id: null },
          { id: "b2", content: "Written grandchild", parent_id: "b1" },
        ]);
        assert.deepEqual(errors, []);
      } finally { await page.close(); }
    }
    console.log("PASS first empty folded parent deletion promotes intact descendants and starts at the first child's text");

    {
      const { page, errors } = await openEditor(browser, { blockContents: ["", "Following text"] });
      try {
        await focus(page, "b0");
        await page.keyboard.press("Backspace");
        await caret(page, "b1", "Following text", 0);
        await page.keyboard.press("Backspace");
        assert.deepEqual(await stored(page), [{ id: "b1", content: "Following text", parent_id: null }]);
        await page.evaluate(() => window.dispatchEvent(new CustomEvent("page-content-reload-blocks",
          { detail: { pageId: "selection-page" } })));
        await caret(page, "b1", "Following text", 0);
        assert.deepEqual(errors, []);
      } finally { await page.close(); }
    }
    console.log("PASS deleting first blank row keeps next text intact at its start and survives reload");
  } finally { await browser.close(); }
})().catch((error) => { console.error(error); process.exitCode = 1; });

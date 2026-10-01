const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor, focus } = require("./keyboardSelection.ui.cjs");

const contents = [
  "Plain block",
  "- some text",
  "* some text",
  "+ some text",
  "- First\n- Second",
  "- Parent\n  + Child\n    * Grandchild",
  "Paragraph\n\n- Separate list",
  "1. Numbered item",
  "_ asdfasdfa\n\n_ asome text",
  "\\- Literal dash",
  "```\n- Code, not a list\n```",
  "- [ ] Task",
];

async function checkRendering(page, { unifiedPage, book }) {
  const root = unifiedPage ? ".unified-rendered-block" : ".block-item";
  const block = (index) => page.locator(`${root}[${unifiedPage ? "data-source-block-id" : "data-block-id"}="b${index}"]`);
  const marker = (locator) => locator.evaluate((element) => getComputedStyle(element).listStyleType);
  const diamond = '"\u25c6 "';
  for (const index of unifiedPage ? [1, 2, 3] : [1, 2, 3, 4, 5]) {
    const style = index === 2 ? "disc" : index === 3 ? "square" : diamond;
    assert.equal(await marker(block(index).locator(".rendered-content > ul > li").first()), book ? style : "none",
      `${contents[index]} uses exactly one leading bullet`);
    if (!book) {
      const bullet = block(index).locator(unifiedPage ? ".unified-rendered-bullet" : ".bullet-container .bullet");
      assert.equal(await bullet.count(), 1);
      assert.equal(await bullet.textContent(), index === 2 ? "\u2022" : index === 3 ? "\u25a0" : "\u25c6");
      if (!unifiedPage) {
        const shape = await bullet.evaluate((element) => {
          const style = getComputedStyle(element);
          return { radius: style.borderRadius, transform: style.transform };
        });
        assert.equal(shape.radius, index === 2 ? "50%" : "0px");
        assert.equal(shape.transform === "none", index === 2 || index === 3);
      }
    }
  }
  // The retained prototype parses continuation lists as separate outline blocks.
  if (!unifiedPage) {
    assert.equal(await marker(block(4).locator(".rendered-content > ul > li").nth(1)), diamond);
    assert.equal(await marker(block(5).locator("li li").first()), "square", "nested + items have a square");
    assert.equal(await marker(block(5).locator("li li li")), "disc", "nested * items have a dot");
    assert.equal(await marker(block(6).locator("li")), diamond, "a list after prose retains its first marker");
    assert.match(await block(10).innerText(), /- Code, not a list/);
  }
  assert.equal(await marker(block(7).locator("li")), "decimal", "ordered list numbering is unchanged");
  assert.match(await block(8).innerText(), /_ asdfasdfa/);
  assert.match(await block(8).innerText(), /_ asome text/);
  assert.match(await block(9).innerText(), /- Literal dash/);
  assert.equal(await block(11).locator(".task-checkbox").count(), 1);
  if (book) assert.equal(await page.locator(".bullet-container").count(), 0);
}

(async () => {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  try {
    for (const options of [{}, { book: true }, { unifiedPage: true, componentHarness: true }]) {
      const { page, errors } = await openEditor(browser, { ...options, blockContents: contents });
      try {
        await checkRendering(page, options);
        if (!options.book && !options.unifiedPage) {
          let previous = "- some text";
          for (const [source, shape] of [["+", "square"], ["*", "dot"], ["-", "diamond"]]) {
            await focus(page, "b1");
            assert.equal(await page.evaluate(() => window.__activeEditorView.state.doc.toString()), previous);
            await page.keyboard.press("ControlOrMeta+a");
            const edited = `${source} Edited list item`;
            await page.keyboard.insertText(edited);
            await page.locator(".page-heading h1").click();
            await page.locator('[data-block-id="b1"] .rendered-content').waitFor();
            assert.equal(await page.locator('[data-block-id="b1"] li').evaluate(
              (element) => getComputedStyle(element).listStyleType), "none");
            assert.equal(await page.locator('[data-block-id="b1"] .bullet').getAttribute("data-list-marker"), shape);
            await page.waitForFunction((edited) => window.__selectionState.blocks.find((block) => block.id === "b1").content === edited, edited);
            previous = edited;
          }
          await page.evaluate(() => {
            const blocks = window.__selectionState.blocks;
            blocks.find((block) => block.id === "b0").content = "+ Parent block";
            blocks.find((block) => block.id === "b1").parent_id = "b0";
            window.dispatchEvent(new CustomEvent("page-content-reload-blocks", {
              detail: { pageId: "selection-page" },
            }));
          });
          const parent = page.locator('.block-item[data-block-id="b0"]');
          await parent.locator(".bullet-default").waitFor();
          assert.equal(await parent.locator(".bullet-default").textContent(), "\u25a0");
          await parent.locator(".bullet-container").click();
          await page.locator('.block-item[data-block-id="b1"]').waitFor({ state: "detached" });
          assert.equal(await parent.locator(".bullet-container").getAttribute("aria-expanded"), "false");
          await parent.locator(".bullet-container").click();
          await page.locator('.block-item[data-block-id="b1"]').waitFor();
          assert.equal(await parent.locator(".bullet-default").textContent(), "\u25a0");
        }
        assert.deepEqual(errors, []);
        console.log(`PASS single outline bullet and preserved Markdown: ${options.book ? "book" : options.unifiedPage ? "isolated continuous editor" : "classic editor"}`);
      } finally {
        await page.close();
      }
    }
  } finally {
    await browser.close();
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});

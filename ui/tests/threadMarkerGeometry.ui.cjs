const { chromium, webkit } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor, focus } = require("./keyboardSelection.ui.cjs");

async function geometry(page, id, editing) {
  return page.locator(`.block-item[data-block-id="${id}"]`).evaluate((row, editing) => {
    const elbow = row.querySelector(".indent-guide-elbow");
    const content = row.querySelector(".block-content");
    const scale = row.getBoundingClientRect().width / row.offsetWidth;
    let text;
    if (editing) {
      const line = row.querySelector(".cm-line");
      const first = document.createTreeWalker(line, NodeFilter.SHOW_TEXT).nextNode();
      const range = document.createRange();
      range.setStart(first, 0);
      range.setEnd(first, 1);
      text = range.getBoundingClientRect();
    } else {
      const item = row.querySelector(".rendered-content > ol > li");
      // Inside markers occupy the item's content box, before its text nodes.
      if (getComputedStyle(item).listStylePosition !== "inside") throw Error("Expected an inside list marker");
      text = item.getBoundingClientRect();
    }
    const rect = elbow?.getBoundingClientRect();
    return {
      gap: rect && (text.left - rect.right) / scale,
      width: rect && rect.width / scale,
      radius: elbow && parseFloat(getComputedStyle(elbow).borderBottomLeftRadius),
      contentInset: (content.getBoundingClientRect().left - row.getBoundingClientRect().left) / scale,
      depth: Number(row.dataset.depth),
      bullets: row.querySelectorAll(".bullet, .bullet-default").length,
      hasGutter: !!row.querySelector(".bullet-container"),
    };
  }, editing);
}

function assertClear(value, label) {
  assert.ok(value.gap >= 3, `${label}: guide crosses the number/text boundary: ${JSON.stringify(value)}`);
  assert.ok(value.width >= 8 && value.radius > 0, `${label}: rounded elbow remains visible`);
  assert.equal(value.bullets, 0, `${label}: no duplicate outline bullet`);
  assert.equal(value.hasGutter, false, `${label}: no new gutter or shifted text`);
  assert.ok(Math.abs(value.contentInset - value.depth * 24) < 0.1, `${label}: original indentation is unchanged`);
}

(async () => {
  const browser = process.env.UI_TEST_BROWSER === "webkit"
    ? await webkit.launch()
    : await chromium.launch({ args: ["--no-sandbox"] });
  try {
    for (const delimiter of [".", ")"]) {
      const { page, errors } = await openEditor(browser, {
        blockContents: ["Tech/Repair", "# MINISFORUM", `1${delimiter} First`, `99${delimiter} Middle`, `100${delimiter} Final`, "Child", `5${delimiter}`],
        beforeNavigate: (page) => page.addInitScript(() =>
          localStorage.setItem("grafium.pageContent.showBlockGuides", "true")),
      });
      try {
        await page.evaluate(() => {
          const blocks = window.__selectionState.blocks;
          for (const [id, parent] of [["b1", "b0"], ["b2", "b1"], ["b3", "b1"], ["b4", "b1"], ["b5", "b2"], ["b6", "b1"]]) {
            blocks.find((block) => block.id === id).parent_id = parent;
          }
          window.dispatchEvent(new CustomEvent("page-content-reload-blocks", { detail: { pageId: "selection-page" } }));
        });
        for (const zoom of [1, 1.25]) {
          await page.evaluate((zoom) => { document.body.style.zoom = String(zoom); }, zoom);
          for (const id of ["b3", "b4", "b6"]) {
            await focus(page, id);
            assertClear(await geometry(page, id, true), `${delimiter} ${id} editing at ${zoom}`);
            await page.locator(".page-heading h1").click();
            await page.locator(`.block-item[data-block-id="${id}"] ol`).waitFor();
            assertClear(await geometry(page, id, false), `${delimiter} ${id} rendered/unfocused at ${zoom}`);
            if (process.env.UI_TEST_SCREENSHOT && id === "b6" && zoom === 1) {
              await page.locator(".page-content").screenshot({ path: `${process.env.UI_TEST_SCREENSHOT}-${delimiter === "." ? "dot" : "paren"}.png` });
            }
          }
        }
        await page.evaluate(() => { document.body.style.zoom = "1"; });
        await focus(page, "b5");
        const parent = page.locator('.block-item[data-block-id="b2"]');
        await parent.locator("ol").waitFor();
        assert.equal(await parent.locator(".bullet-container").evaluate((element) =>
          element.getBoundingClientRect().width), 20, "numbered parents keep the full collapse hit target");
        assert.equal(await parent.locator(".bullet, .bullet-default").count(), 0);
        await parent.locator(".bullet-container").click();
        await page.locator('.block-item[data-block-id="b5"]').waitFor({ state: "detached" });
        await parent.locator(".bullet-container").click();
        await page.locator('.block-item[data-block-id="b5"]').waitFor();
        assert.deepEqual(errors, []);
        console.log(`PASS ${delimiter} marker/guide separation, multi-digit nesting, focus/render, zoom and collapse hit targets`);
      } finally { await page.close(); }
    }
    const { page, errors } = await openEditor(browser, { book: true, blockContents: ["1. First", "10) Tenth"] });
    try {
      assert.equal(await page.locator(".indent-guides, .bullet-container").count(), 0, "book surfaces remain guide-free");
      assert.equal(await page.locator(".block-item ol > li").count(), 2);
      await focus(page, "b1");
      assert.equal(await page.locator(".indent-guides, .bullet-container").count(), 0);
      assert.deepEqual(errors, []);
      console.log("PASS book rendering/editing retains its existing layout without guides or duplicate markers");
    } finally { await page.close(); }
  } finally { await browser.close(); }
})().catch((error) => { console.error(error); process.exitCode = 1; });

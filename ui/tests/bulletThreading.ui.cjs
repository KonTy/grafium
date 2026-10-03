// Check the actual rendered path, not just the per-block flags.
const { chromium, webkit } = require("playwright");
const assert = require("node:assert/strict");
const BASE_URL = process.env.UI_TEST_URL ?? "http://localhost:5199/";

async function openOutline(browser, large = false, duplicateOrder = false) {
  const page = await browser.newPage({ viewport: { width: 1200, height: 1000 } });
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(({ large, duplicateOrder }) => {
    localStorage.setItem("grafium.ui.zoom", "100");
    localStorage.setItem("grafium.pageContent.showBlockGuides", "true");
    const note = {
      id: "thread-page", title: "Threading regression", properties: {}, is_journal: false,
      file_path: "pages/Threading regression.md", created_at: 0, updated_at: 0,
    };
    const makeBlock = (id, parent_id, order_index, content = id) => ({
      id, page_id: note.id, parent_id, order_index, content,
      block_type: "markdown", properties: {}, created_at: 0, updated_at: 0,
    });
    const blocks = [
      makeBlock("root", null, 0, "Topic"),
      makeBlock("first", "root", 0, "First sibling"),
      makeBlock("second", "root", 1, "Second sibling"),
      makeBlock("branch", "root", 2, "A nested branch"),
      makeBlock("inner", "branch", 0, "One more level"),
      makeBlock("leaf-0", "inner", 0, "First leaf"),
      makeBlock("leaf-1", "inner", 1, "Second leaf"),
      makeBlock("leaf-2", "inner", 2, "Third leaf"),
      makeBlock("leaf-3", "inner", 3, "Fourth leaf"),
      makeBlock("multiline", "inner", 4, "A longer block\nwith several lines\nand a third line"),
      makeBlock("heading", "inner", 5, "# A heading"),
      makeBlock("heading-child", "heading", 0, "A heading child"),
      makeBlock("later", "root", 3, "Sibling after the whole subtree"),
      makeBlock("other-root", null, 1, "Unrelated root"),
    ];
    if (duplicateOrder) {
      Object.assign(blocks.find((block) => block.id === "leaf-1"), { id: "z-earlier", created_at: 100 });
      Object.assign(blocks.find((block) => block.id === "leaf-2"), { id: "a-later", order_index: 1, created_at: 200 });
    }
    if (large) {
      blocks.splice(12, 0, ...Array.from({ length: 520 }, (_, i) =>
        makeBlock(`long-${i}`, "inner", i + 6, `Long outline row ${i}`)));
    }
    let sequence = 0;
    window.__threadCalls = [];
    const treeOrder = () => {
      const visit = (parent) => blocks.filter((block) => block.parent_id === parent)
        .sort((a, b) => a.order_index - b.order_index || a.created_at - b.created_at || (a.id < b.id ? -1 : 1))
        .flatMap((block) => [block, ...visit(block.id)]);
      return visit(null);
    };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
      plugins: {}, transformCallback() { return ++sequence; }, unregisterCallback() {},
      invoke: async (cmd, args = {}) => {
        window.__threadCalls.push({ cmd, args: structuredClone(args) });
        switch (cmd) {
          case "get_page":
            if (args.id === note.id || args.title === note.title) return structuredClone(note);
            throw new Error("Page not found");
          case "get_graph_info": return { name: "Thread test", path: "/tmp/thread-test" };
          case "get_app_theme": return "github";
          case "get_layout_preferences": return { sidebarVisible: true, wideMode: true };
          case "list_blocks": return structuredClone(treeOrder());
          case "update_block": {
            const block = blocks.find((item) => item.id === args.id);
            if (!block) throw new Error("Block not found");
            block.content = args.content;
            return;
          }
          case "insert_block": {
            // Mirrors the native insert: number the displayed siblings 0..n.
            const siblings = treeOrder().filter((sibling) => sibling.parent_id === (args.parentId ?? null));
            const at = Math.min(args.position, siblings.length);
            siblings.forEach((sibling, index) => {
              blocks.find((block) => block.id === sibling.id).order_index = index < at ? index : index + 1;
            });
            const created = { ...makeBlock(`typed-${++sequence}`, args.parentId ?? null, at, args.content), created_at: 1000 };
            blocks.push(created);
            return structuredClone(created);
          }
          case "create_blocks": {
            const created = args.blocks.map((item, index) => ({
              ...makeBlock(item.id ?? `created-${index}`, item.parentIndex == null ? item.parentId ?? null
                : args.blocks[item.parentIndex].id, item.orderIndex, item.content),
              properties: structuredClone(item.properties ?? {}),
            }));
            blocks.push(...created);
            return structuredClone(created);
          }
          case "restructure_blocks": {
            // Mirrors the native batch: only listed blocks are deleted.
            const deleted = new Set(args.deleteIds);
            for (const move of args.moves) {
              Object.assign(blocks.find((block) => block.id === move.id),
                { parent_id: move.newParentId, order_index: move.orderIndex });
            }
            const stranded = blocks.find((block) => !deleted.has(block.id) && deleted.has(block.parent_id));
            if (stranded) throw new Error(`deleting ${stranded.parent_id} would also remove ${stranded.id}`);
            const removed = blocks.filter((block) => deleted.has(block.id));
            blocks.splice(0, blocks.length, ...blocks.filter((block) => !deleted.has(block.id)));
            return structuredClone(removed);
          }
          case "plugin:event|listen": return ++sequence;
          default: return [];
        }
      },
    };
  }, { large, duplicateOrder });
  await page.goto(BASE_URL, { waitUntil: "networkidle" });
  await page.evaluate(() => window.dispatchEvent(new CustomEvent("navigate-page", { detail: "Threading regression" })));
  await page.locator('.block-item[data-block-id="root"]').waitFor();
  return { page, errors };
}

async function focusBlock(page, id) {
  await page.locator(`.block-item[data-block-id="${id}"] .block-content`).click();
  await page.locator(`.block-item[data-block-id="${id}"] .cm-content`).waitFor();
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
}

async function readPath(page) {
  return page.evaluate(() => {
    const point = (x, y) => ({ x, y });
    const rectOf = (element) => {
      const r = element.getBoundingClientRect();
      return { left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height };
    };
    return [...document.querySelectorAll(".block-item")].map((row) => {
      const bullet = row.querySelector(".bullet-container");
      const bulletRect = bullet && rectOf(bullet);
      const scale = row.getBoundingClientRect().width / row.offsetWidth;
      const segments = [...row.querySelectorAll(".indent-guide-path, .indent-guide-elbow, .indent-guide-stem")].map((element) => {
        const rect = rectOf(element);
        const elbow = element.classList.contains("indent-guide-elbow");
        return {
          kind: elbow ? "elbow" : element.classList.contains("indent-guide-stem") ? "stem" : "continuation",
          start: point(rect.left + scale, rect.top),
          end: elbow ? point(rect.right, rect.bottom - scale) : point(rect.left + scale, rect.bottom),
          radius: parseFloat(getComputedStyle(element).borderBottomLeftRadius),
        };
      });
      return {
        id: row.dataset.blockId,
        bullet: bulletRect && point((bulletRect.left + bulletRect.right) / 2, (bulletRect.top + bulletRect.bottom) / 2),
        segments,
      };
    });
  });
}

function assertNear(actual, expected, label) {
  assert.ok(Math.abs(actual.x - expected.x) < 0.8 && Math.abs(actual.y - expected.y) < 0.8,
    `${label}: ${JSON.stringify(actual)} != ${JSON.stringify(expected)}`);
}

function assertConnected(rows, startId, endId) {
  const segments = rows.flatMap((row) => row.segments.map((segment) => ({ ...segment, id: row.id })));
  assert.ok(segments.length > 0);
  const firstRow = rows.find((row) => row.id === startId);
  const lastRow = rows.find((row) => row.id === endId);
  assert.ok(firstRow?.bullet && lastRow?.bullet, `missing endpoint bullet: ${JSON.stringify({ firstRow, lastRow })}`);
  assertNear(segments[0].start, firstRow.bullet, "thread starts at root bullet");
  assertNear(segments.at(-1).end, lastRow.bullet, "thread ends at focused bullet");
  for (let i = 1; i < segments.length; i++) {
    assertNear(segments[i].start, segments[i - 1].end, `no gap or extra tail before ${segments[i].id}`);
  }
  for (const row of rows) {
    for (const segment of row.segments) {
      if (segment.kind === "elbow") {
        assertNear(segment.end, row.bullet, `elbow meets ${row.id} bullet`);
        assert.ok(segment.radius > 0, "elbow stays rounded");
      }
    }
  }
}

(async () => {
  const browser = process.env.UI_TEST_BROWSER === "webkit"
    ? await webkit.launch()
    : await chromium.launch({ args: ["--no-sandbox"] });
  try {
    const { page, errors } = await openOutline(browser);
    for (const id of ["leaf-0", "leaf-1", "leaf-2", "leaf-3", "multiline", "heading-child", "later"]) {
      await focusBlock(page, id);
      const rows = await readPath(page);
      assertConnected(rows, "root", id);
      if (id === "leaf-3") {
        for (const ancestor of ["branch", "inner"]) {
          assert.deepEqual(rows.find((row) => row.id === ancestor).segments.map((s) => s.kind), ["elbow", "stem"]);
        }
        for (const leaf of ["leaf-0", "leaf-1", "leaf-2"]) {
          assert.deepEqual(rows.find((row) => row.id === leaf).segments.map((s) => s.kind), ["continuation"]);
        }
        if (process.env.UI_TEST_SCREENSHOT) {
          await page.locator(".page-content").screenshot({ path: process.env.UI_TEST_SCREENSHOT });
        }
      }
      if (id === "later") {
        for (const row of rows.filter((row) => ["inner", "leaf-1", "heading-child"].includes(row.id))) {
          assert.equal(row.segments[0].kind, "continuation");
          assertNear({ x: row.segments[0].start.x, y: 0 }, { x: rows[0].bullet.x, y: 0 }, "subtree carries root column");
        }
      }
    }
    await page.getByRole("heading", { name: "Threading regression" }).click();
    await page.waitForFunction(() => document.querySelectorAll(".cm-content").length === 0);
    assertConnected(await readPath(page), "root", "later");
    await page.locator('.block-item[data-block-id="branch"] .bullet-container').click();
    await page.locator('.block-item[data-block-id="inner"]').waitFor({ state: "detached" });
    await focusBlock(page, "later");
    assertConnected(await readPath(page), "root", "later");
    await page.locator('.block-item[data-block-id="branch"] .bullet-container').click();
    await page.locator('.block-item[data-block-id="inner"]').waitFor();
    await page.evaluate(() => { document.body.style.zoom = "1.25"; });
    await focusBlock(page, "heading-child");
    assertConnected(await readPath(page), "root", "heading-child");
    const headingHeight = await page.locator('.block-item[data-block-id="heading"]').evaluate(
      (element) => element.getBoundingClientRect().height / 1.25,
    );
    assert.ok(headingHeight <= 36, `heading parent should stay compact, got ${headingHeight}px`);
    for (let level = 1; level <= 6; level++) {
      await focusBlock(page, "heading");
      await page.keyboard.press("ControlOrMeta+a");
      await page.keyboard.insertText(`${"#".repeat(level)} A heading`);
      await focusBlock(page, "heading-child");
      const heading = page.locator('.block-item[data-block-id="heading"]');
      const gutter = heading.locator(".bullet-container");
      const arrow = heading.locator(".collapse-arrow");
      for (const collapsed of [false, true]) {
        assert.equal(await arrow.evaluate((element) => getComputedStyle(element).fontSize), "10px",
          `H${level} ${collapsed ? "collapsed" : "expanded"} arrow stays compact`);
        assert.equal(await gutter.evaluate((element) => element.getBoundingClientRect().width / 1.25), 20,
          "the click target does not shrink with the triangle");
        await gutter.click();
        await page.locator('.block-item[data-block-id="heading-child"]').waitFor({
          state: collapsed ? "visible" : "detached",
        });
      }
    }
    await focusBlock(page, "root");
    assert.equal((await readPath(page)).flatMap((row) => row.segments).length, 0);
    assert.deepEqual(errors, []);
    await page.close();
    console.log("PASS rounded staircase, every sibling, nested subtrees, blur, collapse, multiline, headings and zoom");

    const duplicate = await openOutline(browser, false, true);
    for (const id of ["a-later", "z-earlier"]) {
      await focusBlock(duplicate.page, id);
      assertConnected(await readPath(duplicate.page), "root", id);
    }
    await duplicate.page.keyboard.press("End");
    await duplicate.page.keyboard.press("ArrowDown");
    await duplicate.page.locator('.block-item[data-block-id="a-later"] .cm-content').waitFor();
    assertConnected(await readPath(duplicate.page), "root", "a-later");
    await duplicate.page.evaluate(() => window.dispatchEvent(new CustomEvent("page-content-reload-blocks", {
      detail: { pageId: "thread-page" },
    })));
    await focusBlock(duplicate.page, "z-earlier");
    assertConnected(await readPath(duplicate.page), "root", "z-earlier");
    assert.deepEqual(duplicate.errors, []);
    await duplicate.page.close();
    console.log("PASS equal-order siblings follow displayed order on click, arrow navigation and reload");

    const large = await openOutline(browser, true);
    await large.page.evaluate(() => {
      const scroller = document.querySelector(".main-content");
      scroller.scrollTop = scroller.scrollHeight;
    });
    await large.page.locator('.block-item[data-block-id="later"]').waitFor();
    await focusBlock(large.page, "later");
    const rows = await readPath(large.page);
    assert.ok(rows.length < 540, "large outline remains virtualized");
    const carried = rows.filter((row) => row.id.startsWith("long-"));
    assert.ok(carried.length > 0, "virtual window includes preceding subtree rows");
    for (const row of carried) {
      assert.equal(row.segments.length, 1);
      assert.equal(row.segments[0].kind, "continuation");
      assertNear({ x: row.segments[0].start.x, y: 0 }, { x: rows.find((r) => r.id === "later").bullet.x - 24, y: 0 },
        "virtual row continues root column");
    }
    assert.deepEqual(large.errors, []);
    await large.page.close();
    console.log("PASS continuation through virtualized preceding descendants");

    const edits = await openOutline(browser);
    const rowIds = () => edits.page.locator(".block-item").evaluateAll((rows) => rows.map((row) => row.dataset.blockId));
    // Shift+Tab used to leave the block between its old siblings until a
    // restart, drawing it there with guide lines from the wrong parent.
    await focusBlock(edits.page, "first");
    await edits.page.keyboard.press("End");
    await edits.page.keyboard.press("Shift+Tab");
    await edits.page.waitForFunction(() => {
      const ids = [...document.querySelectorAll(".block-item")].map((row) => row.dataset.blockId);
      return ids.indexOf("first") > ids.indexOf("later");
    });
    assert.deepEqual(await rowIds(), ["root", "second", "branch", "inner", "leaf-0", "leaf-1", "leaf-2", "leaf-3",
      "multiline", "heading", "heading-child", "later", "first", "other-root"]);
    await edits.page.waitForFunction(() => document.activeElement?.closest(".block-item")?.dataset.blockId === "first");
    await edits.page.keyboard.type(" typed after outdent");
    await edits.page.waitForFunction(() => document.activeElement?.textContent === "First sibling typed after outdent");
    const structuralCalls = await edits.page.evaluate(() => window.__threadCalls
      .filter((call) => ["move_block", "restructure_blocks"].includes(call.cmd)).map((call) => call.args));
    assert.deepEqual(structuralCalls, [{
      pageId: "thread-page", deleteIds: [], moves: [
        { id: "first", newParentId: null, orderIndex: 1 },
        { id: "other-root", newParentId: null, orderIndex: 2 },
      ],
    }], "one native change saves the outdent and the sibling it pushed down");
    await focusBlock(edits.page, "later");
    assertConnected(await readPath(edits.page), "root", "later");
    await edits.page.waitForFunction(() => window.__threadCalls.some((call) =>
      call.cmd === "update_block" && call.args.id === "first" && call.args.content === "First sibling typed after outdent"));

    // Backspace in an emptied parent deletes only that block: its children
    // move up into its place and keep their own children.
    const innerX = (await readPath(edits.page)).find((row) => row.id === "inner").bullet.x;
    await focusBlock(edits.page, "inner");
    await edits.page.keyboard.press("ControlOrMeta+a");
    await edits.page.keyboard.press("Backspace");
    await edits.page.keyboard.press("Backspace");
    await edits.page.locator('.block-item[data-block-id="inner"]').waitFor({ state: "detached" });
    const lifted = await readPath(edits.page);
    for (const id of ["leaf-0", "leaf-3", "multiline", "heading"]) {
      assertNear({ x: lifted.find((row) => row.id === id).bullet.x, y: 0 }, { x: innerX, y: 0 },
        `${id} moved up into the deleted block's level`);
    }
    assert.ok(lifted.find((row) => row.id === "heading-child").bullet.x > innerX, "grandchildren stay nested");
    await focusBlock(edits.page, "leaf-2");
    assertConnected(await readPath(edits.page), "root", "leaf-2");

    // One app undo brings the block back with its children under it.
    await edits.page.getByRole("heading", { name: "Threading regression" }).click();
    await edits.page.waitForFunction(() => document.querySelectorAll(".cm-content").length === 0);
    await edits.page.evaluate(() => window.dispatchEvent(new CustomEvent("app-undo")));
    await edits.page.locator('.block-item[data-block-id="inner"]').waitFor();
    assert.equal(await edits.page.locator('.block-item[data-block-id="inner"] .block-content').innerText(), "One more level");
    const restored = await readPath(edits.page);
    assert.ok(restored.find((row) => row.id === "leaf-0").bullet.x > innerX, "children are nested under it again");
    await focusBlock(edits.page, "leaf-2");
    assertConnected(await readPath(edits.page), "root", "leaf-2");

    // Enter at the end of a block showing its children starts its first child,
    // right below it, and that is also where it is saved: no reordering on reload.
    await focusBlock(edits.page, "branch");
    await edits.page.keyboard.press("End");
    await edits.page.keyboard.press("Enter");
    await edits.page.waitForFunction(() =>
      document.activeElement?.closest(".block-item")?.dataset.blockId?.startsWith("typed-"));
    const typedId = await edits.page.evaluate(() => document.activeElement.closest(".block-item").dataset.blockId);
    const drawn = await rowIds();
    assert.equal(drawn[drawn.indexOf("branch") + 1], typedId, "the new block appears right below its parent");
    await edits.page.keyboard.type("First of the branch");
    // Empty blocks show no bullet until their text is saved on leaving them.
    await focusBlock(edits.page, "later");
    await edits.page.waitForFunction((id) => window.__threadCalls.some((call) =>
      call.cmd === "update_block" && call.args.id === id && call.args.content === "First of the branch"), typedId);
    await focusBlock(edits.page, typedId);
    assertConnected(await readPath(edits.page), "root", typedId);
    await edits.page.evaluate(() => window.dispatchEvent(new CustomEvent("page-content-reload-blocks", {
      detail: { pageId: "thread-page" },
    })));
    await edits.page.waitForFunction(() => window.__threadCalls.filter((call) => call.cmd === "list_blocks").length > 3);
    await edits.page.locator(`.block-item[data-block-id="${typedId}"]`).waitFor();
    assert.deepEqual(await rowIds(), drawn, "a reload draws the same order the editor showed");
    assert.deepEqual(edits.errors, []);
    await edits.page.close();
    console.log("PASS outdent keeps tree order, caret and guide lines; deleting a parent keeps its children; undo restores; Enter saves where it draws");
  } finally {
    await browser.close();
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});

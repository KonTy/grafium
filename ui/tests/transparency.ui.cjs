// Full app with synthetic IPC only. No native app or personal graph is opened.
const assert = require("node:assert/strict");
const { chromium } = require("playwright");
const { openEditor, frames } = require("./keyboardSelection.ui.cjs");

async function pixels(page, selector) {
  const box = await page.locator(selector).first().boundingBox();
  assert.ok(box, `${selector} is visible`);
  const screenshot = await page.screenshot({ omitBackground: true, clip: {
    x: Math.ceil(box.x), y: Math.ceil(box.y),
    width: Math.max(1, Math.floor(box.width) - 1), height: Math.max(1, Math.floor(box.height) - 1),
  } });
  return page.evaluate(async (base64) => {
    const image = await createImageBitmap(await (await fetch(`data:image/png;base64,${base64}`)).blob());
    const canvas = document.createElement("canvas");
    canvas.width = image.width; canvas.height = image.height;
    const context = canvas.getContext("2d");
    context.drawImage(image, 0, 0);
    const data = context.getImageData(0, 0, canvas.width, canvas.height).data;
    const colors = {};
    for (let i = 0; i < data.length; i += 4) {
      const key = [...data.slice(i, i + 4)].join(",");
      colors[key] = (colors[key] ?? 0) + 1;
    }
    return colors;
  }, screenshot.toString("base64"));
}

function assertAlpha(colors, opacity, label) {
  const expected = Math.round(255 * opacity);
  const count = Object.entries(colors).filter(([rgba]) => Math.abs(Number(rgba.split(",")[3]) - expected) <= 1)
    .reduce((sum, [, count]) => sum + count, 0);
  assert.ok(count > 100, `${label}: expected background alpha ${expected}; got ${JSON.stringify(colors).slice(0, 450)}`);
}

(async () => {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  try {
    for (const unifiedPage of [false, true]) {
      const { page, errors } = await openEditor(browser, {
        unifiedPage, componentHarness: unifiedPage, theme: "auto",
        appearance: { themeName: "github", backgroundOpacity: 0.65, nativeTransparency: true },
        blockContents: ["# Opaque foreground MMMM", "Ordinary reading remains clear."],
      });
      if (unifiedPage) {
        await page.evaluate(async () => {
          // The retained continuous editor is feature-disabled in the app.
          // Give its component fixture the same single-painted window region.
          document.querySelector(".fixture-shell").style.background = "transparent";
          document.querySelector(".main-content").style.background = "var(--window-bg-primary)";
          await (await import("/src/lib/appearance.ts")).appearance.start();
        });
      }
      await page.waitForFunction(() => document.documentElement.hasAttribute("data-window-transparency"));
      await page.locator("#grafium-startup").waitFor({ state: "detached" });
      await frames(page);
      for (const selector of unifiedPage ? [".main-content"] : [".main-content", ".sidebar", ".titlebar-left", ".titlebar-right"]) {
        assertAlpha(await pixels(page, selector), 0.65, selector);
      }
      const headingSelector = unifiedPage ? ".main-content > h1" : ".page-title";
      const heading = await pixels(page, headingSelector);
      assert.ok(heading["31,35,40,255"] > 10, "real heading glyph interiors remain fully opaque");
      const ordinary = await pixels(page, unifiedPage ? ".unified-page-editor .cm-editor" : ".blocks-container");
      assertAlpha(ordinary, 0.65, "reading/editor background paints exactly once");
      const ancestors = await page.locator(headingSelector).evaluate((element) => {
        const values = [];
        for (let node = element; node; node = node.parentElement) values.push(getComputedStyle(node).opacity);
        return values;
      });
      assert.ok(ancestors.every(value => value === "1"), "no text ancestor has CSS opacity");
      if (!unifiedPage) {
        await page.keyboard.press("Alt+z");
        await frames(page);
        assertAlpha(await pixels(page, ".main-content"), 0.65, "zen background has no second paint");
        await page.keyboard.press("Alt+z");
        await page.keyboard.press("Control+l");
        await page.locator(".go-link-dialog").waitFor();
        assertAlpha(await pixels(page, ".go-link-dialog"), 1, "popup readability floor stays opaque");
        await page.keyboard.press("Escape");
        await page.locator(".go-link-dialog").waitFor({ state: "detached" });
        await frames(page);
      }
      await page.evaluate(() => window.dispatchEvent(new Event("blur")));
      assertAlpha(await pixels(page, ".main-content"), 0.65, "inactive background");

      await page.evaluate(() => {
        window.__selectionState.appearance = { themeName: "github", backgroundOpacity: 0.35, nativeTransparency: true };
        window.__selectionState.emit("smplos-theme-changed");
      });
      await page.waitForFunction(() => document.documentElement.style.getPropertyValue("--window-bg-primary") === "rgba(255, 255, 255, 0.35)");
      assert.equal(await page.evaluate(() => document.documentElement.style.getPropertyValue("--window-bg-primary")), "rgba(255, 255, 255, 0.35)");
      assert.notEqual(await page.locator("html").getAttribute("data-window-transparency"), null);
      assertAlpha(await pixels(page, ".main-content"), 0.35, "same-name live palette change");
      await page.evaluate(() => {
        window.__selectionState.appearance.backgroundOpacity = 0;
        window.__selectionState.emit("smplos-theme-changed");
      });
      await page.waitForFunction(() => document.documentElement.style.getPropertyValue("--window-bg-primary") === "rgba(255, 255, 255, 0)");
      assertAlpha(await pixels(page, ".main-content"), 0, "clear endpoint");
      assert.ok((await pixels(page, headingSelector))["31,35,40,255"] > 10, "glyph interiors remain opaque even at zero background opacity");
      await page.evaluate(() => {
        window.__selectionState.appearance.backgroundOpacity = 1;
        window.__selectionState.emit("smplos-theme-changed");
      });
      await page.waitForFunction(() => !document.documentElement.hasAttribute("data-window-transparency"));
      assertAlpha(await pixels(page, ".main-content"), 1, "opaque endpoint");
      assert.equal(await page.locator("html").getAttribute("data-window-transparency"), null);

      if (!unifiedPage) {
        await page.keyboard.press("Alt+s");
        await page.locator('[data-settings-section="theme"] > summary').click();
        await page.getByRole("button", { name: "OLED Black", exact: true }).click();
        await page.waitForFunction(() => window.__selectionState.theme === "oled");
      }
      await page.evaluate(() => {
        window.__selectionState.appearance.backgroundOpacity = 0.4;
        window.__selectionState.emit("smplos-theme-changed");
      });
      await frames(page);
      if (!unifiedPage) {
        assert.equal(await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--bg-primary").trim()), "#000000");
        assert.equal(await page.locator("html").getAttribute("data-window-transparency"), null);
      }
      await page.evaluate(() => {
        window.__selectionState.appearance.nativeTransparency = false;
        window.__selectionState.emit("smplos-theme-changed");
      });
      if (!unifiedPage) await page.getByRole("button", { name: "Auto (github)", exact: true }).click();
      await page.waitForFunction(() => !document.documentElement.hasAttribute("data-window-transparency"));
      assertAlpha(await pixels(page, ".main-content"), 1, "unsupported Linux compositor fallback");
      assert.deepEqual(errors, []);
      await page.close();
    }
    const { page, errors } = await openEditor(browser, {
      theme: "auto",
      appearance: { themeName: "github", backgroundOpacity: 0.65, nativeTransparency: true },
      graphData: { nodes: [{ id: "single", title: "Opaque node", degree: 0 }], edges: [] },
      beforeNavigate: page => page.addInitScript(() => localStorage.setItem("grafium.graphView.mode", "2d")),
    });
    await page.evaluate(() => window.dispatchEvent(new CustomEvent("navigate-page", { detail: "__graph__" })));
    await page.locator(".graph-canvas-wrap canvas").waitFor();
    await frames(page);
    assertAlpha(await pixels(page, ".graph-canvas-wrap canvas"), 0.65, "2D graph desktop background");
    const canvasAlpha = await page.locator(".graph-canvas-wrap canvas").evaluate(canvas => {
      const pixels = canvas.getContext("2d").getImageData(0, 0, canvas.width, canvas.height).data;
      let clear = 0, solid = 0;
      for (let i = 3; i < pixels.length; i += 4) {
        if (pixels[i] === 0) clear++;
        if (pixels[i] === 255) solid++;
      }
      return { clear, solid };
    });
    assert.ok(canvasAlpha.clear > 1000 && canvasAlpha.solid > 10, "canvas clears background but retains opaque node/label interiors");
    assert.deepEqual(errors, []);
    await page.close();
    console.log("PASS rendered background alpha, opaque glyph interiors, live changes, preferences and native fallback in both editors");
  } finally {
    await browser.close();
  }
})().catch((error) => { console.error(error); process.exitCode = 1; });

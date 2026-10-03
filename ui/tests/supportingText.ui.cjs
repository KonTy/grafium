// Real views with synthetic IPC; no personal notes, settings, or jobs are touched.
const assert = require("node:assert/strict");
const { chromium } = require("playwright");
const { openEditor } = require("./keyboardSelection.ui.cjs");

async function checkText(page, selector) {
  const elements = page.locator(selector);
  assert.ok(await elements.count(), `${selector} exists`);
  for (const element of await elements.all()) {
    await element.waitFor({ state: "visible" });
    const result = await element.evaluate(async (element) => {
      const { contrastRatio } = await import("/src/lib/contrast.ts");
      const { themes } = await import("/src/lib/themes.ts");
      const colors = themes.find(t => t.id === window.__selectionState.appearance.themeName).colors;
      const channels = getComputedStyle(element).color.match(/[\d.]+/g).map(Number);
      const rgb = { r: channels[0], g: channels[1], b: channels[2] };
      const opacities = [];
      for (let node = element; node; node = node.parentElement) opacities.push(getComputedStyle(node).opacity);
      return {
        contrast: Math.min(...[colors.bgPrimary, colors.bgSecondary, colors.bgCode].map(bg => contrastRatio(rgb, bg))),
        alpha: channels[3] ?? 1,
        opacities,
        glassStrength: contrastRatio(rgb, colors.bgPrimary) >= contrastRatio(colors.textPrimary, colors.bgPrimary),
      };
    });
    assert.ok(result.contrast >= 4.5, `${selector}: solid palette contrast ${result.contrast}`);
    assert.equal(result.alpha, 1, `${selector}: opaque foreground`);
    assert.ok(result.opacities.every(value => value === "1"), `${selector}: no fading ancestor`);
    if (await page.locator("html").getAttribute("data-window-transparency") !== null) {
      assert.ok(result.glassStrength, `${selector}: at least body-text strength on glass`);
    }
  }
}

(async () => {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  try {
    const { page, errors } = await openEditor(browser, {
      theme: "auto",
      appearance: { themeName: "catppuccin", backgroundOpacity: 0.5, nativeTransparency: true },
      aiConfig: { enabled: true, mode: "local", local: { provider: "openai_compatible" } },
    });
    for (const themeName of ["catppuccin", "catppuccin-latte"]) {
      for (const backgroundOpacity of [0.5, 1]) {
        await page.evaluate(({ themeName, backgroundOpacity }) => {
          window.__selectionState.appearance = { themeName, backgroundOpacity, nativeTransparency: true };
          window.__selectionState.emit("smplos-theme-changed");
        }, { themeName, backgroundOpacity });
        await page.waitForFunction(async ({ themeName, backgroundOpacity }) => {
          const { themes } = await import("/src/lib/themes.ts");
          return document.documentElement.style.getPropertyValue("--bg-primary") === themes.find(t => t.id === themeName).colors.bgPrimary
            && document.documentElement.hasAttribute("data-window-transparency") === (backgroundOpacity < 1);
        }, { themeName, backgroundOpacity });

        await page.evaluate(() => window.dispatchEvent(new CustomEvent("navigate-page", { detail: "__flashcards__" })));
        await page.locator(".state.empty .hint").waitFor();
        await checkText(page, ".state.empty .hint, .state.empty .hint code");

        await page.keyboard.press("Alt+s");
        const search = page.getByRole("searchbox", { name: "Filter settings", exact: true });
        await search.fill("sync");
        await checkText(page, ".sync-empty p");
        // Settings explanations moved behind ? help; check them where they are read.
        await page.getByRole("button", { name: "Help: Sync", exact: true }).click();
        await checkText(page, "dialog[data-settings-help-dialog] .section-desc");
        await page.keyboard.press("Escape");
        await page.locator("dialog[data-settings-help-dialog]").waitFor({ state: "detached" });
        await search.fill("AI / Knowledge Engine");
        await page.locator(".ai-settings .field-hint").first().waitFor();
        await checkText(page, ".ai-settings .field-hint");

        await page.evaluate(() => window.dispatchEvent(new CustomEvent("navigate-page", { detail: "__jobs__" })));
        await page.locator(".jobs-view").waitFor();
        await checkText(page, ".jobs-header p, .empty-state p, .summary-label");
      }
    }
    const running = page.locator(".summary-label").filter({ hasText: /^Running$/ });
    const assertRunning = async (count) => {
      await page.waitForFunction(count => document.querySelectorAll(".summary-count")[1].textContent === String(count), count);
      assert.equal(await running.evaluate(el => el.classList.contains("shimmer")), count > 0);
      assert.equal(await running.evaluate(el => el.classList.contains("shimmer-endless")), count > 0);
      assert.equal(await running.evaluate(el => getComputedStyle(el).animationName.includes("shimmer-sweep")), count > 0);
    };
    const update = (id, status) => page.evaluate(({ id, status }) => {
      window.__selectionState.emit("job://update", {
        id, status, kind: "index", title: `Synthetic job ${id}`, progress: null,
        message: null, link: null, error: null, details: null, cancellable: false,
        started_at: 1, finished_at: status === "running" ? null : 2,
      });
    }, { id, status });
    await assertRunning(0);
    await update("first", "running");
    await assertRunning(1);
    await update("second", "running");
    await assertRunning(2);
    await update("first", "succeeded");
    await assertRunning(1);
    await update("second", "failed");
    await assertRunning(0);
    await update("third", "running");
    await assertRunning(1);
    await update("third", "cancelled");
    await assertRunning(0);
    await update("third", "running");
    await assertRunning(0); // A stale update must not restart the animation.
    await update("fourth", "running");
    await assertRunning(1);
    await page.emulateMedia({ reducedMotion: "reduce" });
    assert.equal(await running.evaluate(el => getComputedStyle(el).animationName), "none");
    assert.deepEqual(errors, []);
    await page.close();
    console.log("PASS Catppuccin dark/light flashcard, Sync, AI and Jobs supporting text; Jobs shimmer follows live activity");
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });

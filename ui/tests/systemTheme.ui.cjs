// Runs against Vite dev or the compiled production frontend with synthetic IPC.
const assert = require("node:assert/strict");
const { chromium } = require("playwright");
const { openEditor } = require("./keyboardSelection.ui.cjs");
const fixtures = require("./fixtures/smplos-theme-contract.json");

(async () => {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  try {
    const grafium = fixtures.find(theme => theme.id === "grafium");
    const appearance = theme => ({
      themeName: theme.id, backgroundOpacity: theme.opacity,
      palette: theme.palette, nativeTransparency: true,
    });
    const { page, errors } = await openEditor(browser, { theme: "auto", appearance: appearance(grafium) });
    await page.keyboard.press("Alt+s");
    await page.getByRole("searchbox", { name: "Filter settings", exact: true }).fill("theme");
    for (const theme of fixtures) {
      await page.evaluate(system => {
        window.__selectionState.appearance = system;
        window.__selectionState.emit("smplos-theme-changed");
      }, appearance(theme));
      await page.waitForFunction(theme => document.documentElement.style.getPropertyValue("--bg-primary") === theme.palette.background
        && document.documentElement.style.getPropertyValue("--text-primary") === theme.palette.foreground, theme);
      const button = page.getByRole("button", { name: `Auto (${theme.id})`, exact: true });
      await button.waitFor();
      const actual = await button.evaluate((button, theme) => {
        const probe = document.createElement("span");
        button.append(probe);
        const normalize = color => { probe.style.color = color; return getComputedStyle(probe).color; };
        const expected = [theme.palette.accent, theme.palette.background, theme.palette.foreground].map(normalize);
        probe.remove();
        return {
          expected,
          actual: [...button.querySelectorAll(".swatch")].map(el => getComputedStyle(el).backgroundColor),
          scheme: document.documentElement.style.colorScheme,
          opacity: getComputedStyle(document.querySelector(".settings-page")).opacity,
          glass: document.documentElement.hasAttribute("data-window-transparency"),
        };
      }, theme);
      assert.deepEqual(actual.actual, actual.expected, `${theme.id} preview matches actual OS palette`);
      assert.equal(actual.scheme, ["catppuccin-latte", "flexoki-light", "rose-pine"].includes(theme.id) ? "light" : "dark");
      assert.equal(actual.opacity, "1", "no whole-content fading");
      assert.equal(actual.glass, theme.opacity < 1);
    }
    await page.evaluate(system => {
      window.__selectionState.appearance = system;
      window.__selectionState.emit("smplos-theme-changed");
    }, appearance(grafium));
    await page.waitForFunction(() => document.documentElement.style.getPropertyValue("--window-bg-primary") === "rgba(0, 0, 0, 1)");
    if (process.env.GRAFIUM_THEME_SCREENSHOT) {
      await page.screenshot({ path: process.env.GRAFIUM_THEME_SCREENSHOT });
    }
    await page.getByRole("button", { name: "GitHub", exact: true }).click();
    await page.waitForFunction(() => document.documentElement.style.getPropertyValue("--bg-primary") === "#ffffff");
    await page.evaluate(() => {
      window.__selectionState.appearance.palette.background = "#101010";
      window.__selectionState.emit("smplos-theme-changed");
    });
    await page.getByRole("button", { name: "Auto (grafium)", exact: true }).click();
    await page.waitForFunction(() => document.documentElement.style.getPropertyValue("--bg-primary") === "#101010");
    await page.evaluate(() => {
      window.__selectionState.appearance.palette = null;
      window.__selectionState.appearance.paletteError = "System palette is missing.";
      window.__selectionState.emit("smplos-theme-changed");
    });
    await page.getByRole("alert").filter({ hasText: "System palette is missing." }).waitFor();
    await page.waitForFunction(() => document.documentElement.style.getPropertyValue("--window-bg-primary") === "rgba(0, 0, 0, 1)");
    assert.deepEqual(errors, []);
    await page.close();
    console.log("PASS all 17 native palette contracts, Settings previews, dark Grafium, manual preference and palette diagnostics");
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });

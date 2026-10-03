// The sidebar's Recent list follows the Settings limit immediately and after restart.
const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor } = require("./keyboardSelection.ui.cjs");

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const { page, errors } = await openEditor(browser, { beforeNavigate: (page) => page.addInitScript(() => {
      if (window !== window.top) return;
      window.__recentLimits = [];
      const install = (internals) => {
        const original = internals.invoke;
        internals.invoke = async (command, args = {}) => {
          if (command !== "list_recent_pages") return original(command, args);
          window.__recentLimits.push(args.limit);
          return Array.from({ length: Math.min(args.limit, 15) }, (_, index) => ({
            id: `recent-${index}`, title: `Recent fixture ${index + 1}`, properties: {}, is_journal: false,
            file_path: `pages/recent-${index}.md`, created_at: 0, updated_at: 0,
          }));
        };
        return internals;
      };
      let internals = window.__TAURI_INTERNALS__;
      if (internals) internals = install(internals);
      Object.defineProperty(window, "__TAURI_INTERNALS__", {
        configurable: true, get: () => internals, set: (value) => { internals = install(value); },
      });
    }) });
    const recent = page.locator(".sidebar .section-title", { hasText: "Recent" }).locator("..").locator("button");
    const sidebarCount = () => recent.count();
    await page.waitForFunction(() => window.__recentLimits.length > 0);
    assert.equal(await sidebarCount(), 10, "Recent shows ten pages by default");
    await page.keyboard.press("Alt+s");
    const limit = page.getByLabel("Recent pages in sidebar", { exact: true });
    await limit.fill("3");
    await limit.press("Tab");
    await page.waitForFunction(() => window.__recentLimits.at(-1) === 3);
    await page.waitForFunction(() => [...document.querySelectorAll(".sidebar .section-title")]
      .find((title) => title.textContent.trim() === "Recent")?.parentElement.querySelectorAll("button").length === 3);
    await limit.fill("99");
    await limit.press("Tab");
    assert.equal(await limit.inputValue(), "50", "out-of-range values are clamped visibly");
    await limit.fill("0");
    await limit.press("Tab");
    await page.waitForFunction(() => ![...document.querySelectorAll(".sidebar .section-title")]
      .some((title) => title.textContent.trim() === "Recent"));
    await page.reload({ waitUntil: "networkidle" });
    await page.waitForTimeout(800);
    assert.equal(await page.evaluate(() => localStorage.getItem("grafium.sidebar.recentPagesLimit")), "0");
    assert.equal(await page.locator(".sidebar .section-title", { hasText: "Recent" }).count(), 0, "0 hides Recent after restart");
    assert.deepEqual(errors, []);
    console.log("Recent pages limit: default, immediate update, clamping, hide and persistence PASS");
  } finally {
    await browser.close();
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});

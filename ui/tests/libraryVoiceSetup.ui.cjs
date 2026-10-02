const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor } = require("./keyboardSelection.ui.cjs");

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const { page, errors } = await openEditor(browser, { beforeNavigate: page => page.addInitScript(() => {
      if (window !== window.top) return;
      Object.defineProperty(navigator, "clipboard", { configurable: true,
        value: { writeText: async text => { window.__voiceSetupCopied = text; } } });
      const install = internals => {
        const original = internals.invoke;
        internals.invoke = async (command, args) => {
          if (command === "private_voice_status") return { available: false, runtime: "piper-onnx-v1",
            selection: null, reason: "Choose a local Piper environment." };
          if (command === "private_voice_installed") return [];
          if (command === "reader_snapshot") return { libraryPath: null, books: [] };
          if (command === "help_get_page") return "# Library reading and listening\n\nOffline voice setup.";
          return original(command, args);
        };
        return internals;
      };
      let internals = window.__TAURI_INTERNALS__;
      if (internals) internals = install(internals);
      Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true,
        get: () => internals, set: value => { internals = install(value); } });
    }) });
    await page.keyboard.press("Alt+s");
    const section = page.locator('[data-settings-section="library"]');
    await section.waitFor();
    assert.equal((await section.locator(":scope > summary").innerText()).trim(), "Library");
    if (!await section.evaluate(node => node.open)) await section.locator(":scope > summary").click();
    assert.equal(await page.getByRole("dialog", { name: "Get offline voices", exact: true }).count(), 0);
    const help = section.getByRole("button", { name: "Help: Get offline voices", exact: true });
    await help.click();
    const dialog = page.getByRole("dialog", { name: "Get offline voices", exact: true });
    await dialog.waitFor();
    assert(await dialog.getByRole("link", { name: "Download Linux model in browser" }).isVisible());
    assert.equal(await page.evaluate(() => window.__voiceSetupCopied), undefined);
    await dialog.getByRole("button", { name: "Copy package preparation command", exact: true }).click();
    assert.match(await page.evaluate(() => window.__voiceSetupCopied), /--runtime piper-onnx-v1/);
    await dialog.getByRole("link", { name: "Download Linux model in browser" }).click();
    assert(await page.evaluate(() => window.__selectionState.calls.some(call =>
      call.cmd === "plugin:shell|open" && call.args.path.includes("en_US-ljspeech-high.onnx?download=true"))));
    await dialog.getByLabel("Guide platform").selectOption("android");
    assert(await dialog.getByRole("link", { name: "Download Android voice bundle in browser" }).isVisible());
    await dialog.getByRole("button", { name: "Copy package preparation command", exact: true }).click();
    assert.match(await page.evaluate(() => window.__voiceSetupCopied), /--runtime sherpa-vits-v1/);
    await page.setViewportSize({ width: 420, height: 850 });
    const bounds = await dialog.boundingBox();
    assert(bounds.width <= 404 && bounds.x >= 0, "Voice guide remains inside a narrow screen");
    await dialog.getByRole("button", { name: "Close", exact: true }).click();
    assert.equal(await help.evaluate(node => node === document.activeElement), true);
    const search = page.getByRole("searchbox", { name: "Filter settings", exact: true });
    await search.fill("LJ Speech");
    await help.waitFor();
    assert.equal(await page.getByRole("dialog").count(), 0, "Searching hidden setup does not expand its prose");
    await help.click();
    await dialog.waitFor();
    await dialog.getByRole("button", { name: "Close", exact: true }).focus();
    await page.keyboard.press("F1");
    await dialog.waitFor({ state: "detached" });
    assert(await page.evaluate(() => window.__selectionState.calls.every(call =>
      !["private_voice_download", "private_voice_configure_runtime", "private_voice_import"].includes(call.cmd))));
    assert.deepEqual(errors, []);
    console.log("Library settings label, platform download guide, exact copied command, narrow layout, search and F1 PASS");
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });

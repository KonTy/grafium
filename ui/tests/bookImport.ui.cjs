const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor } = require("./keyboardSelection.ui.cjs");

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const { page, errors } = await openEditor(browser, {
      beforeNavigate: (page) => page.addInitScript(() => {
        function install(internals) {
          const invoke = internals.invoke;
          window.__bookImports = [];
          window.__failBookImport = true;
          internals.invoke = async (command, args = {}) => {
            if (command === "books_import_originals" || command === "books_import_directory") {
              window.__bookImports.push({ command, args: structuredClone(args) });
              if (window.__failBookImport) throw new Error("Synthetic graph is read-only");
              return "synthetic-book-job";
            }
            return invoke(command, args);
          };
          return internals;
        }
        let internals = window.__TAURI_INTERNALS__;
        if (internals) internals = install(internals);
        Object.defineProperty(window, "__TAURI_INTERNALS__", {
          configurable: true, get: () => internals, set: (value) => { internals = install(value); },
        });
      }),
    });

    await page.keyboard.press("Alt+b");
    const dialog = page.getByRole("dialog", { name: "Import books" });
    await dialog.waitFor();
    assert.equal(await dialog.getByLabel("Import as").inputValue(), "original");
    await dialog.getByLabel("Book file or folder").fill("/synthetic/book.epub");
    await dialog.getByRole("button", { name: "Add to Books", exact: true }).click();
    await dialog.getByRole("alert").filter({ hasText: "Synthetic graph is read-only" }).waitFor();
    assert.equal(await dialog.getByLabel("Book file or folder").inputValue(), "/synthetic/book.epub");

    await page.evaluate(() => { window.__failBookImport = false; });
    await dialog.getByRole("button", { name: "Add to Books", exact: true }).click();
    await dialog.waitFor({ state: "hidden" });
    const originals = await page.evaluate(() => window.__bookImports);
    assert.equal(originals.length, 2);
    assert.deepEqual(originals[1], {
      command: "books_import_originals", args: { sourcePaths: ["/synthetic/book.epub"] },
    });

    await page.keyboard.press("Alt+b");
    await dialog.waitFor();
    await dialog.getByLabel("Import as").selectOption("markdown");
    await dialog.getByLabel("Book file or folder").fill("/synthetic/books");
    await dialog.getByRole("button", { name: "Convert and import", exact: true }).click();
    await dialog.waitFor({ state: "hidden" });
    assert.deepEqual(await page.evaluate(() => window.__bookImports.at(-1)), {
      command: "books_import_directory", args: { sourceDir: "/synthetic/books" },
    });
    assert.deepEqual(errors, []);
    console.log("Book import UI: original default, retry preservation, and explicit conversion passed");
    await page.close();
  } finally {
    await browser.close();
  }
})().catch((error) => { console.error(error); process.exitCode = 1; });

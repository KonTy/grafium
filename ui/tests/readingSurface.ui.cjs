const { chromium } = require("playwright");
const { zipSync, strToU8 } = require("fflate");
const assert = require("node:assert/strict");
const { openEditor } = require("./keyboardSelection.ui.cjs");

function syntheticBook() {
  return zipSync(Object.fromEntries(Object.entries({
    mimetype: "application/epub+zip",
    "META-INF/container.xml": '<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0"><rootfiles><rootfile full-path="book.opf" media-type="application/oebps-package+xml"/></rootfiles></container>',
    "book.opf": '<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">reading-surface-fixture</dc:identifier><dc:title>Reading fixture</dc:title><dc:language>en</dc:language></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="chapter"/></spine></package>',
    "chapter.xhtml": `<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Reading fixture</title><style>body { color:black!important;background:white!important } p {font-size:12px!important;line-height:1.5} span{font-size:9pt!important}</style></head><body>${Array.from({ length: 70 }, (_, i) => `<p id="p${i}"><span>Paragraph ${i}. A synthetic book preserves its place while controls come and go. Larger letters should be genuinely easier to read, not merely further apart.</span></p>`).join("")}</body></html>`,
  }).map(([path, content]) => [path, strToU8(content)])));
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const { page, errors } = await openEditor(browser, {
      beforeNavigate: page => page.addInitScript(bytes => {
        if (window !== window.top) return;
        const book = {
          id: "12345678-1234-4234-8234-123456789abc",
          title: "A very long fixture filename that must not waste the reading page.epub",
          kind: "epub", available: true, tracks: [], position: null, bookmarks: [],
        };
        function install(internals) {
          const original = internals.invoke;
          window.__readingFixture = { book, reads: 0 };
          internals.invoke = async (command, args = {}) => {
            const snapshot = () => structuredClone({ libraryPath: "/synthetic/private", books: [book] });
            if (command === "reader_snapshot" || command === "reader_rescan") return snapshot();
            if (command === "reader_read_epub") { window.__readingFixture.reads++; return Uint8Array.from(bytes).buffer; }
            if (command === "reader_save_position") { book.position = structuredClone(args.position); return; }
            if (command === "reader_record_activity") { book.lastUsedAt = Date.now(); book.progress = args.progress; return snapshot(); }
            if (command === "plugin:window|is_fullscreen") return false;
            if (command === "plugin:window|set_fullscreen") { window.__readingFullscreen = args.value; return; }
            return original(command, args);
          };
          return internals;
        }
        let internals = window.__TAURI_INTERNALS__;
        if (internals) internals = install(internals);
        Object.defineProperty(window, "__TAURI_INTERNALS__", {
          configurable: true, get: () => internals, set: value => { internals = install(value); },
        });
      }, [...syntheticBook()]),
    });
    page.setDefaultTimeout(15000);
    await page.locator(".sidebar").getByRole("button", { name: "Library", exact: true }).click();
    const chapterLoaded = page.waitForEvent("framenavigated", { predicate: frame => frame.url() === "about:srcdoc" });
    await page.locator(".book-title").click();
    const chapter = await chapterLoaded;
    await chapter.locator("#p0 span").waitFor();
    const reader = page.getByRole("region", { name: "Private EPUB reader", exact: true });
    const controls = reader.getByRole("navigation", { name: "Reading controls" });
    assert.equal(await controls.isVisible(), false);
    assert.equal(await page.locator(".private-detail h1").count(), 0);
    const frame = reader.locator("iframe");
    const before = await frame.boundingBox();
    assert(before.height > 500, JSON.stringify(before));
    await reader.getByRole("button", { name: "Show reading controls" }).click();
    assert.equal(await controls.isVisible(), true);
    assert.deepEqual(await frame.boundingBox(), before, "Showing controls must not repaginate the book");
    if (!await page.locator(".app-shell").evaluate(el => el.classList.contains("wide-mode"))) {
      await page.keyboard.press("Alt+w");
      await page.waitForFunction(() => document.querySelector(".app-shell.wide-mode"));
    }
    const wideFrame = await frame.boundingBox();
    await chapter.waitForFunction(width => document.querySelector("#p0").getBoundingClientRect().width > width * .75, wideFrame.width);
    const wideText = await chapter.locator("#p0").evaluate(el => el.getBoundingClientRect().width);
    await page.keyboard.press("Alt+w");
    await page.waitForFunction(() => !document.querySelector(".app-shell.wide-mode"));
    await chapter.waitForFunction(width => document.querySelector("#p0").getBoundingClientRect().width < width * .9, wideText);
    await page.keyboard.press("Alt+w");
    await page.waitForFunction(() => document.querySelector(".app-shell.wide-mode"));
    await chapter.waitForFunction(width => document.querySelector("#p0").getBoundingClientRect().width > width * .95, wideText);
    const small = await chapter.locator("#p0 span").evaluate(element => parseFloat(getComputedStyle(element).fontSize));
    await controls.locator("label select").selectOption("200");
    await page.waitForTimeout(150);
    const large = await chapter.locator("#p0 span").evaluate(element => parseFloat(getComputedStyle(element).fontSize));
    assert(large >= small * 1.9, `Actual lettering did not grow: ${small} -> ${large}`);
    await page.evaluate(() => {
      document.documentElement.style.setProperty("--bg-primary", "#000000");
      document.documentElement.style.setProperty("--text-primary", "#00ff00");
    });
    await page.waitForTimeout(150);
    assert.equal(await chapter.locator("#p0 span").evaluate(element => getComputedStyle(element).color), "rgb(0, 255, 0)");
    const runtime = page.frames().find(frame => frame.url().startsWith("data:text/html"));
    assert(runtime);
    await chapter.locator("#p0 span").click();
    await controls.waitFor({ state: "hidden" });
    assert.equal(await controls.isVisible(), false, "Single tap/click should hide controls");
    await chapter.locator("#p0 span").press("F8");
    await controls.waitFor({ state: "visible" });
    assert.equal(await controls.isVisible(), true, "F8 inside the book should restore controls");
    await controls.getByRole("button", { name: "Fullscreen", exact: true }).click();
    await page.waitForFunction(() => document.querySelector(".reading-surface.expanded"));
    const fullscreen = await frame.boundingBox();
    const viewport = page.viewportSize();
    assert(fullscreen.width >= viewport.width - 2 && fullscreen.height >= viewport.height - 2, JSON.stringify(fullscreen));
    assert.equal(await controls.isVisible(), false);
    await chapter.locator("#p0 span").press("Escape");
    await page.waitForFunction(() => !document.querySelector(".reading-surface.expanded"));
    assert.equal(await page.evaluate(() => window.__readingFixture.reads), 1, "Controls/fullscreen/theme must not reload the EPUB");
    await page.setViewportSize({ width: 390, height: 844 });
    await page.waitForTimeout(150);
    assert(await reader.locator(".controls-handle").isVisible());
    const mobile = await frame.boundingBox();
    assert(mobile.width <= 390 && mobile.height > 400, JSON.stringify(mobile));
    await reader.getByRole("button", { name: "Show reading controls" }).click();
    await controls.getByRole("combobox", { name: "Reading layout" }).selectOption("scrolled");
    await controls.getByRole("button", { name: "Bionic reading", exact: true }).click();
    await runtime.waitForFunction(() => document.querySelector("foliate-view").renderer.scrolled);
    assert.equal(await page.getByTitle("Bionic Speedreader", { exact: true }).getAttribute("aria-pressed"), "true",
      "Book Bionic mode and the app's top-bar preference stay synchronized");
    await reader.getByRole("button", { name: "Hide reading controls" }).click();
    const start = await runtime.evaluate(() => document.querySelector("foliate-view").renderer.start);
    await page.keyboard.press("PageDown");
    await runtime.waitForFunction(start => document.querySelector("foliate-view").renderer.start > start, start);
    assert.equal(await page.evaluate(() => window.__readingFixture.reads), 1, "Mode changes and host Page Down keep the existing reader mounted");
    await page.setViewportSize(viewport);
    await page.locator(".sidebar").getByRole("button", { name: "Library", exact: true }).click();
    await page.locator(".book-title").click();
    await reader.locator("iframe").waitFor();
    await page.waitForFunction(() => window.__readingFixture.reads === 2);
    const reopened = page.frames().find(frame => frame.url().startsWith("data:text/html"));
    await reopened.waitForFunction(() => document.querySelector("foliate-view")?.renderer?.scrolled);
    await reader.getByRole("button", { name: "Show reading controls" }).click();
    assert.equal(await controls.getByRole("combobox", { name: "Reading layout" }).inputValue(), "scrolled");
    assert.equal(await controls.getByRole("button", { name: "Bionic reading", exact: true }).getAttribute("aria-pressed"), "true");
    assert.deepEqual(errors, []);
    console.log("Reading surface: hidden chrome, scaling, theme, F8/fullscreen, mobile, host paging, remembered flow and shared Bionic PASS");
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });

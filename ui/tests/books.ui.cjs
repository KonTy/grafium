// Synthetic, public-domain fixtures only. No graph or native API is accessed.
const { chromium, webkit } = require("playwright");
const { build } = require("esbuild");
const { zipSync, strToU8 } = require("fflate");
const { readFile } = require("node:fs/promises");
const { readFileSync } = require("node:fs");
const { createHash } = require("node:crypto");
const http = require("node:http");
const assert = require("node:assert/strict");
const path = require("node:path");

const UNICODE_PASSAGE = `Unicode ${"x".repeat(311)}\u{1F600} end.`;

function epub(fixed = false, rtl = false) {
  const repeated = Array.from({ length: 145 }, (_, n) => `<p>Offline paragraph ${n}. A book can remember its place across window and font size changes.</p>`).join("")
    + `<p>${UNICODE_PASSAGE}</p>`
    + `<p id="long-chunks">${"CanonicalLongChunk ".repeat(80)}</p>`;
  const font = new Uint8Array(readFileSync(path.join(__dirname, "../node_modules/pdfjs-dist/standard_fonts/LiberationSans-Regular.ttf")));
  const key = createHash("sha1").update("synthetic").digest();
  for (let i = 0; i < 1040; i++) font[i] ^= key[i % key.length];
  return zipSync(Object.fromEntries(Object.entries({
    mimetype: "application/epub+zip",
    "META-INF/container.xml": `<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0"><rootfiles><rootfile full-path="book.opf" media-type="application/oebps-package+xml"/></rootfiles></container>`,
    "META-INF/encryption.xml": `<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><EncryptedData xmlns="http://www.w3.org/2001/04/xmlenc#"><EncryptionMethod Algorithm="http://www.idpf.org/2008/embedding"/><CipherData><CipherReference URI="font.ttf"/></CipherData></EncryptedData></encryption>`,
    "font.ttf": font,
    "book.opf": `<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="uid">synthetic</dc:identifier><dc:title>Offline Reader Test</dc:title><dc:language>en</dc:language>${fixed ? '<meta property="rendition:layout">pre-paginated</meta>' : ""}</metadata><manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="one" href="one.xhtml" media-type="application/xhtml+xml"/><item id="two" href="two.xhtml" media-type="application/xhtml+xml"/><item id="font" href="font.ttf" media-type="font/ttf"/></manifest><spine page-progression-direction="${rtl ? "rtl" : "ltr"}"><itemref idref="one"/><itemref idref="two"/></spine></package>`,
    "nav.xhtml": `<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>Contents</title></head><body><nav epub:type="toc"><ol><li><a href="one.xhtml">First chapter</a></li><li><a href="two.xhtml">Second chapter</a></li></ol></nav></body></html>`,
    "one.xhtml": `<html xmlns="http://www.w3.org/1999/xhtml"><head><title>One</title><meta name="viewport" content="width=600,height=800"/><style>@import "https://blocked.invalid/style";@font-face{font-family:FixtureFont;src:url(font.ttf)}#passage{font-family:FixtureFont}body{background-image:url(https://blocked.invalid/css);background-color:white;color:black}h1{font-size:24px !important}p{font-size:10px !important;line-height:14px !important;color:black !important;background:white !important}</style></head><body onload="top.pwned=true"><script>top.pwned=true;fetch('https://blocked.invalid/script')</script><h1>First chapter</h1><p id="passage" style="font-size:10pt !important;color:black !important;background:white !important">Select this original passage for a saved note.</p><p id="nested-font" style="font-size:12px !important">Hierarchy <span style="font-size:9pt !important">nested <em style="font-size:75% !important">smaller</em></span></p><svg xmlns="http://www.w3.org/2000/svg" id="artwork" width="16" height="16" style="color:#321abc;background:white;width:1em;height:1em"><rect width="8" height="16" fill="currentColor"/></svg><img src="https://blocked.invalid/image" onerror="top.pwned=true"/><a href="javascript:top.pwned=true">Unsafe link</a><a id="local-link" href="#passage" style="color:blue !important">Local link</a>${repeated}</body></html>`,
    "two.xhtml": `<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Two</title><meta name="viewport" content="width=600,height=800"/></head><body style="background:white;color:black"><h1>Second chapter</h1><p style="font-size:9pt !important">Return to the first passage.</p></body></html>`,
  }).map(([name, value]) => [name, typeof value === "string" ? strToU8(value) : value])));
}
function mobi() {
  const text = Buffer.from('<html><head><title>MOBI fixture</title></head><body><h1>MOBI chapter</h1><p id="passage">Select this original passage in MOBI.</p><a filepos="0">Beginning</a></body></html>');
  const pdb = Buffer.alloc(96), header = Buffer.alloc(288);
  pdb.write("Fixture"); pdb.write("BOOKMOBI", 60); pdb.writeUInt16BE(2, 76);
  pdb.writeUInt32BE(pdb.length, 78); pdb.writeUInt32BE(pdb.length + header.length, 86);
  header.writeUInt16BE(1, 0); header.writeUInt32BE(text.length, 4); header.writeUInt16BE(1, 8); header.writeUInt16BE(4096, 10);
  header.write("MOBI", 16); header.writeUInt32BE(248, 20); header.writeUInt32BE(2, 24); header.writeUInt32BE(65001, 28);
  header.writeUInt32BE(6, 36); header.writeUInt32BE(264, 84); header.writeUInt32BE(12, 88); header.writeUInt32BE(2, 108);
  header.writeUInt32BE(0xffffffff, 244); header.write("MOBI Fixture", 264);
  return Buffer.concat([pdb, header, text]);
}
function pdf() {
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R /OpenAction 8 0 R >>",
    "<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 500] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>",
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    null,
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 500] /Rotate 90 /Resources << /Font << /F1 4 0 R >> >> /Contents 7 0 R >>",
    null,
    "<< /Type /Action /S /JavaScript /JS (app.alert\\(\\\"no\\\"\\)) >>",
  ];
  for (const [index, text] of [[4, "Select this PDF passage."], [6, "Rotated second PDF page."]]) {
    const stream = `BT /F1 18 Tf 40 440 Td (${text}) Tj ET`;
    objects[index] = `<< /Length ${stream.length} >>\nstream\n${stream}\nendstream`;
  }
  let content = "%PDF-1.4\n";
  const offsets = [0];
  objects.forEach((object, i) => { offsets.push(content.length); content += `${i + 1} 0 obj\n${object}\nendobj\n`; });
  const xref = content.length;
  content += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n${offsets.slice(1).map(o => String(o).padStart(10, "0") + " 00000 n \n").join("")}trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF`;
  return new Uint8Array(Buffer.from(content));
}

async function main() {
  const root = path.resolve(__dirname, "..");
  const runtime = await readFile(path.join(root, "public/book-reader/runtime.js"), "utf8");
  const bundle = await build({ absWorkingDir: root, entryPoints: ["src/lib/bookReaderSecurity.ts"], bundle: true,
    write: false, format: "iife", globalName: "BookSecurity" });
  const server = http.createServer((req, res) => {
    res.setHeader("Content-Type", "text/html");
    res.end(`<!doctype html><body><script>window.pwned=false;window.__TAURI_INTERNALS__={secret:true};</script><script>${bundle.outputFiles[0].text}</script></body>`);
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const browserType = process.env.BOOK_READER_BROWSER === "webkit" ? webkit : chromium;
  let browser;
  try {
    browser = await browserType.launch({
      headless: true, ...(browserType === chromium ? { args: ["--no-sandbox"] } : {}),
    });
  } catch (error) {
    await new Promise(resolve => server.close(resolve));
    throw error;
  }
  const page = await browser.newPage({ viewport: { width: 1000, height: 850 }, hasTouch: true });
  const matrix = { background: "#000000", text: "#00ff41", link: "#80ffff",
    selectionBackground: "#00ff41", selectionText: "#000000" };
  const requests = [];
  const browserErrors = [];
  await page.route("https://blocked.invalid/**", route => {
    requests.push(route.request().url());
    return route.abort();
  });
  page.on("pageerror", e => { browserErrors.push(e.message); console.error("Browser error:", e.message); });
  page.on("console", message => {
    if ((message.type() === "error" || message.type() === "warning") && !message.text().includes("https://blocked.invalid"))
      console.error("Browser console:", message.text().slice(0, 500));
  });
  try {
    await page.goto(`http://127.0.0.1:${server.address().port}`);
    const open = async (format, bytes, location = null) => {
      await page.evaluate(({ runtime, format, data, location, theme }) => {
        document.querySelector("iframe")?.remove();
        window.messages = [];
        const frame = document.createElement("iframe");
        frame.style.cssText = "width:950px;height:750px";
        frame.sandbox = BookSecurity.BOOK_FRAME_SANDBOX;
        frame.src = BookSecurity.readerFrameURL("test-secret", theme);
        window.send = (type, payload = {}) => frame.contentWindow.postMessage({ channel: "grafium-book", token: "test-secret", type, ...payload }, "*");
        window.onmessage = event => {
          const m = BookSecurity.readReaderMessage(event, frame.contentWindow, "test-secret");
          if (m) window.messages.push(m);
        };
        frame.onload = () => {
          window.send("bootstrap", { runtime });
          window.send("open", { bytes: Uint8Array.from(data).buffer, format, location });
        };
        document.body.append(frame);
      }, { runtime, format, data: [...bytes], location, theme: matrix });
      try {
        await page.waitForFunction(() => messages.some(m => m.type === "location") || messages.some(m => m.type === "error"), null, { timeout: 30000 });
      } catch (e) {
        console.error("Reader messages:", await page.evaluate(() => messages));
        for (const f of page.frames()) console.error("Frame:", f.url().slice(0, 40), await f.evaluate(() => document.body?.innerText.slice(0, 300)));
        throw e;
      }
      const errors = await page.evaluate(() => messages.filter(m => m.type === "error"));
      assert.deepEqual(errors, [], `${format}: ${JSON.stringify(errors)}`);
    };
    const selectBook = async () => {
      const chapter = page.frames().find(f => f.url() === "about:srcdoc");
      assert(chapter, "Foliate chapter rendered in an isolated srcdoc");
      await chapter.locator("#passage").evaluate(el => {
        const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT), nodes = [];
        while (walker.nextNode()) nodes.push(walker.currentNode);
        const range = document.createRange();
        range.setStart(nodes[0], 0); range.setEnd(nodes.at(-1), nodes.at(-1).length);
        getSelection().removeAllRanges(); getSelection().addRange(range);
      });
      await page.waitForFunction(() => messages.some(m => m.type === "selection"));
      return page.evaluate(() => messages.findLast(m => m.type === "selection"));
    };
    await open("epub", epub());
    const initialChapter = page.frames().find(f => f.url() === "about:srcdoc");
    for (const flow of ["paginated", "scrolled", "paginated"]) {
      await page.evaluate(value => send("flow", { value }), flow);
      await page.waitForTimeout(150);
      const wideWidth = await initialChapter.locator("#passage").evaluate(el => el.getBoundingClientRect().width);
      assert(wideWidth > 950 * .8, `${flow} prose must use the available wide reader, not a fixed 720px column: ${wideWidth}`);
      await page.evaluate(() => { document.querySelector("iframe").style.width = "500px"; });
      await initialChapter.waitForFunction(() => document.querySelector("#passage").getBoundingClientRect().width < 500);
      const narrowWidth = await initialChapter.locator("#passage").evaluate(el => el.getBoundingClientRect().width);
      assert(narrowWidth > 500 * .8, `${flow} prose should reflow to the narrower reader: ${narrowWidth}`);
      await page.evaluate(() => { document.querySelector("iframe").style.width = "950px"; });
      await initialChapter.waitForFunction(() => document.querySelector("#passage").getBoundingClientRect().width > 950 * .8);
    }
    assert.equal(await initialChapter.locator("body").evaluate(el => getComputedStyle(el).backgroundColor), "rgb(0, 0, 0)");
    assert.equal(await initialChapter.locator("#passage").evaluate(el => getComputedStyle(el).color), "rgb(0, 255, 65)");
    assert.equal(await initialChapter.locator("#local-link").evaluate(el => getComputedStyle(el).color), "rgb(128, 255, 255)");
    const captureArtwork = async () => {
      const bounds = await initialChapter.locator("#artwork").boundingBox();
      // Exclude fractional bounding-box edges that sample the surrounding page.
      return page.screenshot({ clip: { x: Math.ceil(bounds.x) + 1, y: Math.ceil(bounds.y) + 1,
        width: Math.floor(bounds.width) - 2, height: Math.floor(bounds.height) - 2 } });
    };
    const artworkPixels = await captureArtwork();
    await initialChapter.locator("#passage").evaluate(el => { window.originalPassage = el; });
    const initialLocation = await page.evaluate(() => messages.findLast(m => m.type === "location").location);
    await page.evaluate(theme => send("theme", { theme: { ...theme, background: "#fafafa", text: "#123456" } }), matrix);
    await initialChapter.waitForFunction(() => getComputedStyle(document.querySelector("#passage")).color === "rgb(18, 52, 86)");
    assert(await initialChapter.evaluate(() => originalPassage === document.querySelector("#passage")), "Live themes never reload a chapter");
    assert.deepEqual(await captureArtwork(), artworkPixels,
      "SVG currentColor and publisher image backgrounds retain identical pixels across themes");
    assert.deepEqual(await page.evaluate(() => messages.findLast(m => m.type === "location").location), initialLocation);
    await page.evaluate(theme => send("theme", { theme }), matrix);
    const selection = await selectBook();
    await page.frames().find(f => f.url() === "about:srcdoc").locator("#passage").click();
    await page.keyboard.press("F1");
    await page.waitForFunction(() => messages.some(m => m.type === "help"));
    await page.keyboard.press("F11");
    await page.keyboard.press("Escape");
    await page.waitForFunction(() => messages.some(m => m.type === "toggle-fullscreen") && messages.some(m => m.type === "exit-fullscreen"));
    const shortcutControls = await page.evaluate(() => messages.filter(m => m.type === "toggle-controls").length);
    await page.keyboard.press("Shift+F8");
    await page.keyboard.press("a");
    assert.equal(await page.evaluate(() => messages.filter(m => m.type === "toggle-controls").length), shortcutControls,
      "Modified shortcuts and ordinary typing never toggle controls");
    await page.keyboard.press("F8");
    await page.waitForFunction(n => messages.filter(m => m.type === "toggle-controls").length === n + 1, shortcutControls);
    assert.match(selection.quote, /original passage/);
    const readFontSizes = frame => frame.evaluate(() => {
      const selectors = ["#passage", "h1", "#nested-font", "#nested-font span", "#nested-font em"];
      return selectors.map(selector => parseFloat(getComputedStyle(document.querySelector(selector)).fontSize));
    });
    const baselineFonts = await readFontSizes(initialChapter);
    assert(Math.abs(baselineFonts[0] - 13.3333) < .01, "Fixture includes publisher absolute pt with inline !important");
    assert.equal(baselineFonts[2], 12, "Fixture includes absolute px with !important");
    assert.equal(baselineFonts[3], 12, "Nested publisher 9pt !important computes to 12px");
    assert.equal(baselineFonts[4], 9, "Relative nested typography retains its smaller hierarchy");
    const artSize = await initialChapter.locator("#artwork").evaluate(el => ({ width: el.getBoundingClientRect().width, height: el.getBoundingClientRect().height }));
    await page.evaluate(() => { send("size", { value: 200 }); send("size", { value: 200 }); });
    await initialChapter.waitForFunction(() => parseFloat(getComputedStyle(document.querySelector("#passage")).fontSize) > 26);
    const doubledFonts = await readFontSizes(initialChapter);
    doubledFonts.forEach((value, i) => assert(Math.abs(value - baselineFonts[i] * 2) < .01,
      `Actual computed glyph font-size doubles without compounding: ${value} vs ${baselineFonts[i]}`));
    assert.deepEqual(await initialChapter.locator("#artwork").evaluate(el => ({ width: el.getBoundingClientRect().width, height: el.getBoundingClientRect().height })),
      artSize, "Font scaling preserves em-sized artwork dimensions");
    await page.evaluate(theme => send("theme", { theme: { ...theme, text: "#12ab34" } }), matrix);
    await initialChapter.waitForFunction(() => getComputedStyle(document.querySelector("#passage")).color === "rgb(18, 171, 52)");
    assert.deepEqual(await readFontSizes(initialChapter), doubledFonts, "Theme changes do not compound typography");
    await page.evaluate(location => send("goto", { location }), selection.location);
    await initialChapter.waitForFunction(() => {
      const rect = document.querySelector("#passage").getBoundingClientRect();
      return rect.right > 0 && rect.left < innerWidth;
    });
    assert(await initialChapter.evaluate(() => originalPassage === document.querySelector("#passage")),
      "Scaling retains the live document and saved passage locator");
    await page.evaluate(() => send("toc", { target: "two.xhtml" }));
    await page.waitForFunction(() => messages.findLast(m => m.type === "location")?.label.includes("Second chapter"));
    const sizedSecondChapter = page.frames().find(f => f.url() === "about:srcdoc");
    assert.equal(await sizedSecondChapter.locator("p").evaluate(el => parseFloat(getComputedStyle(el).fontSize)), 24,
      "New chapter scales its own 9pt baseline once");
    await page.evaluate(location => send("goto", { location }), selection.location);
    await page.waitForFunction(() => messages.findLast(m => m.type === "location")?.label.includes("First chapter"));
    const reloadedChapter = page.frames().find(f => f.url() === "about:srcdoc");
    assert.deepEqual(await readFontSizes(reloadedChapter), doubledFonts, "Returning chapters do not compound scaling");
    await page.evaluate(theme => { send("size", { value: 100 }); send("theme", { theme }); }, matrix);
    await reloadedChapter.waitForFunction(() => parseFloat(getComputedStyle(document.querySelector("#passage")).fontSize) < 14);
    assert.deepEqual(await readFontSizes(reloadedChapter), baselineFonts, "100% restores publisher typography exactly");
    console.log("EPUB font scaling: actual px/pt-important glyph sizes double; hierarchy, artwork, locator and repeated/theme/chapter changes passed.");
    assert.equal(requests.length, 0, "book network requests must never leave the browser");
    assert.equal(await page.evaluate(() => pwned), false);
    const chapter = page.frames().find(f => f.url() === "about:srcdoc");
    assert.equal(await chapter.evaluate(async () => {
      await document.fonts.ready;
      return [...document.fonts].find(font => font.family === "FixtureFont")?.status;
    }), "loaded", "IDPF-obfuscated embedded fonts must load offline");
    assert.equal(await chapter.evaluate(() => {
      try { return !!top.__TAURI_INTERNALS__; } catch { return false; }
    }), false, "book cannot reach privileged parent");
    const narration = [];
    for (let section = 0; section < 2; section++) {
      let offset = 0;
      do {
        const requestId = `narration-${section}-${offset}`;
        await page.evaluate(({ requestId, section, offset }) => {
          send("read-aloud-segments", { requestId, section, offset });
        }, { requestId, section, offset });
        await page.waitForFunction(id => messages.some(m =>
          m.type === "read-aloud-segments" && m.requestId === id), requestId);
        const batch = await page.evaluate(id => messages.find(m =>
          m.type === "read-aloud-segments" && m.requestId === id), requestId);
        assert.equal(batch.sectionCount, 2);
        assert(batch.segments.length > 0 && batch.segments.length <= 128);
        if (batch.nextOffset !== null) assert(batch.nextOffset > offset, "narration pagination advances");
        narration.push(...batch.segments);
        offset = batch.nextOffset;
      } while (offset !== null);
    }
    assert(narration.length > 145, "narration includes every paragraph across pagination");
    assert(narration.some(segment => segment.text.includes("Offline paragraph 144.")));
    assert(narration.map(segment => segment.text).join("").includes(UNICODE_PASSAGE),
      "narration chunk boundaries preserve complete Unicode code points");
    assert(narration.every(segment => Buffer.from(segment.text, "utf8").toString("utf8") === segment.text),
      "no segment contains an unpaired surrogate");
    assert(!narration.some(segment => segment.text.includes("top.pwned") || segment.text.includes("@font-face")));
    for (const segment of narration) {
      assert(Buffer.byteLength(segment.text, "utf8") <= 1500);
      assert.equal(segment.locator.kind, "epub");
      assert.match(segment.locator.cfi, /^epubcfi\(/);
      assert.equal(segment.locator.rendererVersion, selection.location.rendererVersion);
    }
    const chunks = narration.filter(segment => segment.text.includes("CanonicalLongChunk"));
    assert(chunks.length >= 3, "a long text node is split into bounded narration chunks");
    assert.equal(new Set(chunks.map(segment => segment.locator.cfi)).size, chunks.length,
      "each chunk of the same paragraph has a distinct actual-renderer CFI");
    const middlePassage = narration.find(segment => segment.text.includes("Offline paragraph 70."));
    await page.evaluate(location => send("goto", { location }), middlePassage.locator);
    const middleParagraph = page.frames().find(f => f.url() === "about:srcdoc").locator("p").filter({ hasText: "Offline paragraph 70." });
    const visibleMiddle = async () => {
      const bounds = await middleParagraph.boundingBox();
      assert(bounds && bounds.x >= 0 && bounds.x < 950 && bounds.y >= 0 && bounds.y < 750,
        `Current reading passage stays visible after text reflow: ${JSON.stringify(bounds)}`);
    };
    await page.waitForTimeout(150);
    await visibleMiddle();
    await page.evaluate(() => send("size", { value: 200 }));
    await middleParagraph.evaluate(el => new Promise(resolve => {
      const check = () => parseFloat(getComputedStyle(el).fontSize) === 20 ? resolve() : requestAnimationFrame(check);
      check();
    }));
    await page.waitForTimeout(150);
    await visibleMiddle();
    await page.evaluate(() => send("size", { value: 100 }));
    const secondChapter = narration.find(segment => segment.text.includes("Return to the first passage."));
    assert(secondChapter, "narration includes the second chapter before it has been displayed");
    await page.evaluate(location => send("goto", { location }), secondChapter.locator);
    await page.waitForFunction(() => messages.findLast(m => m.type === "location")?.label.includes("Second chapter"));
    const secondFrame = page.frames().find(f => f.url() === "about:srcdoc");
    assert.equal(await secondFrame.locator("body").evaluate(el => getComputedStyle(el).backgroundColor), "rgb(0, 0, 0)");
    assert.equal(await secondFrame.locator("p").evaluate(el => getComputedStyle(el).color), "rgb(0, 255, 65)");
    assert.equal(requests.length, 0, "full-spine narration extraction must remain offline");
    console.log("EPUB narration: bounded pagination, full spine, canonical jumpable CFI, and no script/network access passed.");
    await page.evaluate(location => { send("notes", { locations: [location] }); send("toc", { target: "two.xhtml" }); }, selection.location);
    await page.waitForFunction(() => messages.some(m => m.type === "location" && m.label.includes("Second chapter")));
    await page.setViewportSize({ width: 780, height: 700 });
    await page.evaluate(location => {
      document.querySelector("iframe").style.width = "700px";
      send("size", { value: 150 }); send("goto", { location });
    }, selection.location);
    await page.waitForFunction(() => messages.findLast(m => m.type === "location")?.label.includes("First chapter"));
    assert.deepEqual(await page.evaluate(() => messages.filter(m => m.type === "error")), []);
    await open("epub", epub(), selection.location);
    assert.match(await page.evaluate(() => messages.findLast(m => m.type === "location").label), /First chapter/);
    console.log("EPUB: render, TOC, selection, annotation, resize/font jump, reopen, script/network/IPC isolation passed.");

    await open("epub", epub());
    const navigationFrame = page.frames().find(f => f.url().startsWith("data:text/html"));
    const navigationChapter = page.frames().find(f => f.url() === "about:srcdoc");
    const currentCFI = () => page.evaluate(() => messages.findLast(m => m.type === "location").location.cfi);
    await navigationChapter.locator("#passage").click();
    for (const key of ["PageDown", "PageUp", "ArrowRight", "ArrowLeft"]) {
      const before = await currentCFI();
      await page.keyboard.press(key);
      await page.waitForFunction(value => messages.findLast(m => m.type === "location").location.cfi !== value, before);
      await page.waitForTimeout(120);
    }
    let wheelBefore = await currentCFI();
    await page.mouse.move(450, 400); await page.mouse.wheel(0, 120);
    await page.waitForFunction(value => messages.findLast(m => m.type === "location").location.cfi !== value, wheelBefore);
    await page.waitForTimeout(200);
    wheelBefore = await currentCFI(); await page.mouse.wheel(0, -120);
    await page.waitForFunction(value => messages.findLast(m => m.type === "location").location.cfi !== value, wheelBefore);
    await page.waitForTimeout(200);
    let untouched = await currentCFI();
    await navigationChapter.evaluate(() => document.dispatchEvent(new KeyboardEvent("keydown", { key: "PageDown", bubbles: true })));
    await page.keyboard.press("Control+PageDown");
    assert.equal(await currentCFI(), untouched, "Untrusted and modified paging do not navigate");
    await page.evaluate(() => { messages = messages.filter(m => m.type !== "selection"); });
    const canonicalSelection = await selectBook();
    await page.evaluate(() => send("capture-bookmark", { requestId: "selected-bookmark" }));
    await page.waitForFunction(() => messages.some(m => m.type === "bookmark-captured" && m.requestId === "selected-bookmark"));
    const selectedBookmark = await page.evaluate(() => messages.find(m => m.type === "bookmark-captured" && m.requestId === "selected-bookmark"));
    assert.deepEqual(selectedBookmark.location, canonicalSelection.location);
    assert.equal(selectedBookmark.quote, canonicalSelection.quote);
    await page.keyboard.press("Control+Alt+m"); await page.keyboard.press("Control+Alt+b");
    await page.waitForFunction(() => messages.some(m => m.type === "bookmark") && messages.some(m => m.type === "toggle-bionic"));
    await navigationChapter.evaluate(() => getSelection().removeAllRanges());
    const visiblePage = await currentCFI();
    await page.evaluate(() => send("capture-bookmark", { requestId: "page-bookmark" }));
    await page.waitForFunction(() => messages.some(m => m.type === "bookmark-captured" && m.requestId === "page-bookmark"));
    const pageBookmark = await page.evaluate(() => messages.find(m => m.type === "bookmark-captured" && m.requestId === "page-bookmark"));
    assert.equal(pageBookmark.location.cfi, visiblePage, "Without selection, capture the visible page range");
    assert(pageBookmark.quote.trim().length > 0 && pageBookmark.quote.length <= 512);
    await page.evaluate(location => send("goto", { location, select: true }), selectedBookmark.location);
    await navigationChapter.waitForFunction(quote => getSelection().toString() === quote, canonicalSelection.quote);
    untouched = await currentCFI();
    await page.keyboard.press("PageDown");
    assert.equal(await currentCFI(), untouched, "Paging never steals a selected passage");
    await selectBook();
    await page.evaluate(() => send("bionic", { enabled: true }));
    await navigationChapter.waitForFunction(() => {
      const walker = document.createTreeWalker(document.querySelector("#passage"), NodeFilter.SHOW_TEXT);
      while (walker.nextNode()) if (parseFloat(getComputedStyle(walker.currentNode.parentElement).fontWeight) >= 700) return true;
      return false;
    });
    await navigationChapter.waitForFunction(quote => getSelection().toString() === quote, canonicalSelection.quote, { timeout: 5000 }).catch(async cause => {
      console.error("Bionic selection diagnostics:", await navigationFrame.evaluate(cfi => {
        const view = document.querySelector("foliate-view"), doc = view.renderer.getContents()[0].doc;
        const selection = doc.getSelection();
        return { quote: view.resolveCFI(cfi).anchor(doc).toString(), selected: selection.toString(),
          anchor: selection.anchorNode?.nodeName, offset: selection.anchorOffset, focus: selection.focusOffset };
      }, canonicalSelection.location.cfi), await page.evaluate(() => messages.slice(-5)));
      throw cause;
    });
    await page.evaluate(() => { messages = messages.filter(m => m.type !== "selection"); });
    const bionicSelection = await selectBook();
    assert.deepEqual(bionicSelection.location, canonicalSelection.location, "Bionic selection retains the exact canonical CFI");
    const resolveQuote = location => navigationFrame.evaluate(cfi => {
      const view = document.querySelector("foliate-view");
      return view.resolveCFI(cfi).anchor(view.renderer.getContents()[0].doc).toString();
    }, location.cfi);
    assert.equal(await resolveQuote(canonicalSelection.location), canonicalSelection.quote, "Pre-Bionic notes resolve with Bionic on");
    await page.evaluate(location => {
      send("notes", { locations: [location] }); send("flow", { value: "scrolled" }); send("size", { value: 200 });
      document.querySelector("iframe").style.width = "320px";
    }, canonicalSelection.location);
    await navigationFrame.waitForFunction(() => document.querySelector("foliate-view").renderer.scrolled);
    await navigationChapter.waitForFunction(() => parseFloat(getComputedStyle(document.querySelector("#passage")).fontSize) > 26);
    await page.waitForTimeout(250);
    assert.equal(await resolveQuote(canonicalSelection.location), canonicalSelection.quote, "Narrow scrolling reflow preserves notes");
    const narrowParagraph = await navigationChapter.locator("#passage").boundingBox();
    assert(narrowParagraph.width <= 320 && narrowParagraph.height > 30, `Large text genuinely wraps to narrow width: ${JSON.stringify(narrowParagraph)}`);
    await navigationChapter.evaluate(() => getSelection().removeAllRanges());
    await navigationChapter.locator("#passage").click();
    const scrollState = () => navigationFrame.evaluate(() => {
      const renderer = document.querySelector("foliate-view").renderer;
      return { start: renderer.start, end: renderer.end, size: renderer.size,
        total: renderer.viewSize, index: renderer.getContents()[0].index };
    });
    const beforeScroll = await scrollState();
    await page.keyboard.press("PageDown");
    await navigationFrame.waitForFunction(start => document.querySelector("foliate-view").renderer.start > start, beforeScroll.start);
    const afterScroll = await scrollState();
    assert.equal(afterScroll.index, beforeScroll.index, "Large text steps within the chapter, not past unread content");
    assert(afterScroll.start - beforeScroll.start <= (beforeScroll.size - 48) * .91, "Viewport steps retain overlapping text");
    await page.evaluate(location => send("goto", { location }), chunks.at(-1).locator);
    await page.waitForTimeout(150);
    for (let attempt = 0; attempt < 12 && (await scrollState()).index === 0; attempt++) {
      const state = await scrollState();
      await page.mouse.move(160, 350);
      await page.mouse.wheel(0, Math.max(800, state.total));
      await page.waitForTimeout(200);
    }
    assert.equal((await scrollState()).index, 1, "Scrolling flows automatically into the next chapter");
    await page.mouse.wheel(0, -1000); await page.waitForTimeout(250);
    assert.equal((await scrollState()).index, 0, "Scrolling back crosses the chapter boundary");
    await page.evaluate(location => {
      send("bionic", { enabled: false }); send("flow", { value: "paginated" }); send("goto", { location });
    }, bionicSelection.location);
    await navigationFrame.waitForFunction(() => !document.querySelector("foliate-view").renderer.scrolled);
    await page.waitForTimeout(150);
    assert.equal(await resolveQuote(bionicSelection.location), canonicalSelection.quote, "Bionic-created notes resolve after disabling and changing chapters");
    assert.deepEqual(await page.evaluate(() => messages.filter(m => m.type === "error")), []);
    await open("epub", epub(), bionicSelection.location);
    const reopenedRuntime = page.frames().find(f => f.url().startsWith("data:text/html"));
    assert.equal(await reopenedRuntime.evaluate(cfi => {
      const view = document.querySelector("foliate-view");
      return view.resolveCFI(cfi).anchor(view.renderer.getContents()[0].doc).toString();
    }, bionicSelection.location.cfi), canonicalSelection.quote, "Bionic notes survive a pristine reader restart");
    await open("epub", epub(false, true));
    assert.equal(await page.evaluate(() => messages.find(m => m.type === "ready").direction), "rtl");
    await page.frames().find(f => f.url() === "about:srcdoc").locator("#passage").click();
    untouched = await currentCFI();
    await page.keyboard.press("ArrowLeft");
    await page.waitForFunction(value => messages.findLast(m => m.type === "location").location.cfi !== value, untouched);
    console.log("Navigation: trusted page/arrows, RTL, narrow 200% text, continuous chapter boundaries, Bionic canonical notes/restart/selection passed.");

    if (browserType === chromium) {
      await page.setViewportSize({ width: 1000, height: 850 });
      await open("epub", epub());
      const cdp = await page.context().newCDPSession(page);
      const swipe = async (points, multi = false) => {
        const touchPoints = p => [{ x: p[0], y: p[1], id: 1 },
          ...(multi ? [{ x: p[0] + 50, y: p[1] + 50, id: 2 }] : [])];
        await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: touchPoints(points[0]) });
        for (const p of points.slice(1))
          await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: touchPoints(p) });
        await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
      };
      const cfi = () => page.evaluate(() => messages.findLast(m => m.type === "location").location.cfi);
      let before = await cfi();
      await swipe([[650, 400], [590, 400], [450, 400], [300, 400]]);
      await page.waitForFunction(value => messages.findLast(m => m.type === "location").location.cfi !== value, before);
      before = await cfi();
      await swipe([[300, 400], [380, 400], [500, 400], [650, 400]]);
      await page.waitForFunction(value => messages.findLast(m => m.type === "location").location.cfi !== value, before);
      const touchChapter = page.frames().find(f => f.url() === "about:srcdoc");
      await selectBook();
      before = await cfi();
      await swipe([[650, 400], [590, 400], [450, 400], [300, 400]]);
      assert.equal(await cfi(), before, "Swipe never steals an existing text selection");
      await touchChapter.evaluate(() => getSelection().removeAllRanges());
      await swipe([[450, 300], [450, 350], [450, 430]]);
      assert.equal(await cfi(), before, "Vertical gestures never turn a page");
      await swipe([[450, 300], [400, 300], [350, 300]], true);
      assert.equal(await cfi(), before, "Multitouch never turns a page");
      await cdp.send("Emulation.setPageScaleFactor", { pageScaleFactor: 2 });
      await swipe([[650, 400], [590, 400], [450, 400], [300, 400]]);
      assert.equal(await cfi(), before, "Single-finger panning after pinch zoom never turns a page");
      // Reset any visual viewport zoom introduced by the browser's pinch behavior.
      await cdp.send("Emulation.setPageScaleFactor", { pageScaleFactor: 1 });
      const controls = () => page.evaluate(() => messages.filter(m => m.type === "toggle-controls").length);
      let count = await controls();
      await page.touchscreen.tap(485, 400);
      await page.waitForFunction(n => messages.filter(m => m.type === "toggle-controls").length === n + 1, count);
      count = await controls();
      await page.touchscreen.tap(850, 650);
      await page.waitForFunction(n => messages.filter(m => m.type === "toggle-controls").length === n + 1, count);
      count = await controls();
      await selectBook();
      await page.touchscreen.tap(850, 650);
      await page.waitForTimeout(350);
      assert.equal(await controls(), count, "Tapping to dismiss a selection never toggles controls");
      await touchChapter.evaluate(() => getSelection().removeAllRanges());
      await touchChapter.locator("#local-link").tap();
      await page.waitForTimeout(350);
      assert.equal(await controls(), count, "Link taps never toggle controls");
      await touchChapter.evaluate(() => document.dispatchEvent(new KeyboardEvent("keydown", { key: "F11", bubbles: true })));
      assert.equal(await page.evaluate(() => messages.filter(m => m.type === "toggle-fullscreen").length), 0,
        "Untrusted synthetic keyboard events cannot request fullscreen");
      await touchChapter.evaluate(() => document.dispatchEvent(new KeyboardEvent("keydown", { key: "F8", bubbles: true })));
      assert.equal(await controls(), count, "Untrusted synthetic shortcuts cannot toggle controls");
      await page.setViewportSize({ width: 390, height: 844 });
      await page.evaluate(() => { document.querySelector("iframe").style.cssText = "width:350px;height:740px"; });
      await page.waitForTimeout(200);
      before = await cfi();
      await swipe([[290, 410], [240, 410], [180, 410], [90, 410]]);
      await page.waitForFunction(value => messages.findLast(m => m.type === "location").location.cfi !== value, before);
      await page.evaluate(() => send("flow", { value: "scrolled" }));
      const phoneRuntime = page.frames().find(f => f.url().startsWith("data:text/html"));
      await phoneRuntime.waitForFunction(() => document.querySelector("foliate-view").renderer.scrolled);
      await page.waitForTimeout(150);
      const phoneStart = await phoneRuntime.evaluate(() => document.querySelector("foliate-view").renderer.start);
      await swipe([[175, 600], [175, 530], [175, 430], [175, 280]]);
      await phoneRuntime.waitForFunction(start => document.querySelector("foliate-view").renderer.start > start, phoneStart);
      await page.setViewportSize({ width: 1000, height: 850 });
      await cdp.detach();
      console.log("Reader interactions: trusted touch paging, selection/vertical/pinch guards, surface taps, link exclusion, F8/fullscreen keys passed.");
    }

    const fb2 = strToU8(`<?xml version="1.0"?><FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0"><description><title-info><genre>prose</genre><author><first-name>Test</first-name><last-name>Fixture</last-name></author><book-title>FB2 Fixture</book-title><lang>en</lang></title-info></description><body><section id="chapter"><title><p>FB2 chapter</p></title><p id="passage">Select this original passage in FB2.</p></section></body></FictionBook>`);
    await open("fb2", fb2);
    await selectBook();
    console.log("FB2: local render and synthetic CFI selection passed.");

    await open("mobi", mobi());
    await selectBook();
    const mobiFrame = page.frames().find(f => f.url() === "about:srcdoc");
    assert.equal(await mobiFrame.locator("a[href]").getAttribute("href"), "filepos:0");
    await mobiFrame.locator("a[href]").click();
    console.log("MOBI: local PalmDOC render, synthetic CFI selection and filepos navigation passed.");

    await open("epub", epub(true));
    assert.equal(await page.evaluate(() => messages.find(m => m.type === "ready").annotations), false);
    assert.match(await page.evaluate(() => messages.find(m => m.type === "ready").notice), /Fixed-layout/);
    const fixedFrame = page.frames().find(f => f.url() === "about:srcdoc");
    assert.equal(await fixedFrame.locator("body").evaluate(el => getComputedStyle(el).backgroundColor), "rgb(255, 255, 255)",
      "Fixed-layout publisher backgrounds remain unchanged");
    assert.equal(await fixedFrame.locator("#passage").evaluate(el => getComputedStyle(el).color), "rgb(0, 0, 0)");
    const fixedFont = await fixedFrame.locator("#passage").evaluate(el => getComputedStyle(el).fontSize);
    await page.evaluate(() => send("size", { value: 200 }));
    await page.waitForTimeout(100);
    assert.equal(await fixedFrame.locator("#passage").evaluate(el => getComputedStyle(el).fontSize), fixedFont,
      "Fixed-layout publisher typography is not scaled");
    console.log("Fixed-layout EPUB: rendering works; unsupported passage annotations are explicitly unavailable.");

    await open("pdf", pdf());
    const pdfFrame = page.frames().find(f => f.url().startsWith("data:text/html"));
    const pdfPixels = await pdfFrame.locator(".pdf-page canvas").evaluate(canvas => [...canvas.getContext("2d").getImageData(0, 0, 1, 1).data]);
    await page.evaluate(theme => send("theme", { theme: { ...theme, background: "#123456" } }), matrix);
    await pdfFrame.waitForFunction(() => document.body.style.backgroundColor === "rgb(18, 52, 86)");
    assert.deepEqual(await pdfFrame.locator(".pdf-page canvas").evaluate(canvas => [...canvas.getContext("2d").getImageData(0, 0, 1, 1).data]), pdfPixels,
      "Theme changes never recolor or rerender PDF pixels");
    if (browserType === chromium) {
      const cdp = await page.context().newCDPSession(page);
      await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ x: 450, y: 500, id: 1 }] });
      for (const y of [460, 400, 300])
        await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: [{ x: 450, y, id: 1 }] });
      await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
      await pdfFrame.waitForFunction(() => document.querySelector("#reader").scrollTop > 0);
      assert.equal(await page.evaluate(() => messages.findLast(m => m.type === "location").location.page), 1,
        "PDF vertical touch scrolling remains native and does not turn pages");
      await pdfFrame.evaluate(() => { document.querySelector("#reader").scrollTop = 0; });
      await cdp.detach();
    }
    await pdfFrame.locator(".textLayer span").first().click();
    await page.keyboard.press("F1");
    await page.waitForFunction(() => messages.some(m => m.type === "help"));
    assert(await pdfFrame.locator(".textLayer span").first().evaluate(async element => {
      await new Promise(resolve => setTimeout(resolve, 200));
      return element.isConnected;
    }), "Initial observer and height-only changes must not replace an unchanged PDF text layer");
    await pdfFrame.locator(".textLayer span").first().evaluate(el => {
      const range = document.createRange(); range.selectNodeContents(el);
      getSelection().removeAllRanges(); getSelection().addRange(range);
    });
    await page.waitForFunction(() => messages.some(m => m.type === "selection"));
    const pdfSelection = await page.evaluate(() => messages.findLast(m => m.type === "selection"));
    assert.equal(pdfSelection.location.page, 1);
    assert(pdfSelection.location.rects.length);
    for (const rect of pdfSelection.location.rects) for (const value of Object.values(rect)) assert(value >= 0 && value <= 1);
    await page.evaluate(location => { send("notes", { locations: [location] }); send("next"); }, pdfSelection.location);
    await page.waitForFunction(() => messages.findLast(m => m.type === "location")?.location.page === 2);
    await pdfFrame.locator(".textLayer span").first().evaluate(el => {
      const range = document.createRange(); range.selectNodeContents(el);
      getSelection().removeAllRanges(); getSelection().addRange(range);
    });
    await page.waitForFunction(() => messages.findLast(m => m.type === "selection")?.location.page === 2);
    const rotated = await page.evaluate(() => messages.findLast(m => m.type === "selection").location.rects[0]);
    assert(rotated.x > .08 && rotated.x < .12 && rotated.y > 0 && rotated.y < .2,
      `Rotated selection must be normalized against the unrotated page: ${JSON.stringify(rotated)}`);
    assert(rotated.width > .3 && rotated.height < .1);
    await page.evaluate(location => send("goto", { location }), pdfSelection.location);
    await page.waitForFunction(() => messages.findLast(m => m.type === "location")?.location.page === 1);
    assert(await pdfFrame.locator(".pdf-highlights i").count() > 0);
    await page.evaluate(() => send("size", { value: 200 }));
    await pdfFrame.waitForFunction(() => document.querySelector(".pdf-page").getBoundingClientRect().height > 1500);
    await pdfFrame.locator("#reader").evaluate(el => {
      getSelection().removeAllRanges(); el.tabIndex = 0; el.focus(); el.scrollTop = 0;
    });
    await page.keyboard.press("PageDown");
    await pdfFrame.waitForFunction(() => document.querySelector("#reader").scrollTop > 0);
    assert.equal(await page.evaluate(() => messages.findLast(m => m.type === "location").location.page), 1,
      "Page Down moves through enlarged PDF text before changing pages");
    for (let attempt = 0; attempt < 12 && await page.evaluate(() => messages.findLast(m => m.type === "location").location.page === 1); attempt++) {
      await page.keyboard.press("PageDown"); await page.waitForTimeout(120);
    }
    assert.equal(await page.evaluate(() => messages.findLast(m => m.type === "location").location.page), 2);
    await page.keyboard.press("PageUp");
    await page.waitForFunction(() => messages.findLast(m => m.type === "location").location.page === 1);
    assert(await pdfFrame.locator("#reader").evaluate(el => el.scrollTop > 0), "Going back lands at the previous PDF page's bottom");
    await page.mouse.move(450, 400); await page.mouse.wheel(0, 120);
    await page.waitForFunction(() => messages.findLast(m => m.type === "location").location.page === 2);
    await page.waitForTimeout(150);
    await page.mouse.wheel(0, -120);
    await page.waitForFunction(() => messages.findLast(m => m.type === "location").location.page === 1);
    await open("pdf", pdf(), { kind: "pdf", page: 2 });
    assert.equal(await page.evaluate(() => messages.findLast(m => m.type === "location").location.page), 2);
    assert.equal(await page.evaluate(() => pwned), false);
    console.log("PDF: direct PDF.js canvas/text, normalized rectangles, page jump, highlights and reopen passed.");
    assert.deepEqual(browserErrors, [], "Reader lifecycle must not leave uncaught browser errors.");
  } finally { await browser.close(); await new Promise(resolve => server.close(resolve)); }
}

module.exports = { epub, mobi, pdf };
if (require.main === module) main().catch(error => { console.error(error); process.exitCode = 1; });

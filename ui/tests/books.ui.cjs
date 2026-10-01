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

function epub(fixed = false) {
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
    "book.opf": `<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="uid">synthetic</dc:identifier><dc:title>Offline Reader Test</dc:title><dc:language>en</dc:language>${fixed ? '<meta property="rendition:layout">pre-paginated</meta>' : ""}</metadata><manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="one" href="one.xhtml" media-type="application/xhtml+xml"/><item id="two" href="two.xhtml" media-type="application/xhtml+xml"/><item id="font" href="font.ttf" media-type="font/ttf"/></manifest><spine><itemref idref="one"/><itemref idref="two"/></spine></package>`,
    "nav.xhtml": `<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>Contents</title></head><body><nav epub:type="toc"><ol><li><a href="one.xhtml">First chapter</a></li><li><a href="two.xhtml">Second chapter</a></li></ol></nav></body></html>`,
    "one.xhtml": `<html xmlns="http://www.w3.org/1999/xhtml"><head><title>One</title><meta name="viewport" content="width=600,height=800"/><style>@import "https://blocked.invalid/style";@font-face{font-family:FixtureFont;src:url(font.ttf)}#passage{font-family:FixtureFont}body{background-image:url(https://blocked.invalid/css)}</style></head><body onload="top.pwned=true"><script>top.pwned=true;fetch('https://blocked.invalid/script')</script><h1>First chapter</h1><p id="passage">Select this original passage for a saved note.</p><img src="https://blocked.invalid/image" onerror="top.pwned=true"/><a href="javascript:top.pwned=true">Unsafe link</a>${repeated}</body></html>`,
    "two.xhtml": `<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Two</title><meta name="viewport" content="width=600,height=800"/></head><body><h1>Second chapter</h1><p>Return to the first passage.</p></body></html>`,
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
  const page = await browser.newPage({ viewport: { width: 1000, height: 850 } });
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
      await page.evaluate(({ runtime, format, data, location }) => {
        document.querySelector("iframe")?.remove();
        window.messages = [];
        const frame = document.createElement("iframe");
        frame.style.cssText = "width:950px;height:750px";
        frame.sandbox = BookSecurity.BOOK_FRAME_SANDBOX;
        frame.src = BookSecurity.readerFrameURL("test-secret");
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
      }, { runtime, format, data: [...bytes], location });
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
        const range = document.createRange(); range.selectNodeContents(el);
        getSelection().removeAllRanges(); getSelection().addRange(range);
      });
      await page.waitForFunction(() => messages.some(m => m.type === "selection"));
      return page.evaluate(() => messages.findLast(m => m.type === "selection"));
    };
    await open("epub", epub());
    const selection = await selectBook();
    await page.frames().find(f => f.url() === "about:srcdoc").locator("#passage").click();
    await page.keyboard.press("F1");
    await page.waitForFunction(() => messages.some(m => m.type === "help"));
    assert.match(selection.quote, /original passage/);
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
    const secondChapter = narration.find(segment => segment.text.includes("Return to the first passage."));
    assert(secondChapter, "narration includes the second chapter before it has been displayed");
    await page.evaluate(location => send("goto", { location }), secondChapter.locator);
    await page.waitForFunction(() => messages.findLast(m => m.type === "location")?.label.includes("Second chapter"));
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
    console.log("Fixed-layout EPUB: rendering works; unsupported passage annotations are explicitly unavailable.");

    await open("pdf", pdf());
    const pdfFrame = page.frames().find(f => f.url().startsWith("data:text/html"));
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
    await open("pdf", pdf(), { kind: "pdf", page: 2 });
    assert.equal(await page.evaluate(() => messages.findLast(m => m.type === "location").location.page), 2);
    assert.equal(await page.evaluate(() => pwned), false);
    console.log("PDF: direct PDF.js canvas/text, normalized rectangles, page jump, highlights and reopen passed.");
    assert.deepEqual(browserErrors, [], "Reader lifecycle must not leave uncaught browser errors.");
  } finally { await browser.close(); await new Promise(resolve => server.close(resolve)); }
}

module.exports = { epub, mobi, pdf };
if (require.main === module) main().catch(error => { console.error(error); process.exitCode = 1; });

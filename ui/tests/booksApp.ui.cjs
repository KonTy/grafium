const { chromium } = require("playwright");
const { zipSync, strToU8 } = require("fflate");
const assert = require("node:assert/strict");
const { openEditor } = require("./keyboardSelection.ui.cjs");

function syntheticBook() {
  return zipSync(Object.fromEntries(Object.entries({
    mimetype: "application/epub+zip",
    "META-INF/container.xml": '<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0"><rootfiles><rootfile full-path="book.opf" media-type="application/oebps-package+xml"/></rootfiles></container>',
    "book.opf": '<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">grafium-integration</dc:identifier><dc:title>Integration book</dc:title><dc:language>en</dc:language></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="chapter"/></spine></package>',
    "chapter.xhtml": '<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Chapter</title></head><body><h1>Original book</h1><p id="passage">A preserved original can have independently saved reading notes.</p></body></html>',
  }).map(([path, content]) => [path, strToU8(content)])));
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const { page, errors } = await openEditor(browser, {
      beforeNavigate: (page) => page.addInitScript((bytes) => {
        if (window !== window.top) return;
        const id = "759c230d-9aa0-4a9e-9179-30f373dd7d2b";
        const info = {
          id, pageId: id, title: "Books/Integration book", format: "epub",
          filePath: `books/${id}/original.epub`, sourceSha256: "a".repeat(64),
          readingLocation: null, indexingWarning: "Synthetic partial index warning.",
        };
        function install(internals) {
          const state = window.__selectionState;
          state.pages.push({
            ...state.pages[0], id, title: info.title, file_path: info.filePath,
            properties: {
              "book-id": id, "book-format": "epub", "book-source": info.filePath,
              "book-source-sha256": info.sourceSha256,
            },
          });
          const original = internals.invoke;
          window.__bookFixture = { info, notes: [], writes: [], sourceChanged: false };
          internals.invoke = async (command, args = {}) => {
            const fixture = window.__bookFixture;
            if (command === "book_open") {
              if (args.pageId !== id || args.graphPath !== "/synthetic/keyboard-selection") throw new Error("Wrong book context");
              return { ...info, sourceSha256: fixture.sourceChanged ? "b".repeat(64) : info.sourceSha256 };
            }
            if (command === "book_read_bytes") return Uint8Array.from(bytes).buffer;
            if (command === "book_notes_list") return structuredClone(fixture.notes);
            if (command === "book_save_position") {
              fixture.writes.push({ command, args: structuredClone(args) });
              info.readingLocation = structuredClone(args.location);
              return;
            }
            if (command === "book_note_save") {
              fixture.writes.push({ command, args: structuredClone(args) });
              const existing = fixture.notes.find(note => note.id === args.noteId);
              if (existing && existing.revision !== args.expectedRevision) throw new Error("Revision conflict");
              const note = {
                id: args.noteId, bookId: id, notePageId: args.noteId,
                filePath: `pages/Reading Notes/Books/${id}/${args.noteId}.md`,
                body: args.body, quote: args.quote, locator: structuredClone(args.locator),
                sourceSha256: args.sourceSha256, revision: `revision-${fixture.writes.length}`,
                createdAt: "2026-09-30T06:00:00Z", updatedAt: "2026-09-30T06:00:00Z", status: "attached",
              };
              fixture.notes = fixture.notes.filter(saved => saved.id !== note.id).concat(note);
              return structuredClone(note);
            }
            return original(command, args);
          };
          return internals;
        }
        let internals = window.__TAURI_INTERNALS__;
        if (internals) internals = install(internals);
        Object.defineProperty(window, "__TAURI_INTERNALS__", {
          configurable: true, get: () => internals, set: (value) => { internals = install(value); },
        });
      }, [...syntheticBook()]),
    });
    page.setDefaultTimeout(15000);
    const openBook = () => page.evaluate(() => window.dispatchEvent(new CustomEvent("navigate-page", {
      detail: { pageId: window.__bookFixture.info.pageId, pageName: window.__bookFixture.info.title },
    })));
    const chapterLoaded = page.waitForEvent("framenavigated", { predicate: frame => frame.url() === "about:srcdoc" });
    await openBook();
    const reader = page.getByRole("region", { name: "Original book reader" });
    await reader.waitFor();
    await reader.getByRole("button", { name: "Next", exact: true }).waitFor();
    await page.waitForFunction(() => !document.querySelector(".book-reader button")?.disabled);
    const chapter = await chapterLoaded;
    const passage = chapter.locator("#passage");
    try {
      await passage.waitFor();
    } catch (error) {
      console.error("Reader state:", await reader.innerText(), "Browser errors:", errors);
      for (const frame of page.frames()) {
        console.error("Frame:", frame.url().slice(0, 70), await frame.evaluate(() => document.body?.innerText.slice(0, 300)));
      }
      throw error;
    }
    await passage.evaluate(element => {
      const range = document.createRange();
      range.selectNodeContents(element);
      getSelection().removeAllRanges();
      getSelection().addRange(range);
    });
    await reader.getByRole("button", { name: /notes/i }).first().click();
    const notes = page.getByRole("region", { name: "Original book notes" });
    await notes.waitFor();
    await notes.getByRole("button", { name: "Use selection", exact: true }).click();
    await notes.getByRole("textbox", { name: "Book note", exact: true }).fill("My independent reading note #study");
    await notes.getByRole("button", { name: "Save note", exact: true }).click();
    await notes.locator(".book-note-card").filter({ hasText: "My independent reading note" }).waitFor();
    const stored = await page.evaluate(() => window.__bookFixture.notes);
    assert.equal(stored.length, 1);
    assert.equal(stored[0].quote, "A preserved original can have independently saved reading notes.");
    assert.equal(stored[0].locator.kind, "epub");
    assert.match(stored[0].locator.cfi, /^epubcfi\(/);
    assert.equal(stored[0].sourceSha256, "a".repeat(64));
    await notes.getByRole("button", { name: "Return to passage", exact: true }).click();
    await page.evaluate(() => window.dispatchEvent(new CustomEvent("navigate-page", { detail: "Keyboard selection" })));
    await page.locator('[data-block-id="b0"]').first().waitFor();
    await openBook();
    await reader.waitFor();
    await page.waitForFunction(() => !document.querySelector(".book-reader button")?.disabled);
    await reader.getByRole("button", { name: /notes/i }).first().click();
    await notes.locator(".book-note-card").filter({ hasText: "My independent reading note" }).waitFor();
    await notes.getByRole("button", { name: "Return to passage", exact: true }).click();
    await page.evaluate(() => {
      window.__bookFixture.sourceChanged = true;
      window.dispatchEvent(new Event("focus"));
    });
    await reader.getByText(/Stale read-only snapshot/).waitFor();
    assert.equal(await reader.getByRole("button", { name: "Next", exact: true }).isDisabled(), true);
    assert.deepEqual(errors, []);
    console.log("Book app integration: original routing, reader selection, companion note save/reopen/jump and source invalidation passed");
    await page.close();
  } finally {
    await browser.close();
  }
})().catch((error) => { console.error(error); process.exitCode = 1; });

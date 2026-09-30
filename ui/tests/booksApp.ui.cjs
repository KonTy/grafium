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
          window.__bookFixture = { info, notes: [], writes: [], sourceChanged: false, sourceMissing: false, staleNextResolve: false };
          function indexNote(note) {
            const annotationPage = {
              ...state.pages[0], id: note.id, title: `Reading Notes/Books/${id}/${note.id}`,
              file_path: `${note.filePath}#${note.id}`,
              properties: { "book-annotation": true, "book-note": JSON.stringify({ id: note.id, bookId: id }) },
            };
            const index = state.pages.findIndex(page => page.id === note.id);
            if (index < 0) state.pages.push(annotationPage);
            else state.pages[index] = annotationPage;
          }
          internals.invoke = async (command, args = {}) => {
            const fixture = window.__bookFixture;
            if (command === "book_open" || command === "book_notes_context") {
              if ((args.pageId ?? args.bookId) !== id || args.graphPath !== "/synthetic/keyboard-selection") throw new Error("Wrong book context");
              if (command === "book_open" && fixture.sourceMissing) throw new Error("Original source is missing");
              return { ...info, sourceAvailable: !fixture.sourceMissing,
                sourceSha256: fixture.sourceMissing ? "" : fixture.sourceChanged ? "b".repeat(64) : info.sourceSha256 };
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
              if (existing?.conflicts.length) throw new Error("Unresolved annotation conflict");
              if (existing && existing.revision !== args.expectedRevision) throw new Error("Revision conflict");
              const note = {
                id: args.noteId, bookId: id, notePageId: args.noteId,
                filePath: `books/${id}/original.jsonld`,
                body: args.body, quote: args.quote, locator: structuredClone(args.locator),
                sourceSha256: args.sourceSha256, revision: `revision-${fixture.writes.length}`,
                createdAt: "2026-09-30T06:00:00Z", updatedAt: "2026-09-30T06:00:00Z", status: "attached", conflicts: [],
              };
              fixture.notes = fixture.notes.filter(saved => saved.id !== note.id).concat(note);
              indexNote(note);
              return structuredClone(note);
            }
            if (command === "book_note_resolve") {
              fixture.writes.push({ command, args: structuredClone(args) });
              const current = fixture.notes.find(note => note.id === args.noteId);
              if (!current?.conflicts.length || current.revision !== args.expectedRevision)
                throw new Error("Revision conflict");
              if (!/^[a-f0-9]{64}$/.test(args.sourceSha256)) throw new Error("Invalid source fingerprint");
              if (fixture.staleNextResolve) {
                fixture.staleNextResolve = false;
                current.conflicts.push({ ...current.conflicts[0], revision: "third-head", body: "Newest remote idea" });
                current.revision = "heads-2";
                throw new Error("Annotation changed; reload before saving");
              }
              if (args.delete) {
                fixture.notes = fixture.notes.filter(note => note.id !== args.noteId);
                state.pages = state.pages.filter(page => page.id !== args.noteId);
              } else {
                Object.assign(current, { body: args.body, quote: args.quote, locator: structuredClone(args.locator),
                  sourceSha256: args.sourceSha256, revision: "resolved-head", conflicts: [], status: "attached" });
                indexNote(current);
              }
              return;
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
    assert.match(stored[0].filePath, /\/original\.jsonld$/);
    await notes.getByRole("button", { name: "Open note", exact: true }).click();
    const annotationEditor = page.getByRole("region", { name: "Book annotation editor", exact: true });
    await annotationEditor.waitFor();
    await annotationEditor.getByRole("textbox", { name: "Book note", exact: true }).waitFor();
    assert.equal(await annotationEditor.getByRole("textbox", { name: "Book note", exact: true }).inputValue(),
      "My independent reading note #study");
    assert.equal(await annotationEditor.locator(".cm-editor").count(), 0);
    await annotationEditor.getByRole("button", { name: "Return to book", exact: true }).click();
    await reader.waitFor();
    await page.waitForFunction(() => !document.querySelector(".book-reader button")?.disabled);
    await reader.getByRole("button", { name: /notes/i }).first().click();
    await notes.waitFor();
    await notes.getByRole("button", { name: "Return to passage", exact: true }).click();
    await page.evaluate(() => {
      const fixture = window.__bookFixture, note = fixture.notes[0];
      const first = { revision: "first-head", body: "First device idea", quote: "First device passage",
        locator: note.locator, sourceSha256: note.sourceSha256, updatedAt: "2026-09-30T07:00:00Z", deleted: false };
      const second = { ...first, revision: "second-head", body: "Second device idea", quote: "Second device passage" };
      Object.assign(note, { body: "", quote: "", sourceSha256: "", locator: null, status: "conflicted",
        revision: "heads-1", conflicts: [first, second] });
    });
    await notes.getByRole("button", { name: "Refresh", exact: true }).click();
    await notes.locator(".conflict-candidate").filter({ hasText: "Second device idea" }).waitFor();
    assert.equal(await notes.locator(".conflict-candidate").count(), 2);
    assert.equal(await notes.getByRole("button", { name: "Remove", exact: true }).count(), 0);
    await notes.getByRole("button", { name: "Use candidate 1 text and attachment", exact: true }).click();
    const composer = notes.getByRole("textbox", { name: "Book note", exact: true });
    await composer.fill("Manual merge of both device ideas");
    assert.equal(await page.evaluate(() => window.__bookFixture.writes.filter(write => write.command === "book_note_resolve").length), 0);
    await page.evaluate(() => { window.__bookFixture.staleNextResolve = true; });
    await notes.getByRole("button", { name: "Resolve with merged note", exact: true }).click();
    await notes.getByRole("alert").filter({ hasText: "Could not resolve" }).waitFor();
    assert.equal(await composer.inputValue(), "Manual merge of both device ideas");
    assert.equal(await notes.locator(".conflict-candidate").count(), 3);
    assert.equal(await notes.getByRole("button", { name: "Resolve with merged note", exact: true }).isDisabled(), true);
    await notes.getByRole("button", { name: "Use candidate 2 attachment only", exact: true }).click();
    assert.equal(await composer.inputValue(), "Manual merge of both device ideas");
    await notes.getByRole("button", { name: "Resolve with merged note", exact: true }).click();
    await notes.getByRole("status").filter({ hasText: "Conflict resolved" }).waitFor();
    const resolution = await page.evaluate(() => window.__bookFixture.writes.filter(write => write.command === "book_note_resolve").at(-1).args);
    assert.equal(resolution.expectedRevision, "heads-2");
    assert.equal(resolution.quote, "Second device passage");
    assert.equal(resolution.body, "Manual merge of both device ideas");
    assert.equal(await notes.locator(".conflict-candidate").count(), 0);
    await page.evaluate(() => {
      const note = window.__bookFixture.notes[0];
      const candidate = { revision: "edited-head", body: note.body, quote: note.quote, locator: note.locator,
        sourceSha256: note.sourceSha256, updatedAt: "2026-09-30T08:00:00Z", deleted: false };
      Object.assign(note, { body: "", quote: "", sourceSha256: "", locator: null, status: "conflicted",
        revision: "deletion-heads", conflicts: [candidate, { ...candidate, revision: "deleted-head", deleted: true }] });
    });
    await notes.getByRole("button", { name: "Refresh", exact: true }).click();
    await notes.locator(".conflict-candidate").filter({ hasText: "Deleted" }).waitFor();
    await notes.getByRole("button", { name: "Resolve as deleted…", exact: true }).click();
    const confirmation = notes.getByRole("alertdialog", { name: "Resolve book note as deleted", exact: true });
    await confirmation.waitFor();
    assert.equal(await page.evaluate(() => window.__bookFixture.writes.filter(write => write.command === "book_note_resolve").length), 2);
    await confirmation.getByRole("button", { name: "Cancel", exact: true }).click();
    await notes.getByRole("button", { name: "Resolve as deleted…", exact: true }).click();
    await confirmation.getByRole("button", { name: "Confirm deletion resolution", exact: true }).click();
    await notes.getByRole("status").filter({ hasText: "Deletion resolved" }).waitFor();
    const deletion = await page.evaluate(() => window.__bookFixture.writes.filter(write => write.command === "book_note_resolve").at(-1).args);
    assert.equal(deletion.delete, true);
    assert.equal(deletion.expectedRevision, "deletion-heads");
    assert.equal(deletion.sourceSha256, "a".repeat(64));
    assert.equal(deletion.locator, null);
    assert.equal(await composer.inputValue(), "Manual merge of both device ideas");
    await page.evaluate(() => window.dispatchEvent(new CustomEvent("navigate-page", { detail: "Keyboard selection" })));
    await page.locator('[data-block-id="b0"]').first().waitFor();
    await openBook();
    await reader.waitFor();
    await page.waitForFunction(() => !document.querySelector(".book-reader button")?.disabled);
    await reader.getByRole("button", { name: /notes/i }).first().click();
    await notes.getByRole("textbox", { name: "Book note", exact: true }).waitFor();
    assert.equal(await notes.getByRole("textbox", { name: "Book note", exact: true }).inputValue(), "Manual merge of both device ideas");
    assert.equal(await notes.locator(".book-note-card").count(), 0);
    await page.evaluate(() => {
      window.__bookFixture.sourceChanged = true;
      window.dispatchEvent(new Event("focus"));
    });
    await reader.getByText(/Stale read-only snapshot/).waitFor();
    assert.equal(await reader.getByRole("button", { name: "Next", exact: true }).isDisabled(), true);
    await page.evaluate(saved => {
      const fixture = window.__bookFixture, state = window.__selectionState;
      fixture.sourceMissing = true;
      const candidate = { revision: "orphan-edit", body: saved.body, quote: saved.quote, locator: saved.locator,
        sourceSha256: saved.sourceSha256, updatedAt: "2026-09-30T09:00:00Z", deleted: false };
      fixture.notes = [{ ...saved, body: "", quote: "", locator: null, sourceSha256: "", status: "conflicted",
        revision: "orphan-heads", conflicts: [candidate, { ...candidate, revision: "orphan-delete", deleted: true }] }];
      state.pages = state.pages.filter(page => page.id !== fixture.info.id && page.id !== saved.id);
      state.pages.push({ ...state.pages[0], id: saved.id, title: "Orphan annotation",
        file_path: `${saved.filePath}#${saved.id}`, properties: { "book-annotation": true,
          "book-note-id": saved.id, "book-note-book-page-id": fixture.info.id } });
      window.dispatchEvent(new CustomEvent("navigate-page", { detail: { pageId: saved.id, pageName: "Orphan annotation" } }));
    }, stored[0]);
    await annotationEditor.getByText(/Original unavailable/).waitFor();
    assert.equal(await annotationEditor.getByRole("button", { name: "New note", exact: true }).isDisabled(), true);
    assert.equal(await annotationEditor.getByRole("button", { name: "Whole-book note", exact: true }).isDisabled(), true);
    await annotationEditor.getByRole("button", { name: "Resolve as deleted…", exact: true }).click();
    const orphanConfirmation = annotationEditor.getByRole("alertdialog", { name: "Resolve book note as deleted", exact: true });
    assert.equal(await orphanConfirmation.getByRole("button", { name: "Confirm deletion resolution", exact: true }).isDisabled(), true);
    await orphanConfirmation.getByRole("button", { name: "Cancel", exact: true }).click();
    await annotationEditor.getByRole("button", { name: "Use candidate 2 attachment only", exact: true }).click();
    await annotationEditor.getByRole("button", { name: "Resolve as deleted…", exact: true }).click();
    await orphanConfirmation.getByRole("button", { name: "Confirm deletion resolution", exact: true }).click();
    await annotationEditor.getByRole("status").filter({ hasText: "Deletion resolved" }).waitFor();
    const orphanDeletion = await page.evaluate(() => window.__bookFixture.writes.filter(write => write.command === "book_note_resolve").at(-1).args);
    assert.equal(orphanDeletion.expectedRevision, "orphan-heads");
    assert.equal(orphanDeletion.sourceSha256, stored[0].sourceSha256);
    assert.equal(orphanDeletion.quote, stored[0].quote);
    assert.deepEqual(orphanDeletion.locator, stored[0].locator);
    assert.deepEqual(errors, []);
    console.log("Book app integration: adjacent notes, virtual/orphan editor, explicit merge/deletion, stale drafts and reader navigation passed");
    await page.close();
  } finally {
    await browser.close();
  }
})().catch((error) => { console.error(error); process.exitCode = 1; });

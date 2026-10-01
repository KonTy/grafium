// Real Notes UI; synthetic persistence survives browser reload independently of component state.
const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor, focus, focusContinuous, row, frames, shift } = require("./keyboardSelection.ui.cjs");
const nativeFixture = require("./fixtures/inline-reading-notes-native.json");

const ROOT = "/synthetic/keyboard-selection";
const panel = (page) => page.locator(".reading-notes-panel");
const input = (page) => panel(page).getByRole("textbox", { name: "Reading note", exact: true });
const button = (page, name) => panel(page).getByRole("button", { name, exact: true });
const cards = (page) => panel(page).locator(".reading-note-card");

async function openNativeNotes(browser, options) {
  const initialBlockId = options.unifiedPage ? nativeFixture.notes[0].noteBlockId : nativeFixture.blocks[0].id;
  return openEditor(browser, {
    ...options, initialBlockId, componentHarness: !!options.unifiedPage,
    beforeNavigate: (page) => page.addInitScript(({ fixture, unifiedPage }) => {
      if (window !== window.top) return;
      localStorage.setItem("grafium.session.lastLocation", JSON.stringify({ kind: "page", title: fixture.page.title }));
      if (unifiedPage) localStorage.setItem(`grafium.experimental.unifiedPageEditor:${fixture.page.id}`, "1");
      function install(internals) {
        const state = window.__selectionState;
        state.pages = [fixture.page, ...state.pages.filter(({ id }) => id !== "selection-page")];
        state.blocks = [...fixture.blocks, ...state.blocks.filter(({ page_id }) => page_id !== "selection-page")];
        state.nativeSource = fixture.sourceMarkdown;
        state.nativeWrites = [];
        const original = internals.invoke;
        internals.invoke = async (cmd, args = {}) => {
          if (cmd === "ai_health_check") return { enabled: false, llm_available: false, embedder_available: false, vector_count: 0 };
          if (cmd === "reading_notes_list") return {
            notes: fixture.notes.filter((note) => !args.pageId || note.source.pageId === args.pageId), warnings: [],
          };
          if (cmd === "get_page_source" && args.pageId === fixture.page.id) return state.nativeSource;
          if (cmd === "update_page_source" && args.pageId === fixture.page.id) {
            state.nativeWrites.push(structuredClone(args));
            if (args.graphPath !== "/synthetic/keyboard-selection" || args.expectedSource !== state.nativeSource) {
              throw new Error("Source revision conflict. Newer annotations were not overwritten.");
            }
            state.nativeSource = args.content;
            return;
          }
          return original(cmd, args);
        };
        return internals;
      }
      let internals = window.__TAURI_INTERNALS__;
      if (internals) internals = install(internals);
      Object.defineProperty(window, "__TAURI_INTERNALS__", {
        configurable: true, get: () => internals, set: (value) => { internals = install(value); },
      });
    }, { fixture: nativeFixture, unifiedPage: options.unifiedPage }),
  });
}

async function openNotes(browser, options = {}) {
  if (options.nativeFixture) return openNativeNotes(browser, options);
  const backend = {
    notes: [{
      id: "orphan-note", notePageId: "orphan-note-page", filePath: "pages/Reading Notes/orphan-note.md",
      body: "Keep this orphaned reflection.", revision: "orphan-r1",
      source: { pageId: null, pageTitle: "Missing book", filePath: "pages/Books/Missing.md" },
      quote: "An unavailable passage.", status: "orphaned", statusMessage: "Source is missing; the note is preserved.",
      storage: "file", footnoteLabel: null, noteBlockId: "orphan-note-body",
      targetBlockId: null, createdAt: "2026-09-13T12:00:00Z", updatedAt: "2026-09-13T12:00:00Z",
    }],
    calls: [], hold: false, pending: [], fail: false, partialDelete: false,
  };
  const beforeNavigate = async (page) => {
    await page.exposeFunction("__notesBackend", async (cmd, args, source) => {
      backend.calls.push({ cmd, args: structuredClone(args) });
      if (cmd === "reading_notes_list") return {
        notes: backend.notes.filter((note) => !args.pageId || note.source.pageId === args.pageId),
        warnings: [],
      };
      if (backend.hold) await new Promise((resolve) => backend.pending.push(resolve));
      if (backend.fail) throw new Error("Synthetic disk is unavailable. Your draft was not saved.");
      if (cmd === "reading_notes_delete_for_page") {
        const notes = backend.notes.filter((note) => note.source.pageId === args.sourcePageId);
        assert.deepEqual(notes.map(({ id, revision }) => ({ id, revision })).sort((a, b) => a.id.localeCompare(b.id)),
          [...args.expectedNotes].sort((a, b) => a.id.localeCompare(b.id)));
        const deletedIds = (backend.partialDelete ? notes.slice(0, 1) : notes).map(({ id }) => id);
        backend.notes = backend.notes.filter((note) => !deletedIds.includes(note.id));
        return { deletedIds, deletedCount: deletedIds.length, failures: backend.partialDelete
          ? [{ id: notes[0].id, message: "Index reload failed after deletion." },
            { id: notes[1].id, message: "The second note file changed." }] : [], backups: [] };
      }
      if (cmd === "reading_note_create") {
        if (args.graphPath !== ROOT || !source || source.id !== args.sourcePageId) throw new Error("Wrong source graph or page");
        const existing = backend.notes.find(({ id }) => id === args.noteId);
        if (existing) throw new Error("Duplicate note creation");
        const note = {
          id: args.noteId, notePageId: source.id, filePath: source.file_path,
          storage: "inline", noteBlockId: `body-${args.noteId}`,
          footnoteLabel: `grafium-note-${backend.notes.filter((note) => note.source.pageId === source.id).length + 1}`,
          body: args.body, revision: "r1",
          source: { pageId: source.id, pageTitle: source.title, filePath: source.file_path },
          quote: args.selection?.text ?? "", status: "attached", statusMessage: "Attached to the source.",
          targetBlockId: args.selection?.blockIds[0] ?? null,
          createdAt: "2026-09-13T12:00:00Z", updatedAt: "2026-09-13T12:00:00Z",
        };
        backend.notes.push(note);
        return structuredClone(note);
      }
      const note = backend.notes.find(({ id }) => id === args.noteId);
      if (!note) throw new Error("Missing note");
      if (args.expectedRevision !== note.revision) throw new Error("The note changed on disk. Your draft has been preserved.");
      if (cmd === "reading_note_delete") {
        backend.notes = backend.notes.filter((item) => item.id !== note.id);
        return { deletedIds: [note.id], deletedCount: 1, failures: [], backups: [] };
      } else if (cmd === "reading_note_update") note.body = args.body;
      else if (cmd === "reading_note_reattach") {
        note.source = { pageId: source.id, pageTitle: source.title, filePath: source.file_path };
        note.quote = args.selection?.text ?? "";
        note.targetBlockId = args.selection?.blockIds[0] ?? null;
        note.status = "attached";
        note.statusMessage = "Attached to the selected passage.";
      } else throw new Error(`Unexpected note operation: ${cmd}`);
      note.revision += "-next";
      return structuredClone(note);
    });
    await page.addInitScript(() => {
      if (window !== window.top) return;
      function install(internals) {
        const state = window.__selectionState;
        const original = internals.invoke;
        state.pages.push({
          ...structuredClone(state.pages[0]), id: "second-reading-source", title: "Second reading source",
          file_path: "pages/second-reading-source.md",
        });
        state.blocks.push({
          ...structuredClone(state.blocks[0]), id: "second-reading-block", page_id: "second-reading-source",
        });
        window.__notesCalls = [];
        internals.invoke = async (cmd, args = {}) => {
          window.__notesCalls.push({ cmd, args: structuredClone(args) });
          if (cmd === "ai_health_check") return { enabled: false, llm_available: false, embedder_available: false, vector_count: 0 };
          if (cmd.startsWith("reading_note")) {
            const result = await window.__notesBackend(cmd, args, state.pages.find(({ id }) => id === args.sourcePageId));
            if (cmd === "reading_note_delete" || cmd === "reading_notes_delete_for_page") {
              const saved = await window.__notesBackend("reading_notes_list", { graphPath: args.graphPath });
              const labels = new Set(saved.notes.map((note) => note.footnoteLabel));
              for (const block of state.blocks) {
                block.content = block.content.replace(/\[\^(grafium-note-\d+)\]/g, (marker, label) => labels.has(label) ? marker : "");
              }
              return result;
            }
            for (const note of result.notes ?? [result]) {
              if (note.storage === "inline") {
                const reference = `[^${note.footnoteLabel}]`;
                const block = state.blocks.find(({ id }) => id === note.targetBlockId)
                  ?? state.blocks.find(({ page_id }) => page_id === note.source.pageId);
                if (block && !block.content.includes(reference)) block.content += reference;
                continue;
              }
              if (!state.pages.some(({ id }) => id === note.notePageId)) {
                state.pages.push({
                  ...structuredClone(state.pages[0]), id: note.notePageId,
                  title: `Reading Notes/${note.id}`, file_path: note.filePath,
                });
                state.blocks.push({
                  ...structuredClone(state.blocks[0]), id: `body-${note.id}`, page_id: note.notePageId, content: note.body,
                });
              }
            }
            return result;
          }
          return original(cmd, args);
        };
        return internals;
      }
      let internals = window.__TAURI_INTERNALS__;
      if (internals) internals = install(internals);
      Object.defineProperty(window, "__TAURI_INTERNALS__", {
        configurable: true, get: () => internals, set: (value) => { internals = install(value); },
      });
    });
  };
  const fixture = await openEditor(browser, { ...options, componentHarness: !!options.unifiedPage, beforeNavigate });
  const page = fixture.page;
  if (options.journal) {
    await row(page, "day-1-b0").scrollIntoViewIfNeeded();
    await focus(page, "day-1-b0");
  } else if (options.unifiedPage) await focusContinuous(page, "selection-page", "b0");
  else await focus(page, "b0");
  await page.evaluate(() => window.dispatchEvent(new CustomEvent("toggle-reference-panel")));
  await page.locator(".reference-panel .panel-tabs").getByRole("tab", { name: "Notes", exact: true }).click();
  await panel(page).waitFor();
  return { ...fixture, backend };
}

async function beginNote(page, body, useSelection = false) {
  if (!await input(page).count()) await button(page, "New note").click();
  if (useSelection) await button(page, "Use selection").click();
  await input(page).fill(body);
}
async function save(page, body) {
  await button(page, "Save note").click();
  await cards(page).filter({ hasText: body }).waitFor();
}
async function deleteAll(page) {
  const actions = panel(page).locator(".more-actions");
  if (await actions.getAttribute("open") === null) await actions.locator("summary").click();
  await button(page, "Delete all notes on this page…").click();
}
const cases = [
  ["Deletion uses a focused modal without jumping the notes list", {}, async ({ page, backend }) => {
    await page.setViewportSize({ width: 1000, height: 600 });
    await beginNote(page, "First reflection.");
    await save(page, "First reflection.");
    const base = backend.notes.find(({ body }) => body === "First reflection.");
    for (let index = 1; index <= 8; index++) backend.notes.push({
      ...structuredClone(base), id: `modal-note-${index}`, footnoteLabel: `grafium-note-${index + 1}`,
      body: `Specific reflection ${index} to review before deleting.`,
    });
    await button(page, "Refresh").click();
    const card = cards(page).filter({ hasText: "Specific reflection 8" });
    const trigger = card.getByRole("button", { name: "Delete note…", exact: true });
    await trigger.scrollIntoViewIfNeeded();
    const scrollBefore = await panel(page).evaluate((element) => element.scrollTop);
    assert.ok(scrollBefore > 100, "fixture is scrolled down to a saved note");
    await trigger.click();
    const confirmation = panel(page).getByRole("alertdialog", { name: "Confirm note deletion" });
    await confirmation.waitFor();
    assert.equal(await confirmation.evaluate((element) => element.matches(":modal")), true);
    assert.ok(Math.abs(await panel(page).evaluate((element) => element.scrollTop) - scrollBefore) <= 1);
    assert.match(await confirmation.innerText(), /Specific reflection 8/);
    assert.equal(await confirmation.getByRole("button").count(), 2);
    const cancel = confirmation.getByRole("button", { name: "Cancel", exact: true });
    assert.equal(await cancel.evaluate((element) => element === document.activeElement), true);
    const bounds = await confirmation.boundingBox();
    assert.ok(Math.abs(bounds.x + bounds.width / 2 - 500) < 2, "confirmation is centered horizontally");
    assert.ok(bounds.y >= 0 && bounds.y + bounds.height <= 600, "confirmation fits a compact viewport");
    if (process.env.UI_TEST_SCREENSHOT) await page.screenshot({ path: process.env.UI_TEST_SCREENSHOT });
    await page.keyboard.press("Shift+Tab");
    assert.equal(await confirmation.getByRole("button", { name: "Delete saved note", exact: true })
      .evaluate((element) => element === document.activeElement), true);
    await page.keyboard.press("Tab");
    assert.equal(await cancel.evaluate((element) => element === document.activeElement), true);
    await page.keyboard.press("Enter");
    await confirmation.waitFor({ state: "detached" });
    assert.equal(await trigger.evaluate((element) => element === document.activeElement), true);
    assert.ok(Math.abs(await panel(page).evaluate((element) => element.scrollTop) - scrollBefore) <= 1);
    assert.equal(backend.calls.some(({ cmd }) => cmd === "reading_note_delete"), false);
    await trigger.click();
    await page.keyboard.press("Escape");
    await confirmation.waitFor({ state: "detached" });
    assert.equal(await trigger.evaluate((element) => element === document.activeElement), true);
    await trigger.click();
    await confirmation.getByRole("button", { name: "Delete saved note", exact: true }).click();
    await confirmation.waitFor({ state: "detached" });
    await card.waitFor({ state: "detached" });
    assert.ok(await panel(page).evaluate((element) => element.scrollTop) > 100, "deletion does not return to the top");
    assert.equal(backend.calls.filter(({ cmd }) => cmd === "reading_note_delete").length, 1);
  }],
  ["Partial bulk deletion reports all failures and keeps the notes that were not deleted", {}, async ({ page, backend }) => {
    await beginNote(page, "First partial note.");
    await save(page, "First partial note.");
    await button(page, "New note").click();
    await beginNote(page, "Second partial note.");
    await save(page, "Second partial note.");
    backend.partialDelete = true;
    await deleteAll(page);
    await panel(page).getByRole("button", { name: "Delete all saved notes", exact: true }).click();
    await panel(page).getByRole("alert").filter({
      hasText: "1 of 2 saved notes deleted, but the operation needs attention: Index reload failed after deletion. The second note file changed.",
    }).waitFor();
    await cards(page).filter({ hasText: "First partial note." }).waitFor({ state: "detached" });
    await cards(page).filter({ hasText: "Second partial note." }).waitFor();
    assert.equal(backend.notes.some(({ body }) => body === "Second partial note."), true);
  }],
  ["Mouse-selected blocks remain attached when opening the Notes panel", {}, async ({ page, backend }) => {
    await page.evaluate(() => window.dispatchEvent(new CustomEvent("toggle-reference-panel")));
    await row(page, "b0").getByRole("button", { name: "Select block", exact: true }).click();
    await row(page, "b1").getByRole("button", { name: "Select block", exact: true }).click({ modifiers: ["Shift"] });
    await page.evaluate(() => window.dispatchEvent(new CustomEvent("toggle-reference-panel")));
    await page.locator(".reference-panel .panel-tabs").getByRole("tab", { name: "Notes", exact: true }).click();
    await panel(page).locator(".quote-preview").waitFor();
    await beginNote(page, "Mouse-selected passage.");
    await save(page, "Mouse-selected passage.");
    assert.deepEqual(backend.calls.find(({ cmd }) => cmd === "reading_note_create").args.selection.blockIds, ["b0", "b1"]);
  }],
  ["Continuous cross-block selections attach automatically to new notes", { unifiedPage: true }, async ({ page, backend }) => {
    await focusContinuous(page, "selection-page", "b0", "start");
    await page.evaluate(async () => {
      const view = window.__activeEditorView;
      const { parsePageSourceMap } = await import("/src/lib/pageSourceMap.ts");
      const blocks = parsePageSourceMap(view.state.doc.toString()).blocks;
      view.dispatch({ selection: { anchor: blocks.find(({ id }) => id === "b0").contentFrom,
        head: blocks.find(({ id }) => id === "b1").contentTo } });
    });
    await frames(page);
    await panel(page).locator(".quote-preview").waitFor();
    await beginNote(page, "Continuous selected blocks.");
    await save(page, "Continuous selected blocks.");
    assert.deepEqual(backend.calls.find(({ cmd }) => cmd === "reading_note_create").args.selection.blockIds, ["b0", "b1"]);
  }],
  ["Saved notes have confirmed deletion that keeps unsaved edits", {}, async ({ page, backend }) => {
    await beginNote(page, "Delete this saved note.");
    await save(page, "Delete this saved note.");
    await input(page).fill("Keep this unfinished revision.");
    const card = cards(page).filter({ hasText: "Delete this saved note." });
    await card.getByRole("button", { name: "Delete note…", exact: true }).click();
    const confirmation = panel(page).getByRole("alertdialog", { name: "Confirm note deletion" });
    await confirmation.getByRole("button", { name: "Cancel", exact: true }).click();
    assert.equal(backend.calls.some(({ cmd }) => cmd === "reading_note_delete"), false);
    await card.getByRole("button", { name: "Delete note…", exact: true }).click();
    await confirmation.getByRole("button", { name: "Delete saved note", exact: true }).click();
    await card.waitFor({ state: "detached" });
    assert.equal(await input(page).inputValue(), "Keep this unfinished revision.");
    await save(page, "Keep this unfinished revision.");
    const creates = backend.calls.filter(({ cmd }) => cmd === "reading_note_create");
    assert.equal(creates.length, 2);
    assert.notEqual(creates[0].args.noteId, creates[1].args.noteId);
  }],
  ["Delete all is limited to the current page even while showing All notes", {}, async ({ page, backend }) => {
    await beginNote(page, "First page note.");
    await save(page, "First page note.");
    await button(page, "New note").click();
    await beginNote(page, "Second page note.");
    await save(page, "Second page note.");
    await panel(page).getByRole("combobox", { name: "Notes scope", exact: true }).selectOption("all");
    await cards(page).filter({ hasText: "Keep this orphaned reflection." }).waitFor();
    await deleteAll(page);
    const confirmation = panel(page).getByRole("alertdialog", { name: "Confirm note deletion" });
    assert.match(await confirmation.innerText(), /All 2 saved notes/);
    await confirmation.getByRole("button", { name: "Cancel", exact: true }).click();
    assert.equal(backend.calls.some(({ cmd }) => cmd === "reading_notes_delete_for_page"), false);
    await deleteAll(page);
    await confirmation.getByRole("button", { name: "Delete all saved notes", exact: true }).click();
    await cards(page).filter({ hasText: "Second page note." }).waitFor({ state: "detached" });
    assert.equal(backend.notes.length, 1);
    assert.equal(backend.notes[0].id, "orphan-note");
    await cards(page).filter({ hasText: "Keep this orphaned reflection." }).waitFor();
    assert.equal(await button(page, "Delete all notes on this page…").isDisabled(), true);
  }],
  ["Deletion refuses an externally changed note and reloads it for review", {}, async ({ page, backend }) => {
    await beginNote(page, "Original note.");
    await save(page, "Original note.");
    await cards(page).filter({ hasText: "Original note." }).getByRole("button", { name: "Delete note…", exact: true }).click();
    const note = backend.notes.find(({ body }) => body === "Original note.");
    note.revision = "changed-after-confirmation";
    note.body = "New external note body.";
    await panel(page).getByRole("button", { name: "Delete saved note", exact: true }).click();
    await panel(page).getByRole("alert").filter({ hasText: "note changed on disk" }).waitFor();
    await cards(page).filter({ hasText: "New external note body." }).waitFor();
    assert.equal(backend.notes.some(({ id }) => id === note.id), true);
  }],
  ["New notes automatically attach to keyboard-selected blocks instead of the whole page", {}, async ({ page, backend }) => {
    await focus(page, "b0");
    await shift(page, "Down", ["b0", "b1"]);
    await shift(page, "Down", ["b0", "b1", "b2"]);
    await panel(page).locator(".quote-preview").waitFor();
    await beginNote(page, "About these three selected blocks.");
    await save(page, "About these three selected blocks.");
    const create = backend.calls.find(({ cmd }) => cmd === "reading_note_create");
    assert.deepEqual(create.args.selection.blockIds, ["b0", "b1", "b2"]);
    const expected = await page.evaluate(() => window.__selectionState.blocks
      .filter(({ id }) => ["b0", "b1", "b2"].includes(id)).map(({ content }) => content.replace(/\[\^grafium-note-\d+\]/g, "")));
    assert.deepEqual(create.args.selection.parts.map(({ text }) => text), expected);
    assert.equal(create.args.selection.kind, "source");
  }],
  ["Explicit page-level choice is retained despite a frozen block selection", {}, async ({ page, backend }) => {
    await focus(page, "b0");
    await shift(page, "Down", ["b0", "b1"]);
    await button(page, "Make page-level note").click();
    await beginNote(page, "Deliberately about the whole page.");
    await save(page, "Deliberately about the whole page.");
    assert.equal(backend.calls.find(({ cmd }) => cmd === "reading_note_create").args.selection, null);
  }],
  ["Notes works without AI and saved Markdown notes return after reload", { book: true }, async ({ page, backend }) => {
    await beginNote(page, "A durable reading reflection.");
    await save(page, "A durable reading reflection.");
    const create = backend.calls.find(({ cmd }) => cmd === "reading_note_create");
    assert.equal(create.args.sourcePageId, "selection-page");
    assert.equal(create.args.selection, null);
    assert.equal(create.args.graphPath, ROOT);
    const note = backend.notes.find(({ id }) => id === create.args.noteId);
    assert.equal(note.storage, "inline");
    assert.equal(note.filePath, "pages/Books/Keyboard selection.md");
    assert.equal(note.notePageId, "selection-page");
    assert.equal(await page.evaluate(() => window.__notesCalls.some(({ cmd }) => cmd === "ai_ask" || cmd === "research_scoped")), false);
    await page.reload({ waitUntil: "networkidle" });
    if (!await page.locator(".reference-panel").isVisible()) {
      await page.evaluate(() => window.dispatchEvent(new CustomEvent("toggle-reference-panel")));
    }
    await page.locator(".reference-panel .panel-tabs").getByRole("tab", { name: "Notes", exact: true }).click();
    await cards(page).filter({ hasText: "A durable reading reflection." }).waitFor();
    assert.equal(backend.calls.filter(({ cmd }) => cmd === "reading_note_create").length, 1);
  }],
  ["Notes uses the focused journal day, not the whole journal feed", { journal: true }, async ({ page, backend }) => {
    await beginNote(page, "Reflection on the selected day.");
    await save(page, "Reflection on the selected day.");
    assert.equal(backend.calls.find(({ cmd }) => cmd === "reading_note_create").args.sourcePageId, "day-1");
  }],
  ["Notes keeps drafts and in-flight saves attached to their original source", {}, async ({ page, backend }) => {
    await beginNote(page, "Keep my draft on source A.");
    await page.evaluate(() => window.dispatchEvent(new CustomEvent("navigate-page", { detail: "Second reading source" })));
    await row(page, "second-reading-block").waitFor();
    if (!await input(page).count()) await button(page, "New note").click();
    assert.equal(await input(page).inputValue(), "");
    await page.evaluate(() => window.dispatchEvent(new CustomEvent("navigate-page", { detail: "Keyboard selection" })));
    await row(page, "b0").waitFor();
    await page.waitForFunction(() => document.querySelector('.reading-notes-panel textarea')?.value === "Keep my draft on source A.");
    backend.hold = true;
    await button(page, "Save note").click();
    await page.waitForFunction(() => window.__notesCalls.some(({ cmd }) => cmd === "reading_note_create"));
    await page.evaluate(() => window.dispatchEvent(new CustomEvent("navigate-page", { detail: "Second reading source" })));
    await row(page, "second-reading-block").waitFor();
    backend.hold = false;
    backend.pending.splice(0).forEach((resolve) => resolve());
    await frames(page);
    assert.equal(await cards(page).filter({ hasText: "Keep my draft on source A." }).count(), 0);
    await page.evaluate(() => window.dispatchEvent(new CustomEvent("navigate-page", { detail: "Keyboard selection" })));
    await row(page, "b0").waitFor();
    await cards(page).filter({ hasText: "Keep my draft on source A." }).waitFor();
    assert.equal(backend.notes.find(({ body }) => body === "Keep my draft on source A.").source.pageId, "selection-page");
  }],
  ["Notes preserves failed drafts and rejects stale revisions instead of overwriting external edits", {}, async ({ page, backend }) => {
    backend.fail = true;
    await beginNote(page, "Never discard this draft.");
    await button(page, "Save note").click();
    await panel(page).getByText(/Synthetic disk is unavailable/).waitFor();
    assert.equal(await input(page).inputValue(), "Never discard this draft.");
    backend.fail = false;
    await save(page, "Never discard this draft.");
    await cards(page).filter({ hasText: "Never discard this draft." }).getByRole("button", { name: "Edit note", exact: true }).click();
    await input(page).fill("My unsaved revision.");
    const note = backend.notes.find(({ body }) => body === "Never discard this draft.");
    note.revision = "external-revision";
    note.body = "An external edit.";
    await button(page, "Save note").click();
    await panel(page).getByText(/note changed on disk/).waitFor();
    assert.equal(await input(page).inputValue(), "My unsaved revision.");
    assert.equal(note.body, "An external edit.");
  }],
  ["All notes keeps orphaned passages visible without guessing a destination", {}, async ({ page }) => {
    await panel(page).getByRole("combobox", { name: "Notes scope", exact: true }).selectOption("all");
    const orphan = cards(page).filter({ hasText: "Keep this orphaned reflection." });
    await orphan.waitFor();
    assert.match(await orphan.innerText(), /missing|orphan|review/i);
    const passage = orphan.getByRole("button", { name: "Go to passage", exact: true });
    if (await passage.count()) assert.equal(await passage.isDisabled(), true);
  }],
  ["Explicit reattachment preserves unsaved body edits and uses the selected source", {}, async ({ page, backend }) => {
    await panel(page).getByRole("combobox", { name: "Notes scope", exact: true }).selectOption("all");
    const orphan = cards(page).filter({ hasText: "Keep this orphaned reflection." });
    await orphan.getByRole("button", { name: "Edit note", exact: true }).click();
    await input(page).fill("My unsaved additional reflection.");
    await focus(page, "b0");
    await page.keyboard.press("End");
    for (let i = 0; i < 4; i++) await page.keyboard.press("Shift+ArrowLeft");
    await frames(page);
    await button(page, "Use selection").click();
    await button(page, "Reattach to selection").click();
    await panel(page).getByText(/Passage reattached\. Body edits are still unsaved/).waitFor();
    const repair = backend.calls.find(({ cmd }) => cmd === "reading_note_reattach");
    assert.equal(repair.args.sourcePageId, "selection-page");
    assert.equal(repair.args.expectedRevision, "orphan-r1");
    assert.equal(repair.args.selection.text, "line");
    assert.equal(backend.notes.find(({ id }) => id === "orphan-note").body, "Keep this orphaned reflection.");
    assert.equal(await input(page).inputValue(), "My unsaved additional reflection.");
  }],
  ["Notes remains reachable in a compact window", {}, async ({ page }) => {
    await page.setViewportSize({ width: 1000, height: 600 });
    await beginNote(page, "A compact note.");
    await button(page, "Save note").scrollIntoViewIfNeeded();
    const geometry = await button(page, "Save note").evaluate((element) => {
      const button = element.getBoundingClientRect();
      const panel = element.closest(".reference-panel").getBoundingClientRect();
      return { bottom: button.bottom, right: button.right, panelBottom: panel.bottom, panelRight: panel.right, height: innerHeight };
    });
    assert.ok(geometry.bottom <= Math.min(geometry.panelBottom, geometry.height) + 1, JSON.stringify(geometry));
    assert.ok(geometry.right <= geometry.panelRight + 1, JSON.stringify(geometry));
    await save(page, "A compact note.");
  }],
];

for (const unifiedPage of [false, true]) cases.push([
  `Notes freezes selected words and navigates by source ID in ${unifiedPage ? "continuous" : "classic"} mode`,
  { unifiedPage }, async ({ page, backend }) => {
    if (unifiedPage) await focusContinuous(page, "selection-page", "b0");
    else await focus(page, "b0");
    await page.keyboard.press("End");
    for (let i = 0; i < 4; i++) await page.keyboard.press("Shift+ArrowLeft");
    await frames(page);
    await beginNote(page, "Reflection about the highlighted word.", true);
    await save(page, "Reflection about the highlighted word.");
    const create = backend.calls.find(({ cmd }) => cmd === "reading_note_create");
    assert.equal(create.args.selection.text, "line");
    assert.deepEqual(create.args.selection.blockIds, ["b0"]);
    assert.ok(create.args.selection.parts[0].prefix.includes("visual"));
    const card = cards(page).filter({ hasText: "Reflection about the highlighted word." });
    await page.evaluate(() => { window.__notesCalls = []; });
    await card.getByRole("button", { name: "Go to passage", exact: true }).click();
    await page.waitForFunction(() => window.__notesCalls.some(({ cmd, args }) => cmd === "get_page" && args.id === "selection-page"));
    assert.equal(await page.evaluate(() => window.__notesCalls.some(({ cmd }) => cmd === "create_page")), false);
  },
]);

for (const unifiedPage of [false, true]) cases.push([
  `Inline footnote markers open the matching note without losing its draft in ${unifiedPage ? "continuous" : "classic"} mode`,
  { unifiedPage }, async ({ page }) => {
    await beginNote(page, "A note inside this source file.");
    await save(page, "A note inside this source file.");
    await page.evaluate(() => window.dispatchEvent(new CustomEvent("toggle-reference-panel")));
    if (unifiedPage) await focusContinuous(page, "selection-page", "b1");
    else await focus(page, "b1");
    const marker = page.locator('.main-content [data-reading-note-label="grafium-note-1"]');
    await marker.waitFor();
    await marker.click();
    await page.waitForFunction(() => document.querySelector('.reading-notes-panel textarea')?.value === "A note inside this source file.");
    await input(page).fill("Keep this unsaved footnote revision.");
    await page.evaluate(() => window.dispatchEvent(new CustomEvent("toggle-reference-panel")));
    await marker.click();
    await page.waitForFunction(() => document.querySelector('.reading-notes-panel textarea')?.value === "Keep this unsaved footnote revision.");
    assert.equal(await page.evaluate(() => window.__notesCalls.some(({ cmd }) => cmd === "create_page")), false);
  },
]);

for (const unifiedPage of [false, true]) cases.push([
  `Actual native source footnotes render and open their exact note in ${unifiedPage ? "continuous" : "classic"} mode`,
  { unifiedPage, nativeFixture: true }, async ({ page }) => {
    for (const note of nativeFixture.notes) {
      const footer = page.locator(".main-content section.reading-note-footer").filter({
        has: page.locator(`[data-reading-note-label="${note.footnoteLabel}"]`),
      });
      await footer.scrollIntoViewIfNeeded();
      assert.match(await footer.innerText(), /café/);
      assert.doesNotMatch(await footer.innerText(), /sourceFileSha256|bodyBlockId|grafium-reading-note/);
      if (note.body.includes("owner::")) {
        assert.match(await footer.innerText(), /owner:: reader, not metadata/);
        assert.match(await footer.innerText(), /id:: literal body text/);
        assert.match(await footer.locator("pre code").innerText(), /body:: not metadata/);
      }
      const marker = page.locator(`.main-content [data-reading-note-label="${note.footnoteLabel}"]`).first();
      await marker.click();
      await panel(page).waitFor();
      await page.waitForFunction((body) =>
        document.querySelector(".reading-notes-panel textarea")?.value === body, note.body);
      assert.equal(await input(page).inputValue(), note.body);
      await page.evaluate(() => window.dispatchEvent(new CustomEvent("toggle-reference-panel")));
    }
  },
]);

cases.push([
  "Continuous source save preserves the loaded base and refuses to overwrite newer native annotations",
  { unifiedPage: true, nativeFixture: true }, async ({ page }) => {
    await page.locator(".unified-rendered-content").getByText("Final source paragraph.", { exact: true }).click();
    await page.waitForFunction(() => document.activeElement?.closest(".unified-page-editor") != null);
    await page.keyboard.press("End");
    await page.keyboard.type(" My unsaved sentence.");
    await page.evaluate(() => {
      window.__selectionState.nativeSource = window.__selectionState.nativeSource.replace(
        "A second note.", "A newer external annotation.",
      );
    });
    const editor = page.locator(".unified-page-editor");
    await editor.getByRole("button", { name: "Save source", exact: true }).click();
    await editor.getByRole("alert").filter({ hasText: "Source revision conflict" }).waitFor();
    const state = await page.evaluate(() => ({
      writes: window.__selectionState.nativeWrites,
      source: window.__selectionState.nativeSource,
      draft: window.__unifiedPageEditorView.state.doc.toString(),
    }));
    assert.equal(state.writes.length, 1);
    assert.equal(state.writes[0].expectedSource, nativeFixture.sourceMarkdown);
    assert.equal(state.writes[0].graphPath, ROOT);
    assert.match(state.writes[0].content, /My unsaved sentence/);
    assert.match(state.source, /A newer external annotation/);
    assert.doesNotMatch(state.source, /My unsaved sentence/);
    assert.match(state.draft, /My unsaved sentence/);
    await editor.locator(".dirty-status").filter({ hasText: "Unsaved" }).waitFor();
  },
]);

if (require.main === module) (async () => {
  const browser = await chromium.launch({ args: ["--no-sandbox"] });
  let failed = 0;
  try {
    for (const [name, options, run] of cases) {
      if (process.env.UI_TEST_CASE && !process.env.UI_TEST_CASE.split("|").some((part) => name.includes(part))) continue;
      let fixture;
      try {
        fixture = await openNotes(browser, options);
        await run(fixture);
        assert.deepEqual(fixture.errors, []);
        assert.deepEqual(await fixture.page.evaluate(() => window.__selectionState.unhandledIpc), []);
        console.log(`PASS ${options.unifiedPage ? "[isolated component] " : ""}${name}`);
      } catch (error) {
        failed++;
        console.error(`FAIL ${name}\n${error.stack ?? error}`);
        if (fixture) console.error(await panel(fixture.page).textContent().catch(() => "Panel unavailable"));
      } finally { await fixture?.page.close(); }
    }
  } finally { await browser.close(); }
  if (failed) process.exitCode = 1;
})().catch((error) => { console.error(error); process.exitCode = 1; });

// Real Notes UI; synthetic persistence survives browser reload independently of component state.
const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor, focus, focusContinuous, row, frames } = require("./keyboardSelection.ui.cjs");
const nativeFixture = require("./fixtures/inline-reading-notes-native.json");

const ROOT = "/synthetic/keyboard-selection";
const panel = (page) => page.locator(".reading-notes-panel");
const input = (page) => panel(page).getByRole("textbox", { name: "Reading note", exact: true });
const button = (page, name) => panel(page).getByRole("button", { name, exact: true });
const cards = (page) => panel(page).locator(".reading-note-card");

async function openNativeNotes(browser, options) {
  const initialBlockId = options.unifiedPage ? nativeFixture.notes[0].noteBlockId : nativeFixture.blocks[0].id;
  return openEditor(browser, {
    ...options, initialBlockId,
    beforeNavigate: (page) => page.addInitScript(({ fixture, unifiedPage }) => {
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
    calls: [], hold: false, pending: [], fail: false,
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
      if (cmd === "reading_note_update") note.body = args.body;
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
  const fixture = await openEditor(browser, { ...options, beforeNavigate });
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
const cases = [
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
      let fixture;
      try {
        fixture = await openNotes(browser, options);
        await run(fixture);
        assert.deepEqual(fixture.errors, []);
        console.log(`PASS ${name}`);
      } catch (error) {
        failed++;
        console.error(`FAIL ${name}\n${error.stack ?? error}`);
        if (fixture) console.error(await panel(fixture.page).textContent().catch(() => "Panel unavailable"));
      } finally { await fixture?.page.close(); }
    }
  } finally { await browser.close(); }
  if (failed) process.exitCode = 1;
})().catch((error) => { console.error(error); process.exitCode = 1; });

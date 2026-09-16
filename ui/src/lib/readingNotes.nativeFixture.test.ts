import { afterEach, describe, expect, it } from "vitest";
// Regenerate with GRAFIUM_READING_NOTES_FIXTURE_PATH=../ui/tests/fixtures/inline-reading-notes-native.json
// cargo test -p grafium-core inline_native_cross_language_fixture
import fixture from "../../tests/fixtures/inline-reading-notes-native.json";
import { assetBaseDirFor, renderBlock } from "./markdown";
import { parsePageSourceMap, sourceBlockContentReplacement } from "./pageSourceMap";
import { readingNoteBlockLabel, readingSourceBlocks } from "./readingNoteFormat";
import {
  readingNoteLocationTarget, readingNotePassageTarget, renderReadingNoteFooter, type ReadingNote,
} from "./readingNotes";
import { captureRenderedReadingSelection, continuousReadingSelection } from "./readingSelection";

const blocks = fixture.blocks;
const notes = fixture.notes as ReadingNote[];
const baseDir = assetBaseDirFor(fixture.page.file_path);

afterEach(() => {
  window.getSelection()?.removeAllRanges();
  document.body.innerHTML = "";
});

describe("actual Rust-created, reopened and reindexed reading notes", () => {
  it("directs ID-less imported passages to classic selection without guessing a block identity", () => {
    const from = fixture.sourceMarkdown.indexOf("short passage");
    const selected = continuousReadingSelection(fixture.page.id, fixture.sourceMarkdown, from, from + 13);
    expect(selected.selection).toBeNull();
    expect(selected.error).toMatch(/no saved block ID.*classic editor/);
  });

  it("maps both native footer IDs and literal bodies exactly, including trailing blank lines", () => {
    expect(fixture.warnings).toEqual([]);
    const map = parsePageSourceMap(fixture.sourceMarkdown);
    expect(map.blocks.filter((block) => block.readingNote?.storage === "inline")).toHaveLength(notes.length);
    for (const note of notes) {
      const nativeBlock = blocks.find((block) => block.id === note.noteBlockId)!;
      const mapped = map.blocks.find((block) => block.id === note.noteBlockId)!;
      expect(mapped.content).toBe(nativeBlock.content);
      expect(mapped.content).toBe(note.body);
      expect(mapped.propertyLines).toEqual([]);
      expect(mapped.readingNote?.footnoteLabel).toBe(note.footnoteLabel);
      expect(sourceBlockContentReplacement(mapped, note.body).insert)
        .toBe(fixture.sourceMarkdown.slice(mapped.previewFrom, mapped.previewTo));
    }
    expect(map.blocks.filter((block) => !block.readingNote).every((block) => block.id === null)).toBe(true);
  });

  it("renders native source markers and footers with the real Markdown pipeline, without anchor JSON", () => {
    for (const note of notes) {
      const source = blocks.find((block) => block.id === note.targetBlockId)!;
      const sourcePreview = document.createElement("div");
      sourcePreview.innerHTML = renderBlock(source.content, baseDir);
      expect(sourcePreview.querySelector(`[data-reading-note-label="${note.footnoteLabel}"]`)?.textContent)
        .toBe(note.footnoteLabel!.slice("grafium-note-".length));
      expect(sourcePreview.querySelectorAll("[data-reading-note-label]")).toHaveLength(1);
      if (source.content.includes("[^author]")) expect(sourcePreview.textContent).toContain("[^author]");
      const block = blocks.find((block) => block.id === note.noteBlockId)!;
      const label = readingNoteBlockLabel(block);
      expect(label).toBe(note.footnoteLabel);
      const footer = document.createElement("div");
      footer.innerHTML = renderReadingNoteFooter(block.content, label!, baseDir);
      expect(footer.querySelector("[data-reading-note-footer]")).not.toBeNull();
      expect(footer.textContent).not.toContain("sourceFileSha256");
      expect(footer.textContent).not.toContain(note.id);
      expect(footer.textContent).toContain("café");
      expect(footer.textContent).toContain("😀");
      if (block.content.includes("owner::")) {
        expect(footer.querySelector("h1")?.textContent).toBe("Why this matters");
        expect(footer.textContent).toContain("owner:: reader, not metadata");
        expect(footer.textContent).toContain("id:: literal body text");
        expect(footer.querySelector("pre code")?.textContent).toContain('let label = "body:: not metadata";');
        expect(footer.querySelector("strong")?.textContent).toBe("bold");
      } else {
        expect(footer.querySelectorAll("li")).toHaveLength(2);
      }
    }
  });

  it("uses reindexed DTO navigation identities, never stale metadata source IDs", () => {
    for (const note of notes) {
      const block = blocks.find((block) => block.id === note.noteBlockId)!;
      const metadata = JSON.parse(block.properties["reading-note"] as string);
      expect(note.source.pageId).not.toBe(metadata.anchor.sourcePageId);
      expect(readingNotePassageTarget(note)).toEqual({
        pageId: fixture.page.id, pageName: fixture.page.title, targetBlockId: note.targetBlockId,
      });
      expect(readingNoteLocationTarget(note)).toEqual({
        pageId: fixture.page.id, pageName: fixture.page.title, targetBlockId: note.noteBlockId,
      });
    }
  });

  it("captures the native Unicode rendered passage beside an existing reference without marker chrome", () => {
    const note = notes.find((note) => note.footnoteLabel === "grafium-note-1")!;
    const block = blocks.find((block) => block.id === note.targetBlockId)!;
    document.body.innerHTML = `<main class="main-content"><div class="page-content" data-page-id="${fixture.page.id}"><div data-block-id="${block.id}"><div class="rendered-content">${renderBlock(block.content, baseDir)}</div></div></div></main>`;
    const text = document.querySelector("strong")!.firstChild!;
    const range = document.createRange();
    range.selectNodeContents(text);
    const selected = window.getSelection()!;
    selected.addRange(range);
    const captured = captureRenderedReadingSelection(selected)!;
    expect(captured.error).toBeNull();
    expect(captured.selection).toMatchObject({
      pageId: fixture.page.id, blockIds: [block.id], kind: "rendered", text: "short passage",
      parts: [{ blockId: block.id, from: 2, to: 15, text: "short passage", prefix: "A ", suffix: " about café and 😀." }],
    });
  });

  it("keeps author footnotes in the source-only projection and leaves the native snapshot unchanged", () => {
    const snapshot = JSON.stringify(blocks);
    const source = readingSourceBlocks(blocks);
    expect(source).toHaveLength(blocks.length - notes.length);
    expect(source.some((block) => block.content.startsWith("[^author]:"))).toBe(true);
    expect(source.some((block) => block.content.includes("owner:: reader"))).toBe(false);
    expect(JSON.stringify(blocks)).toBe(snapshot);
  });

  it("preserves a v1 counterpart derived from actual native metadata without consuming body properties", () => {
    const block = blocks.find((block) => block.id === notes[0].noteBlockId)!;
    const metadata = JSON.parse(block.properties["reading-note"] as string);
    metadata.version = 1;
    delete metadata.footnoteLabel;
    delete metadata.anchor.hashMode;
    const legacy = `title:: Legacy note\nreading-note:: ${JSON.stringify(metadata)}\ncategory:: Reading\n\n${block.content}`;
    const map = parsePageSourceMap(legacy);
    expect(map.blocks).toHaveLength(1);
    expect(map.blocks[0].id).toBe(block.id);
    expect(map.blocks[0].content).toBe(block.content);
    expect(map.blocks[0].propertyLines).toEqual([]);
    expect(map.hiddenRanges).toEqual([{ from: 0, to: legacy.length - block.content.length }]);
  });
});

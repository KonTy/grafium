import { describe, expect, it } from "vitest";
import { managedReadingNoteRanges, legacyReadingNoteBody, readingNoteBlockLabel, touchesReadingNoteDefinition, readingSourceBlocks } from "./readingNoteFormat";
import { parsePageSourceMap, sourceBlockContentReplacement } from "./pageSourceMap";
import { continuousReadingSelection, captureRenderedReadingSelection } from "./readingSelection";
import { renderReadingNoteFooter } from "./readingNotes";

const metadata = {
  version: 2, id: "11111111-1111-4111-8111-111111111111",
  bodyBlockId: "22222222-2222-4222-8222-222222222222", footnoteLabel: "grafium-note-1",
};
const opening = `<!-- grafium-reading-note ${JSON.stringify(metadata)} -->`;
const body = "**Observation**\nid:: literal prose\nreading-note:: not metadata\n\n- Nested Markdown item";
const footer = `${opening}\n[^grafium-note-1]: ${body.split("\n").join("\n    ")}\n<!-- /grafium-reading-note -->\n`;
const source = `title:: Book\n\n- First paragraph.[^grafium-note-1]\n  id:: source-a\n- Second paragraph.\n  id:: source-b\n\n${footer}`;

describe("managed inline footnote source mapping", () => {
  it("maps one footer body block without consuming property-looking text or nested Markdown", () => {
    const ranges = managedReadingNoteRanges(source);
    expect(ranges).toHaveLength(1);
    const map = parsePageSourceMap(source);
    expect(map.blocks.map((block) => block.id)).toEqual(["source-a", "source-b", metadata.bodyBlockId]);
    const note = map.blocks[2];
    expect(note.content).toBe(body);
    expect(note.readingNote?.footnoteLabel).toBe("grafium-note-1");
    expect(note.propertyLines).toEqual([]);
    expect(note.contentSegments.map((segment) => source.slice(segment.from, segment.to)).join("\n")).toBe(body);
    expect(source.slice(note.previewFrom, note.previewTo)).toBe(footer.trimEnd());
    expect(map.blocks[1].subtreeTo).toBeLessThan(note.previewFrom);
  });

  it("preserves the original wrapper and source IDs when replacing the body", () => {
    const map = parsePageSourceMap(source);
    const replacement = sourceBlockContentReplacement(map.blocks[2], "id:: still prose\n\nNew body");
    const next = source.slice(0, replacement.from) + replacement.insert + source.slice(replacement.to);
    expect(next.slice(0, replacement.from)).toBe(source.slice(0, replacement.from));
    expect(next).toContain(opening);
    const parsed = parsePageSourceMap(next);
    expect(parsed.blocks[2].content).toBe("id:: still prose\n\nNew body");
    expect(parsed.blocks[2].id).toBe(metadata.bodyBlockId);
  });

  it("does not claim standard author definitions, malformed metadata or fenced examples", () => {
    expect(managedReadingNoteRanges("[^author]: Definition")).toEqual([]);
    expect(managedReadingNoteRanges(footer.replace("grafium-note-1]:", "grafium-note-2]:"))).toEqual([]);
    expect(managedReadingNoteRanges(footer.replace('"version":2', '"version":99'))).toEqual([]);
    expect(managedReadingNoteRanges(`\`\`\`markdown\n${footer}\`\`\`\n`)).toEqual([]);
    expect(managedReadingNoteRanges(footer.replace("    id::", "id::"))).toEqual([]);
    expect(managedReadingNoteRanges(footer.replace("<!-- /grafium-reading-note -->", ""))).toEqual([]);
  });

  it("protects annotation wrappers from continuous editor changes, not adjacent book text", () => {
    const note = managedReadingNoteRanges(source)[0];
    expect(touchesReadingNoteDefinition(source, [{ from: 20, to: 23 }])).toBe(false);
    expect(touchesReadingNoteDefinition(source, [{ from: note.from, to: note.from }])).toBe(false);
    expect(touchesReadingNoteDefinition(source, [{ from: note.from + 3, to: note.from + 3 }])).toBe(true);
    expect(touchesReadingNoteDefinition(source, [{ from: note.to - 1, to: note.to + 2 }])).toBe(true);
  });

  it("excludes annotation footers from continuous and rendered book selections", () => {
    const note = parsePageSourceMap(source).blocks[2];
    expect(continuousReadingSelection("book", source, note.contentFrom, note.contentTo).error).toContain("annotation footer");
    document.body.innerHTML = `<main class="main-content"><div class="page-content" data-page-id="book"><div data-block-id="${metadata.bodyBlockId}" data-reading-note-footer><div class="rendered-content">Note words</div></div></div></main>`;
    const text = document.querySelector(".rendered-content")!.firstChild!;
    const range = document.createRange();
    range.selectNodeContents(text);
    const selection = window.getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);
    expect(captureRenderedReadingSelection(selection)).toBeNull();
    selection.removeAllRanges();
  });

  it("renders a readable, safe footer from the annotation block rather than raw metadata", () => {
    const label = readingNoteBlockLabel({
      id: metadata.bodyBlockId,
      properties: { "reading-note-storage": "inline", "reading-note-label": "grafium-note-1", "reading-note": JSON.stringify(metadata) },
    });
    expect(label).toBe("grafium-note-1");
    const node = document.createElement("div");
    node.innerHTML = renderReadingNoteFooter(body + "\n<script>bad()</script>", label!, "pages/Books");
    expect(node.querySelector("[data-reading-note-footer]")).not.toBeNull();
    expect(node.querySelector("[data-reading-note-label]")?.textContent).toBe("Note 1");
    expect(node.querySelector("strong")?.textContent).toBe("Observation");
    expect(node.querySelector("script")).toBeNull();
    expect(node.textContent).toContain("id:: literal prose");
    expect(node.textContent).not.toContain(metadata.bodyBlockId);
  });
});

describe("legacy raw note source mapping", () => {
  it("maps the entire literal body to bodyBlockId and hides the property header", () => {
    const legacy = `title:: Existing note\nreading-note:: ${JSON.stringify({ ...metadata, version: 1, footnoteLabel: undefined })}\n\n${body}\n`;
    const info = legacyReadingNoteBody(legacy)!;
    const map = parsePageSourceMap(legacy);
    expect(map.hiddenRanges).toEqual([{ from: 0, to: info.bodyFrom }]);
    expect(map.blocks).toHaveLength(1);
    expect(map.blocks[0].id).toBe(metadata.bodyBlockId);
    expect(map.blocks[0].content).toBe(`${body}\n`);
    expect(map.blocks[0].propertyLines).toEqual([]);
    const replacement = sourceBlockContentReplacement(map.blocks[0], "- Still a literal body\nid:: preserve");
    expect(legacy.slice(0, replacement.from)).toContain('"bodyBlockId"');
    expect(replacement.insert).toBe("- Still a literal body\nid:: preserve");
    expect(replacement.to).toBe(legacy.length);
  });

  describe("source-only prompt projection", () => {
    it("excludes v1/v2 annotation roots and descendants without altering the full concurrency snapshot", () => {
      const legacyId = "33333333-3333-4333-8333-333333333333";
      const blocks = [
        { id: "grandchild", parent_id: "child", content: "Nested annotation", properties: {} },
        { id: "source", parent_id: null, content: "Original source", properties: {} },
        { id: "child", parent_id: metadata.bodyBlockId, content: "Annotation child", properties: {} },
        { id: metadata.bodyBlockId, parent_id: "source", content: "Inline annotation", properties: { "reading-note": JSON.stringify(metadata) } },
        { id: legacyId, parent_id: null, content: "Legacy annotation", properties: {
          "reading-note": JSON.stringify({ ...metadata, version: 1, bodyBlockId: legacyId, footnoteLabel: undefined }),
        } },
        { id: "other-source", parent_id: "source", content: "More source", properties: { "reading-note": "ordinary prose" } },
      ];
      const snapshot = JSON.stringify(blocks);
      const projected = readingSourceBlocks(blocks);
      expect(projected.map((block) => block.id)).toEqual(["source", "other-source"]);
      expect(projected[0]).toBe(blocks[1]);
      expect(JSON.stringify(blocks)).toBe(snapshot);
      expect(blocks).toHaveLength(6);
    });
  });

});

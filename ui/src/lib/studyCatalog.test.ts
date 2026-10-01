import { describe, expect, it } from "vitest";
import { isStudyLinkInput, searchStudyCatalog, studyCatalog } from "./studyCatalog";

describe("unified study catalog", () => {
  const catalog = studyCatalog([
    { id: "page", title: "Health/Supplements", is_journal: false },
    { id: "book", title: "Chinese grammar", is_journal: false, is_book: true },
    { id: "journal", title: "2026-09-30", is_journal: true },
  ], [{ topic: "chinese", total: 12, due: 3 }, { topic: "", total: 2, due: 1 }],
  ["assets/Chinese-lesson.mp3", "pages/course/assets/Lecture.MP4", "assets/cover.png", "assets/notes.pdf"]);

  it("merges typed pages, original books, flashcard decks, and playable assets without images", () => {
    expect(catalog).toHaveLength(7);
    expect(catalog.find(item => item.source === "book")).toMatchObject({ kind: "book", title: "Chinese grammar" });
    expect(catalog.find(item => item.source === "chinese")).toMatchObject({ kind: "flashcards", title: "#chinese", detail: "12 cards · 3 due" });
    expect(catalog.find(item => item.key === "cards:")).toMatchObject({ source: "", title: "Untagged cards" });
    expect(catalog.find(item => item.kind === "audio")).toMatchObject({ title: "Chinese lesson", source: "assets/Chinese-lesson.mp3" });
    expect(catalog.find(item => item.kind === "video")?.source).toBe("pages/course/assets/Lecture.MP4");
    expect(new Set(catalog.map(item => item.key)).size).toBe(catalog.length);
  });
  it("searches titles, types, filenames, namespaces, and pasted graph links across categories", () => {
    expect(searchStudyCatalog(catalog, "chinese").map(item => item.kind).sort()).toEqual(["audio", "book", "flashcards"]);
    expect(searchStudyCatalog(catalog, "[[Health/Supplements]]")[0].source).toBe("page");
    expect(searchStudyCatalog(catalog, "#chinese").some(item => item.kind === "flashcards")).toBe(true);
    expect(searchStudyCatalog(catalog, "mp3")[0].kind).toBe("audio");
    expect(searchStudyCatalog(catalog, "lecture")[0].kind).toBe("video");
  });
  it("matches the entire catalog before applying any display limit", () => {
    const pages = Array.from({ length: 150 }, (_, index) => ({ id: `${index}`, title: `Source ${index}`, is_journal: false }));
    expect(searchStudyCatalog(studyCatalog(pages, [], []), "Source 149")[0].source).toBe("149");
  });
  it("distinguishes explicit links from ordinary page titles without network guessing", () => {
    for (const value of ["https://youtu.be/dQw4w9WgXcQ", "http://example.com", "www.example.com", "file:///book", "javascript:alert(1)"])
      expect(isStudyLinkInput(value)).toBe(true);
    for (const value of ["Health: supplements", "[[Health/Supplements]]", "#chinese", "grammar.pdf", "assets/lesson.mp3", ""])
      expect(isStudyLinkInput(value)).toBe(false);
  });
});

import { describe, expect, it } from "vitest";
import { syncConflictTarget } from "./syncConflictTarget";

describe("sync conflict navigation", () => {
  it("opens original-book sources by identity, not a fabricated Markdown title", () => {
    const id = "759c230d-9aa0-4a9e-9179-30f373dd7d2b";
    for (const file of ["original.epub", "original.pdf", "book.json", "position.json"]) {
      expect(syncConflictTarget(`books/${id}/${file}`)).toEqual({ id });
    }
  });
  it("keeps Markdown conflicts in their own namespace", () => {
    expect(syncConflictTarget("pages/Reading Notes/Book.md")).toEqual({ title: "Reading Notes/Book" });
    expect(syncConflictTarget("journals/2026-09-30.md")).toEqual({ title: "2026-09-30" });
    expect(syncConflictTarget("knowledge/Links.md")).toEqual({ title: "Knowledge/Links" });
  });
  it("never creates notes for assets, caches, or unsafe paths", () => {
    for (const path of ["assets/book.epub", "pages/Books/assets/figure.png", "pages/Books/assets/README.md",
      "books/not-a-book/original.epub", "books/id/cache.json", "../pages/a.md", "/pages/a.md", "pages/a\\b.md"]) {
      expect(syncConflictTarget(path)).toBeNull();
    }
  });
});

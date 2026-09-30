import { beforeEach, describe, expect, it, vi } from "vitest";
import { ORIGINAL_BOOK_EXTENSIONS, CONVERTIBLE_BOOK_EXTENSIONS, queueBookImport } from "./bookImport";
import dialog from "../components/BookImportDialog.svelte?raw";

const native = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: native.invoke }));

describe("book import modes", () => {
  beforeEach(() => native.invoke.mockReset().mockResolvedValue("book-job"));

  it("copies originals through their own command, never through the converter", async () => {
    expect(await queueBookImport("original", " /library/book.epub ")).toBe("book-job");
    expect(native.invoke).toHaveBeenCalledTimes(1);
    expect(native.invoke).toHaveBeenCalledWith("books_import_originals", {
      sourcePaths: ["/library/book.epub"],
    });
  });

  it("preserves explicit file and recursive-folder Markdown conversion", async () => {
    await queueBookImport("markdown", "/library");
    expect(native.invoke).toHaveBeenCalledTimes(1);
    expect(native.invoke).toHaveBeenCalledWith("books_import_directory", { sourceDir: "/library" });
  });

  it("rejects missing input and preserves native errors", async () => {
    await expect(queueBookImport("original", " ")).rejects.toThrow("Choose a book");
    expect(native.invoke).not.toHaveBeenCalled();
    native.invoke.mockRejectedValueOnce(new Error("Graph is unavailable"));
    await expect(queueBookImport("original", "/library/book.epub")).rejects.toThrow("Graph is unavailable");
  });

  it("defaults to originals and clearly separates supported reader and conversion formats", () => {
    expect(dialog).toContain('$state<BookImportMode>("original")');
    expect(dialog).toContain('data-help-context="books"');
    expect(ORIGINAL_BOOK_EXTENSIONS).toEqual(["epub", "fb2", "mobi", "azw3", "pdf"]);
    expect(CONVERTIBLE_BOOK_EXTENSIONS).toContain("docx");
    expect(ORIGINAL_BOOK_EXTENSIONS).not.toContain("docx");
  });
});

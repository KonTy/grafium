import { beforeEach, describe, expect, it, vi } from "vitest";
import position from "../../tests/fixtures/private-reader-position.json";
import { BOOK_RENDERER_VERSION, isBookLocation } from "./bookLocations";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { privateLibrary, savePrivateBookmark, savePrivatePosition } from "./privateReader";

beforeEach(() => {
  invoke.mockReset();
  privateLibrary.set({ libraryPath: null, books: [] });
});

describe("shared native EPUB position contract", () => {
  it("uses the visual reader's kind discriminator and actual renderer version", () => {
    expect(isBookLocation(position.locator)).toBe(true);
    expect(position.locator.rendererVersion).toBe(BOOK_RENDERER_VERSION);
    const { kind, ...rest } = position.locator;
    expect(isBookLocation({ type: kind, ...rest })).toBe(false);
  });

  it("sends the same locator and narration offset unchanged to native persistence", async () => {
    if (!isBookLocation(position.locator)) throw new Error("Invalid shared EPUB contract fixture");
    invoke.mockResolvedValue(undefined);
    await savePrivatePosition("private-book", { offsetMs: position.offsetMs, locator: position.locator });
    expect(invoke).toHaveBeenCalledOnce();
    expect(invoke).toHaveBeenCalledWith("reader_save_position", {
      bookId: "private-book", position,
    });
  });

  it("sends identical canonical locations for durable bookmarks", async () => {
    if (!isBookLocation(position.locator)) throw new Error("Invalid shared EPUB contract fixture");
    const bookmark = { id: "mark", bookId: "private-book", position, createdAt: 1760000000000, note: "" };
    invoke.mockResolvedValueOnce(bookmark).mockResolvedValueOnce({ libraryPath: null, books: [] });
    expect(await savePrivateBookmark("private-book", {
      offsetMs: position.offsetMs, locator: position.locator,
    })).toEqual(bookmark);
    expect(invoke).toHaveBeenNthCalledWith(1, "reader_add_bookmark", {
      bookId: "private-book", position, note: "",
    });
  });
});

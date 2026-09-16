import { describe, expect, it, vi } from "vitest";
import { isPageNotFoundError, loadPageForNavigation, resolvePageLookup } from "./navigation";

describe("page navigation resolution", () => {
  it("resolves ReferencePanel page-id navigation by id", async () => {
    const page = {
      id: "page-2",
      title: "Renamed page",
      is_journal: false,
      created_at: "0",
      updated_at: "0",
      properties: {},
    };
    const getPage = vi.fn(async () => page);

    const result = await loadPageForNavigation({ id: "page-2" }, getPage);

    expect(getPage).toHaveBeenCalledWith({ id: "page-2" });
    expect(result).toEqual(page);
  });

  it("keeps title-based navigation for string targets", () => {
    expect(resolvePageLookup("Welcome To Grafium")).toEqual({
      title: "Welcome To Grafium",
    });
  });

  it("allows creation only for an authoritative missing-page response", () => {
    expect(isPageNotFoundError({
      code: "page_not_found",
      message: "No page has this title or alias.",
    })).toBe(true);
    for (const failure of [
      { code: "page_lookup_failed", message: "The approved alias is ambiguous." },
      { code: "page_lookup_failed", message: "Database unavailable." },
      new Error("Loading page timed out"),
      "not found",
      null,
      undefined,
    ]) {
      expect(isPageNotFoundError(failure)).toBe(false);
    }
  });

  it("preserves ambiguous lookup errors instead of treating them as a missing page", async () => {
    const ambiguity = { code: "page_lookup_failed", message: "Niacin is ambiguous." };
    const getPage = vi.fn().mockRejectedValue(ambiguity);
    await expect(loadPageForNavigation("Niacin", getPage)).rejects.toEqual(ambiguity);
    expect(isPageNotFoundError(ambiguity)).toBe(false);
  });
});

import { describe, expect, it, vi } from "vitest";
import { loadPageForNavigation, resolvePageLookup } from "./navigation";

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
});

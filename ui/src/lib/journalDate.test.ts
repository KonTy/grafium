import { describe, expect, it } from "vitest";
import { formatLocalIsoDate, insertJournalPageByTitleDesc, isJournalDateTitle, shiftIsoDate } from "./journalDate";

describe("journalDate", () => {
  it("formats local calendar dates without UTC conversion", () => {
    expect(formatLocalIsoDate(new Date(2024, 0, 4))).toBe("2024-01-04");
  });

  it("recognizes journal titles", () => {
    expect(isJournalDateTitle("2024-01-04")).toBe(true);
    expect(isJournalDateTitle("Journal")).toBe(false);
    expect(isJournalDateTitle(undefined)).toBe(false);
  });

  it("shifts by whole local days across month boundaries", () => {
    expect(shiftIsoDate("2024-01-31", 1)).toBe("2024-02-01");
    expect(shiftIsoDate("2024-03-01", -1)).toBe("2024-02-29");
  });

  it("inserts journal pages newest-first without duplicating titles", () => {
    const pages = [
      { id: "a", title: "2024-03-02" },
      { id: "b", title: "2024-03-01" },
    ];
    expect(insertJournalPageByTitleDesc(pages, { id: "c", title: "2024-03-03" }).map((page) => page.title))
      .toEqual(["2024-03-03", "2024-03-02", "2024-03-01"]);
    expect(insertJournalPageByTitleDesc(pages, { id: "d", title: "2024-02-28" }).map((page) => page.title))
      .toEqual(["2024-03-02", "2024-03-01", "2024-02-28"]);
    expect(insertJournalPageByTitleDesc(pages, { id: "a", title: "2024-03-02" })).toEqual(pages);
  });
});

import { describe, expect, it } from "vitest";
import {
  calendarDate,
  calendarDateKey,
  calendarMonthLength,
  mondayWeekday,
  navigateCalendarDay,
  navigateCalendarGrid,
  parseCalendarDate,
  shiftCalendarMonth,
} from "./datePickerNavigation";

describe("date picker local dates", () => {
  it("parses local calendar fields rather than UTC and preserves early years", () => {
    const date = parseCalendarDate("0099-02-28")!;
    expect([date.getFullYear(), date.getMonth(), date.getDate(), date.getHours()]).toEqual([99, 1, 28, 12]);
    expect(calendarDateKey(date)).toBe("0099-02-28");
    expect(calendarDateKey(calendarDate(2024, 1, 29))).toBe("2024-02-29");
  });

  it.each([undefined, "", "2024-2-01", "2024-01-1", "2024-01-01T00:00:00Z", "2023-02-29", "2024-04-31", "2024-13-01", "2024-00-01", "2024-01-00", "0000-01-01"])(
    "rejects invalid selection %s so callers can default to today",
    (value) => expect(parseCalendarDate(value)).toBeNull(),
  );

  it("handles Gregorian leap years and Monday-first week offsets", () => {
    expect(calendarMonthLength(2024, 1)).toBe(29);
    expect(calendarMonthLength(2023, 1)).toBe(28);
    expect(calendarMonthLength(1900, 1)).toBe(28);
    expect(calendarMonthLength(2000, 1)).toBe(29);
    expect(mondayWeekday(calendarDate(2024, 3, 1))).toBe(0);
    expect(mondayWeekday(calendarDate(2024, 2, 31))).toBe(6);
  });
});

describe("date picker day keyboard navigation", () => {
  it.each([
    ["2024-01-31", "ArrowRight", false, "2024-02-01"],
    ["2024-03-01", "ArrowLeft", false, "2024-02-29"],
    ["2023-12-31", "ArrowRight", false, "2024-01-01"],
    ["2024-01-01", "ArrowLeft", false, "2023-12-31"],
    ["2024-03-04", "ArrowUp", false, "2024-02-26"],
    ["2024-12-29", "ArrowDown", false, "2025-01-05"],
    ["2024-05-01", "Home", false, "2024-04-29"],
    ["2024-05-31", "End", false, "2024-06-02"],
    ["2024-06-02", "Home", false, "2024-05-27"],
    ["2024-05-27", "Home", false, "2024-05-27"],
    ["2024-06-02", "End", false, "2024-06-02"],
    ["2024-01-31", "PageDown", false, "2024-02-29"],
    ["2023-01-31", "PageDown", false, "2023-02-28"],
    ["2024-03-31", "PageUp", false, "2024-02-29"],
    ["2024-12-15", "PageDown", false, "2025-01-15"],
    ["2024-01-15", "PageUp", false, "2023-12-15"],
    ["2024-02-29", "PageDown", true, "2025-02-28"],
    ["2024-02-29", "PageUp", true, "2023-02-28"],
    ["2024-03-09", "ArrowRight", false, "2024-03-10"],
    ["2024-03-10", "ArrowRight", false, "2024-03-11"],
    ["2024-11-03", "ArrowDown", false, "2024-11-10"],
    ["0001-01-01", "ArrowLeft", false, "0001-01-01"],
    ["9999-12-31", "ArrowRight", false, "9999-12-31"],
  ])("%s + %s (shift: %s) → %s", (iso, key, shift, expected) => {
    const date = parseCalendarDate(iso as string)!;
    const before = date.getTime();
    const next = navigateCalendarDay(date, key as string, shift as boolean)!;
    expect(calendarDateKey(next)).toBe(expected);
    expect(next.getHours()).toBe(12);
    expect(date.getTime()).toBe(before);
  });

  it("leaves activation and unrelated or modified keys to the caller", () => {
    const date = calendarDate(2024, 0, 1);
    for (const key of ["Enter", " ", "Escape", "Tab", "g"]) {
      expect(navigateCalendarDay(date, key)).toBeNull();
    }
    expect(navigateCalendarDay(date, "ArrowRight", true)).toBeNull();
    expect(navigateCalendarDay(date, "Home", true)).toBeNull();
  });

  it("clamps month shifts within four-digit years", () => {
    expect(calendarDateKey(shiftCalendarMonth(calendarDate(1, 0, 1), -12))).toBe("0001-01-01");
    expect(calendarDateKey(shiftCalendarMonth(calendarDate(9999, 11, 31), 12))).toBe("9999-12-31");
  });
});

describe("date picker month and year grid navigation", () => {
  it.each([
    [5, "ArrowLeft", 4],
    [5, "ArrowRight", 6],
    [5, "ArrowUp", 2],
    [5, "ArrowDown", 8],
    [5, "Home", 3],
    [4, "End", 5],
    [11, "ArrowRight", 12],
    [12, "ArrowLeft", 11],
    [2, "ArrowUp", -1],
    [10, "ArrowDown", 13],
    [8, "PageUp", -4],
    [8, "PageDown", 20],
    [2027, "ArrowRight", 2028],
    [2028, "ArrowLeft", 2027],
    [2029, "Home", 2028],
    [2029, "End", 2030],
  ])("moves index %s with %s to %s", (index, key, expected) => {
    expect(navigateCalendarGrid(index as number, key as string)).toBe(expected);
  });

  it("wraps month-grid navigation into the adjacent year without invalid dates", () => {
    const date = calendarDate(2024, 11, 31);
    const index = date.getFullYear() * 12 + date.getMonth();
    const moved = navigateCalendarGrid(index, "ArrowDown")!;
    expect(calendarDateKey(shiftCalendarMonth(date, moved - index))).toBe("2025-03-31");
  });

  it("does not treat selection, cancellation or global shortcuts as navigation", () => {
    for (const key of ["Enter", " ", "Escape", "Tab", "g"]) {
      expect(navigateCalendarGrid(2024, key)).toBeNull();
    }
  });
});

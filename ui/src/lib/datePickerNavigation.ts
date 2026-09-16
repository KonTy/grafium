/** Construct local calendar dates at noon, avoiding UTC parsing and DST midnight gaps. */
export function calendarDate(year: number, month: number, day = 1): Date {
  const date = new Date(0);
  date.setHours(12, 0, 0, 0);
  date.setFullYear(year, month, day);
  return date;
}

export function calendarDateKey(date: Date): string {
  return `${String(date.getFullYear()).padStart(4, "0")}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

export function parseCalendarDate(value?: string): Date | null {
  if (!value || !/^\d{4}-\d{2}-\d{2}$/.test(value)) return null;
  const [year, month, day] = value.split("-").map(Number);
  if (year < 1 || month < 1 || month > 12 || day < 1 || day > 31) return null;
  const date = calendarDate(year, month - 1, day);
  return calendarDateKey(date) === value ? date : null;
}

export function calendarMonthLength(year: number, month: number): number {
  return calendarDate(year, month + 1, 0).getDate();
}

export function mondayWeekday(date: Date): number {
  return (date.getDay() + 6) % 7;
}

export function shiftCalendarMonth(date: Date, months: number): Date {
  const index = Math.max(12, Math.min(9999 * 12 + 11, date.getFullYear() * 12 + date.getMonth() + months));
  const year = Math.floor(index / 12);
  const month = index % 12;
  return calendarDate(year, month, Math.min(date.getDate(), calendarMonthLength(year, month)));
}

export function navigateCalendarDay(date: Date, key: string, shift = false): Date | null {
  if (key === "PageUp" || key === "PageDown") {
    return shiftCalendarMonth(date, (key === "PageUp" ? -1 : 1) * (shift ? 12 : 1));
  }
  if (shift) return null;
  let days: number;
  switch (key) {
    case "ArrowLeft": days = -1; break;
    case "ArrowRight": days = 1; break;
    case "ArrowUp": days = -7; break;
    case "ArrowDown": days = 7; break;
    case "Home": days = -mondayWeekday(date); break;
    case "End": days = 6 - mondayWeekday(date); break;
    default: return null;
  }
  const next = calendarDate(date.getFullYear(), date.getMonth(), date.getDate() + days);
  if (next.getFullYear() < 1) return calendarDate(1, 0, 1);
  if (next.getFullYear() > 9999) return calendarDate(9999, 11, 31);
  return next;
}

/** Move through a paged grid, including wrapping into adjacent pages. */
export function navigateCalendarGrid(index: number, key: string, columns = 3, pageSize = 12): number | null {
  switch (key) {
    case "ArrowLeft": return index - 1;
    case "ArrowRight": return index + 1;
    case "ArrowUp": return index - columns;
    case "ArrowDown": return index + columns;
    case "Home": return index - index % columns;
    case "End": return index + columns - 1 - index % columns;
    case "PageUp": return index - pageSize;
    case "PageDown": return index + pageSize;
    default: return null;
  }
}

import { describe, expect, it } from "vitest";
import {
  bucketFor,
  compareTasks,
  effectiveDate,
  groupTasks,
  humanDuration,
  localDateKey,
  paceTrend,
  type OpenTaskRow,
  type TaskFlowStats,
} from "./taskBoard";

const task = (over: Partial<OpenTaskRow> = {}): OpenTaskRow => ({
  block_id: over.block_id ?? "b1",
  content: "TODO thing",
  page_title: "work",
  state: "TODO",
  priority: null,
  scheduled_date: null,
  scheduled_time: null,
  deadline_date: null,
  created_at: 0,
  updated_at: 0,
  ...over,
});

// A Wednesday, so "this week" boundaries are not accidentally week-aligned.
const TODAY = new Date(2026, 8, 9);

describe("bucketFor", () => {
  it("puts a past date in 'from earlier'", () => {
    expect(bucketFor(task({ scheduled_date: "2026-09-08" }), TODAY)).toBe("earlier");
  });

  it("recognises today and tomorrow", () => {
    expect(bucketFor(task({ scheduled_date: "2026-09-09" }), TODAY)).toBe("today");
    expect(bucketFor(task({ scheduled_date: "2026-09-10" }), TODAY)).toBe("tomorrow");
  });

  it("treats 'this week' as the next seven days", () => {
    // Calendar-week grouping would leave this nearly empty on a Saturday.
    expect(bucketFor(task({ scheduled_date: "2026-09-16" }), TODAY)).toBe("week");
    expect(bucketFor(task({ scheduled_date: "2026-09-17" }), TODAY)).toBe("later");
  });

  it("puts a task with no date under 'no date' rather than dropping it", () => {
    expect(bucketFor(task(), TODAY)).toBe("someday");
  });

  it("crosses a month boundary correctly", () => {
    const monthEnd = new Date(2026, 8, 30);
    expect(bucketFor(task({ scheduled_date: "2026-10-01" }), monthEnd)).toBe("tomorrow");
    expect(bucketFor(task({ scheduled_date: "2026-09-29" }), monthEnd)).toBe("earlier");
  });

  it("crosses a year boundary correctly", () => {
    const yearEnd = new Date(2026, 11, 31);
    expect(bucketFor(task({ scheduled_date: "2027-01-01" }), yearEnd)).toBe("tomorrow");
  });
});

describe("effectiveDate", () => {
  it("prefers the deadline over the scheduled date", () => {
    // Scheduled is when you meant to start; the deadline is when it must be
    // done, and that is the one you need to see coming.
    const t = task({ scheduled_date: "2026-09-20", deadline_date: "2026-09-10" });
    expect(effectiveDate(t)).toBe("2026-09-10");
    expect(bucketFor(t, TODAY)).toBe("tomorrow");
  });

  it("falls back to the scheduled date", () => {
    expect(effectiveDate(task({ scheduled_date: "2026-09-20" }))).toBe("2026-09-20");
  });
});

describe("compareTasks", () => {
  it("sorts priority A before B before none", () => {
    const sorted = [task({ priority: null }), task({ priority: "B" }), task({ priority: "A" })]
      .sort(compareTasks)
      .map((t) => t.priority);
    expect(sorted).toEqual(["A", "B", null]);
  });

  it("sorts by date within the same priority", () => {
    const sorted = [
      task({ block_id: "late", scheduled_date: "2026-09-20" }),
      task({ block_id: "soon", scheduled_date: "2026-09-10" }),
    ]
      .sort(compareTasks)
      .map((t) => t.block_id);
    expect(sorted).toEqual(["soon", "late"]);
  });

  it("puts the longest-waiting task first when all else is equal", () => {
    const sorted = [
      task({ block_id: "new", created_at: 2000 }),
      task({ block_id: "old", created_at: 1000 }),
    ]
      .sort(compareTasks)
      .map((t) => t.block_id);
    expect(sorted).toEqual(["old", "new"]);
  });

  it("ranks a dated task above an undated one", () => {
    const sorted = [task({ block_id: "none" }), task({ block_id: "dated", scheduled_date: "2026-09-20" })]
      .sort(compareTasks)
      .map((t) => t.block_id);
    expect(sorted).toEqual(["dated", "none"]);
  });
});

describe("groupTasks", () => {
  it("returns groups in calendar order and omits empty ones", () => {
    const groups = groupTasks(
      [
        task({ block_id: "later", scheduled_date: "2026-10-30" }),
        task({ block_id: "past", scheduled_date: "2026-09-01" }),
        task({ block_id: "today", scheduled_date: "2026-09-09" }),
      ],
      TODAY,
    );
    expect(groups.map((g) => g.bucket)).toEqual(["earlier", "today", "later"]);
    expect(groups.map((g) => g.tasks.length)).toEqual([1, 1, 1]);
  });

  it("keeps every task", () => {
    const tasks = [
      task({ block_id: "a", scheduled_date: "2026-09-01" }),
      task({ block_id: "b" }),
      task({ block_id: "c", deadline_date: "2026-09-09" }),
    ];
    const total = groupTasks(tasks, TODAY).reduce((n, g) => n + g.tasks.length, 0);
    expect(total).toBe(tasks.length);
  });

  it("returns nothing for no tasks", () => {
    expect(groupTasks([], TODAY)).toEqual([]);
  });
});

describe("humanDuration", () => {
  it("scales the unit to the size", () => {
    expect(humanDuration(45 * 60_000)).toBe("45m");
    expect(humanDuration(3 * 3_600_000)).toBe("3h");
    expect(humanDuration(3 * 86_400_000)).toBe("3d");
    expect(humanDuration(21 * 86_400_000)).toBe("3w");
    expect(humanDuration(120 * 86_400_000)).toBe("4mo");
  });

  it("has nothing to say about missing data", () => {
    expect(humanDuration(null)).toBeNull();
    expect(humanDuration(-1)).toBeNull();
  });
});

describe("paceTrend", () => {
  const stats = (now: number, before: number): TaskFlowStats => ({
    throughput_7d: now,
    throughput_prev_7d: before,
    weekly_completions: [],
    median_cycle_ms: null,
    median_wait_ms: null,
    on_time_rate: null,
    oldest_open_days: null,
    open_count: 0,
    done_count: 0,
    by_page: [],
  });

  it("ignores small wobbles", () => {
    // Week-to-week noise is not a trend, and reporting it as one turns an
    // ordinary quiet week into a failure.
    expect(paceTrend(stats(1.05, 1))).toBe("steady");
    expect(paceTrend(stats(0.95, 1))).toBe("steady");
  });

  it("reports a real change", () => {
    expect(paceTrend(stats(2, 1))).toBe("up");
    expect(paceTrend(stats(0.5, 1))).toBe("down");
  });

  it("handles a first week with nothing to compare against", () => {
    expect(paceTrend(stats(0, 0))).toBe("steady");
    expect(paceTrend(stats(3, 0))).toBe("up");
  });
});

describe("localDateKey", () => {
  it("uses local time, not UTC", () => {
    // A UTC-based key rolls over at the wrong moment and puts "today" in the
    // wrong bucket for anyone west of Greenwich.
    expect(localDateKey(new Date(2026, 8, 9, 23, 30))).toBe("2026-09-09");
    expect(localDateKey(new Date(2026, 0, 1, 0, 30))).toBe("2026-01-01");
  });
});

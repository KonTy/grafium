/**
 * Placing open tasks on a calendar, and the shape of the Tasks dashboard.
 *
 * Grouping lives here rather than in the component so it can be tested against
 * awkward dates — month ends, week boundaries, a task due today at a time that
 * has already passed — without standing up a browser.
 */

export interface OpenTaskRow {
  block_id: string;
  content: string;
  page_title: string;
  state: string;
  priority: string | null;
  scheduled_date: string | null;
  scheduled_time: string | null;
  deadline_date: string | null;
  created_at: number;
  updated_at: number;
}

export interface TaskFlowStats {
  throughput_7d: number;
  throughput_prev_7d: number;
  weekly_completions: number[];
  median_cycle_ms: number | null;
  median_wait_ms: number | null;
  on_time_rate: number | null;
  oldest_open_days: number | null;
  open_count: number;
  done_count: number;
  by_page: [string, number][];
}

/**
 * Buckets, in the order they are shown.
 *
 * "earlier" rather than "overdue", and no count badge on it. A growing red
 * number is the thing task apps get wrong: it reads as an accusation and
 * drives avoidance rather than action. These are simply the ones that need a
 * decision first.
 */
export type TaskBucket = "earlier" | "today" | "tomorrow" | "week" | "later" | "someday";

export const BUCKET_ORDER: TaskBucket[] = [
  "earlier",
  "today",
  "tomorrow",
  "week",
  "later",
  "someday",
];

export const BUCKET_LABEL: Record<TaskBucket, string> = {
  earlier: "From earlier",
  today: "Today",
  tomorrow: "Tomorrow",
  week: "This week",
  later: "Later",
  someday: "No date",
};

/** Local calendar date as `YYYY-MM-DD`, which is how dates arrive from the DB. */
export function localDateKey(date: Date): string {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, "0");
  const d = String(date.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

function addDays(date: Date, days: number): Date {
  const next = new Date(date);
  next.setDate(next.getDate() + days);
  return next;
}

/**
 * The date a task is placed by.
 *
 * A deadline wins over a scheduled date when both exist: the scheduled date is
 * when you meant to start, the deadline is when it actually has to be done, and
 * the second is the one you need to see coming.
 */
export function effectiveDate(task: OpenTaskRow): string | null {
  return task.deadline_date ?? task.scheduled_date ?? null;
}

export function bucketFor(task: OpenTaskRow, today: Date): TaskBucket {
  const date = effectiveDate(task);
  if (!date) return "someday";

  const todayKey = localDateKey(today);
  if (date < todayKey) return "earlier";
  if (date === todayKey) return "today";
  if (date === localDateKey(addDays(today, 1))) return "tomorrow";
  // "This week" means the next seven days rather than up to Sunday: on a
  // Saturday the calendar week is nearly over and would be a near-empty group.
  if (date <= localDateKey(addDays(today, 7))) return "week";
  return "later";
}

const PRIORITY_RANK: Record<string, number> = { A: 0, B: 1, C: 2 };

/**
 * Order within a bucket: priority first, then by date, then oldest first.
 *
 * Oldest-first as the final tiebreak is deliberate — within a day, the thing
 * that has been waiting longest should not be at the bottom.
 */
export function compareTasks(a: OpenTaskRow, b: OpenTaskRow): number {
  const pa = PRIORITY_RANK[a.priority?.toUpperCase() ?? ""] ?? 3;
  const pb = PRIORITY_RANK[b.priority?.toUpperCase() ?? ""] ?? 3;
  if (pa !== pb) return pa - pb;

  const da = effectiveDate(a);
  const db = effectiveDate(b);
  if (da && db && da !== db) return da < db ? -1 : 1;
  if (da && !db) return -1;
  if (!da && db) return 1;

  return a.created_at - b.created_at;
}

export interface TaskGroup {
  bucket: TaskBucket;
  label: string;
  tasks: OpenTaskRow[];
}

/** Group open tasks for display, dropping buckets that would be empty. */
export function groupTasks(tasks: readonly OpenTaskRow[], today: Date): TaskGroup[] {
  const byBucket = new Map<TaskBucket, OpenTaskRow[]>();
  for (const task of tasks) {
    const bucket = bucketFor(task, today);
    const list = byBucket.get(bucket);
    if (list) list.push(task);
    else byBucket.set(bucket, [task]);
  }

  return BUCKET_ORDER.filter((bucket) => byBucket.has(bucket)).map((bucket) => ({
    bucket,
    label: BUCKET_LABEL[bucket],
    tasks: byBucket.get(bucket)!.sort(compareTasks),
  }));
}

/** A duration, said the way a person would say it. */
export function humanDuration(ms: number | null): string | null {
  if (ms === null || ms < 0) return null;
  const minutes = Math.round(ms / 60000);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.round(hours / 24);
  if (days < 14) return `${days}d`;
  const weeks = Math.round(days / 7);
  if (weeks < 9) return `${weeks}w`;
  return `${Math.round(days / 30)}mo`;
}

/**
 * How this week's pace compares with last week's.
 *
 * Returned as a direction rather than a percentage: a hard number invites
 * treating a quiet week as a failure, when the useful signal is just whether
 * things are moving.
 */
export function paceTrend(stats: TaskFlowStats): "up" | "down" | "steady" {
  const now = stats.throughput_7d;
  const before = stats.throughput_prev_7d;
  if (before === 0 && now === 0) return "steady";
  if (before === 0) return "up";
  const change = (now - before) / before;
  if (change > 0.15) return "up";
  if (change < -0.15) return "down";
  return "steady";
}

/**
 * Background job tracking.
 *
 * Long AI work no longer belongs to the panel that started it. The backend
 * returns a job id immediately and reports progress on `job://update`; this
 * store mirrors that stream so any part of the UI can show activity, and so a
 * completion toast can be raised even if the user has navigated
 * somewhere else entirely.
 */

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { showToast } from "./toast.svelte";

export type JobStatus = "running" | "succeeded" | "failed" | "cancelled";

export interface JobLink {
  page_id: string;
  page_title?: string;
  label: string;
}

export interface Job {
  id: string;
  kind: string;
  title: string;
  status: JobStatus;
  progress: number | null;
  message: string | null;
  link: JobLink | null;
  error: string | null;
  details: string | null;
  cancellable: boolean;
  started_at: number;
  finished_at: number | null;
}

export const jobs = $state<Job[]>([]);

export function isTerminal(status: JobStatus): boolean {
  return status !== "running";
}

/** Jobs still in flight — what an activity indicator should count. */
export function runningJobs(): Job[] {
  return jobs.filter((j) => j.status === "running");
}

const SEEN_KEY = "grafium.jobs.seenUntil";

function loadSeenUntil(): number {
  try {
    const value = Number(localStorage.getItem(SEEN_KEY));
    return Number.isFinite(value) && value > 0 ? value : 0;
  } catch {
    return 0;
  }
}

/**
 * Finish time of the newest job the Jobs page has shown. Results that finish
 * after it are what the title-bar bell announces. Kept on this device, like
 * the job history itself.
 */
export const jobsSeen = $state({ until: loadSeenUntil() });

/** Completions and failures the Jobs page has not shown yet. */
export function unseenResults(list: readonly Job[], seenUntil: number): Job[] {
  return list.filter(
    (job) => (job.status === "succeeded" || job.status === "failed") && (job.finished_at ?? 0) > seenUntil
  );
}

/** The newest finish time in `list`, or 0 when nothing has finished. */
export function latestFinish(list: readonly Job[]): number {
  return list.reduce((latest, job) => Math.max(latest, job.finished_at ?? 0), 0);
}

/** Record that the Jobs page has shown every job finished up to `finishedAt`. */
export function markJobsSeen(finishedAt: number): void {
  if (!(finishedAt > jobsSeen.until)) return;
  jobsSeen.until = finishedAt;
  try {
    localStorage.setItem(SEEN_KEY, String(finishedAt));
  } catch {
    // Without storage the bell still clears until Grafium restarts.
  }
}

export interface JobIndicator {
  /** The number on the bell; 0 shows no badge. */
  count: number;
  /** What the bell is reporting, e.g. "2 running" or "1 new result". */
  summary: string;
  running: boolean;
}

/**
 * What the title-bar bell shows: jobs in flight first, otherwise results the
 * Jobs page has not shown yet. History the user has already seen adds nothing.
 */
export function jobIndicator(list: readonly Job[], seenUntil: number): JobIndicator {
  const running = list.filter((job) => job.status === "running").length;
  const unseen = unseenResults(list, seenUntil);
  const failed = unseen.filter((job) => job.status === "failed").length;
  const parts: string[] = [];
  if (running > 0) parts.push(`${running} running`);
  if (unseen.length > 0) parts.push(`${unseen.length} new ${unseen.length === 1 ? "result" : "results"}`);
  if (failed > 0) parts.push(`${failed} failed`);
  return { count: running > 0 ? running : unseen.length, summary: parts.join(", "), running: running > 0 };
}

export interface JobGroups {
  running: Job[];
  failed: Job[];
  completed: Job[];
  cancelled: Job[];
}

/** Jobs by state for the Jobs page, each newest first. */
export function groupJobs(list: readonly Job[]): JobGroups {
  const recency = (job: Job) => job.finished_at ?? job.started_at;
  // Reversed first so that, between equal times, the later-added job leads.
  const newestFirst = [...list].reverse().sort((a, b) => recency(b) - recency(a));
  const withStatus = (status: JobStatus) => newestFirst.filter((job) => job.status === status);
  return {
    running: withStatus("running"),
    failed: withStatus("failed"),
    completed: withStatus("succeeded"),
    cancelled: withStatus("cancelled"),
  };
}

/**
 * Apply an update from the backend.
 *
 * Exported for tests. Matching is by id, and a job that has already reached a
 * terminal state is never moved back to running: the backend guards this too,
 * but events can in principle arrive out of order and a job flickering back to
 * "running" after the user saw it finish would be worse than a dropped update.
 */
export function applyJobUpdate(update: Job): { isNewlyFinished: boolean } {
  const index = jobs.findIndex((j) => j.id === update.id);

  if (index === -1) {
    jobs.push(update);
    return { isNewlyFinished: isTerminal(update.status) };
  }

  const previous = jobs[index];
  if (isTerminal(previous.status) && !isTerminal(update.status)) {
    return { isNewlyFinished: false };
  }

  jobs[index] = update;
  return {
    isNewlyFinished: !isTerminal(previous.status) && isTerminal(update.status),
  };
}

/**
 * Drop a job the backend discarded: an automatic run that found nothing to
 * do, which is not kept as history. Exported for tests.
 */
export function removeJob(id: string): void {
  const index = jobs.findIndex((j) => j.id === id);
  if (index !== -1) jobs.splice(index, 1);
}

/** The toast text for a job that has just finished. */
export function describeFinishedJob(job: Job): string | null {
  switch (job.status) {
    case "succeeded":
      return job.message ?? `${job.title} finished`;
    case "failed":
      return job.error ? `${job.title} failed: ${job.error}` : `${job.title} failed`;
    // A cancellation is something the user just asked for. Telling them it
    // happened is noise.
    case "cancelled":
      return null;
    default:
      return null;
  }
}

export async function cancelJob(jobId: string): Promise<boolean> {
  return invoke("jobs_cancel", { jobId });
}

/** Clear the job history, also on disk. Running jobs stay. */
export async function clearFinishedJobs(): Promise<void> {
  await invoke("jobs_clear_finished");
  for (let i = jobs.length - 1; i >= 0; i--) {
    if (isTerminal(jobs[i].status)) jobs.splice(i, 1);
  }
}

/**
 * Subscribe to job events and rehydrate the running jobs and the history kept
 * from earlier runs. Loading history raises no toasts.
 *
 * Listening is set up *before* the initial list is fetched, so a job that
 * finishes during startup can't slip through the gap between the two.
 */
export async function initJobs(
  onFinished?: (job: Job) => void
): Promise<UnlistenFn> {
  const stopUpdates = await listen<Job>("job://update", (event) => {
    const { isNewlyFinished } = applyJobUpdate(event.payload);
    if (isNewlyFinished) onFinished?.(event.payload);
  });
  const stopRemovals = await listen<string>("job://removed", (event) => removeJob(event.payload));
  const unlisten: UnlistenFn = () => {
    stopUpdates();
    stopRemovals();
  };

  try {
    const existing = await invoke<Job[]>("jobs_list");
    for (const job of existing) applyJobUpdate(job);
  } catch {
    // A missing job list is not worth blocking startup or nagging over; live
    // events will still populate the store.
  }

  return unlisten;
}

/**
 * Default completion handler: toast, and offer a way to reach the result.
 */
export function notifyJobFinished(job: Job, openPage?: (pageId: string) => void): void {
  const message = describeFinishedJob(job);
  if (!message) return;

  const action =
    job.status === "succeeded" && job.link && openPage
      ? { label: `Open ${job.link.label}`, run: () => openPage(job.link!.page_id) }
      : undefined;

  showToast(message, job.status === "succeeded" ? "success" : "error", action);
}

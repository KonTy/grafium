/**
 * Background job tracking.
 *
 * Long AI work no longer belongs to the panel that started it. The backend
 * returns a job id immediately and reports progress on `job://update`; this
 * store mirrors that stream so any part of the UI can show activity, and so a
 * completion notification can be raised even if the user has navigated
 * somewhere else entirely.
 */

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { showToast } from "./toast.svelte";

export type JobStatus = "running" | "succeeded" | "failed" | "cancelled";

export interface JobLink {
  page_id: string;
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

/** The notification text for a job that has just finished. */
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

export async function clearFinishedJobs(): Promise<void> {
  await invoke("jobs_clear_finished");
  for (let i = jobs.length - 1; i >= 0; i--) {
    if (isTerminal(jobs[i].status)) jobs.splice(i, 1);
  }
}

/**
 * Subscribe to job events and rehydrate anything already running.
 *
 * Listening is set up *before* the initial list is fetched, so a job that
 * finishes during startup can't slip through the gap between the two.
 */
export async function initJobs(
  onFinished?: (job: Job) => void
): Promise<UnlistenFn> {
  const unlisten = await listen<Job>("job://update", (event) => {
    const { isNewlyFinished } = applyJobUpdate(event.payload);
    if (isNewlyFinished) onFinished?.(event.payload);
  });

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
 * Default completion handler: notify, and offer a way to reach the result.
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

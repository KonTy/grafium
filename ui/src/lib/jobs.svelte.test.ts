import { describe, it, expect, beforeEach, vi } from "vitest";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { jobs, applyJobUpdate, describeFinishedJob, isTerminal, type Job } from "./jobs.svelte";
import { toasts } from "./toast.svelte";

function job(overrides: Partial<Job> = {}): Job {
  return {
    id: "job-1",
    kind: "ai_index_all",
    title: "Indexing graph for AI search",
    status: "running",
    progress: null,
    message: null,
    link: null,
    error: null,
    cancellable: true,
    started_at: 0,
    finished_at: null,
    ...overrides,
  };
}

describe("job store", () => {
  beforeEach(() => {
    jobs.splice(0, jobs.length);
    toasts.splice(0, toasts.length);
  });

  it("adds a job it has not seen before", () => {
    const { isNewlyFinished } = applyJobUpdate(job());
    expect(jobs).toHaveLength(1);
    expect(isNewlyFinished).toBe(false);
  });

  it("updates in place rather than accumulating duplicates", () => {
    applyJobUpdate(job({ progress: 0.1 }));
    applyJobUpdate(job({ progress: 0.9 }));

    expect(jobs).toHaveLength(1);
    expect(jobs[0].progress).toBe(0.9);
  });

  it("reports the transition to finished exactly once", () => {
    applyJobUpdate(job());
    const first = applyJobUpdate(job({ status: "succeeded" }));
    const second = applyJobUpdate(job({ status: "succeeded" }));

    expect(first.isNewlyFinished).toBe(true);
    // Otherwise a duplicated event would raise a second notification for work
    // the user was already told about.
    expect(second.isNewlyFinished).toBe(false);
  });

  it("treats a job first seen as finished as newly finished", () => {
    // Rehydration after a restart shouldn't be the only path; a job that
    // completes between subscribing and listing still needs to notify.
    const { isNewlyFinished } = applyJobUpdate(job({ status: "succeeded" }));
    expect(isNewlyFinished).toBe(true);
  });

  it("never moves a finished job back to running", () => {
    applyJobUpdate(job({ status: "succeeded" }));
    const result = applyJobUpdate(job({ status: "running", message: "late tick" }));

    expect(result.isNewlyFinished).toBe(false);
    expect(jobs[0].status).toBe("succeeded");
    expect(jobs[0].message).not.toBe("late tick");
  });

  it("tracks several jobs independently", () => {
    applyJobUpdate(job({ id: "a" }));
    applyJobUpdate(job({ id: "b" }));
    applyJobUpdate(job({ id: "a", status: "failed", error: "boom" }));

    expect(jobs).toHaveLength(2);
    expect(jobs.find((j) => j.id === "a")?.status).toBe("failed");
    expect(jobs.find((j) => j.id === "b")?.status).toBe("running");
  });
});

describe("isTerminal", () => {
  it("counts every non-running state as terminal", () => {
    expect(isTerminal("running")).toBe(false);
    expect(isTerminal("succeeded")).toBe(true);
    expect(isTerminal("failed")).toBe(true);
    expect(isTerminal("cancelled")).toBe(true);
  });
});

describe("describeFinishedJob", () => {
  it("prefers the backend's summary for a success", () => {
    expect(
      describeFinishedJob(job({ status: "succeeded", message: "Indexed 12 chunks" }))
    ).toBe("Indexed 12 chunks");
  });

  it("falls back to the title when there is no summary", () => {
    expect(describeFinishedJob(job({ status: "succeeded", title: "Indexing" }))).toBe(
      "Indexing finished"
    );
  });

  it("always surfaces the reason a job failed", () => {
    const text = describeFinishedJob(job({ status: "failed", error: "no embedder" }));
    expect(text).toContain("no embedder");
  });

  it("still reports a failure that carries no error text", () => {
    expect(describeFinishedJob(job({ status: "failed", title: "Indexing" }))).toBe(
      "Indexing failed"
    );
  });

  it("stays silent about a cancellation the user asked for", () => {
    expect(describeFinishedJob(job({ status: "cancelled" }))).toBeNull();
  });

  it("says nothing about work still in progress", () => {
    expect(describeFinishedJob(job({ status: "running" }))).toBeNull();
  });
});

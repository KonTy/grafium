import { describe, it, expect, afterEach, beforeEach, vi } from "vitest";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import {
  jobs,
  jobsSeen,
  applyJobUpdate,
  describeFinishedJob,
  groupJobs,
  isTerminal,
  jobIndicator,
  latestFinish,
  markJobsSeen,
  removeJob,
  unseenResults,
  type Job,
} from "./jobs.svelte";
import { toasts } from "./toast.svelte";

const saved = new Map<string, string>();

function stubStorage() {
  saved.clear();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => saved.get(key) ?? null,
    setItem: (key: string, value: string) => { saved.set(key, value); },
  });
}

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
    details: null,
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

describe("removeJob", () => {
  beforeEach(() => {
    jobs.length = 0;
  });

  it("drops a discarded automatic run and leaves the rest", () => {
    applyJobUpdate(job({ id: "kept", status: "succeeded" }));
    applyJobUpdate(job({ id: "noop", status: "running" }));
    removeJob("noop");
    removeJob("unknown");
    expect(jobs.map((j) => j.id)).toEqual(["kept"]);
  });
});

describe("what the title-bar bell reports", () => {
  beforeEach(() => {
    jobsSeen.until = 0;
    stubStorage();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  const finished = [
    job({ id: "done", status: "succeeded", finished_at: 20 }),
    job({ id: "broken", status: "failed", finished_at: 30 }),
    job({ id: "stopped", status: "cancelled", finished_at: 40 }),
  ];

  it("counts running jobs first and still names unseen results", () => {
    expect(jobIndicator([job({ id: "busy" }), ...finished], 0)).toEqual({
      count: 1,
      summary: "1 running, 2 new results, 1 failed",
      running: true,
    });
  });

  it("counts completions and failures the Jobs page has not shown yet", () => {
    expect(jobIndicator(finished, 0)).toEqual({ count: 2, summary: "2 new results, 1 failed", running: false });
    expect(jobIndicator(finished, 20)).toEqual({ count: 1, summary: "1 new result, 1 failed", running: false });
    expect(jobIndicator(finished.slice(0, 1), 0)).toEqual({ count: 1, summary: "1 new result", running: false });
  });

  it("shows no number for history that has been seen", () => {
    expect(jobIndicator(finished, 40)).toEqual({ count: 0, summary: "", running: false });
  });

  it("never announces cancellations, including jobs stopped when Grafium closed", () => {
    const stopped = [
      job({ id: "asked", status: "cancelled", finished_at: 50 }),
      job({ id: "closed", status: "cancelled", finished_at: null }),
    ];
    expect(unseenResults(stopped, 0)).toEqual([]);
  });

  it("remembers the newest result shown, on this device, and never goes back", () => {
    markJobsSeen(latestFinish(finished));
    expect(jobsSeen.until).toBe(40);
    expect(saved.get("grafium.jobs.seenUntil")).toBe("40");
    markJobsSeen(latestFinish([]));
    markJobsSeen(25);
    markJobsSeen(Number.NaN);
    expect(jobsSeen.until).toBe(40);
    expect(saved.get("grafium.jobs.seenUntil")).toBe("40");
  });

  it("restores what was seen after a restart and ignores a damaged value", async () => {
    saved.set("grafium.jobs.seenUntil", "123");
    vi.resetModules();
    expect((await import("./jobs.svelte")).jobsSeen.until).toBe(123);
    saved.set("grafium.jobs.seenUntil", "not a time");
    vi.resetModules();
    expect((await import("./jobs.svelte")).jobsSeen.until).toBe(0);
  });
});

describe("groupJobs", () => {
  const ids = (list: Job[]) => list.map((j) => j.id);

  it("separates the states and lists each one newest first", () => {
    const groups = groupJobs([
      job({ id: "old-done", status: "succeeded", started_at: 1, finished_at: 10 }),
      job({ id: "busy", status: "running", started_at: 5 }),
      job({ id: "new-done", status: "succeeded", started_at: 2, finished_at: 30 }),
      job({ id: "broken", status: "failed", started_at: 3, finished_at: 20 }),
      job({ id: "earlier-busy", status: "running", started_at: 4 }),
      job({ id: "closed", status: "cancelled", started_at: 6, finished_at: null }),
    ]);
    expect(ids(groups.running)).toEqual(["busy", "earlier-busy"]);
    expect(ids(groups.failed)).toEqual(["broken"]);
    expect(ids(groups.completed)).toEqual(["new-done", "old-done"]);
    expect(ids(groups.cancelled)).toEqual(["closed"]);
  });

  it("puts the later-added job first when two finished at the same time", () => {
    const groups = groupJobs([
      job({ id: "first", status: "succeeded", finished_at: 9 }),
      job({ id: "second", status: "succeeded", finished_at: 9 }),
    ]);
    expect(ids(groups.completed)).toEqual(["second", "first"]);
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

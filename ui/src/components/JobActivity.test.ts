import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import JobActivity from "./JobActivity.svelte";
import { applyJobUpdate, jobs, jobsSeen, markJobsSeen, type Job } from "../lib/jobs.svelte";

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
    kind: "library_index",
    title: "Indexing the Library",
    status: "running",
    progress: null,
    message: null,
    link: null,
    error: null,
    details: null,
    cancellable: false,
    started_at: 1,
    finished_at: null,
    ...overrides,
  };
}

let host: HTMLDivElement;
let component: ReturnType<typeof mount>;
const onOpen = vi.fn();
const bell = () => host.querySelector<HTMLButtonElement>("button.job-toggle");
const badge = () => host.querySelector(".job-badge")?.textContent ?? null;

beforeEach(() => {
  jobs.splice(0, jobs.length);
  jobsSeen.until = 0;
  stubStorage();
  onOpen.mockReset();
  host = document.createElement("div");
  document.body.append(host);
  component = mount(JobActivity, { target: host, props: { toolbar: true, onOpen } });
  flushSync();
});

afterEach(async () => {
  vi.unstubAllGlobals();
  await unmount(component);
  host.remove();
});

describe("title-bar jobs bell", () => {
  it("appears once there is a job to show", () => {
    expect(bell()).toBeNull();
    applyJobUpdate(job());
    flushSync();
    expect(bell()).not.toBeNull();
  });

  it("opens the Jobs page from anywhere on the bell instead of a pop-up", () => {
    applyJobUpdate(job({ status: "succeeded", finished_at: 10 }));
    flushSync();
    const button = bell()!;
    expect(button.getAttribute("data-tauri-drag-region")).toBe("false");
    expect(button.hasAttribute("aria-haspopup")).toBe(false);
    for (const target of [button, button.querySelector("svg path")!, host.querySelector(".job-badge")!]) {
      target.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    }
    flushSync();
    expect(onOpen).toHaveBeenCalledTimes(3);
    expect(host.querySelector('[role="dialog"]')).toBeNull();
  });

  it("counts running jobs, then new results, and clears once Jobs has shown them", () => {
    applyJobUpdate(job({ id: "a" }));
    flushSync();
    expect(badge()).toBe("1");
    expect(bell()!.getAttribute("aria-label")).toBe("Jobs: 1 running");
    expect(bell()!.classList.contains("running")).toBe(true);

    applyJobUpdate(job({ id: "a", status: "failed", finished_at: 20, error: "Synthetic failure" }));
    applyJobUpdate(job({ id: "b", status: "succeeded", finished_at: 30 }));
    flushSync();
    expect(badge()).toBe("2");
    expect(bell()!.getAttribute("aria-label")).toBe("Jobs: 2 new results, 1 failed");
    expect(bell()!.title).toBe("Jobs: 2 new results, 1 failed");
    expect(bell()!.classList.contains("running")).toBe(false);

    markJobsSeen(30);
    flushSync();
    expect(badge()).toBeNull();
    expect(bell()!.getAttribute("aria-label")).toBe("Jobs");
  });

  it("caps a long run of unseen results", () => {
    for (let i = 1; i <= 100; i++) applyJobUpdate(job({ id: `done-${i}`, status: "succeeded", finished_at: i }));
    flushSync();
    expect(badge()).toBe("99+");
    expect(bell()!.getAttribute("aria-label")).toBe("Jobs: 100 new results");
  });
});

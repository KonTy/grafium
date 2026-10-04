import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import JobsView from "./JobsView.svelte";
import { applyJobUpdate, jobs, jobsSeen, type Job } from "../lib/jobs.svelte";

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
    title: "Synthetic job",
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
let component: ReturnType<typeof mount> | null;

function open() {
  component = mount(JobsView, { target: host, props: {} });
  flushSync();
}

const text = (elements: Iterable<Element>) => [...elements].map((element) => element.textContent?.trim());
const section = (name: string) => host.querySelector(`section[aria-labelledby="jobs-${name}"]`);
const titles = (name: string) => text(section(name)?.querySelectorAll("h3") ?? []);
const newTitles = () =>
  text([...host.querySelectorAll(".job-card")].filter((card) => card.querySelector(".new-label")).map((card) => card.querySelector("h3")!));
const summary = () =>
  Object.fromEntries(
    [...host.querySelectorAll(".summary-card")].map((card) => [
      card.querySelector(".summary-label")?.textContent?.trim(),
      card.querySelector(".summary-count")?.textContent?.trim(),
    ])
  );

beforeEach(() => {
  jobs.splice(0, jobs.length);
  jobsSeen.until = 0;
  stubStorage();
  host = document.createElement("div");
  document.body.append(host);
  component = null;
});

afterEach(async () => {
  vi.unstubAllGlobals();
  if (component) await unmount(component);
  host.remove();
});

describe("Jobs page", () => {
  it("lists completed jobs apart from running, failed and cancelled ones, newest first", () => {
    for (const update of [
      job({ id: "earlier", title: "Earlier import", status: "succeeded", finished_at: 10 }),
      job({ id: "busy", title: "Library indexing", status: "running", started_at: 40 }),
      job({ id: "latest", title: "Latest import", status: "succeeded", finished_at: 30 }),
      job({ id: "broken", title: "Broken import", status: "failed", finished_at: 20, error: "Synthetic failure" }),
      job({ id: "closed", title: "Stopped transcription", status: "cancelled", message: "Stopped when Grafium closed" }),
    ]) {
      applyJobUpdate(update);
    }
    open();

    expect(text(host.querySelectorAll(".job-section > h2"))).toEqual(["Running", "Failed", "Completed", "Cancelled"]);
    expect(titles("running")).toEqual(["Library indexing"]);
    expect(titles("failed")).toEqual(["Broken import"]);
    expect(titles("completed")).toEqual(["Latest import", "Earlier import"]);
    expect(titles("cancelled")).toEqual(["Stopped transcription"]);
    expect(text(section("completed")!.querySelectorAll(".status"))).toEqual(["Completed", "Completed"]);
    expect(summary()).toEqual({ Total: "5", Running: "1", Completed: "2", Failed: "1" });
    expect(section("completed")!.querySelector(".job-head p")?.textContent).toMatch(/^Started .+ · Finished .+$/);
  });

  it("marks the results the bell announced as new, and records that they were seen", () => {
    applyJobUpdate(job({ id: "seen", title: "Seen import", status: "succeeded", finished_at: 10 }));
    applyJobUpdate(job({ id: "fresh", title: "Fresh import", status: "succeeded", finished_at: 30 }));
    applyJobUpdate(job({ id: "broken", title: "Broken import", status: "failed", finished_at: 20 }));
    jobsSeen.until = 15;
    open();

    expect(newTitles()).toEqual(["Broken import", "Fresh import"]);
    expect(jobsSeen.until).toBe(30);
    expect(saved.get("grafium.jobs.seenUntil")).toBe("30");

    applyJobUpdate(job({ id: "later", title: "Later import", status: "succeeded", finished_at: 50 }));
    flushSync();
    expect(jobsSeen.until).toBe(50);
    expect(newTitles()).toEqual(["Broken import", "Later import", "Fresh import"]);
  });

  it("shows the empty state when there is no job history", () => {
    open();
    expect(host.querySelector(".empty-state h2")?.textContent).toBe("No jobs yet");
    expect(host.querySelector(".job-section")).toBeNull();
    expect(jobsSeen.until).toBe(0);
  });
});

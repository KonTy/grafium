import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import Statistics from "./Statistics.svelte";

const mocks = vi.hoisted(() => ({
  getCompletionCounts: vi.fn(),
  getCompletedTasks: vi.fn(),
  listOpenTaskRows: vi.fn(),
  taskFlowStats: vi.fn(),
  getNoteEditCounts: vi.fn(),
}));

vi.mock("../lib/api", () => ({
  ...mocks,
  cycleTaskState: vi.fn(),
  getBlock: vi.fn(),
  getNoteEditsForDay: vi.fn(),
  updateTaskState: vi.fn(),
}));

vi.mock("../lib/markdown", () => ({
  renderBlock: (content: string) => content,
}));

vi.mock("../lib/renderedMedia", () => ({
  hydrateRenderedMedia: () => ({}),
}));

vi.mock("../lib/undoStack", () => ({
  pushUndo: vi.fn(),
}));

let host: HTMLDivElement;
let component: Record<string, unknown> | null = null;

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
  mocks.listOpenTaskRows.mockResolvedValue([]);
  mocks.getCompletionCounts.mockResolvedValue([["2026-09-10", 3]]);
  mocks.getCompletedTasks.mockResolvedValue([]);
  mocks.taskFlowStats.mockResolvedValue({
    throughput_7d: 0,
    throughput_prev_7d: 0,
    weekly_completions: [],
    median_cycle_ms: null,
    median_wait_ms: null,
    on_time_rate: null,
    oldest_open_days: null,
    open_count: 0,
    done_count: 0,
    by_page: [],
  });
  mocks.getNoteEditCounts.mockResolvedValue([["2026-09-11", 2]]);
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  host.remove();
  vi.clearAllMocks();
});

async function showActivity() {
  component = mount(Statistics, { target: host, props: {} }) as Record<string, unknown>;
  flushSync();
  await vi.waitFor(() => {
    expect(Array.from(host.querySelectorAll("button")).some((button) => button.textContent?.includes("Show activity"))).toBe(true);
  });
  Array.from(host.querySelectorAll("button"))
    .find((button) => button.textContent?.includes("Show activity"))!
    .click();
  await vi.waitFor(() => {
    expect(host.querySelectorAll(".activity-heatmap-panel")).toHaveLength(2);
  });
  return Array.from(host.querySelectorAll<HTMLElement>(".activity-heatmap-panel"));
}

// jsdom has no layout, so the on-screen behaviour is covered by
// tests/heatmapLabels.ui.cjs; this pins the structure that behaviour relies on.
describe("Statistics activity heatmaps", () => {
  it("names each heatmap and keeps the colour key beside it", async () => {
    const [tasks, notes] = await showActivity();

    expect(tasks.querySelector("h2")?.textContent).toBe("Task completions");
    expect(notes.querySelector("h2")?.textContent).toBe("Note edits");
    for (const panel of [tasks, notes]) {
      const title = panel.querySelector("h2")!;
      expect(panel.getAttribute("role")).toBe("group");
      expect(panel.getAttribute("aria-labelledby")).toBe(title.id);
      expect(title.id).not.toBe("");
      expect(panel.querySelector(".heatmap-legend")?.textContent).toMatch(/Less[\s\S]*More/);
    }
    expect(tasks.querySelector("h2")!.id).not.toBe(notes.querySelector("h2")!.id);
  });

  it("scrolls only the grid so the title and key cannot be scrolled away", async () => {
    for (const panel of await showActivity()) {
      const scroller = panel.querySelector(".heatmap-scroll");
      expect(scroller).not.toBeNull();
      expect(scroller!.querySelector(".heatmap-grid")).not.toBeNull();
      expect(scroller!.querySelector(".month-labels")).not.toBeNull();
      expect(scroller!.querySelector("h2")).toBeNull();
      expect(scroller!.querySelector(".heatmap-title-row")).toBeNull();
      expect(scroller!.querySelector(".heatmap-legend")).toBeNull();
      expect(panel.querySelector(".heatmap-title-row")?.parentElement).toBe(panel);
      expect(panel.querySelector(".heatmap-legend")?.parentElement).toBe(panel);
    }
  });
});

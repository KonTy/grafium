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
  mocks.listOpenTaskRows.mockResolvedValue([{
    block_id: "open-1",
    content: "TODO Visible before history",
    page_title: "Plan",
    state: "TODO",
    priority: null,
    scheduled_date: null,
    scheduled_time: null,
    deadline_date: null,
    created_at: 1,
    updated_at: 1,
  }]);
  mocks.getCompletionCounts.mockResolvedValue([]);
  mocks.getCompletedTasks.mockResolvedValue([]);
  mocks.taskFlowStats.mockResolvedValue({
    throughput_7d: 0,
    throughput_prev_7d: 0,
    weekly_completions: [],
    median_cycle_ms: null,
    median_wait_ms: null,
    on_time_rate: null,
    oldest_open_days: null,
    open_count: 1,
    done_count: 0,
    by_page: [],
  });
  mocks.getNoteEditCounts.mockResolvedValue([]);
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  host.remove();
  vi.clearAllMocks();
});

describe("Statistics progressive loading", () => {
  it("renders open tasks before requesting historical analytics", async () => {
    component = mount(Statistics, { target: host, props: {} }) as Record<string, unknown>;
    flushSync();

    await vi.waitFor(() => {
      expect(host.textContent).toContain("Visible before history");
    });
    expect(host.textContent).not.toContain("Loading activity history...");
    expect(mocks.getCompletionCounts).not.toHaveBeenCalled();

    const showActivity = Array.from(host.querySelectorAll("button"))
      .find((button) => button.textContent?.includes("Show activity"));
    expect(showActivity).toBeDefined();
    showActivity!.click();
    await vi.waitFor(() => {
      expect(mocks.getCompletionCounts).toHaveBeenCalledWith(3650);
    });
    expect(mocks.getCompletedTasks).toHaveBeenCalledWith(3650);
    expect(mocks.taskFlowStats).toHaveBeenCalledWith(12);
    expect(mocks.getNoteEditCounts).toHaveBeenCalledWith(3650);
  });
});

import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import AssistantDiagnostics from "./AssistantDiagnostics.svelte";

const mocks = vi.hoisted(() => ({
  status: vi.fn(),
  index: vi.fn(),
  retry: vi.fn(),
  listen: vi.fn(),
}));

vi.mock("../lib/knowledge", () => ({
  aiIndexStatus: mocks.status,
  aiIndexAllPages: mocks.index,
  aiRetryLlmOnGpu: mocks.retry,
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));

let host: HTMLDivElement;
let component: ReturnType<typeof mount> | undefined;

beforeEach(() => {
  vi.clearAllMocks();
  mocks.listen.mockResolvedValue(() => {});
  mocks.status.mockResolvedValue({
    indexed_chunks: 0, total_blocks: 0, pending_pages: 0,
    embedder_ready: false, llm_ready: true, accelerator: null,
    runtime_warnings: ["GPU headroom cannot be measured; using CPU."],
  });
  host = document.createElement("div");
  document.body.appendChild(host);
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  host.remove();
});

describe("native runtime diagnostics", () => {
  it("shows safety warnings without opening the diagnostics disclosure", async () => {
    component = mount(AssistantDiagnostics, { target: host });
    flushSync();
    await vi.waitFor(() => expect(host.querySelector('[role="status"]')?.textContent).toContain("using CPU"));
    expect(host.querySelector('[role="status"]')?.closest("details")).toBeNull();
  });

  it("does not fetch status while an answer is running", () => {
    component = mount(AssistantDiagnostics, { target: host, props: { running: true } });
    flushSync();
    expect(mocks.status).not.toHaveBeenCalled();
  });

  it("renders warning text as text rather than markup", async () => {
    mocks.status.mockResolvedValue({
      indexed_chunks: 0, total_blocks: 0, pending_pages: 0,
      embedder_ready: false, llm_ready: true, accelerator: null,
      runtime_warnings: ["<img src=x onerror=alert(1)>"],
    });
    component = mount(AssistantDiagnostics, { target: host });
    flushSync();
    await vi.waitFor(() => expect(host.textContent).toContain("<img"));
    expect(host.querySelector("img")).toBeNull();
  });
});

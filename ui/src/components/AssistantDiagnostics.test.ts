import { flushSync, mount, unmount } from "svelte";
import { SvelteMap } from "svelte/reactivity";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import AssistantDiagnostics from "./AssistantDiagnostics.svelte";
import type { IndexStatus } from "../lib/knowledge";

const mocks = vi.hoisted(() => ({
  status: vi.fn(),
  index: vi.fn(),
  retry: vi.fn(),
  retryModel: vi.fn(),
  useCpu: vi.fn(),
  listen: vi.fn(),
}));

vi.mock("../lib/knowledge", () => ({
  aiIndexStatus: mocks.status,
  aiIndexAllPages: mocks.index,
  aiRetryLlmOnGpu: mocks.retry,
  aiAllowGpuRetry: mocks.retryModel,
  aiUseCpuForModel: mocks.useCpu,
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));

let host: HTMLDivElement;
let component: ReturnType<typeof mount> | undefined;
let extraComponent: ReturnType<typeof mount> | undefined;
const ready: IndexStatus = {
  indexed_chunks: 12, total_blocks: 20, pending_pages: 2,
  embedder_ready: true, llm_ready: true, accelerator: null,
  runtime_warnings: [],
};
const cpu: IndexStatus = {
  ...ready,
  accelerator: {
    gpu_supported: true, on_gpu: false, gpu_layers: 0,
    free_vram_mib_at_load: null, model_mib: 100, explicit: false,
  },
};
const trigger = () => host.querySelector<HTMLButtonElement>(".model-status-trigger")!;
const menu = () => host.querySelector<HTMLDivElement>('[popover="auto"]')!;
const button = (text: string) => [...menu().querySelectorAll("button")].find(item => item.textContent?.trim() === text)!;
const changed = async () => { await Promise.resolve(); flushSync(); };
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function toggle(element: HTMLElement, open: boolean) {
  const event = new Event("beforetoggle");
  Object.defineProperty(event, "newState", { value: open ? "open" : "closed" });
  element.dispatchEvent(event);
  element.dataset.testPopoverOpen = String(open);
}

beforeEach(() => {
  vi.resetAllMocks();
  mocks.listen.mockResolvedValue(() => {});
  mocks.status.mockResolvedValue(ready);
  mocks.index.mockResolvedValue({ pages_processed: 4, pages_failed: 0 });
  mocks.retry.mockResolvedValue(null);
  vi.stubGlobal("ResizeObserver", class {
    observe() {}
    disconnect() {}
  });
  Object.defineProperty(HTMLElement.prototype, "showPopover", {
    configurable: true, value: function () { toggle(this, true); },
  });
  Object.defineProperty(HTMLElement.prototype, "hidePopover", {
    configurable: true, value: function () { toggle(this, false); },
  });
  host = document.createElement("div");
  document.body.appendChild(host);
});

afterEach(async () => {
  if (component) await unmount(component);
  if (extraComponent) await unmount(extraComponent);
  component = undefined;
  extraComponent = undefined;
  host.remove();
  delete (HTMLElement.prototype as Partial<HTMLElement>).showPopover;
  delete (HTMLElement.prototype as Partial<HTMLElement>).hidePopover;
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("native runtime diagnostics", () => {
  it("reports safety warnings to the composer while keeping the full text in the menu", async () => {
    const onNotice = vi.fn();
    mocks.status.mockResolvedValue({ ...ready, runtime_warnings: ["Unexpected native warning."] });
    component = mount(AssistantDiagnostics, { target: host, props: { onNotice } });
    flushSync();
    await vi.waitFor(() => expect(trigger().dataset.status).toBe("warning"));
    expect(trigger().getAttribute("aria-expanded")).toBe("false");
    expect(onNotice).toHaveBeenLastCalledWith({
      text: "The model has a warning. Open status for details.", error: false,
    });
    trigger().click();
    flushSync();
    expect(menu().dataset.testPopoverOpen).toBe("true");
    expect(menu().textContent).toContain("Unexpected native warning.");
    expect(menu().querySelector(".runtime-details")?.hasAttribute("open")).toBe(false);
  });

  it("does not fetch status while an answer is running", () => {
    component = mount(AssistantDiagnostics, { target: host, props: { running: true } });
    flushSync();
    expect(mocks.status).not.toHaveBeenCalled();
  });

  it("renders warning text as text rather than markup", async () => {
    mocks.status.mockResolvedValue({ ...ready, runtime_warnings: ["<img src=x onerror=alert(1)>"] });
    component = mount(AssistantDiagnostics, { target: host });
    flushSync();
    await vi.waitFor(() => expect(host.textContent).toContain("<img"));
    expect(host.querySelector("img")).toBeNull();
  });

  it("provides distinct labelled checking, ready, working, fallback and error icons", async () => {
    const result = deferred<IndexStatus>();
    mocks.status.mockReturnValueOnce(result.promise);
    const props = new SvelteMap([["checking", false], ["running", false]]);
    component = mount(AssistantDiagnostics, { target: host, props: {
      get checking() { return props.get("checking"); },
      get running() { return props.get("running"); },
    } });
    flushSync();
    const icons = new Set<string>();
    function assertState(state: string, label: string) {
      expect(trigger().dataset.status).toBe(state);
      expect(trigger().getAttribute("aria-label")).toContain(label);
      const svg = trigger().querySelector("svg")!;
      expect(svg.dataset.statusIcon).toBe(state);
      expect(svg.getAttribute("aria-hidden")).toBe("true");
      icons.add(svg.innerHTML);
    }
    assertState("checking", "Checking");
    result.resolve(ready);
    await vi.waitFor(() => expect(trigger().dataset.status).toBe("ready"));
    assertState("ready", "Model ready");
    props.set("running", true); flushSync();
    assertState("working", "Model working");
    mocks.status.mockResolvedValueOnce(cpu);
    props.set("running", false); flushSync();
    await vi.waitFor(() => expect(trigger().dataset.status).toBe("fallback"));
    assertState("fallback", "Slower mode");
    mocks.retry.mockRejectedValueOnce(new Error("memory refused"));
    trigger().click(); flushSync();
    button("Try faster mode").click();
    await vi.waitFor(() => expect(trigger().dataset.status).toBe("error"));
    assertState("error", "needs help");
    expect(icons.size).toBe(5);
  });

  it("opens an accessible top-layer popup, closes with Escape, and restores trigger focus", async () => {
    component = mount(AssistantDiagnostics, { target: host });
    flushSync(); await changed();
    trigger().focus();
    trigger().click(); flushSync();
    expect(menu().getAttribute("role")).toBe("dialog");
    expect(menu().id).toBe(trigger().getAttribute("aria-controls"));
    expect(menu().getAttribute("aria-labelledby")).toBe(host.querySelector("h2")!.id);
    expect(trigger().getAttribute("aria-haspopup")).toBe("dialog");
    expect(trigger().getAttribute("aria-expanded")).toBe("true");
    expect(document.activeElement).toBe(button("Close"));
    button("Close").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    flushSync();
    expect(trigger().getAttribute("aria-expanded")).toBe("false");
    expect(menu().dataset.testPopoverOpen).toBe("false");
    expect(document.activeElement).toBe(trigger());
  });

  it("synchronizes native light dismissal and allows a notice to open the popup", async () => {
    component = mount(AssistantDiagnostics, { target: host });
    flushSync(); await changed();
    (component as { openMenu: () => void }).openMenu(); flushSync();
    expect(trigger().getAttribute("aria-expanded")).toBe("true");
    toggle(menu(), false); flushSync();
    expect(trigger().getAttribute("aria-expanded")).toBe("false");
    trigger().click(); flushSync();
    expect(trigger().getAttribute("aria-expanded")).toBe("true");
  });

  it("dismisses for F1 without blocking contextual help handling", async () => {
    component = mount(AssistantDiagnostics, { target: host });
    flushSync(); await changed();
    trigger().click(); flushSync();
    expect(trigger().dataset.helpContext).toBe("chat");
    expect(menu().dataset.helpContext).toBe("chat");
    const help = vi.fn();
    host.addEventListener("keydown", help);
    const event = new KeyboardEvent("keydown", { key: "F1", bubbles: true, cancelable: true });
    button("Close").dispatchEvent(event); flushSync();
    expect(event.defaultPrevented).toBe(false);
    expect(help).toHaveBeenCalledOnce();
    expect(trigger().getAttribute("aria-expanded")).toBe("false");
    expect(menu().dataset.testPopoverOpen).toBe("false");
    expect(document.activeElement).toBe(trigger());
  });

  it("closes for F1 even when the app capture handler has already stopped propagation", async () => {
    const appHelp = vi.fn((event: KeyboardEvent) => {
      if (event.key === "F1") {
        event.preventDefault();
        event.stopPropagation();
      }
    });
    window.addEventListener("keydown", appHelp, true);
    try {
      component = mount(AssistantDiagnostics, { target: host });
      flushSync(); await changed();
      trigger().click(); flushSync();
      button("Close").dispatchEvent(new KeyboardEvent("keydown", { key: "F1", bubbles: true, cancelable: true }));
      flushSync();
      expect(appHelp).toHaveBeenCalledOnce();
      expect(menu().dataset.testPopoverOpen).toBe("false");
      expect(trigger().getAttribute("aria-expanded")).toBe("false");
    } finally {
      window.removeEventListener("keydown", appHelp, true);
    }
  });

  it("bounds popup placement to the visual viewport on a narrow screen", async () => {
    vi.stubGlobal("visualViewport", {
      width: 240, height: 300, offsetLeft: 0, offsetTop: 0,
      addEventListener: vi.fn(), removeEventListener: vi.fn(),
    });
    component = mount(AssistantDiagnostics, { target: host });
    flushSync(); await changed();
    vi.spyOn(trigger(), "getBoundingClientRect").mockReturnValue({
      x: 200, y: 260, left: 200, top: 260, right: 232, bottom: 292, width: 32, height: 32,
      toJSON() {},
    });
    vi.spyOn(menu(), "getBoundingClientRect").mockReturnValue({
      x: 0, y: 0, left: 0, top: 0, right: 224, bottom: 284, width: 224, height: 284,
      toJSON() {},
    });
    trigger().click(); flushSync();
    expect(menu().style.maxWidth).toBe("224px");
    expect(menu().style.maxHeight).toBe("284px");
    expect(menu().style.left).toBe("8px");
    expect(menu().style.top).toBe("8px");
  });

  it("uses unique IDs for two placements and never submits the composer form", async () => {
    const form = document.createElement("form");
    host.appendChild(form);
    const submit = vi.fn((event: Event) => event.preventDefault());
    form.addEventListener("submit", submit);
    component = mount(AssistantDiagnostics, { target: form });
    extraComponent = mount(AssistantDiagnostics, { target: form });
    flushSync(); await changed();
    const triggers = [...host.querySelectorAll<HTMLButtonElement>(".model-status-trigger")];
    expect(triggers[0].getAttribute("aria-controls")).not.toBe(triggers[1].getAttribute("aria-controls"));
    expect([...host.querySelectorAll("button")].every(item => item.type === "button")).toBe(true);
    triggers[0].click(); flushSync();
    button("Close").click();
    expect(submit).not.toHaveBeenCalled();
  });

  it("keeps provider details and model/web privacy inside the popup", async () => {
    component = mount(AssistantDiagnostics, { target: host, props: {
      provider: { label: "Self-hosted", detail: "Configured endpoint" },
      connected: true, noNotesExcludesHistory: true,
    } });
    flushSync(); await changed();
    trigger().click(); flushSync();
    expect(menu().textContent).toContain("Self-hosted");
    expect(menu().textContent).toContain("Configured endpoint");
    const privacy = menu().querySelector<HTMLDetailsElement>(".privacy-note")!;
    expect(privacy.querySelector("summary")?.textContent).toBe("Model & web privacy");
    privacy.open = true;
    expect(privacy.textContent).toContain("No notes excludes earlier note-backed answers");
    expect(privacy.textContent).toContain("Prompts and selected excerpts go to the configured model");
    expect(privacy.textContent).toContain("forwards results to your model server");
    expect(privacy.textContent).toContain("Endpoint location alone does not guarantee privacy");
    expect(privacy.textContent).toContain("never switches models automatically");
  });

  it("reports disconnected providers only after checking finishes and opens Settings explicitly", async () => {
    const values = new SvelteMap([["checking", true], ["connected", false]]);
    const onNotice = vi.fn();
    const onOpenSettings = vi.fn();
    component = mount(AssistantDiagnostics, { target: host, props: {
      get checking() { return values.get("checking"); },
      get connected() { return values.get("connected"); },
      onNotice, onOpenSettings,
    } });
    flushSync(); await changed();
    expect(trigger().dataset.status).toBe("checking");
    expect(onNotice).toHaveBeenLastCalledWith(null);
    values.set("checking", false); flushSync();
    expect(trigger().getAttribute("aria-label")).toContain("Choose a model");
    expect(onNotice).toHaveBeenLastCalledWith({
      text: "Choose a model to start chatting.", error: false,
    });
    trigger().click(); flushSync();
    expect(menu().textContent).not.toContain("No notes excludes earlier note-backed answers");
    button("Choose a model").click();
    expect(onOpenSettings).toHaveBeenCalledOnce();
    values.set("connected", true); flushSync();
    expect(onNotice).toHaveBeenLastCalledWith(null);
  });

  it("indexes only on demand, reports partial failures, and retries failed indexing", async () => {
    const onNotice = vi.fn();
    const result = deferred<{ pages_processed: number; pages_failed: number }>();
    mocks.index.mockReturnValueOnce(result.promise);
    component = mount(AssistantDiagnostics, { target: host, props: { onNotice } });
    flushSync(); await changed();
    trigger().click(); flushSync();
    expect(mocks.index).not.toHaveBeenCalled();
    button("Index now").click(); flushSync();
    expect(button("Indexing…").disabled).toBe(true);
    expect(trigger().dataset.status).toBe("working");
    result.resolve({ pages_processed: 3, pages_failed: 1 });
    await vi.waitFor(() => expect(button("Index now")).toBeDefined());
    expect(menu().textContent).toContain("Indexed 3 pages; 1 failed.");
    expect(onNotice).toHaveBeenLastCalledWith(expect.objectContaining({ error: false }));
    expect(trigger().dataset.status).toBe("warning");
    button("Index now").click();
    await vi.waitFor(() => expect(trigger().dataset.status).toBe("ready"));
    expect(onNotice).toHaveBeenLastCalledWith(null);
  });

  it("offers per-model retry without an approval detour and preserves embedding readiness", async () => {
    const onOpenSettings = vi.fn();
    const onNotice = vi.fn();
    mocks.status.mockResolvedValue({ ...cpu, embedder_ready: false, runtime_recovery: [
      { key: "private-native-key", label: "chat: model.gguf", reason: "Previous worker crashed", state: "cpu_only" },
    ] });
    component = mount(AssistantDiagnostics, { target: host, props: { onOpenSettings, onNotice } });
    flushSync(); await changed();
    trigger().click(); flushSync();
    expect(button("Index now").disabled).toBe(true);
    expect(button("Try faster mode").disabled).toBe(false);
    button("Try faster mode").click(); flushSync();
    expect(button("Try faster mode").disabled).toBe(true);
    await vi.waitFor(() => expect(mocks.retryModel).toHaveBeenCalledWith("private-native-key"));
    expect(mocks.retry).not.toHaveBeenCalled();
    expect(menu().textContent).toContain("Previous worker crashed");
    expect(menu().textContent).not.toContain("private-native-key");
    await changed();
    expect(onNotice).toHaveBeenLastCalledWith(null);
    button("Change model").click(); flushSync();
    expect(onOpenSettings).toHaveBeenCalledOnce();
    expect(trigger().getAttribute("aria-expanded")).toBe("false");
  });

  it("disables GPU retry until running or cancelling finishes and retains failed retries", async () => {
    const running = new SvelteMap([["value", false]]);
    const onNotice = vi.fn();
    mocks.status.mockResolvedValue(cpu);
    component = mount(AssistantDiagnostics, { target: host, props: {
      get running() { return running.get("value"); }, onNotice,
    } });
    flushSync(); await changed();
    trigger().click(); flushSync();
    running.set("value", true); flushSync();
    expect(button("Try faster mode").disabled).toBe(true);
    expect(menu().textContent).toContain("when the current request finishes");
    button("Try faster mode").click();
    expect(mocks.retry).not.toHaveBeenCalled();
    running.set("value", false); flushSync(); await changed();
    const retry = deferred<null>();
    mocks.retry.mockReturnValueOnce(retry.promise);
    button("Try faster mode").click(); flushSync();
    expect(button("Checking…").disabled).toBe(true);
    retry.reject(new Error("GPU admission refused"));
    await vi.waitFor(() => expect(menu().textContent).toContain("GPU admission refused"));
    expect(onNotice).toHaveBeenLastCalledWith(expect.objectContaining({ error: true }));
    expect(button("Try faster mode").disabled).toBe(false);
    const updated = mocks.listen.mock.calls.find(([name]) => name === "ai-index-updated")![1];
    updated(); await changed();
    expect(menu().textContent).toContain("GPU admission refused");
    button("Try faster mode").click();
    await vi.waitFor(() => expect(button("Try faster mode")).toBeDefined());
    expect(menu().textContent).not.toContain("GPU admission refused");
  });

  it("shows status failures outside the menu via callback and recovers on Retry status", async () => {
    const onNotice = vi.fn();
    mocks.status.mockRejectedValueOnce(new Error("native status unavailable"));
    component = mount(AssistantDiagnostics, { target: host, props: { onNotice } });
    flushSync(); await changed();
    await vi.waitFor(() => expect(trigger().dataset.status).toBe("error"));
    expect(onNotice).toHaveBeenLastCalledWith(expect.objectContaining({ error: true }));
    trigger().click(); flushSync();
    expect(menu().querySelector('[role="alert"]')?.textContent).toBe("Couldn't check the model. Try again.");
    expect(menu().querySelector(".runtime-details")?.textContent).toContain("native status unavailable");
    button("Retry status").click(); await changed();
    await vi.waitFor(() => expect(trigger().dataset.status).toBe("ready"));
    expect(onNotice).toHaveBeenLastCalledWith(null);
  });

  it("keeps handled fallback and limited memory protection out of the composer alerts", async () => {
    const onNotice = vi.fn();
    mocks.status.mockResolvedValue({ ...cpu, runtime_warnings: [
      "GPU headroom cannot be measured; using CPU.",
      "Native worker hard RAM containment unavailable: cgroup is not delegated",
    ] });
    component = mount(AssistantDiagnostics, { target: host, props: { onNotice } });
    flushSync(); await changed();
    await vi.waitFor(() => expect(trigger().dataset.status).toBe("fallback"));
    expect(onNotice).toHaveBeenLastCalledWith(null);
    expect(host.textContent).toContain("Memory protection is limited on this computer.");
    expect(host.querySelector(".runtime-details")?.hasAttribute("open")).toBe(false);
  });

  it("shows automatic recovery accurately without authorizing anything from status polling", async () => {
    mocks.status.mockResolvedValue({ ...cpu, runtime_recovery: [
      { key: "chat-key", label: "chat: model.gguf", reason: "previous exit", state: "retry_pending" },
    ] });
    component = mount(AssistantDiagnostics, { target: host });
    flushSync(); await changed();
    await vi.waitFor(() => expect(trigger().getAttribute("aria-label")).toContain("Automatic recovery is ready"));
    expect(host.textContent).toContain("once on your next request");
    expect(mocks.retry).not.toHaveBeenCalled();
    expect(mocks.retryModel).not.toHaveBeenCalled();
    trigger().click(); flushSync();
    button("Keep slower mode").click();
    await vi.waitFor(() => expect(mocks.useCpu).toHaveBeenCalledWith("chat-key"));
  });

  it("does not request a second retry while the native recovery attempt is active", async () => {
    const onNotice = vi.fn();
    mocks.status.mockResolvedValue({ ...cpu, runtime_recovery: [
      { key: "chat-key", label: "chat: model.gguf", reason: "previous exit", state: "retrying" },
    ] });
    component = mount(AssistantDiagnostics, { target: host, props: { onNotice } });
    flushSync(); await changed();
    await vi.waitFor(() => expect(trigger().dataset.status).toBe("working"));
    expect(host.textContent).toContain("No action needed");
    expect(button("Try faster mode")).toBeUndefined();
    expect(onNotice).toHaveBeenLastCalledWith(null);
  });

  it("refreshes active idle status on native events and after a request ends", async () => {
    const values = new SvelteMap([["active", true], ["running", false]]);
    component = mount(AssistantDiagnostics, { target: host, props: {
      get active() { return values.get("active"); },
      get running() { return values.get("running"); },
    } });
    flushSync(); await changed();
    const updated = mocks.listen.mock.calls[0][1];
    updated(); await changed();
    expect(mocks.status).toHaveBeenCalledTimes(2);
    values.set("running", true); flushSync();
    updated(); await changed();
    expect(mocks.status).toHaveBeenCalledTimes(2);
    values.set("running", false); flushSync(); await changed();
    expect(mocks.status).toHaveBeenCalledTimes(3);
    values.set("active", false); flushSync();
    updated(); await changed();
    expect(mocks.status).toHaveBeenCalledTimes(3);
  });

  it("closes when inactive and ignores prior lifecycle index results after reactivation", async () => {
    const active = new SvelteMap([["value", true]]);
    const result = deferred<{ pages_processed: number; pages_failed: number }>();
    mocks.index.mockReturnValueOnce(result.promise);
    component = mount(AssistantDiagnostics, { target: host, props: {
      get active() { return active.get("value"); },
    } });
    flushSync(); await changed();
    trigger().click(); flushSync();
    button("Index now").click(); flushSync();
    active.set("value", false); flushSync();
    expect(trigger().getAttribute("aria-expanded")).toBe("false");
    expect(menu().dataset.testPopoverOpen).toBe("false");
    active.set("value", true); flushSync(); await changed();
    const requests = mocks.status.mock.calls.length;
    result.resolve({ pages_processed: 999, pages_failed: 4 });
    await changed();
    expect(menu().textContent).not.toContain("999");
    expect(mocks.status).toHaveBeenCalledTimes(requests);
    expect(trigger().dataset.status).toBe("ready");
  });

  it("ignores older refreshes, inactive responses and responses after unmount", async () => {
    const first = deferred<IndexStatus>();
    const second = deferred<IndexStatus>();
    const third = deferred<IndexStatus>();
    const onNotice = vi.fn();
    mocks.status.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise).mockReturnValueOnce(third.promise);
    const active = new SvelteMap([["value", true]]);
    component = mount(AssistantDiagnostics, { target: host, props: {
      get active() { return active.get("value"); }, onNotice,
    } });
    flushSync();
    const updated = mocks.listen.mock.calls[0][1];
    updated();
    second.resolve(ready); await changed();
    first.resolve(cpu); await changed();
    expect(trigger().dataset.status).toBe("ready");
    updated();
    active.set("value", false); flushSync();
    third.resolve(cpu); await changed();
    expect(onNotice).toHaveBeenLastCalledWith(null);
    active.set("value", true); flushSync(); await changed();
    const fourth = deferred<IndexStatus>();
    mocks.status.mockReturnValueOnce(fourth.promise);
    mocks.listen.mock.calls.at(-1)![1]();
    await unmount(component!); component = undefined;
    onNotice.mockClear();
    fourth.reject(new Error("late graph response")); await changed();
    expect(onNotice).not.toHaveBeenCalled();
  });

  it("logs subscription and asynchronous unsubscribe failures", async () => {
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    const subscription = deferred<() => void>();
    mocks.listen.mockReturnValueOnce(subscription.promise);
    component = mount(AssistantDiagnostics, { target: host });
    flushSync();
    await unmount(component!); component = undefined;
    subscription.resolve(() => { throw new Error("unsubscribe failure"); });
    await vi.waitFor(() => expect(log).toHaveBeenCalledWith(
      "Could not unsubscribe from model index updates", expect.any(Error),
    ));
    mocks.listen.mockRejectedValueOnce(new Error("subscribe failure"));
    component = mount(AssistantDiagnostics, { target: host });
    flushSync();
    await vi.waitFor(() => expect(log).toHaveBeenCalledWith(
      "Could not subscribe to model index updates", expect.any(Error),
    ));
  });
});

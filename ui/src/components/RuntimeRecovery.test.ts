import { mount, unmount, flushSync } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import RuntimeRecovery from "./RuntimeRecovery.svelte";
import { aiAllowGpuRetry, aiUseCpuForModel, type RuntimeRecovery as RecoveryRecord } from "../lib/knowledge";

const api = vi.hoisted(() => ({ invoke: vi.fn().mockResolvedValue(undefined) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: api.invoke }));

let component: ReturnType<typeof mount> | undefined;
let host: HTMLDivElement | undefined;
const dialogMethods = ["showModal", "close"] as const;
const descriptors = dialogMethods.map(method => Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, method));
const retryButton = (host: HTMLElement) => [...host.querySelectorAll("button")]
  .find(button => button.textContent === "Try faster mode")!;

beforeEach(() => {
  Object.defineProperties(HTMLDialogElement.prototype, {
    showModal: { configurable: true, value(this: HTMLDialogElement) { this.open = true; } },
    close: { configurable: true, value(this: HTMLDialogElement) { this.open = false; } },
  });
});

afterEach(async () => {
  if (component) await unmount(component);
  host?.remove();
  component = undefined;
  api.invoke.mockClear();
  dialogMethods.forEach((method, index) => {
    if (descriptors[index]) Object.defineProperty(HTMLDialogElement.prototype, method, descriptors[index]!);
    else Reflect.deleteProperty(HTMLDialogElement.prototype, method);
  });
});

function render(busy = false, state: RecoveryRecord["state"] = "cpu_only") {
  host = document.createElement("div");
  document.body.append(host);
  component = mount(RuntimeRecovery, {
    target: host,
    props: {
      records: [{ key: "selected-model", label: "chat: selected.gguf", reason: "Unconfirmed native exit", state }],
      busy,
      onRetry: aiAllowGpuRetry,
      onUseCpu: aiUseCpuForModel,
    },
  });
  flushSync();
  return host;
}

describe("automatic model recovery controls", () => {
  it("does not authorize GPU during rendering and only retries the clicked identity", async () => {
    const host = render();
    expect(api.invoke).not.toHaveBeenCalled();
    retryButton(host).click();
    await vi.waitFor(() => expect(api.invoke).toHaveBeenCalledWith("ai_allow_gpu_retry", { key: "selected-model" }));
    expect(api.invoke).toHaveBeenCalledTimes(1);
  });

  it("prevents repeated authorization while a retry is in progress", () => {
    const host = render(true);
    expect(retryButton(host).disabled).toBe(true);
    retryButton(host).click();
    expect(api.invoke).not.toHaveBeenCalled();
  });

  it("keeps a short status visible and technical reasons in collapsed details", async () => {
    const host = render();
    const explanation = [...host.querySelectorAll("p")]
      .find(p => p.textContent?.includes("tries faster mode once automatically"))!;
    expect(explanation.closest("[hidden]")).not.toBeNull();
    const reason = [...host.querySelectorAll("p")].find(p => p.textContent?.includes("Unconfirmed native exit"))!;
    expect(reason.closest("details")?.open).toBe(false);
    expect(host.querySelector('[role="status"]')?.textContent).toBe("Slower mode selected. Faster mode will not retry on its own.");
    expect(retryButton(host).closest("[hidden], dialog")).toBeNull();
    const help = host.querySelector<HTMLButtonElement>('button[aria-label="Help: Automatic model recovery"]')!;
    help.click();
    await vi.waitFor(() => expect(host.querySelector("dialog")?.open).toBe(true));
    expect(host.querySelector("dialog")?.textContent).toContain("No approval is needed");
    expect(api.invoke).not.toHaveBeenCalled();
    host.querySelector<HTMLButtonElement>("dialog button")!.click();
    flushSync();
    expect(host.querySelector("dialog")).toBeNull();
    expect(document.activeElement).toBe(help);
  });

  it("lets the user keep slower mode instead of taking the pending automatic attempt", async () => {
    const host = render(false, "retry_pending");
    expect(host.querySelector('[role="status"]')?.textContent).toContain("once on your next request");
    expect(retryButton(host)).toBeUndefined();
    expect(api.invoke).not.toHaveBeenCalled();
    [...host.querySelectorAll("button")].find(button => button.textContent === "Keep slower mode")!.click();
    await vi.waitFor(() => expect(api.invoke).toHaveBeenCalledWith("ai_use_cpu_for_model", { key: "selected-model" }));
  });

  it("does not offer another retry or approval while recovery is active", () => {
    const host = render(false, "retrying");
    expect(host.querySelector('[role="status"]')?.textContent).toBe("Trying faster mode. No action needed.");
    expect(retryButton(host)).toBeUndefined();
    expect(api.invoke).not.toHaveBeenCalled();
  });
});

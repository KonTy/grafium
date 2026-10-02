import { mount, unmount, flushSync } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import RuntimeRecovery from "./RuntimeRecovery.svelte";
import { aiAllowGpuRetry } from "../lib/knowledge";

const api = vi.hoisted(() => ({ invoke: vi.fn().mockResolvedValue(undefined) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: api.invoke }));

let component: ReturnType<typeof mount> | undefined;
let host: HTMLDivElement | undefined;
const dialogMethods = ["showModal", "close"] as const;
const descriptors = dialogMethods.map(method => Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, method));
const retryButton = (host: HTMLElement) => [...host.querySelectorAll("button")]
  .find(button => button.textContent === "Allow one GPU attempt")!;

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

function render(busy = false) {
  host = document.createElement("div");
  document.body.append(host);
  component = mount(RuntimeRecovery, {
    target: host,
    props: {
      records: [{ key: "selected-model", label: "Chat model", reason: "Unconfirmed native exit" }],
      busy,
      onRetry: aiAllowGpuRetry,
    },
  });
  flushSync();
  return host;
}

describe("explicit native recovery", () => {
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

  it("hides recovery instructions in help while keeping the reason and retry control visible", async () => {
    const host = render();
    const explanation = [...host.querySelectorAll("p")]
      .find(p => p.textContent?.includes("disables automatic GPU attempts"))!;
    expect(explanation.closest("[hidden]")).not.toBeNull();
    const reason = [...host.querySelectorAll("p")].find(p => p.textContent?.includes("Chat model"))!;
    expect(reason.closest("[hidden], dialog")).toBeNull();
    expect(retryButton(host).closest("[hidden], dialog")).toBeNull();
    const help = host.querySelector<HTMLButtonElement>('button[aria-label="Help: Native model recovery"]')!;
    help.click();
    await vi.waitFor(() => expect(host.querySelector("dialog")?.open).toBe(true));
    expect(host.querySelector("dialog")?.textContent).toContain("CPU remains available when RAM permits");
    expect(api.invoke).not.toHaveBeenCalled();
    host.querySelector<HTMLButtonElement>("dialog button")!.click();
    flushSync();
    expect(host.querySelector("dialog")).toBeNull();
    expect(document.activeElement).toBe(help);
  });
});

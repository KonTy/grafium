import { mount, unmount, flushSync } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import RuntimeRecovery from "./RuntimeRecovery.svelte";
import { aiAllowGpuRetry } from "../lib/knowledge";

const api = vi.hoisted(() => ({ invoke: vi.fn().mockResolvedValue(undefined) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: api.invoke }));

let component: ReturnType<typeof mount> | undefined;
let host: HTMLDivElement | undefined;

afterEach(async () => {
  if (component) await unmount(component);
  host?.remove();
  component = undefined;
  api.invoke.mockClear();
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
    host.querySelector("button")!.click();
    await vi.waitFor(() => expect(api.invoke).toHaveBeenCalledWith("ai_allow_gpu_retry", { key: "selected-model" }));
    expect(api.invoke).toHaveBeenCalledTimes(1);
  });

  it("prevents repeated authorization while a retry is in progress", () => {
    const host = render(true);
    expect(host.querySelector("button")!.disabled).toBe(true);
    host.querySelector("button")!.click();
    expect(api.invoke).not.toHaveBeenCalled();
  });
});

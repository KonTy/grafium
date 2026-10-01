import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import PrivateReaderSettings from "./PrivateReaderSettings.svelte";

let component: ReturnType<typeof mount> | undefined;
let requests: { command: string; args: Record<string, unknown> }[] = [];
let corrupt = false;
beforeEach(() => {
  requests = []; corrupt = false;
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Android");
  window.PrivateReaderBridge = { request(raw) {
    const request = JSON.parse(raw);
    requests.push(request);
    const result = request.command === "capabilities" ? { volume: {
      settings: { enabled: false, key: "up", gesture: "longPress" }, accessibilityConnected: false,
    } } : request.command === "voiceStatus" ? { available: true, runtime: "sherpa-vits-v1", selection: null, installed: [], installationErrors: [] }
      : request.command === "exportState" ? { exported: true, bytes: 2 * 1024 * 1024, scope: "private-library-history" }
      : request.command === "restoreState" ? { configured: false, books: [], restoreMode: "merge", restoreNotice: "Existing progress and edited notes retained." }
      : { configured: false, locationLabel: "", error: "", books: [] };
    queueMicrotask(() => window.dispatchEvent(new CustomEvent("private-reader-response", {
      detail: { id: request.id, ok: !(corrupt && request.command === "restoreState"), result,
        error: corrupt && request.command === "restoreState" ? "INVALID_BACKUP: existing state was not changed" : undefined },
    })));
  } };
});
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined; document.body.replaceChildren(); delete window.PrivateReaderBridge; vi.restoreAllMocks();
});
const button = (label: string) => [...document.querySelectorAll("button")].find(element => element.textContent?.trim() === label)!;
describe("native private history backup UI", () => {
  it("uses matching native export/restore pickers and accepts the actual successful merge response", async () => {
    component = mount(PrivateReaderSettings, { target: document.body });
    flushSync();
    button("Export private history").click();
    await vi.waitFor(() => expect(document.body.textContent).toContain("Private history exported"));
    await vi.waitFor(() => expect(button("Restore private history…").disabled).toBe(false));
    button("Restore private history…").click();
    await vi.waitFor(() => expect(document.body.textContent).toContain("Existing progress and edited notes retained."));
    expect(requests.filter(request => ["exportState", "restoreState"].includes(request.command))).toEqual([
      { id: expect.any(String), command: "exportState", args: {} },
      { id: expect.any(String), command: "restoreState", args: {} },
    ]);
    expect(document.querySelector('input[type="file"]')).toBeNull();
    expect(requests.some(request => "data" in request.args)).toBe(false);
  });
  it("shows native corruption failures without claiming a successful restore", async () => {
    corrupt = true;
    component = mount(PrivateReaderSettings, { target: document.body });
    flushSync(); button("Restore private history…").click();
    await vi.waitFor(() => expect(document.body.textContent).toContain("INVALID_BACKUP"));
    expect(document.body.textContent).not.toContain("Private history restored.");
    expect(document.body.textContent).not.toContain("Existing progress and edited notes retained.");
  });
});

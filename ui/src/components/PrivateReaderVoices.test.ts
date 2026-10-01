import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
const invoke = vi.hoisted(() => vi.fn());
const open = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open }));
import PrivateReaderVoices from "./PrivateReaderVoices.svelte";
import { privateVoiceLanguageSuggestion } from "../lib/privateReader";

let component: ReturnType<typeof mount> | undefined;
const manifest = { schema_version: 1, id: "local", name: "Local voice", language: "en-US",
  runtime: "sherpa-vits-v1", license: "Test license", license_url: "https://example.test/license",
  sample_rate: 22050, artifacts: [] };
const button = (label: string) => [...document.querySelectorAll("button")].find(element => element.textContent?.trim() === label)!;
beforeEach(() => { invoke.mockReset(); open.mockReset(); privateVoiceLanguageSuggestion.set(null); });
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined; document.body.replaceChildren(); delete window.PrivateReaderBridge; vi.restoreAllMocks();
});
describe("cross-platform offline voice settings", () => {
  it("renders native installation errors and offers metadata language without selecting or downloading", async () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Android");
    privateVoiceLanguageSuggestion.set({ bookId: "book", title: "Private EPUB", language: "eu-ES" });
    const calls: { command: string; args: Record<string, unknown> }[] = [];
    window.PrivateReaderBridge = { request(raw) {
      const request = JSON.parse(raw); calls.push(request);
      const result = request.command === "voiceStatus" ? {
        available: true, runtime: "sherpa-vits-v1", selection: { voice_id: manifest.id, language: manifest.language },
        installed: [manifest], installationErrors: [{ id: "damaged", available: false, error: "MODEL_HASH_MISMATCH" }],
        reason: "Reimport the damaged voice package.",
      } : manifest;
      queueMicrotask(() => window.dispatchEvent(new CustomEvent("private-reader-response", { detail: { id: request.id, ok: true, result } })));
    } };
    component = mount(PrivateReaderVoices, { target: document.body });
    await vi.waitFor(() => expect(document.body.textContent).toContain("MODEL_HASH_MISMATCH"));
    await vi.waitFor(() => expect(button("Use book language suggestion").disabled).toBe(false));
    expect(document.body.textContent).toContain("MODEL_HASH_MISMATCH");
    expect(document.body.textContent).toContain("Reimport the damaged voice package");
    expect(document.body.textContent).not.toContain("unavailable in this build");
    expect(button("Choose local Piper environment…")).toBeUndefined();
    button("Use book language suggestion").click(); flushSync();
    expect(document.querySelector<HTMLInputElement>('input[list="offline-voice-languages"]')?.value).toBe("eu-ES");
    expect(calls.map(call => call.command)).toEqual(["voiceStatus"]);
    button("Import offline model…").click();
    await vi.waitFor(() => expect(calls.some(call => call.command === "importVoice")).toBe(true));
    expect(open).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(button("Download and verify").disabled).toBe(true));
    const textarea = document.querySelector("textarea")!;
    textarea.value = JSON.stringify(manifest); textarea.dispatchEvent(new Event("input", { bubbles: true }));
    const consent = document.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
    consent.checked = true; consent.dispatchEvent(new Event("change", { bubbles: true })); flushSync();
    await vi.waitFor(() => expect(button("Download and verify").disabled).toBe(false));
    button("Download and verify").click();
    await vi.waitFor(() => expect(calls.find(call => call.command === "downloadVoice")?.args).toEqual({ manifest, authorized: true }));
  });
  it("retains the Linux dedicated-Piper environment picker and exact native command", async () => {
    invoke.mockImplementation(async command => command === "private_voice_status"
      ? { available: true, runtime: "piper-onnx-v1", selection: null, runtime_executable: "/chosen/venv/bin/piper" } : []);
    open.mockResolvedValue("/new/venv/bin/piper");
    component = mount(PrivateReaderVoices, { target: document.body });
    await vi.waitFor(() => expect(document.body.textContent).toContain("/chosen/venv/bin/piper"));
    await vi.waitFor(() => expect(button("Choose local Piper environment…").disabled).toBe(false));
    expect(document.body.textContent).toContain("/chosen/venv/bin/piper");
    button("Choose local Piper environment…").click();
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("private_voice_configure_runtime", { executablePath: "/new/venv/bin/piper" }));
  });
});

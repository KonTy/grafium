import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, unmount } from "svelte";
const openExternal = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/plugin-shell", () => ({ open: openExternal }));
import ReaderVoiceSetup from "./ReaderVoiceSetup.svelte";
import { VOICE_SETUP_URLS, voicePackageCommand } from "../lib/voiceSetup";
let component: ReturnType<typeof mount>;
const copy = vi.fn();
const previousClipboard = Object.getOwnPropertyDescriptor(navigator, "clipboard");
beforeEach(() => {
  openExternal.mockReset().mockResolvedValue(undefined); copy.mockReset().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: copy } });
});
afterEach(async () => {
  await unmount(component); document.body.replaceChildren();
  if (previousClipboard) Object.defineProperty(navigator, "clipboard", previousClipboard);
  else Reflect.deleteProperty(navigator, "clipboard");
});
describe("actionable offline voice setup", () => {
  it("can show Android preparation on a Linux computer without changing the native engine", async () => {
    component = mount(ReaderVoiceSetup, { target: document.body, props: { android: false } });
    await vi.waitFor(() => expect(document.querySelector("select")).not.toBeNull());
    const select = document.querySelector("select")!;
    select.value = "android"; select.dispatchEvent(new Event("change", { bubbles: true }));
    await vi.waitFor(() => expect(document.body.textContent).toContain("sherpa-vits-v1"));
    expect(document.body.textContent).toContain("entire");
    expect(document.body.textContent).toContain("espeak-ng-data");
    expect(openExternal).not.toHaveBeenCalled();
    expect(copy).not.toHaveBeenCalled();
  });
  it.each([false, true])("gives platform-correct files, preparation and import steps (Android=%s)", async android => {
    component = mount(ReaderVoiceSetup, { target: document.body, props: { android } });
    await vi.waitFor(() => expect(document.body.textContent).toContain("Prepare Grafium"));
    const text = document.body.textContent!;
    expect(text).toContain("no built-in one-click voice catalog");
    expect(text).toContain("public domain in the US");
    expect(text).toContain("does not convert the model");
    expect(text).toContain("Save voice and language");
    expect(text).toContain(android ? "sherpa-vits-v1" : "piper-onnx-v1");
    expect(text).toContain(android ? "tokens.txt" : "en_US-ljspeech-high.onnx.json");
    expect(text).toContain(android ? "select that folder" : "not the ONNX file");
    expect(openExternal).not.toHaveBeenCalled();
    const link = document.querySelector<HTMLAnchorElement>(`a[href="${android ? VOICE_SETUP_URLS.androidModel : VOICE_SETUP_URLS.linuxModel}"]`)!;
    link.click();
    expect(openExternal).toHaveBeenCalledWith(android ? VOICE_SETUP_URLS.androidModel : VOICE_SETUP_URLS.linuxModel);
    [...document.querySelectorAll("button")].find(button => button.textContent === "Copy package preparation command")!.click();
    await vi.waitFor(() => expect(copy).toHaveBeenCalledWith(voicePackageCommand(android)));
    await vi.waitFor(() => expect(document.querySelector('[role="status"]')?.textContent).toContain("Review it"));
  });
  it("reports browser and clipboard failures with manual alternatives", async () => {
    component = mount(ReaderVoiceSetup, { target: document.body, props: { android: false } });
    await vi.waitFor(() => expect(document.querySelector("a")).not.toBeNull());
    openExternal.mockRejectedValue(new Error("Browser unavailable"));
    document.querySelector("a")!.click();
    await vi.waitFor(() => expect(document.querySelector('[role="alert"]')?.textContent).toContain("Copy the displayed link"));
    copy.mockRejectedValue(new Error("Clipboard denied"));
    document.querySelector("button")!.click();
    await vi.waitFor(() => expect(document.querySelector('[role="alert"]')?.textContent).toContain("Select and copy"));
    expect(document.querySelector('[role="status"]')).toBeNull();
  });
});

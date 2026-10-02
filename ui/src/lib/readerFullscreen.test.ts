import { afterEach, describe, expect, it, vi } from "vitest";
const api = vi.hoisted(() => ({ isTauri: vi.fn(), isFullscreen: vi.fn(), setFullscreen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ isTauri: api.isTauri }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => api }));
import { enterReaderFullscreen } from "./readerFullscreen";

afterEach(() => { vi.resetAllMocks(); vi.restoreAllMocks(); });
describe("reader fullscreen ownership", () => {
  it.each([false, true])("restores existing native fullscreen=%s", async initiallyFullscreen => {
    api.isTauri.mockReturnValue(true);
    api.isFullscreen.mockResolvedValue(initiallyFullscreen);
    api.setFullscreen.mockResolvedValue(undefined);
    const session = await enterReaderFullscreen(document.createElement("section"));
    expect(api.setFullscreen).toHaveBeenCalledTimes(initiallyFullscreen ? 0 : 1);
    await session.exit();
    if (initiallyFullscreen) expect(api.setFullscreen).not.toHaveBeenCalled();
    else expect(api.setFullscreen.mock.calls).toEqual([[true], [false]]);
  });
  it("uses browser fullscreen without closing another element's fullscreen", async () => {
    api.isTauri.mockReturnValue(false);
    const element = document.createElement("section");
    element.requestFullscreen = vi.fn().mockResolvedValue(undefined);
    const exit = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(document, "exitFullscreen", { value: exit, configurable: true });
    Object.defineProperty(document, "fullscreenElement", { get: () => null, configurable: true });
    const current = vi.spyOn(document, "fullscreenElement", "get").mockReturnValue(element);
    const session = await enterReaderFullscreen(element);
    expect(await session.isActive()).toBe(true);
    current.mockReturnValue(document.body);
    await session.exit();
    expect(exit).not.toHaveBeenCalled();
  });
  it("does not pretend to enter fullscreen when the platform refuses", async () => {
    api.isTauri.mockReturnValue(true);
    api.isFullscreen.mockResolvedValue(false);
    api.setFullscreen.mockRejectedValue(new Error("Permission denied"));
    await expect(enterReaderFullscreen(document.createElement("section"))).rejects.toThrow("Permission denied");
  });
});

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { get } from "svelte/store";
import { createAppearanceController } from "./appearance";
import { applyTheme, getThemeById, readableSupportingText } from "./themes";
import type { SystemAppearance } from "./api";

const glass = (): SystemAppearance => ({
  themeName: "tokyo-night", backgroundOpacity: 0.8, nativeTransparency: true,
});

beforeEach(() => {
  vi.stubGlobal("localStorage", { setItem: vi.fn() });
});

function setup(preference = "auto", system = glass()) {
  let notify = () => {};
  const stop = vi.fn();
  const deps = {
    readPreference: vi.fn(async () => preference),
    savePreference: vi.fn(async (_id: string) => {}),
    readSystem: vi.fn(async () => system),
    listen: vi.fn(async (callback: () => void): Promise<() => void> => { notify = callback; return stop; }),
  };
  const controller = createAppearanceController(deps);
  return { controller, deps, notify: () => notify(), stop };
}

afterEach(() => {
  document.documentElement.removeAttribute("style");
  document.documentElement.removeAttribute("data-window-transparency");
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("background appearance", () => {
  it.each(["catppuccin", "catppuccin-latte"])("keeps helper text strong on %s glass and restores opaque colors", (id) => {
    const colors = getThemeById(id)!.colors;
    const root = document.documentElement;
    for (const opacity of [0.5, 1, 0, 0.9]) {
      applyTheme(colors, opacity);
      const expected = readableSupportingText(colors, opacity < 1);
      expect(root.style.getPropertyValue("--text-muted")).toBe(expected.muted);
      expect(root.style.getPropertyValue("--text-secondary")).toBe(expected.secondary);
      expect(root.style.getPropertyValue("--text-primary")).toBe(colors.textPrimary);
      expect(root.style.opacity).toBe("");
    }
  });
  it("changes window paint without fading body text or solid surfaces", () => {
    const theme = getThemeById("github")!;
    applyTheme(theme.colors, 0.65);
    const root = document.documentElement;
    expect(root.style.getPropertyValue("--window-bg-primary")).toBe("rgba(255, 255, 255, 0.65)");
    expect(root.style.getPropertyValue("--bg-primary")).toBe(theme.colors.bgPrimary);
    expect(root.style.getPropertyValue("--text-primary")).toBe(theme.colors.textPrimary);
    expect(root.style.getPropertyValue("--surface-overlay")).toBe(theme.colors.surfaceOverlay);
    expect(root.style.opacity).toBe("");
    expect(root.hasAttribute("data-window-transparency")).toBe(true);
    applyTheme(theme.colors);
    expect(root.hasAttribute("data-window-transparency")).toBe(false);
  });

  it.each([NaN, Infinity, -1, 1.01])("rejects invalid opacity %s safely", (opacity) => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    applyTheme(getThemeById("github")!.colors, opacity);
    expect(document.documentElement.hasAttribute("data-window-transparency")).toBe(false);
    expect(error).toHaveBeenCalled();
  });

  it("keeps auto live including same-name changes and zero/opaque endpoints", async () => {
    const { controller, deps, notify } = setup();
    await controller.start();
    expect(document.documentElement.hasAttribute("data-window-transparency")).toBe(true);
    deps.readSystem.mockResolvedValue({ ...glass(), backgroundOpacity: 0 });
    notify();
    await vi.waitFor(() => expect(get(controller).system.backgroundOpacity).toBe(0));
    expect(document.documentElement.style.getPropertyValue("--window-bg-primary")).toMatch(/, 0\)$/);
    deps.readSystem.mockResolvedValue({ ...glass(), backgroundOpacity: 1 });
    await controller.refresh();
    expect(document.documentElement.hasAttribute("data-window-transparency")).toBe(false);
    controller.stop();
  });

  it("never replaces a manually selected palette on a system event", async () => {
    const { controller, deps } = setup();
    await controller.start();
    await controller.select("oled");
    deps.readSystem.mockResolvedValue({ ...glass(), themeName: "github", backgroundOpacity: 0.5 });
    await controller.refresh();
    expect(get(controller).preference).toBe("oled");
    expect(document.documentElement.style.getPropertyValue("--bg-primary")).toBe("#000000");
    expect(document.documentElement.hasAttribute("data-window-transparency")).toBe(false);
    await controller.select("auto");
    expect(document.documentElement.style.getPropertyValue("--bg-primary")).toBe("#ffffff");
    expect(document.documentElement.hasAttribute("data-window-transparency")).toBe(true);
    controller.stop();
  });

  it.each([
    { ...glass(), nativeTransparency: false },
    { ...glass(), themeName: null },
    { ...glass(), themeName: "unknown-custom-theme" },
  ])("uses opaque fallback for unsupported/missing/unknown appearance %j", async (system) => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const { controller } = setup("auto", system);
    await controller.start();
    expect(document.documentElement.hasAttribute("data-window-transparency")).toBe(false);
    controller.stop();
  });

  it("discards a stale read after rapid theme replacement", async () => {
    const { controller, deps } = setup();
    await controller.start();
    let finish!: (value: SystemAppearance) => void;
    deps.readSystem.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const old = controller.refresh();
    deps.readSystem.mockResolvedValue({ ...glass(), themeName: "github", backgroundOpacity: 0.6 });
    await controller.refresh();
    finish(glass());
    await old;
    expect(get(controller).system.themeName).toBe("github");
    expect(document.documentElement.style.getPropertyValue("--window-bg-primary")).toBe("rgba(255, 255, 255, 0.6)");
    controller.stop();
  });

  it("explains an opaque renderer override only when Auto requests transparency", async () => {
    const reason = "WEBKIT_DISABLE_DMABUF_RENDERER requires opaque rendering.";
    const { controller, deps } = setup("auto", {
      ...glass(), nativeTransparency: false, transparencyUnavailableReason: reason,
    });
    await controller.start();
    expect(get(controller).error).toBe(reason);
    expect(document.documentElement.hasAttribute("data-window-transparency")).toBe(false);
    await controller.select("github");
    expect(get(controller).error).toBe("");
    await controller.select("auto");
    deps.readSystem.mockResolvedValue(glass());
    await controller.refresh();
    expect(get(controller).error).toBe("");
    expect(document.documentElement.hasAttribute("data-window-transparency")).toBe(true);
    controller.stop();
  });

  it("does not let slow startup preferences overwrite a user click", async () => {
    const { controller, deps } = setup();
    let finish!: (id: string) => void;
    deps.readPreference.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const start = controller.start();
    await controller.select("oled");
    finish("auto");
    await start;
    expect(get(controller).preference).toBe("oled");
    controller.stop();
  });

  it("serializes preference writes and exposes persistence failure", async () => {
    const { controller, deps } = setup();
    await controller.start();
    let finish!: () => void;
    deps.savePreference.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const first = controller.select("oled");
    const second = controller.select("github");
    await vi.waitFor(() => expect(deps.savePreference).toHaveBeenCalledTimes(1));
    finish();
    await Promise.all([first, second]);
    expect(deps.savePreference.mock.calls.map(([id]) => id)).toEqual(["oled", "github"]);
    deps.savePreference.mockRejectedValueOnce(new Error("read-only"));
    vi.spyOn(console, "error").mockImplementation(() => {});
    await expect(controller.select("auto")).rejects.toThrow("read-only");
    expect(get(controller).error).toContain("Could not save");
    controller.stop();
  });

  it("falls back safely on read errors and releases its listener", async () => {
    const { controller, deps, stop } = setup();
    await controller.start();
    vi.spyOn(console, "error").mockImplementation(() => {});
    deps.readSystem.mockRejectedValueOnce(new Error("unavailable"));
    await controller.refresh();
    expect(document.documentElement.hasAttribute("data-window-transparency")).toBe(false);
    expect(get(controller).error).toContain("opaque");
    controller.stop();
    expect(stop).toHaveBeenCalledOnce();
  });

  it("keeps failed watcher registration visible after a successful snapshot", async () => {
    const { controller, deps } = setup();
    deps.listen.mockRejectedValueOnce(new Error("no event channel"));
    vi.spyOn(console, "error").mockImplementation(() => {});
    await controller.start();
    expect(get(controller).error).toContain("Live theme updates");
    controller.stop();
  });

  it("removes a listener whose registration completes after teardown", async () => {
    const { controller, deps } = setup();
    let finish!: (stop: () => void) => void;
    deps.listen.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const start = controller.start();
    controller.stop();
    const stop = vi.fn();
    finish(stop);
    await start;
    expect(stop).toHaveBeenCalledOnce();
    expect(document.documentElement.hasAttribute("data-window-transparency")).toBe(false);
  });
});

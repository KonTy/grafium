import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { revealStartupWindow } from "./startupWindow";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn().mockResolvedValue(undefined) }));

afterEach(() => {
  vi.clearAllMocks();
  vi.restoreAllMocks();
  document.documentElement.removeAttribute("style");
});

describe("startup window readiness", () => {
  it("passes the applied dark background without waiting for hidden-window animation frames", async () => {
    document.documentElement.style.setProperty("--bg-primary", "#0d1117");
    const frame = vi.spyOn(window, "requestAnimationFrame").mockImplementation(() => 1);
    await revealStartupWindow();
    expect(frame).not.toHaveBeenCalled();
    expect(invoke).toHaveBeenCalledWith("reveal_startup_window", { background: [13, 17, 23] });
  });

  it("uses the applied light theme instead of assuming dark mode", async () => {
    document.documentElement.style.setProperty("--bg-primary", "#ffffff");
    await revealStartupWindow();
    expect(invoke).toHaveBeenCalledWith("reveal_startup_window", { background: [255, 255, 255] });
  });

  it("surfaces native show failures", async () => {
    document.documentElement.style.setProperty("--bg-primary", "#000000");
    vi.mocked(invoke).mockRejectedValueOnce(new Error("Window unavailable"));
    await expect(revealStartupWindow()).rejects.toThrow("Window unavailable");
  });
});

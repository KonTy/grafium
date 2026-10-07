import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  revealStartupWindow,
  reportStartupContent,
  resetStartupContentReportForTests,
} from "./startupWindow";

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

describe("first content reporting", () => {
  beforeEach(() => resetStartupContentReportForTests());

  it("reports the first painted page separately from the revealed window", async () => {
    await reportStartupContent();
    expect(invoke).toHaveBeenCalledWith("startup_content_ready", expect.anything());
  });

  it("only reports once so a later navigation does not overwrite the startup timing", async () => {
    await reportStartupContent();
    await reportStartupContent();
    await reportStartupContent();
    expect(vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === "startup_content_ready")).toHaveLength(1);
  });
});

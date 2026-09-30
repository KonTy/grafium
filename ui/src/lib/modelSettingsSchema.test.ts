import { describe, expect, it, vi } from "vitest";

const calls = vi.hoisted(() => ({ invoke: vi.fn().mockResolvedValue({}) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: calls.invoke }));
import { aiModelSettingsSchema, aiRuntimeSettings } from "./knowledge";

describe("shared model settings bridge", () => {
  it("uses the shared schema and secret-reference settings endpoints", async () => {
    await aiModelSettingsSchema();
    expect(calls.invoke).toHaveBeenLastCalledWith("ai_model_settings_schema");
    await aiRuntimeSettings();
    expect(calls.invoke).toHaveBeenLastCalledWith("ai_runtime_settings");
  });
});

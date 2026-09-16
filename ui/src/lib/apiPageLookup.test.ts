import { describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { getPage } from "./api";
import { isPageNotFoundError } from "./navigation";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

describe("page lookup errors", () => {
  it("keeps native ambiguity readable without granting permission to create", async () => {
    vi.mocked(invoke).mockRejectedValueOnce({ code: "page_lookup_failed", message: "Niacin is ambiguous." });
    const error = await getPage({ title: "Niacin" }).catch((error: unknown) => error);
    expect(error).toBeInstanceOf(Error);
    expect(String(error)).toBe("Error: Niacin is ambiguous.");
    expect(isPageNotFoundError(error)).toBe(false);
  });

  it("preserves the explicit missing-page code", async () => {
    vi.mocked(invoke).mockRejectedValueOnce({ code: "page_not_found", message: "No such page." });
    const error = await getPage({ title: "New topic" }).catch((error: unknown) => error);
    expect(isPageNotFoundError(error)).toBe(true);
    expect(String(error)).toBe("Error: No such page.");
  });

  it("preserves unexpected IPC failures", async () => {
    const offline = new Error("IPC disconnected");
    vi.mocked(invoke).mockRejectedValueOnce(offline);
    await expect(getPage({ id: "source" })).rejects.toBe(offline);
  });
});

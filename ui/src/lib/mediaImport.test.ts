import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

import { invoke } from "@tauri-apps/api/core";
import { mediaImportVideo } from "./api";

const mockInvoke = vi.mocked(invoke);

beforeEach(() => {
  mockInvoke.mockReset();
});

describe("mediaImportVideo API wrapper", () => {
  it("calls media_import_video with the url and optional args", async () => {
    mockInvoke.mockResolvedValue({ id: "1", title: "My Video" });
    await mediaImportVideo("https://youtube.com/watch?v=abc", "My Video", "en");
    expect(mockInvoke).toHaveBeenCalledWith("media_import_video", {
      url: "https://youtube.com/watch?v=abc",
      pageTitle: "My Video",
      lang: "en",
    });
  });

  it("omits optional args when not provided", async () => {
    mockInvoke.mockResolvedValue({ id: "1", title: "My Video" });
    await mediaImportVideo("https://youtube.com/watch?v=abc");
    expect(mockInvoke).toHaveBeenCalledWith("media_import_video", {
      url: "https://youtube.com/watch?v=abc",
      pageTitle: undefined,
      lang: undefined,
    });
  });
});

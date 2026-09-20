import { afterEach, describe, expect, it, vi } from "vitest";
import {
  canRequestPersistentStorageAccess,
  hasPersistentStorageAccess,
  isStoragePermissionError,
  requestPersistentStorageAccess,
} from "./androidStorageAccess";

afterEach(() => {
  delete (window as typeof window & { FolderPickerBridge?: unknown }).FolderPickerBridge;
});

describe("Android persistent graph access", () => {
  it("is inert on desktop", () => {
    expect(hasPersistentStorageAccess()).toBe(true);
    expect(canRequestPersistentStorageAccess()).toBe(false);
    expect(requestPersistentStorageAccess()).toBe(false);
  });

  it("queries and opens Android's persistent storage settings", () => {
    const request = vi.fn();
    (window as typeof window & { FolderPickerBridge?: unknown }).FolderPickerBridge = {
      hasPersistentStorageAccess: () => false,
      requestPersistentStorageAccess: request,
    };

    expect(hasPersistentStorageAccess()).toBe(false);
    expect(canRequestPersistentStorageAccess()).toBe(true);
    expect(requestPersistentStorageAccess()).toBe(true);
    expect(request).toHaveBeenCalledOnce();
  });

  it("recognizes filesystem permission failures", () => {
    expect(isStoragePermissionError("IO error: Operation not permitted (os error 1)")).toBe(true);
    expect(isStoragePermissionError("Permission denied")).toBe(true);
    expect(isStoragePermissionError("Database is busy")).toBe(false);
  });
});

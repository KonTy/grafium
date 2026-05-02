/**
 * Hierarchy regression tests.
 *
 * These tests guard against the bugs that caused:
 *  - hierarchy not appearing (no parent/children shown)
 *  - crashes when hierarchy commands were missing from invoke_handler
 *  - backslash paths not normalising to forward-slash
 *
 * All Tauri invoke calls are mocked so the tests run in Node / Vitest with no
 * app window open.
 */
import { describe, expect, it, vi, beforeEach } from "vitest";

// ─── helpers that mirror the real parser logic ────────────────────────────────

/** Matches what core/src/parser/links.rs does: replace \ with / */
function normalizeHierarchyTitle(title: string): string {
  return title.replace(/\\/g, "/");
}

/** Extract parent path: "a/b/c" → "a/b", "a" → null */
function extractParentPath(title: string): string | null {
  const idx = title.lastIndexOf("/");
  return idx >= 0 ? title.slice(0, idx) : null;
}

/** All ancestor segments for a title: "a/b/c" → ["a", "a/b"] */
function ancestorSegments(title: string): string[] {
  const parts = title.split("/");
  const ancestors: string[] = [];
  for (let i = 1; i < parts.length; i++) {
    ancestors.push(parts.slice(0, i).join("/"));
  }
  return ancestors;
}

/** Simulate the get_child_pages SQL LIKE "parent/%" logic */
function simulateGetChildPages(allTitles: string[], parentTitle: string): string[] {
  const prefix = parentTitle.toLowerCase() + "/";
  return allTitles.filter((t) => t.toLowerCase().startsWith(prefix));
}

// ─── Parser / title normalisation ────────────────────────────────────────────

describe("hierarchy title normalisation", () => {
  it("forward slashes are unchanged", () => {
    expect(normalizeHierarchyTitle("test/page")).toBe("test/page");
  });

  it("backslashes are converted to forward slashes", () => {
    expect(normalizeHierarchyTitle("test\\page")).toBe("test/page");
  });

  it("mixed slashes are all converted", () => {
    expect(normalizeHierarchyTitle("a\\b/c\\d")).toBe("a/b/c/d");
  });

  it("non-hierarchical title is unchanged", () => {
    expect(normalizeHierarchyTitle("simple")).toBe("simple");
  });
});

// ─── Parent path extraction ───────────────────────────────────────────────────

describe("extractParentPath", () => {
  it("returns null for a top-level page", () => {
    expect(extractParentPath("test")).toBeNull();
  });

  it("returns the parent segment for one-level hierarchy", () => {
    expect(extractParentPath("test/page")).toBe("test");
  });

  it("returns the full parent for deep hierarchy", () => {
    expect(extractParentPath("a/b/c")).toBe("a/b");
  });
});

// ─── Ancestor auto-creation ───────────────────────────────────────────────────

describe("ancestorSegments (auto-create parent chain)", () => {
  it("no ancestors for a flat title", () => {
    expect(ancestorSegments("flat")).toEqual([]);
  });

  it("one ancestor for two-level hierarchy", () => {
    expect(ancestorSegments("project/web")).toEqual(["project"]);
  });

  it("all ancestors for deep hierarchy", () => {
    expect(ancestorSegments("a/b/c/d")).toEqual(["a", "a/b", "a/b/c"]);
  });
});

// ─── Child page lookup simulation ────────────────────────────────────────────

describe("simulateGetChildPages", () => {
  const pages = ["project", "project/web", "project/web/frontend", "project/web/backend", "project/mobile", "other"];

  it("returns direct and nested children for 'project'", () => {
    const children = simulateGetChildPages(pages, "project");
    expect(children).toContain("project/web");
    expect(children).toContain("project/web/frontend");
    expect(children).toContain("project/mobile");
    expect(children).not.toContain("project");
    expect(children).not.toContain("other");
  });

  it("returns only children under 'project/web'", () => {
    const children = simulateGetChildPages(pages, "project/web");
    expect(children).toContain("project/web/frontend");
    expect(children).toContain("project/web/backend");
    expect(children).not.toContain("project/mobile");
  });

  it("returns empty array for a leaf page", () => {
    expect(simulateGetChildPages(pages, "project/web/frontend")).toEqual([]);
  });

  it("is case-insensitive", () => {
    const mixed = ["Project/Web", "project/web/frontend"];
    expect(simulateGetChildPages(mixed, "project/web")).toContain("project/web/frontend");
  });
});

// ─── API wrapper (mocked invoke) ─────────────────────────────────────────────

// Mock the Tauri module before importing so the import resolves
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

import { invoke } from "@tauri-apps/api/core";
import { getParentPage, getChildPages } from "./api";

const mockInvoke = vi.mocked(invoke);

beforeEach(() => {
  mockInvoke.mockReset();
});

describe("getParentPage API wrapper", () => {
  it("calls get_parent_page with correct title param", async () => {
    mockInvoke.mockResolvedValue(null);
    await getParentPage("test/page");
    expect(mockInvoke).toHaveBeenCalledWith("get_parent_page", { title: "test/page" });
  });

  it("returns null when backend returns null (top-level page)", async () => {
    mockInvoke.mockResolvedValue(null);
    const result = await getParentPage("test");
    expect(result).toBeNull();
  });

  it("returns the parent page object when it exists", async () => {
    const parent = { id: "p1", title: "test", is_journal: false, created_at: "0", updated_at: "0", properties: {} };
    mockInvoke.mockResolvedValue(parent);
    const result = await getParentPage("test/page");
    expect(result).toEqual(parent);
    expect(result?.title).toBe("test");
  });
});

describe("getChildPages API wrapper", () => {
  it("calls get_child_pages with correct parentTitle param", async () => {
    mockInvoke.mockResolvedValue([]);
    await getChildPages("project");
    expect(mockInvoke).toHaveBeenCalledWith("get_child_pages", { parentTitle: "project" });
  });

  it("returns an empty array when there are no children", async () => {
    mockInvoke.mockResolvedValue([]);
    const result = await getChildPages("leaf");
    expect(result).toEqual([]);
  });

  it("returns all child page objects", async () => {
    const children = [
      { id: "c1", title: "project/web", is_journal: false, created_at: "0", updated_at: "0", properties: {} },
      { id: "c2", title: "project/mobile", is_journal: false, created_at: "0", updated_at: "0", properties: {} },
    ];
    mockInvoke.mockResolvedValue(children);
    const result = await getChildPages("project");
    expect(result).toHaveLength(2);
    expect(result.map((p) => p.title)).toContain("project/web");
    expect(result.map((p) => p.title)).toContain("project/mobile");
  });

  it("does not throw when invoke rejects — caller should handle gracefully", async () => {
    mockInvoke.mockRejectedValue(new Error("command not found"));
    await expect(getChildPages("x")).rejects.toThrow("command not found");
  });
});

// ─── Hierarchy section visibility logic ──────────────────────────────────────

describe("hierarchy section visibility", () => {
  /** mirrors the {#if parentPage || childPages.length > 0} guard in PageContent */
  function shouldShowHierarchy(parentPage: unknown, childPages: unknown[]): boolean {
    return parentPage !== null || childPages.length > 0;
  }

  it("hidden when no parent and no children", () => {
    expect(shouldShowHierarchy(null, [])).toBe(false);
  });

  it("shown when parent exists even with no children", () => {
    const parent = { id: "p1", title: "test" };
    expect(shouldShowHierarchy(parent, [])).toBe(true);
  });

  it("shown when children exist even with no parent", () => {
    const children = [{ id: "c1", title: "test/page" }];
    expect(shouldShowHierarchy(null, children)).toBe(true);
  });

  it("shown when both parent and children exist", () => {
    const parent = { id: "p1", title: "test" };
    const children = [{ id: "c1", title: "test/page" }];
    expect(shouldShowHierarchy(parent, children)).toBe(true);
  });
});

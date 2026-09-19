import { describe, expect, it } from "vitest";
import {
  distributeTreeGroups,
  treeColumnCount,
  virtualizeTreeColumns,
} from "./pageTreeVirtualization";

interface Row {
  id: string;
}

const group = (root: string, size: number): Row[] =>
  Array.from({ length: size }, (_, index) => ({
    id: index === 0 ? root : `${root}/child-${index}`,
  }));

describe("tree column distribution", () => {
  it("keeps root groups whole and preserves reading order", () => {
    const groups = [group("a", 5), group("b", 2), group("c", 4), group("d", 1)];
    const columns = distributeTreeGroups(groups, 3);

    expect(columns.flat()).toEqual(groups);
    expect(columns.flat().map((candidate) => candidate[0].id)).toEqual(["a", "b", "c", "d"]);
    expect(columns.map((column) => column.flat().length)).toEqual([5, 2, 5]);
  });

  it("uses up to four roughly 22rem columns", () => {
    expect(treeColumnCount(351)).toBe(1);
    expect(treeColumnCount(760)).toBe(2);
    expect(treeColumnCount(1_520)).toBe(4);
    expect(treeColumnCount(4_000)).toBe(4);
  });
});

describe("tree column virtualization", () => {
  it("keeps an 800-row tree bounded to the viewport", () => {
    const groups = Array.from({ length: 800 }, (_, index) => group(`page-${index}`, 1));
    const columns = distributeTreeGroups(groups, 4);
    const virtual = virtualizeTreeColumns(columns, {
      scrollTop: 0,
      viewportHeight: 600,
      rowStride: 34,
      overscanPx: 170,
    });
    const rendered = virtual.flatMap((column) => column.groups)
      .reduce((sum, candidate) => sum + candidate.visibleRows.length, 0);

    expect(rendered).toBeLessThan(120);
    expect(rendered).toBeLessThan(groups.length / 6);
  });

  it("windows rows inside one oversized root group", () => {
    const columns = distributeTreeGroups([group("root", 800)], 4);
    const virtual = virtualizeTreeColumns(columns, {
      scrollTop: 10_000,
      viewportHeight: 600,
      rowStride: 34,
      overscanPx: 170,
    });

    expect(virtual[0].groups[0].visibleRows.length).toBeLessThan(40);
  });

  it("keeps an offscreen keyboard or reveal anchor mounted", () => {
    const groups = Array.from({ length: 800 }, (_, index) => group(`page-${index}`, 1));
    const columns = distributeTreeGroups(groups, 4);
    const virtual = virtualizeTreeColumns(columns, {
      scrollTop: 0,
      viewportHeight: 600,
      rowStride: 34,
      overscanPx: 170,
      anchorId: "page-799",
    });

    expect(
      virtual.some((column) =>
        column.groups.some((candidate) =>
          candidate.visibleRows.some(({ row }) => row.id === "page-799"),
        ),
      ),
    ).toBe(true);
    expect(
      virtual.some((column) =>
        column.groups.some((candidate) =>
          candidate.visibleRows.some(({ row }) => row.id === "page-0"),
        ),
      ),
    ).toBe(true);
  });
});

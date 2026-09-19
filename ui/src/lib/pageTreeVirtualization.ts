import { computeVirtualWindow } from "./pageContentVirtualization";

export interface VirtualTreeRow {
  id: string;
}

export interface VirtualTreeGroup<T extends VirtualTreeRow> {
  id: string;
  rows: readonly T[];
  top: number;
  height: number;
  visibleRows: Array<{ row: T; index: number }>;
}

export interface VirtualTreeColumn<T extends VirtualTreeRow> {
  groups: VirtualTreeGroup<T>[];
  height: number;
}

export interface VirtualTreeOptions {
  scrollTop: number;
  viewportHeight: number;
  rowStride: number;
  overscanPx: number;
  anchorId?: string | null;
}

/**
 * Split root groups into contiguous, approximately balanced columns.
 *
 * Keeping the groups contiguous preserves the old top-to-bottom,
 * left-to-right reading order while ensuring a root and its descendants never
 * move into different columns.
 */
export function distributeTreeGroups<T>(
  groups: readonly (readonly T[])[],
  requestedColumnCount: number,
): (readonly (readonly T[])[])[] {
  if (groups.length === 0) return [];

  const columnCount = Math.min(
    groups.length,
    Math.max(1, Math.floor(requestedColumnCount)),
  );
  const columns: (readonly (readonly T[])[])[] = [];
  let nextGroup = 0;
  let remainingRows = groups.reduce((sum, group) => sum + group.length, 0);

  for (let columnIndex = 0; columnIndex < columnCount; columnIndex += 1) {
    const column: (readonly T[])[] = [];
    const columnsRemaining = columnCount - columnIndex;
    const targetRows = remainingRows / columnsRemaining;
    let columnRows = 0;

    while (nextGroup < groups.length) {
      const groupsRemaining = groups.length - nextGroup;
      if (column.length > 0 && groupsRemaining === columnsRemaining - 1) break;

      const next = groups[nextGroup];
      const withNext = columnRows + next.length;
      if (
        column.length > 0
        && Math.abs(columnRows - targetRows) <= Math.abs(withNext - targetRows)
      ) {
        break;
      }

      column.push(next);
      columnRows = withNext;
      nextGroup += 1;
    }

    columns.push(column);
    remainingRows -= columnRows;
  }

  return columns;
}

export function treeColumnCount(
  width: number,
  minColumnWidth = 22 * 16,
  columnGap = 28,
  maxColumns = 4,
): number {
  const available = Math.max(0, width);
  return Math.max(
    1,
    Math.min(maxColumns, Math.floor((available + columnGap) / (minColumnWidth + columnGap))),
  );
}

export function virtualizeTreeColumns<T extends VirtualTreeRow>(
  columns: readonly (readonly (readonly T[])[])[],
  options: VirtualTreeOptions,
): VirtualTreeColumn<T>[] {
  const rowStride = Math.max(1, options.rowStride);

  return columns.map((groups) => {
    let top = 0;
    const positioned = groups.map((rows) => {
      const group = {
        id: rows[0]?.id ?? `empty-${top}`,
        rows,
        top,
        height: rows.length * rowStride,
      };
      top += group.height;
      return group;
    });
    const heights = new Map(positioned.map((group) => [group.id, group.height]));
    const anchorGroupIndex = options.anchorId
      ? positioned.findIndex((group) => group.rows.some((row) => row.id === options.anchorId))
      : -1;
    const groupWindow = computeVirtualWindow(positioned, {
      scrollTop: options.scrollTop,
      viewportHeight: options.viewportHeight,
      measuredHeights: heights,
      defaultHeight: rowStride,
      overscanPx: options.overscanPx,
    });
    const groupsToRender = [...groupWindow.items];
    if (
      anchorGroupIndex >= 0
      && !groupsToRender.some((group) => group.id === positioned[anchorGroupIndex].id)
    ) {
      groupsToRender.push(positioned[anchorGroupIndex]);
      groupsToRender.sort((a, b) => a.top - b.top);
    }

    const visibleGroups = groupsToRender.map((group) => {
      const anchorIndex = options.anchorId
        ? group.rows.findIndex((row) => row.id === options.anchorId)
        : -1;
      const rowWindow = computeVirtualWindow(group.rows, {
        scrollTop: Math.max(0, options.scrollTop - group.top),
        viewportHeight: options.viewportHeight,
        defaultHeight: rowStride,
        overscanPx: options.overscanPx,
      });
      const visibleRows = rowWindow.items.map((row, offset) => ({
        row,
        index: rowWindow.startIndex + offset,
      }));
      if (
        anchorIndex >= 0
        && !visibleRows.some((candidate) => candidate.index === anchorIndex)
      ) {
        visibleRows.push({ row: group.rows[anchorIndex], index: anchorIndex });
        visibleRows.sort((a, b) => a.index - b.index);
      }
      return {
        ...group,
        visibleRows,
      };
    });

    return { groups: visibleGroups, height: top };
  });
}

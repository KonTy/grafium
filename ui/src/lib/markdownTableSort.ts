export type TableSortDirection = "asc" | "desc";

interface TableRange {
  dataStart: number;
  dataEnd: number;
}

function splitPipeTableRow(line: string): string[] | null {
  const trimmed = line.trim();
  if (!trimmed.includes("|")) return null;

  let body = trimmed;
  if (body.startsWith("|")) body = body.slice(1);
  if (body.endsWith("|")) body = body.slice(0, -1);

  const cells: string[] = [];
  let current = "";
  let escaped = false;
  for (const ch of body) {
    if (escaped) {
      current += ch;
      escaped = false;
      continue;
    }
    if (ch === "\\") {
      current += ch;
      escaped = true;
      continue;
    }
    if (ch === "|") {
      cells.push(current.trim());
      current = "";
      continue;
    }
    current += ch;
  }
  cells.push(current.trim());
  return cells.some((cell) => cell.length > 0) ? cells : null;
}

function isTableDelimiterCells(cells: readonly string[]): boolean {
  return cells.length > 0 && cells.every((cell) => /^:?-{3,}:?$/.test(cell));
}

function isLoosePipeTableRow(line: string): boolean {
  const trimmed = line.trim();
  if (!trimmed.startsWith("|") || !trimmed.endsWith("|")) return false;
  const cells = splitPipeTableRow(line);
  return !!cells && cells.length >= 2 && cells.some((cell) => cell.length > 0);
}

function fenceMarker(line: string): { char: "`" | "~"; length: number } | null {
  const match = /^\s*(`{3,}|~{3,})/.exec(line);
  if (!match) return null;
  const marker = match[1];
  return { char: marker[0] as "`" | "~", length: marker.length };
}

function toggleFenceState(
  line: string,
  active: { char: "`" | "~"; length: number } | null,
): { char: "`" | "~"; length: number } | null {
  const marker = fenceMarker(line);
  if (!marker) return active;
  if (!active) return marker;
  return marker.char === active.char && marker.length >= active.length ? null : active;
}

function isNumericCell(value: string): boolean {
  const trimmed = value.trim().replace(/,/g, "");
  return /^-?\d+(?:\.\d+)?$/.test(trimmed);
}

function compareCells(a: string, b: string, direction: TableSortDirection): number {
  const left = a.trim();
  const right = b.trim();
  if (left === "" && right === "") return 0;
  if (left === "") return 1;
  if (right === "") return -1;

  let cmp: number;
  if (isNumericCell(left) && isNumericCell(right)) {
    cmp = Number(left.replace(/,/g, "")) - Number(right.replace(/,/g, ""));
  } else {
    cmp = left.localeCompare(right, undefined, { numeric: true, sensitivity: "base" });
  }
  return direction === "asc" ? cmp : -cmp;
}

function findTables(lines: readonly string[]): TableRange[] {
  const tables: TableRange[] = [];
  let activeFence: { char: "`" | "~"; length: number } | null = null;

  for (let i = 0; i < lines.length - 1; i += 1) {
    activeFence = toggleFenceState(lines[i], activeFence);
    if (activeFence) continue;

    const headerCells = splitPipeTableRow(lines[i]);
    const nextCells = splitPipeTableRow(lines[i + 1]);
    if (!headerCells || !nextCells) continue;

    const hasDelimiter = isTableDelimiterCells(nextCells);
    const loose = !hasDelimiter && isLoosePipeTableRow(lines[i]) && isLoosePipeTableRow(lines[i + 1]);
    if (!hasDelimiter && !loose) continue;

    const dataStart = hasDelimiter ? i + 2 : i + 1;
    let dataEnd = dataStart;
    while (dataEnd < lines.length) {
      const dataCells = splitPipeTableRow(lines[dataEnd]);
      if (!dataCells || isTableDelimiterCells(dataCells)) break;
      if (loose && !isLoosePipeTableRow(lines[dataEnd])) break;
      dataEnd += 1;
    }
    tables.push({ dataStart, dataEnd });
    i = dataEnd - 1;
  }

  return tables;
}

export function sortMarkdownTableColumn(
  content: string,
  tableIndex: number,
  columnIndex: number,
  direction: TableSortDirection,
): string | null {
  if (tableIndex < 0 || columnIndex < 0) return null;
  const lines = content.split("\n");
  const tables = findTables(lines);
  const table = tables[tableIndex];
  if (!table || table.dataStart >= table.dataEnd) return null;

  const rows = lines.slice(table.dataStart, table.dataEnd).map((line, offset) => ({
    line,
    cells: splitPipeTableRow(line) ?? [],
    offset,
  }));
  if (rows.length === 0) return null;

  const sorted = [...rows].sort((a, b) =>
    compareCells(a.cells[columnIndex] ?? "", b.cells[columnIndex] ?? "", direction)
    || a.offset - b.offset
  );
  if (sorted.every((row, i) => row.offset === i)) {
    return content;
  }

  const next = [...lines];
  sorted.forEach((row, i) => {
    next[table.dataStart + i] = row.line;
  });
  return next.join("\n");
}

const TASK_PREFIX_RE = /^(\s*)(TODO|DOING|DONE|CANCELED|CANCELLED|LATER|NOW)\b:?\s*/i;
const TASK_KEYWORD_RE = /^(todo|doing|done|later|now|canceled|cancelled)\b(:?)/i;
const CHECKBOX_PREFIX_RE = /^(\s*)(?:[-*+]\s*)?\[(?: |x|X|-)\]\s*/;
const LEADING_PRIORITY_RE = /^\s*\[#([ABC])\]\s*/i;
const TASK_METADATA_RE = /^\s*(?:CLOSED|SCHEDULED|DEADLINE):\s*/i;
const LOGBOOK_START_RE = /^\s*:LOGBOOK:\s*$/i;
const DRAWER_END_RE = /^\s*:END:\s*$/i;

export function isTaskContent(content: string): boolean {
  const firstLine = content.split(/\r?\n/, 1)[0] ?? "";
  return TASK_PREFIX_RE.test(firstLine) || CHECKBOX_PREFIX_RE.test(firstLine);
}

/** Uppercase a Logseq task keyword even when the user typed `todo` with no trailing space. */
export function normalizeTaskPrefix(content: string): string {
  return content.replace(
    TASK_KEYWORD_RE,
    (_match, keyword: string, colon: string) => `${keyword.toUpperCase()}${colon}`,
  );
}

/**
 * Android IMEs often insert a newline instead of firing Enter.
 * Keep the first line in the current block and return the rest for a new block.
 */
export function splitImeEnterContent(content: string): { head: string; remainder: string } {
  const match = /\r?\n/.exec(content);
  if (!match || match.index === undefined) {
    return { head: normalizeTaskPrefix(content), remainder: "" };
  }
  return {
    head: normalizeTaskPrefix(content.slice(0, match.index)),
    remainder: content.slice(match.index + match[0].length),
  };
}

function stripTaskMetadata(lines: string[]): string[] {
  const result: string[] = [];
  let inLogbook = false;

  for (const line of lines) {
    if (inLogbook) {
      if (DRAWER_END_RE.test(line)) {
        inLogbook = false;
      }
      continue;
    }

    if (LOGBOOK_START_RE.test(line)) {
      inLogbook = true;
      continue;
    }

    if (TASK_METADATA_RE.test(line)) {
      continue;
    }

    result.push(line);
  }

  return result;
}

function stripTaskPrefix(line: string): string | null {
  const taskMatch = line.match(TASK_PREFIX_RE);
  if (taskMatch) {
    const prefix = taskMatch[1] ?? "";
    const rest = line.slice(taskMatch[0].length).replace(LEADING_PRIORITY_RE, "");
    return `${prefix}${rest.trimStart()}`;
  }

  const checkboxMatch = line.match(CHECKBOX_PREFIX_RE);
  if (checkboxMatch) {
    const prefix = checkboxMatch[1] ?? "";
    const rest = line.slice(checkboxMatch[0].length).replace(LEADING_PRIORITY_RE, "");
    return `${prefix}${rest.trimStart()}`;
  }

  return null;
}

export function taskToBulletContent(content: string): string {
  const lines = content.split(/\r?\n/);
  const firstLine = stripTaskPrefix(lines[0] ?? "");
  if (firstLine === null) return content;

  const stripped = stripTaskMetadata([firstLine, ...lines.slice(1)]);
  while (stripped.length > 1 && stripped[0].trim() === "") {
    stripped.shift();
  }

  return stripped.join("\n").trimEnd();
}

export function bulletToTodoContent(content: string): string {
  if (isTaskContent(content)) return content;

  const lines = content.split(/\r?\n/);
  const firstLine = lines[0] ?? "";
  const indent = firstLine.match(/^\s*/)?.[0] ?? "";
  const text = firstLine.slice(indent.length).trimStart();
  lines[0] = text ? `${indent}TODO ${text}` : `${indent}TODO `;
  return lines.join("\n");
}

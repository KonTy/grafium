/**
 * AI edit actions.
 *
 * Lets the assistant *do* things ("add the above answer to today's journal and
 * file it under [[health/supplements]]") instead of only answering. Nothing here
 * touches the graph: this module turns a natural-language request plus the
 * conversation into a reviewable {@link EditPlan}, and the caller applies it
 * only after the user clicks Apply.
 *
 * Two deliberate constraints shape the design:
 *
 * 1. **No provider tool-calling.** Grafium talks to plain OpenAI-compatible
 *    `/v1/chat/completions` endpoints including local llama.cpp servers, and a
 *    test pins the payload to contain no `tools`/`tool_choice`. So the plan is
 *    requested as JSON in an ordinary text completion and parsed tolerantly,
 *    exactly like the research query planner does.
 * 2. **The model never re-types the answer.** An action can set
 *    `source: "answer"` to reuse the previous assistant message verbatim.
 *    Asking a model to echo a long answer invites silent truncation and
 *    paraphrasing, which on a notes app means quietly losing the user's content.
 */

export const EDIT_ACTION_TYPES = [
  "append_to_journal",
  "append_to_page",
  "create_page",
  "add_tags",
  "create_task",
  "replace_block",
  "find_links",
] as const;

export type EditActionType = (typeof EDIT_ACTION_TYPES)[number];

/** Where an action's body text comes from. */
export type EditActionSource = "answer" | "text";

export interface EditAction {
  type: EditActionType;
  /** Short label used as the parent bullet. */
  title: string;
  /** Body markdown. Empty when {@link source} is `"answer"` until hydrated. */
  content: string;
  source: EditActionSource;
  /** Normalised page-style tags, e.g. `health/supplements`. */
  tags: string[];
  /** Target page title for page-scoped actions. */
  page?: string;
  /** Local `YYYY-MM-DD` for journal/task actions. */
  date?: string;
  /** Run link suggestions on the resulting page. */
  findLinks?: boolean;
}

export interface EditPlan {
  actions: EditAction[];
}

/** Actions whose body is the note text the user cares about. */
const CONTENT_ACTIONS: ReadonlySet<EditActionType> = new Set([
  "append_to_journal",
  "append_to_page",
  "create_page",
  "create_task",
  "replace_block",
]);

export function actionUsesContent(type: EditActionType): boolean {
  return CONTENT_ACTIONS.has(type);
}

/**
 * Cheap local gate deciding whether to spend a model call on planning.
 *
 * Running the planner on every message would double latency for ordinary
 * questions, so an imperative-looking request is required first. This only ever
 * *offers* a plan — the model still decides, and the user still has to click
 * Apply — so a false positive costs a round trip and a false negative costs
 * nothing but rephrasing.
 */
export function looksLikeEditRequest(text: string): boolean {
  const trimmed = text.trim().toLowerCase();
  if (!trimmed || trimmed.length > 600) return false;
  // A trailing question mark means they are asking, not instructing — even when
  // the sentence opens with a verb ("add salt to the recipe?").
  if (trimmed.endsWith("?")) return false;

  const verb =
    /^(?:please\s+|can you\s+|could you\s+|now\s+|also\s+|and\s+)*(add|append|save|store|put|file|record|log|create|make|write|insert|turn|convert|copy|move|tag|mark|note|capture|draft|start|new)\b/;
  if (verb.test(trimmed)) return true;

  const target =
    /\b(?:to|into|in|as|under|onto)\s+(?:my |the |today's |todays |a |an )?(journal|diary|daily note|new page|page|note|task|todo|to-do|flashcard)\b/;
  return target.test(trimmed);
}

/** Strip `[[ ]]`, `#`, quotes and stray slashes from a tag the model produced. */
export function normalizeTag(raw: string): string {
  let tag = String(raw ?? "").trim();
  tag = tag.replace(/^\[\[/, "").replace(/\]\]$/, "");
  tag = tag.replace(/^#/, "");
  tag = tag.replace(/^["']|["']$/g, "");
  // Collapse separator noise; `health / supplements` and `health//supplements`
  // both mean the same namespace and must not create two different pages.
  tag = tag
    .split("/")
    .map((part) => part.trim())
    .filter(Boolean)
    .join("/");
  return tag.trim();
}

/** Render tags as the wiki links Grafium actually resolves. */
export function renderTags(tags: string[]): string {
  return tags.map((tag) => `[[${tag}]]`).join(" ");
}

/**
 * Resolve a date the model wrote into a local `YYYY-MM-DD` journal title.
 *
 * Returns null for anything unrecognised so the caller can fall back to today
 * rather than inventing a page with a garbage title.
 */
export function resolveJournalDate(spec: string | undefined, today: Date = new Date()): string | null {
  const iso = (date: Date) =>
    `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
  const value = String(spec ?? "").trim().toLowerCase();
  if (!value || value === "today" || value === "now") return iso(today);

  const shift = (days: number) => {
    const date = new Date(today.getFullYear(), today.getMonth(), today.getDate());
    date.setDate(date.getDate() + days);
    return iso(date);
  };
  if (value === "yesterday") return shift(-1);
  if (value === "tomorrow") return shift(1);
  if (/^\d{4}-\d{2}-\d{2}$/.test(value)) return value;
  return null;
}

/**
 * Split answer markdown into one block per idea.
 *
 * Grafium blocks are already bullets, so list markers are stripped rather than
 * nested inside another bullet. Fenced code is kept intact: splitting a code
 * block across bullets would corrupt it.
 */
export function splitIntoBlocks(markdown: string): string[] {
  const lines = String(markdown ?? "").replace(/\r\n/g, "\n").split("\n");
  const blocks: string[] = [];
  let paragraph: string[] = [];
  let fence: string | null = null;
  let fenced: string[] = [];

  const flushParagraph = () => {
    const text = paragraph.join("\n").trim();
    if (text) blocks.push(text);
    paragraph = [];
  };

  for (const line of lines) {
    const fenceMatch = line.match(/^\s*(```+|~~~+)/);
    if (fence) {
      fenced.push(line);
      if (fenceMatch && fenceMatch[1].startsWith(fence[0]) && fenceMatch[1].length >= fence.length) {
        blocks.push(fenced.join("\n"));
        fenced = [];
        fence = null;
      }
      continue;
    }
    if (fenceMatch) {
      flushParagraph();
      fence = fenceMatch[1];
      fenced = [line];
      continue;
    }
    if (!line.trim()) {
      flushParagraph();
      continue;
    }
    const listItem = line.match(/^\s*(?:[-*+]|\d+[.)])\s+(.*)$/);
    if (listItem) {
      flushParagraph();
      blocks.push(listItem[1].trim());
      continue;
    }
    if (/^\s*#{1,6}\s+/.test(line)) {
      flushParagraph();
      blocks.push(line.trim());
      continue;
    }
    paragraph.push(line);
  }

  if (fence) blocks.push(fenced.join("\n").trim());
  flushParagraph();
  return blocks.filter(Boolean);
}

/** The bullet an action writes, with its tags, plus one child per idea. */
export function actionBlocks(action: EditAction): { parent: string; children: string[] } {
  const tags = renderTags(action.tags);
  const title = action.title.trim();
  const children = splitIntoBlocks(action.content);

  if (action.type === "create_task") {
    const head = `TODO ${title || children.shift() || "Task"}`;
    return { parent: tags ? `${head} ${tags}` : head, children };
  }
  // Without a title the first idea becomes the bullet, so tags still land on a
  // real line instead of an empty one.
  const head = title || children.shift() || "Note";
  return { parent: tags ? `${head} ${tags}` : head, children };
}

/** One-line plain-language description for the preview card. */
export function describeAction(action: EditAction): string {
  const tags = action.tags.length ? ` under ${renderTags(action.tags)}` : "";
  switch (action.type) {
    case "append_to_journal":
      return `Add to your ${action.date} journal${tags}`;
    case "append_to_page":
      return `Add to the page “${action.page}”${tags}`;
    case "create_page":
      return `Create the page “${action.page}”${tags}${action.findLinks ? ", then look for links" : ""}`;
    case "add_tags":
      return `Tag “${action.page}” with ${renderTags(action.tags)}`;
    case "create_task":
      return `Add a task to your ${action.date} journal${tags}`;
    case "replace_block":
      return "Replace the block this conversation is about";
    case "find_links":
      return `Look for links on “${action.page}”`;
  }
}

const PLANNER_INSTRUCTIONS = `You turn a request into a plan of edits for a personal notes app.

Reply with ONLY a JSON object, no commentary and no code fence:
{"actions": [ ... ]}

Return {"actions": []} when the message is a question, a request for information,
or anything that does not ask you to change the user's notes.

Each action is an object with:
  "type"    one of: append_to_journal, append_to_page, create_page, add_tags, create_task, replace_block, find_links
  "title"   a short label (max ~80 chars) used as the bullet heading
  "source"  "answer" to reuse the previous assistant answer verbatim, or "text" to use your own "content"
  "content" the body markdown; use "" when source is "answer"
  "tags"    array of page names to link, e.g. ["health/supplements"]; [] when none
  "page"    target page title, for append_to_page, create_page, add_tags and find_links
  "date"    "today", "yesterday", "tomorrow" or YYYY-MM-DD, for append_to_journal and create_task
  "findLinks" true to look for connections after creating a page

Rules:
- Prefer "source": "answer" whenever the user refers to the answer above. Never retype or summarise the answer.
- Keep tags as the user wrote them, without the [[ ]] brackets. Preserve nesting like health/supplements.
- Use several actions when the user asks for several things.
- Never invent a destination the user did not ask for.`;

/** Build the planning prompt. Kept pure so the wording itself is testable. */
export function buildPlannerPrompt(request: string, lastAnswer: string, blockAvailable: boolean): string {
  const answer = lastAnswer.trim();
  const excerpt = answer.length > 2000 ? `${answer.slice(0, 2000)}…` : answer;
  return [
    PLANNER_INSTRUCTIONS,
    blockAvailable ? "" : 'The conversation is not attached to a block, so never use "replace_block".',
    answer ? `PREVIOUS ANSWER (the user may refer to this as "the answer above"):\n${excerpt}` : "There is no previous answer yet.",
    `REQUEST:\n${request.trim()}`,
  ]
    .filter(Boolean)
    .join("\n\n");
}

function coerceAction(raw: unknown): EditAction | null {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const record = raw as Record<string, unknown>;
  const type = String(record.type ?? "").trim() as EditActionType;
  if (!EDIT_ACTION_TYPES.includes(type)) return null;

  const tags = Array.isArray(record.tags)
    ? record.tags.map((tag) => normalizeTag(String(tag))).filter(Boolean)
    : [];
  const page = typeof record.page === "string" ? record.page.trim() : "";
  const content = typeof record.content === "string" ? record.content : "";
  // A model that forgets "source" but supplies no content clearly meant the
  // answer; treating that as an empty note would silently write a blank bullet.
  const source: EditActionSource =
    record.source === "text" ? "text" : record.source === "answer" || !content.trim() ? "answer" : "text";

  const action: EditAction = {
    type,
    title: typeof record.title === "string" ? record.title.trim().slice(0, 120) : "",
    content,
    source,
    tags: [...new Set(tags)],
  };

  if (type === "append_to_journal" || type === "create_task") {
    action.date = resolveJournalDate(typeof record.date === "string" ? record.date : undefined) ?? resolveJournalDate("today")!;
  }
  if (type === "append_to_page" || type === "create_page" || type === "add_tags" || type === "find_links") {
    if (!page) return null;
    action.page = page;
  }
  if (type === "create_page") action.findLinks = record.findLinks === true;
  if (type === "add_tags" && !action.tags.length) return null;

  return action;
}

/**
 * Parse a planner response into a plan.
 *
 * Tolerant on purpose, mirroring the research planner: small local models wrap
 * JSON in prose, reasoning tags or fences, and hard-failing on that would make
 * the feature unusable on exactly the setups Grafium exists to support. An
 * unparseable response is reported as "no actions" rather than an error,
 * because the fallback — answering the question normally — is always safe.
 */
export function parseEditPlan(raw: string): EditPlan {
  const cleaned = String(raw ?? "")
    .replace(/<think>[\s\S]*?<\/think>/gi, "")
    .replace(/<reasoning>[\s\S]*?<\/reasoning>/gi, "")
    .replace(/```(?:json)?\s*/gi, "")
    .replace(/```/g, "")
    .trim();
  if (!cleaned) return { actions: [] };

  const candidates: unknown[] = [];
  const pushParsed = (text: string) => {
    try {
      candidates.push(JSON.parse(text));
    } catch {
      // Not JSON; the next extraction strategy gets a turn.
    }
  };

  pushParsed(cleaned);
  const object = cleaned.match(/\{[\s\S]*\}/);
  if (object) pushParsed(object[0]);
  const array = cleaned.match(/\[[\s\S]*\]/);
  if (array) pushParsed(array[0]);

  for (const candidate of candidates) {
    const list = Array.isArray(candidate)
      ? candidate
      : candidate && typeof candidate === "object" && Array.isArray((candidate as { actions?: unknown }).actions)
        ? ((candidate as { actions: unknown[] }).actions)
        : candidate && typeof candidate === "object" && "type" in (candidate as object)
          ? [candidate]
          : null;
    if (!list) continue;
    const actions = list.map(coerceAction).filter((action): action is EditAction => action !== null);
    if (actions.length) return { actions };
    // An explicit empty list is a real answer ("this is just a question"), so
    // stop rather than letting a later, sloppier candidate override it.
    if (Array.isArray(list)) return { actions: [] };
  }

  return { actions: [] };
}

/** Fill in `source: "answer"` bodies and drop actions left with nothing to write. */
export function hydratePlan(plan: EditPlan, lastAnswer: string): EditPlan {
  const answer = lastAnswer.trim();
  const actions = plan.actions
    .map((action) => (action.source === "answer" ? { ...action, content: answer } : action))
    .filter((action) => !actionUsesContent(action.type) || action.content.trim() || action.title.trim());
  return { actions };
}

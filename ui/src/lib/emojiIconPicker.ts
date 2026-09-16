import type { SlashCommand } from "./slashCommands";

export type EmojiIconKind = "emoji" | "icon";

export const EMOJI_ICON_SLASH_COMMANDS: SlashCommand[] = [
  { label: "/em", detail: "Find an emoji or icon", apply: "/em " },
  { label: "/emoji", detail: "Find an emoji", apply: "/emoji " },
  { label: "/icon", detail: "Find a symbolic icon", apply: "/icon " },
];

export interface EmojiIconEntry {
  kind: EmojiIconKind;
  name: string;
  preview: string;
  insert: string;
  group: string;
  keywords: string[];
}

type EntrySeed = {
  name: string;
  preview: string;
  group: string;
  keywords?: string[];
};

const EMOJI_SEEDS: EntrySeed[] = [
  { name: "grinning", preview: "😀", group: "Smileys", keywords: ["smile", "happy", "face"] },
  { name: "smile", preview: "😄", group: "Smileys", keywords: ["happy", "joy", "face"] },
  { name: "laughing", preview: "😆", group: "Smileys", keywords: ["lol", "happy", "face"] },
  { name: "joy", preview: "😂", group: "Smileys", keywords: ["laugh", "tears", "funny"] },
  { name: "wink", preview: "😉", group: "Smileys", keywords: ["face"] },
  { name: "thinking", preview: "🤔", group: "Smileys", keywords: ["question", "idea"] },
  { name: "heart-eyes", preview: "😍", group: "Smileys", keywords: ["love", "face"] },
  { name: "sunglasses", preview: "😎", group: "Smileys", keywords: ["cool", "face"] },
  { name: "eyes", preview: "👀", group: "People", keywords: ["look", "watch", "see"] },
  { name: "thumbs-up", preview: "👍", group: "People", keywords: ["yes", "approve", "like"] },
  { name: "clap", preview: "👏", group: "People", keywords: ["applause", "great"] },
  { name: "pray", preview: "🙏", group: "People", keywords: ["thanks", "please"] },
  { name: "muscle", preview: "💪", group: "People", keywords: ["strong", "workout"] },
  { name: "heart", preview: "❤️", group: "Symbols", keywords: ["love", "red"] },
  { name: "sparkles", preview: "✨", group: "Symbols", keywords: ["magic", "clean"] },
  { name: "fire", preview: "🔥", group: "Symbols", keywords: ["hot", "important"] },
  { name: "rocket", preview: "🚀", group: "Objects", keywords: ["launch", "ship"] },
  { name: "star", preview: "⭐", group: "Symbols", keywords: ["favorite", "important"] },
  { name: "warning", preview: "⚠️", group: "Symbols", keywords: ["alert", "caution"] },
  { name: "check", preview: "✅", group: "Symbols", keywords: ["done", "complete", "yes"] },
  { name: "cross", preview: "❌", group: "Symbols", keywords: ["no", "x", "cancel"] },
  { name: "question", preview: "❓", group: "Symbols", keywords: ["help", "ask"] },
  { name: "bulb", preview: "💡", group: "Objects", keywords: ["idea", "tip", "light"] },
  { name: "book", preview: "📚", group: "Objects", keywords: ["read", "knowledge"] },
  { name: "memo", preview: "📝", group: "Objects", keywords: ["note", "write"] },
  { name: "calendar", preview: "📅", group: "Objects", keywords: ["date", "schedule"] },
  { name: "clock", preview: "⏰", group: "Objects", keywords: ["time", "alarm"] },
  { name: "pin", preview: "📌", group: "Objects", keywords: ["pinned", "location"] },
  { name: "link", preview: "🔗", group: "Objects", keywords: ["url", "chain"] },
  { name: "computer", preview: "💻", group: "Objects", keywords: ["laptop", "code"] },
  { name: "bug", preview: "🐛", group: "Nature", keywords: ["debug", "issue"] },
  { name: "coffee", preview: "☕", group: "Food", keywords: ["drink", "break"] },
  { name: "party", preview: "🎉", group: "Objects", keywords: ["celebrate", "done"] },
  { name: "target", preview: "🎯", group: "Objects", keywords: ["goal", "focus"] },
];

const ICON_SEEDS: EntrySeed[] = [
  { name: "star", preview: "★", group: "Font Awesome-style", keywords: ["favorite", "bookmark"] },
  { name: "star-outline", preview: "☆", group: "Font Awesome-style", keywords: ["favorite", "empty"] },
  { name: "heart", preview: "♡", group: "Font Awesome-style", keywords: ["love", "like"] },
  { name: "check", preview: "✓", group: "Font Awesome-style", keywords: ["done", "complete", "yes"] },
  { name: "xmark", preview: "✕", group: "Font Awesome-style", keywords: ["close", "cancel", "no"] },
  { name: "plus", preview: "+", group: "Font Awesome-style", keywords: ["add", "new"] },
  { name: "minus", preview: "−", group: "Font Awesome-style", keywords: ["remove", "delete"] },
  { name: "arrow-right", preview: "→", group: "Font Awesome-style", keywords: ["next", "forward"] },
  { name: "arrow-left", preview: "←", group: "Font Awesome-style", keywords: ["back", "previous"] },
  { name: "external-link", preview: "↗", group: "Font Awesome-style", keywords: ["open", "outbound"] },
  { name: "rocket", preview: "⇧", group: "Font Awesome-style", keywords: ["launch", "ship"] },
  { name: "gear", preview: "⚙", group: "Font Awesome-style", keywords: ["settings", "cog"] },
  { name: "flag", preview: "⚑", group: "Font Awesome-style", keywords: ["mark", "milestone"] },
  { name: "warning", preview: "⚠", group: "Font Awesome-style", keywords: ["alert", "caution"] },
  { name: "info", preview: "ⓘ", group: "Font Awesome-style", keywords: ["help", "about"] },
  { name: "home", preview: "⌂", group: "Font Awesome-style", keywords: ["house", "root"] },
  { name: "edit", preview: "✎", group: "Font Awesome-style", keywords: ["write", "pencil"] },
  { name: "search", preview: "⌕", group: "Font Awesome-style", keywords: ["find", "magnifier"] },
  { name: "link", preview: "⛓", group: "Font Awesome-style", keywords: ["chain", "url"] },
  { name: "lock", preview: "🔒", group: "Font Awesome-style", keywords: ["secure", "private"] },
  { name: "unlock", preview: "🔓", group: "Font Awesome-style", keywords: ["open", "public"] },
  { name: "book", preview: "▣", group: "Font Awesome-style", keywords: ["read", "page"] },
  { name: "bookmark", preview: "▮", group: "Font Awesome-style", keywords: ["save", "marker"] },
  { name: "code", preview: "</>", group: "Font Awesome-style", keywords: ["developer", "program"] },
  { name: "database", preview: "◫", group: "Font Awesome-style", keywords: ["db", "storage"] },
  { name: "table", preview: "▦", group: "Font Awesome-style", keywords: ["grid", "spreadsheet"] },
  { name: "list", preview: "☷", group: "Font Awesome-style", keywords: ["menu", "bars"] },
  { name: "image", preview: "▧", group: "Font Awesome-style", keywords: ["photo", "picture"] },
  { name: "video", preview: "▻", group: "Font Awesome-style", keywords: ["play", "media"] },
  { name: "bell", preview: "🔔", group: "Font Awesome-style", keywords: ["notify", "alert"] },
  { name: "robot", preview: "⚇", group: "Font Awesome-style", keywords: ["ai", "bot"] },
  { name: "brain", preview: "☁", group: "Font Awesome-style", keywords: ["mind", "ai", "idea"] },
  { name: "bolt", preview: "ϟ", group: "Font Awesome-style", keywords: ["flash", "fast"] },
];

function emojiEntry(seed: EntrySeed): EmojiIconEntry {
  return {
    kind: "emoji",
    name: seed.name,
    preview: seed.preview,
    insert: seed.preview,
    group: seed.group,
    keywords: seed.keywords ?? [],
  };
}

function iconEntry(seed: EntrySeed): EmojiIconEntry {
  return {
    kind: "icon",
    name: seed.name,
    preview: seed.preview,
    insert: `:icon-${seed.name}:`,
    group: seed.group,
    keywords: ["icon", "fa", "fontawesome", "font awesome", ...(seed.keywords ?? [])],
  };
}

export const EMOJI_ICON_ENTRIES: EmojiIconEntry[] = [
  ...EMOJI_SEEDS.map(emojiEntry),
  ...ICON_SEEDS.map(iconEntry),
];

const ICON_BY_NAME = new Map(
  EMOJI_ICON_ENTRIES
    .filter((entry) => entry.kind === "icon")
    .map((entry) => [entry.name, entry])
);

export interface EmojiIconMenu {
  from: number;
  command: "/em" | "/emoji" | "/icon";
  query: string;
  entries: EmojiIconEntry[];
}

function entryMatches(entry: EmojiIconEntry, query: string): boolean {
  if (!query) return true;
  const haystack = [entry.name, entry.preview, entry.group, ...entry.keywords].join(" ").toLowerCase();
  return query
    .split(/\s+/)
    .filter(Boolean)
    .every((part) => haystack.includes(part));
}

function entryScore(entry: EmojiIconEntry, query: string): number {
  if (!query) return entry.kind === "emoji" ? 1 : 0;
  if (entry.name.toLowerCase().startsWith(query)) return 30;
  if (entry.keywords.some((keyword) => keyword.toLowerCase().startsWith(query))) return 20;
  if (entry.name.toLowerCase().includes(query)) return 10;
  return 0;
}

export function emojiIconMenuBeforeCursor(beforeCursor: string, limit = 80): EmojiIconMenu | null {
  const match = beforeCursor.match(/(?:^|\s)(\/(?:emoji|icon|em)(?:\s+[^\n]*)?)$/i);
  if (!match) return null;

  const raw = match[1];
  const commandMatch = raw.match(/^\/(?:emoji|icon|em)(?=$|\s)/i);
  if (!commandMatch) return null;

  const command = commandMatch[0].toLowerCase() as EmojiIconMenu["command"];
  const query = raw.slice(command.length).trim().toLowerCase();
  const allowedKind: EmojiIconKind | "all" =
    command === "/emoji" ? "emoji" : command === "/icon" ? "icon" : "all";

  const entries = EMOJI_ICON_ENTRIES
    .filter((entry) => allowedKind === "all" || entry.kind === allowedKind)
    .filter((entry) => entryMatches(entry, query))
    .sort((a, b) => entryScore(b, query) - entryScore(a, query) || a.name.localeCompare(b.name))
    .slice(0, limit);

  return {
    from: beforeCursor.length - raw.length,
    command,
    query,
    entries,
  };
}

function escapeAttr(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/"/g, "&quot;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

export function iconHtmlForName(name: string): string | null {
  const icon = ICON_BY_NAME.get(name);
  if (!icon) return null;
  const label = escapeAttr(name.replace(/-/g, " "));
  return `<span class="grafium-icon" role="img" aria-label="${label}" title="${label}">${icon.preview}</span>`;
}
